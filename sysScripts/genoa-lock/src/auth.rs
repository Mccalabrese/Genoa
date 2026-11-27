use std::{
    fs,
    io::{ErrorKind, Read, Write},
    os::fd::OwnedFd,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};

use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};
use rustix::io::{self, retry_on_intr};
use zeroize::Zeroizing;

use crate::protocol::{REQUEST_MAGIC, RESPONSE_DENY, RESPONSE_ERROR, RESPONSE_OK};

#[cfg(not(debug_assertions))]
use std::{os::unix::fs::MetadataExt, path::Path};

pub type AuthenticationResult = Result<bool, String>;

pub fn ensure_helper_available() -> Result<(), String> {
    #[cfg(debug_assertions)]
    {
        let path = helper_path()?;
        fs::metadata(path.as_path()).map(|_| ()).map_err(|error| {
            format!(
                "could not inspect debug PAM helper {}: {error}",
                path.display()
            )
        })
    }
    #[cfg(not(debug_assertions))]
    {
        let path = helper_path();
        ensure_release_helper(&path)
    }
}

#[cfg(not(debug_assertions))]
fn ensure_release_helper(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect PAM helper {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("the PAM helper is not a regular file".to_string());
    }
    if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 || metadata.mode() & 0o111 == 0 {
        return Err("the PAM helper does not have safe ownership or permissions".to_string());
    }
    validate_parent_directories(path)
}

pub fn start_authentication(
    password: Zeroizing<String>,
    completion_event: Arc<OwnedFd>,
) -> Result<Receiver<AuthenticationResult>, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("genoa-lock-auth".to_string())
        .spawn(move || authenticate(password, sender, completion_event))
        .map_err(|error| format!("could not start the authentication worker: {error}"))?;
    Ok(receiver)
}

fn authenticate(
    password: Zeroizing<String>,
    sender: SyncSender<AuthenticationResult>,
    completion_event: Arc<OwnedFd>,
) {
    let mut completion = AuthenticationCompletion::new(sender, completion_event);
    let result = authenticate_with_helper(&password);
    drop(password);
    completion.finish(result);
}

fn authenticate_with_helper(password: &Zeroizing<String>) -> AuthenticationResult {
    #[cfg(debug_assertions)]
    let helper = helper_path()?;
    #[cfg(not(debug_assertions))]
    let helper = helper_path();
    let mut child = Command::new(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start the PAM helper: {error}"))?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| "could not open the PAM helper input".to_string())?;
    let mut output = child
        .stdout
        .take()
        .ok_or_else(|| "could not open the PAM helper result channel".to_string())?;
    let write_result = write_request(&mut input, password);
    drop(input);
    if let Err(error) = write_result {
        let _ = child.wait();
        return Err(format!(
            "could not send credentials to the PAM helper: {error}"
        ));
    }
    let status = child
        .wait()
        .map_err(|error| format!("could not reap the PAM helper: {error}"))?;
    let flags = fcntl_getfl(&output)
        .map_err(|error| format!("could not inspect the PAM helper result channel: {error}"))?;
    fcntl_setfl(&output, flags | OFlags::NONBLOCK).map_err(|error| {
        format!("could not make the PAM helper result channel nonblocking: {error}")
    })?;
    let mut response = [0_u8; 9];
    let read_result = loop {
        let result = output.read(&mut response);
        if !matches!(
            result,
            Err(ref error) if error.kind() == ErrorKind::Interrupted
        ) {
            break result;
        }
    };
    let read = match read_result {
        Ok(read) => read,
        Err(error) if error.kind() == ErrorKind::WouldBlock => 0,
        Err(error) => return Err(format!("could not read the PAM helper result: {error}")),
    };
    if !status.success() || read != RESPONSE_OK.len() {
        return Err("the PAM helper did not complete correctly".to_string());
    }
    if response[..RESPONSE_OK.len()] == RESPONSE_OK {
        Ok(true)
    } else if response[..RESPONSE_DENY.len()] == RESPONSE_DENY {
        Ok(false)
    } else if response[..RESPONSE_ERROR.len()] == RESPONSE_ERROR {
        Err("the PAM helper could not complete authentication".to_string())
    } else {
        Err("the PAM helper returned an invalid result".to_string())
    }
}

fn write_request(input: &mut impl Write, password: &str) -> std::io::Result<()> {
    let length = u16::try_from(password.len()).map_err(|_| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            "password is too long for the PAM helper",
        )
    })?;
    input.write_all(&REQUEST_MAGIC)?;
    input.write_all(&length.to_be_bytes())?;
    input.write_all(password.as_bytes())
}

