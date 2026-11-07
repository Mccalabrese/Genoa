//! A deliberately narrow Wayland session locker.
//!
//! Keep this binary boring. It owns the lock/unlock protocol and authentication
//! only; desktop policy belongs in the caller and presentation belongs in a
//! future, separately reviewed change.

#![deny(unsafe_op_in_unsafe_fn)]

mod auth;
mod input;
mod password;
mod wayland;

use std::process::ExitCode;

use wayland::LockOptions;

fn main() -> ExitCode {
    match parse_options().and_then(wayland::run) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("genoa-lock: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_options() -> Result<LockOptions, String> {
    let mut options = LockOptions::default();

    for argument in std::env::args_os().skip(1) {
        match argument.to_str() {
            Some("--suspend-after-lock") => options.suspend_after_lock = true,
            Some("--help" | "-h") => {
                println!("Usage: genoa-lock [--suspend-after-lock]");
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

    Ok(options)
}
