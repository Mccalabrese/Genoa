//! Prevent a stale idle timeout from immediately suspending after resume.
//!
//! NVIDIA keeps user sessions runnable around sleep so it can preserve GPU
//! state. `swayidle` timers therefore continue to age while a laptop sleeps.
//! This helper records the cumulative suspend time when its idle service
//! starts, and refuses a timeout that has crossed a real sleep interval.

#![deny(unsafe_op_in_unsafe_fn)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    time::Duration,
};

use rustix::{
    fs::{FlockOperation, flock},
    time::{ClockId, Timespec, clock_gettime},
};
use uzers::get_current_uid;

const DIRECTORY_NAME: &str = "genoa";
const STATE_FILE_NAME: &str = "idle-suspend-baseline";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const MAX_STATE_BYTES: usize = 64;
const SUSPEND_DETECTION_TOLERANCE: Duration = Duration::from_secs(1);

fn main() -> ExitCode {
    let result = match std::env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => suspend_if_timer_is_current(),
        [flag] if flag == "--record-baseline" => record_baseline(),
        [flag] if flag == "--help" || flag == "-h" => {
            println!("Usage: genoa-idle-suspend [--record-baseline]");
            Ok(())
        }
        _ => Err("Usage: genoa-idle-suspend [--record-baseline]".to_string()),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("genoa-idle-suspend: {error}");
            ExitCode::FAILURE
        }
    }
}

fn record_baseline() -> Result<(), String> {
    let mut state = open_state_file()?;
    write_baseline(&mut state, cumulative_suspend_time()?)
}

fn suspend_if_timer_is_current() -> Result<(), String> {
    let mut state = open_state_file()?;
    let now = cumulative_suspend_time()?;
    let baseline = match read_baseline(&mut state) {
        Ok(baseline) => baseline,
        Err(error) => {
            eprintln!("genoa-idle-suspend: invalid idle baseline; resetting it: {error}");
            None
        }
    };

    if baseline.is_none_or(|baseline| timer_aged_through_suspend(baseline, now)) {
        write_baseline(&mut state, now)?;
        eprintln!("genoa-idle-suspend: discarded an idle timer that aged through suspend");
        return Ok(());
    }

    let status = Command::new(SYSTEMCTL)
        .arg("suspend")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| format!("could not start system suspend: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("systemctl suspend exited with {status}"))
    }
}

fn timer_aged_through_suspend(baseline: Duration, now: Duration) -> bool {
    now.saturating_sub(baseline) >= SUSPEND_DETECTION_TOLERANCE
}

fn cumulative_suspend_time() -> Result<Duration, String> {
    let boottime = duration_from_timespec(clock_gettime(ClockId::Boottime))?;
    let monotonic = duration_from_timespec(clock_gettime(ClockId::Monotonic))?;
    boottime
        .checked_sub(monotonic)
        .ok_or_else(|| "monotonic clock exceeded boottime clock".to_string())
}

fn duration_from_timespec(timespec: Timespec) -> Result<Duration, String> {
    let seconds = u64::try_from(timespec.tv_sec)
        .map_err(|_| "clock returned a negative seconds value".to_string())?;
    let nanoseconds = u32::try_from(timespec.tv_nsec)
        .map_err(|_| "clock returned an invalid nanoseconds value".to_string())?;
    Ok(Duration::new(seconds, nanoseconds))
}

fn open_state_file() -> Result<File, String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "XDG_RUNTIME_DIR is unavailable".to_string())?;
    open_state_file_in(&runtime)
}

fn open_state_file_in(runtime: &Path) -> Result<File, String> {
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

    let path = directory.join(STATE_FILE_NAME);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    ensure_private_regular_file(&file, &path)?;
    flock(&file, FlockOperation::NonBlockingLockExclusive)
        .map_err(|error| format!("another idle-suspend helper is running: {error}"))?;
    Ok(file)
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

fn read_baseline(file: &mut File) -> Result<Option<Duration>, String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("could not read idle baseline: {error}"))?;

    let mut bytes = [0_u8; MAX_STATE_BYTES + 1];
    let length = file
        .read(&mut bytes)
        .map_err(|error| format!("could not read idle baseline: {error}"))?;
    if length == 0 {
        return Ok(None);
    }
    if length > MAX_STATE_BYTES {
        return Err("idle baseline is too large".to_string());
    }

    let text = std::str::from_utf8(&bytes[..length])
        .map_err(|_| "idle baseline is not valid UTF-8".to_string())?;
    let nanoseconds = text
        .trim()
        .parse::<u64>()
        .map_err(|_| "idle baseline is not a duration".to_string())?;
    Ok(Some(Duration::from_nanos(nanoseconds)))
}

fn write_baseline(file: &mut File, baseline: Duration) -> Result<(), String> {
    let nanoseconds = u64::try_from(baseline.as_nanos())
        .map_err(|_| "idle baseline exceeds the supported range".to_string())?;
    file.set_len(0)
        .map_err(|error| format!("could not reset idle baseline: {error}"))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("could not write idle baseline: {error}"))?;
    file.write_all(nanoseconds.to_string().as_bytes())
        .map_err(|error| format!("could not write idle baseline: {error}"))?;
    file.sync_data()
        .map_err(|error| format!("could not save idle baseline: {error}"))
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::PermissionsExt, time::Duration};

    use super::{open_state_file_in, read_baseline, timer_aged_through_suspend, write_baseline};

    #[test]
    fn detects_idle_timers_that_crossed_a_real_suspend() {
        assert!(timer_aged_through_suspend(
            Duration::from_secs(5),
            Duration::from_secs(7)
        ));
    }

    #[test]
    fn ignores_clock_read_jitter_while_the_system_is_awake() {
        assert!(!timer_aged_through_suspend(
            Duration::from_secs(5),
            Duration::from_millis(5_500)
        ));
    }

    #[test]
    fn baseline_state_is_private_and_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let temporary = tempfile::tempdir()?;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))?;
        let mut state = open_state_file_in(temporary.path())?;

        write_baseline(&mut state, Duration::from_secs(42))?;
        assert_eq!(read_baseline(&mut state)?, Some(Duration::from_secs(42)));
        assert_eq!(state.metadata()?.permissions().mode() & 0o777, 0o600);
        Ok(())
    }
}
