//! A deliberately narrow Wayland session locker.
//!
//! Keep this binary boring. It owns the lock/unlock protocol and authentication
//! only; desktop policy belongs in the caller and presentation belongs in a
//! future, separately reviewed change.

#![deny(unsafe_op_in_unsafe_fn)]

mod auth;
mod input;
mod instance;
mod password;
mod protocol;
mod visual;
mod wayland;

use std::{process::ExitCode, time::Duration};

use rustix::process::{DumpableBehavior, set_dumpable_behavior};
use wayland::LockOptions;

enum Command {
    Lock(LockOptions),
    WaitUntilLocked,
}

fn main() -> ExitCode {
    let result = set_dumpable_behavior(DumpableBehavior::NotDumpable)
        .map_err(|error| format!("could not disable process dumps: {error}"))
        .and_then(|()| parse_command())
        .and_then(|command| match command {
            Command::Lock(options) => wayland::run(options),
            Command::WaitUntilLocked => {
                instance::InstanceGuard::wait_until_locked(Duration::from_secs(4))
            }
        });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("genoa-lock: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_command() -> Result<Command, String> {
    let mut options = LockOptions::default();
    let mut wait_until_locked = false;

    for argument in std::env::args_os().skip(1) {
        match argument.to_str() {
            Some("--suspend-after-lock") => options.suspend_after_lock = true,
            Some("--wait-until-locked") => wait_until_locked = true,
            Some("--help" | "-h") => {
                println!("Usage: genoa-lock [--suspend-after-lock | --wait-until-locked]");
                std::process::exit(0);
            }
            Some("--version" | "-V") => {
                println!("genoa-lock {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            Some(other) => return Err(format!("unrecognised option: {other}")),
            None => return Err("options must be valid UTF-8".to_string()),
        }
    }

    if wait_until_locked {
        if options.suspend_after_lock {
            return Err(
                "--wait-until-locked cannot be combined with --suspend-after-lock".to_string(),
            );
        }
        Ok(Command::WaitUntilLocked)
    } else {
        Ok(Command::Lock(options))
    }
}
