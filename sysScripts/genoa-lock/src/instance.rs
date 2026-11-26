//! Per-session ownership for Genoa Lock.
//!
//! Every launcher reaches this guard: compositor hotkeys, the sidebar,
//! Waybar, and idle management. Keeping the guard in the locker avoids two
//! session-lock clients competing for the same keyboard focus.

use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use rustix::fs::{FlockOperation, flock};
use uzers::get_current_uid;

const DIRECTORY_NAME: &str = "genoa";
const LOCK_FILE_NAME: &str = "genoa-lock.lock";

pub struct InstanceGuard {
    _file: File,
}

impl InstanceGuard {
    pub fn acquire() -> Result<Self, String> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| {
                "XDG_RUNTIME_DIR is unavailable; cannot safely own the lock".to_string()
            })?;
        Self::acquire_in(&runtime)
    }

    fn acquire_in(runtime: &Path) -> Result<Self, String> {
        let directory = private_runtime_directory(runtime)?;
        let lock_path = directory.join(LOCK_FILE_NAME);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)
            .map_err(|error| format!("could not open {}: {error}", lock_path.display()))?;
        ensure_private_regular_file(&file, &lock_path)?;
        flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
            format!("another Genoa Lock instance already owns this session: {error}")
        })?;
        Ok(Self { _file: file })
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

    use super::InstanceGuard;

    #[test]
    fn only_one_instance_can_own_a_private_runtime_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))?;

        let first = InstanceGuard::acquire_in(temporary.path())?;
        assert!(InstanceGuard::acquire_in(temporary.path()).is_err());
        drop(first);
        assert!(InstanceGuard::acquire_in(temporary.path()).is_ok());
        Ok(())
    }
}