#[cfg(debug_assertions)]
fn helper_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("could not locate the debug PAM helper: {error}"))?;
    executable
        .parent()
        .map(|parent| parent.join("genoa-lock-pam"))
        .ok_or_else(|| "could not locate the debug PAM helper directory".to_string())
}

#[cfg(not(debug_assertions))]
fn helper_path() -> PathBuf {
    PathBuf::from("/usr/local/lib/genoa/genoa-lock-pam")
}

#[cfg(not(debug_assertions))]
fn validate_parent_directories(path: &Path) -> Result<(), String> {
    for parent in path.ancestors().skip(1) {
        let metadata = fs::symlink_metadata(parent).map_err(|error| {
            format!(
                "could not inspect PAM helper directory {}: {error}",
                parent.display()
            )
        })?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata.uid() != 0
            || metadata.mode() & 0o022 != 0
        {
            return Err("the PAM helper directory is not safely owned".to_string());
        }
    }
    Ok(())
}

struct AuthenticationCompletion {
    sender: Option<SyncSender<AuthenticationResult>>,
    completion_event: Arc<OwnedFd>,
    notified: bool,
}

impl AuthenticationCompletion {
    const fn new(sender: SyncSender<AuthenticationResult>, completion_event: Arc<OwnedFd>) -> Self {
        Self {
            sender: Some(sender),
            completion_event,
            notified: false,
        }
    }

    fn finish(&mut self, result: AuthenticationResult) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(result);
        }
        self.notify();
    }

    fn notify(&mut self) {
        if self.notified {
            return;
        }
        self.notified = true;
        let _ = retry_on_intr(|| io::write(self.completion_event.as_ref(), &1_u64.to_ne_bytes()));
    }
}

impl Drop for AuthenticationCompletion {
    fn drop(&mut self) {
        drop(self.sender.take());
        self.notify();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};

    use rustix::{
        event::{EventfdFlags, eventfd},
        io,
    };

    use super::AuthenticationCompletion;

    #[test]
    fn completion_delivers_result_and_notifies_once() -> Result<(), Box<dyn std::error::Error>> {
        let event = Arc::new(eventfd(0, EventfdFlags::CLOEXEC)?);
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut completion = AuthenticationCompletion::new(sender, event.clone());

        completion.finish(Ok(true));

        assert!(receiver.recv()??);
        let mut counter = [0_u8; 8];
        assert_eq!(io::read(event.as_ref(), &mut counter)?, 8);
        assert_eq!(u64::from_ne_bytes(counter), 1);
        Ok(())
    }

    #[test]
    fn dropped_completion_wakes_disconnected_receiver() -> Result<(), Box<dyn std::error::Error>> {
        let event = Arc::new(eventfd(0, EventfdFlags::CLOEXEC)?);
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(AuthenticationCompletion::new(sender, event.clone()));

        assert!(matches!(
            receiver.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
        let mut counter = [0_u8; 8];
        assert_eq!(io::read(event.as_ref(), &mut counter)?, 8);
        assert_eq!(u64::from_ne_bytes(counter), 1);
        Ok(())
    }

    #[test]
    fn completion_preserves_an_infrastructure_error() -> Result<(), Box<dyn std::error::Error>> {
        let event = Arc::new(eventfd(0, EventfdFlags::CLOEXEC)?);
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut completion = AuthenticationCompletion::new(sender, event);

        completion.finish(Err("PAM service unavailable".to_string()));

        assert_eq!(receiver.recv()?, Err("PAM service unavailable".to_string()));
        Ok(())
    }
}
