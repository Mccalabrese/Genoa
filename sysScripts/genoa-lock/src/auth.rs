use std::ffi::{CStr, CString};

use pam_client2::{Context, ConversationHandler, ErrorCode, Flag};
use uzers::get_current_username;
use zeroize::Zeroizing;

const PAM_SERVICE: &str = "genoa-lock";

/// Authenticate the effective desktop user, never a name supplied by the
/// environment or command line.
pub fn authenticate_current_user(password: &Zeroizing<String>) -> Result<bool, String> {
    let username = effective_username()?;
    let conversation = Credentials {
        username: username.clone(),
        password: password.clone(),
    };
    let mut client = Context::new(PAM_SERVICE, Some(&username), conversation)
        .map_err(|_| format!("could not initialise PAM service {PAM_SERVICE:?}"))?;

    // Do not print PAM's result. PAM errors may reveal policy or account state.
    Ok(client.authenticate(Flag::NONE).is_ok() && client.acct_mgmt(Flag::NONE).is_ok())
}

fn effective_username() -> Result<String, String> {
    get_current_username()
        .ok_or_else(|| "could not determine the effective user's account name".to_string())?
        .into_string()
        .map_err(|_| "effective account name is not valid UTF-8".to_string())
}

struct Credentials {
    username: String,
    password: Zeroizing<String>,
}

impl ConversationHandler for Credentials {
    fn prompt_echo_on(&mut self, _: &CStr) -> Result<CString, ErrorCode> {
        CString::new(self.username.as_bytes()).map_err(|_| ErrorCode::CONV_ERR)
    }

    fn prompt_echo_off(&mut self, _: &CStr) -> Result<CString, ErrorCode> {
        CString::new(self.password.as_bytes()).map_err(|_| ErrorCode::CONV_ERR)
    }

    fn text_info(&mut self, _: &CStr) {}

    fn error_msg(&mut self, _: &CStr) {}
}
