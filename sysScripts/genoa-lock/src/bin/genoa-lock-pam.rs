#![allow(unsafe_code)]

#[path = "../pam.rs"]
mod pam;
#[path = "../protocol.rs"]
mod protocol;

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::fd::AsFd,
    process::ExitCode,
};

use rustix::{
    io::fcntl_dupfd_cloexec,
    process::{DumpableBehavior, set_dumpable_behavior},
    stdio::{dup2_stderr, dup2_stdin, dup2_stdout},
};
use zeroize::Zeroizing;

use protocol::{MAX_PASSWORD_BYTES, REQUEST_MAGIC, RESPONSE_DENY, RESPONSE_ERROR, RESPONSE_OK};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

fn run() -> Result<(), ()> {
    set_dumpable_behavior(DumpableBehavior::NotDumpable).map_err(|_| ())?;
    close_inherited_descriptors()?;
    let result = fcntl_dupfd_cloexec(std::io::stdout(), 3).map_err(|_| ())?;
    let password = read_password()?;
    redirect_standard_streams()?;
    let response = match pam::authenticate_current_user(password) {
        Ok(true) => RESPONSE_OK,
        Ok(false) => RESPONSE_DENY,
        Err(_) => RESPONSE_ERROR,
    };
    let mut result = File::from(result);
    result.write_all(&response).map_err(|_| ())
}

fn close_inherited_descriptors() -> Result<(), ()> {
    // SAFETY: this helper has no required descriptors above stderr before it
    // duplicates its private result channel. The syscall only closes this
    // process' descriptors and receives plain integer bounds.
    let result = unsafe { libc::close_range(3, u32::MAX, 0) };
    if result == 0 { Ok(()) } else { Err(()) }
}

fn read_password() -> Result<Zeroizing<String>, ()> {
    let mut input = std::io::stdin().lock();
    let mut magic = [0_u8; REQUEST_MAGIC.len()];
    input.read_exact(&mut magic).map_err(|_| ())?;
    if magic != REQUEST_MAGIC {
        return Err(());
    }
    let mut length = [0_u8; 2];
    input.read_exact(&mut length).map_err(|_| ())?;
    let length = usize::from(u16::from_be_bytes(length));
    if length > MAX_PASSWORD_BYTES {
        return Err(());
    }
    let mut bytes = Zeroizing::new(vec![0_u8; length]);
    input.read_exact(&mut bytes[..]).map_err(|_| ())?;
    drop(input);
    match String::from_utf8(std::mem::take(&mut *bytes)) {
        Ok(password) => Ok(Zeroizing::new(password)),
        Err(error) => {
            drop(Zeroizing::new(error.into_bytes()));
            Err(())
        }
    }
}

fn redirect_standard_streams() -> Result<(), ()> {
    let null = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .map_err(|_| ())?;
    dup2_stdin(null.as_fd()).map_err(|_| ())?;
    dup2_stdout(null.as_fd()).map_err(|_| ())?;
    dup2_stderr(null.as_fd()).map_err(|_| ())
}
