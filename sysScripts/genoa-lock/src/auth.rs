use std::{
    ffi::{CStr, CString},
    os::fd::OwnedFd,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
};

use pam_client2::{Context, ConversationHandler, ErrorCode, Flag};
use rustix::io::{self, retry_on_intr};
use uzers::get_current_username;
use zeroize::Zeroizing;

const PAM_SERVICE: &str = "genoa-lock";

pub fn start_authentication(
    password: Zeroizing<String>,
    completion_event: Arc<OwnedFd>,
) -> Result<Receiver<bool>, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::Builder::new()
        .name("genoa-lock-auth".to_string())
        .spawn(move || authenticate(password, sender, completion_event))
        .map_err(|error| format!("could not start the authentication worker: {error}"))?;
    Ok(receiver)
}

fn authenticate(
    password: Zeroizing<String>,
    sender: SyncSender<bool>,
    completion_event: Arc<OwnedFd>,
) {
    let mut completion = AuthenticationCompletion::new(sender, completion_event);
    let authenticated = authenticate_current_user(password).unwrap_or(false);
    completion.finish(authenticated);
}

pub fn authenticate_current_user(password: Zeroizing<String>) -> Result<bool, String> {
    let username = effective_username()?;
    let conversation = Credentials { password };
    let mut client = Context::new(PAM_SERVICE, Some(&username), conversation)
        .map_err(|_| format!("could not initialise PAM service {PAM_SERVICE:?}"))?;

    if client.authenticate(Flag::NONE).is_err() || client.acct_mgmt(Flag::NONE).is_err() {
        return Ok(false);
    }
    let _ = client.reinitialize_credentials(Flag::NONE);
    Ok(true)
}

fn effective_username() -> Result<String, String> {
    get_current_username()
        .ok_or_else(|| "could not determine the effective user's account name".to_string())?
        .into_string()
        .map_err(|_| "effective account name is not valid UTF-8".to_string())
}

struct Credentials {
    password: Zeroizing<String>,
}

struct AuthenticationCompletion {
    sender: Option<SyncSender<bool>>,
    completion_event: Arc<OwnedFd>,
    notified: bool,
}

impl AuthenticationCompletion {
    const fn new(sender: SyncSender<bool>, completion_event: Arc<OwnedFd>) -> Self {
        Self {
            sender: Some(sender),
            completion_event,
            notified: false,
        }
    }

    fn finish(&mut self, authenticated: bool) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(authenticated);
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

impl ConversationHandler for Credentials {
    fn prompt_echo_on(&mut self, _: &CStr) -> Result<CString, ErrorCode> {
        Err(ErrorCode::CONV_ERR)
    }

    fn prompt_echo_off(&mut self, _: &CStr) -> Result<CString, ErrorCode> {
        CString::new(self.password.as_bytes()).map_err(|_| ErrorCode::CONV_ERR)
    }

    fn text_info(&mut self, _: &CStr) {}

    fn error_msg(&mut self, _: &CStr) {}
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

        completion.finish(true);

        assert!(receiver.recv()?);
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
}
