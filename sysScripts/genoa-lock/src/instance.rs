//! Per-session ownership for Genoa Lock.
//!
//! Every launcher reaches this guard: compositor hotkeys, the sidebar,
//! Waybar, and idle management. Keeping the guard in the locker avoids two
//! session-lock clients competing for the same keyboard focus.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use rustix::{
    fs::{FlockOperation, flock},
    io::Errno,
};
use uzers::get_current_uid;

const DIRECTORY_NAME: &str = "genoa";
const LOCK_FILE_NAME: &str = "genoa-lock.lock";
const LOCK_READY: &[u8] = b"locked\n";

/// Why a process could not become the session locker.
#[derive(Debug, Eq, PartialEq)]
pub enum AcquireError {
    /// A different Genoa Lock process owns the session guard.
    AlreadyRunning,
    /// The runtime directory or guard file was not safe to use.
    Setup(String),
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyRunning => {
                formatter.write_str("another Genoa Lock instance already owns this session")
            }
            Self::Setup(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for AcquireError {}

pub struct InstanceGuard {
    file: File,
}

impl InstanceGuard {
    pub fn acquire() -> Result<Self, AcquireError> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| {
                AcquireError::Setup(
                    "XDG_RUNTIME_DIR is unavailable; cannot safely own the lock".to_string(),
                )
            })?;
        Self::acquire_in(&runtime)
    }

    /// Returns whether the process that currently owns the guard has received
    /// the compositor's `locked` event. This is intentionally separate from
    /// ownership: a process that is still starting must not authorize suspend.
    pub fn active_session_is_locked() -> Result<bool, String> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| {
                "XDG_RUNTIME_DIR is unavailable; cannot inspect lock state".to_string()
            })?;
        Self::active_session_is_locked_in(&runtime)
    }

    /// Waits briefly for a locker that is already starting to receive the
    /// compositor's `locked` event. This is used only by the sleep hook, where
    /// proceeding before that event could suspend an exposed desktop.
    pub fn wait_until_locked(timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            if Self::active_session_is_locked()? {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(
                    "the compositor did not confirm the lock before the sleep deadline".to_string(),
                );
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Records that the compositor has confirmed this session is locked.
    pub fn mark_session_locked(&mut self) -> Result<(), String> {
        self.file
            .set_len(0)
            .map_err(|error| format!("could not clear lock readiness: {error}"))?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|error| format!("could not seek lock readiness: {error}"))?;
        self.file
            .write_all(LOCK_READY)
            .map_err(|error| format!("could not record lock readiness: {error}"))?;
        self.file
            .sync_data()
            .map_err(|error| format!("could not save lock readiness: {error}"))
    }

    fn acquire_in(runtime: &Path) -> Result<Self, AcquireError> {
        let directory = private_runtime_directory(runtime).map_err(AcquireError::Setup)?;
        let lock_path = directory.join(LOCK_FILE_NAME);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)
            .map_err(|error| {
                AcquireError::Setup(format!("could not open {}: {error}", lock_path.display()))
            })?;
        ensure_private_regular_file(&file, &lock_path).map_err(AcquireError::Setup)?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => {}
            Err(error) if error == Errno::AGAIN || error == Errno::WOULDBLOCK => {
                return Err(AcquireError::AlreadyRunning);
            }
            Err(error) => {
                return Err(AcquireError::Setup(format!(
                    "could not acquire the Genoa Lock session guard: {error}"
                )));
            }
        }
        // A previous process may have left a readiness marker behind. Clear it
        // while we exclusively own the guard, before connecting to Wayland.
        file.set_len(0).map_err(|error| {
            AcquireError::Setup(format!("could not clear lock readiness: {error}"))
        })?;
        file.sync_data().map_err(|error| {
            AcquireError::Setup(format!("could not save lock readiness: {error}"))
        })?;
        Ok(Self { file })
    }

    fn active_session_is_locked_in(runtime: &Path) -> Result<bool, String> {
        let directory = private_runtime_directory(runtime)?;
        let lock_path = directory.join(LOCK_FILE_NAME);
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!("could not open {}: {error}", lock_path.display())),
        };
        ensure_private_regular_file(&file, &lock_path)?;

        let length = file
            .metadata()
            .map_err(|error| format!("could not inspect lock readiness: {error}"))?
            .len();
        if length != LOCK_READY.len() as u64 {
            return Ok(false);
        }

        let mut readiness = [0; LOCK_READY.len()];
        file.read_exact(&mut readiness)
            .map_err(|error| format!("could not read lock readiness: {error}"))?;
        Ok(readiness == LOCK_READY)
    }
}

fn private_runtime_directory(runtime: &Path) -> Result<PathBuf, String> {
    ensure_private_directory(runtime)?;

    let directory = runtime.join(DIRECTORY_NAME);
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("could not create {}: {error}", directory.display())),
    }
    ensure_private_directory(&directory)?;
    Ok(directory)
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("{} is not a real directory", path.display()));
    }
    if metadata.uid() != get_current_uid() {
        return Err(format!(
            "{} is not owned by the active user",
            path.display()
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!("{} is accessible by another user", path.display()));
    }
    Ok(())
}

fn ensure_private_regular_file(file: &File, path: &Path) -> Result<(), String> {
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.uid() != get_current_uid() {
        return Err(format!("{} is not a private regular file", path.display()));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(format!("{} is accessible by another user", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::{AcquireError, InstanceGuard};

    #[test]
    fn only_one_instance_can_own_a_private_runtime_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))?;

        let mut first = InstanceGuard::acquire_in(temporary.path())?;
        assert!(matches!(
            InstanceGuard::acquire_in(temporary.path()),
            Err(AcquireError::AlreadyRunning)
        ));
        assert!(!InstanceGuard::active_session_is_locked_in(
            temporary.path()
        )?);
        first.mark_session_locked()?;
        assert!(InstanceGuard::active_session_is_locked_in(
            temporary.path()
        )?);
        drop(first);
        assert!(InstanceGuard::acquire_in(temporary.path()).is_ok());
        Ok(())
    }
}
