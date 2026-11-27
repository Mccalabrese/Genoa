use std::ffi::{CStr, CString};

use pam_client2::{Context, ConversationHandler, ErrorCode, Flag};
use uzers::get_current_username;
use zeroize::Zeroizing;

const PAM_SERVICE: &str = "genoa-lock";

pub fn authenticate_current_user(password: Zeroizing<String>) -> Result<bool, String> {
    let username = get_current_username()
        .ok_or_else(|| "could not determine the effective user's account name".to_string())?
        .into_string()
        .map_err(|_| "effective account name is not valid UTF-8".to_string())?;
    let mut client = Context::new(PAM_SERVICE, Some(&username), Credentials { password })
        .map_err(|_| "could not initialise the PAM service".to_string())?;

    match client.authenticate(Flag::NONE) {
        Ok(()) => {}
        Err(error) if is_denial(error.code()) => return Ok(false),
        Err(_) => return Err("the PAM authentication service is unavailable".to_string()),
    }
    match client.acct_mgmt(Flag::NONE) {
        Ok(()) => {}
        Err(error) if is_denial(error.code()) => return Ok(false),
        Err(_) => return Err("the PAM account service is unavailable".to_string()),
    }
    let _ = client.reinitialize_credentials(Flag::NONE);
    Ok(true)
}

const fn is_denial(code: ErrorCode) -> bool {
    matches!(
        code,
        ErrorCode::AUTH_ERR
            | ErrorCode::CRED_INSUFFICIENT
            | ErrorCode::PERM_DENIED
            | ErrorCode::USER_UNKNOWN
            | ErrorCode::MAXTRIES
            | ErrorCode::ACCT_EXPIRED
            | ErrorCode::NEW_AUTHTOK_REQD
    )
}

struct Credentials {
    password: Zeroizing<String>,
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
