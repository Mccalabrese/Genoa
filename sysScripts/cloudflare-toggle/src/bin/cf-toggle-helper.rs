//! Root-only DNS toggle helper invoked through a narrowly scoped Polkit action.
//!
//! This binary is installed as root:root at `/usr/libexec/genoa/cf-toggle-helper`.
//! It accepts one fixed operation, so the user-owned Waybar helper never
//! becomes privileged code.

use anyhow::{Result, bail};
use cloudflare_toggle::DnsManager;
use std::env;

fn is_toggle_action(args: &[String]) -> bool {
    matches!(args, [action] if action == "toggle")
}

fn main() -> Result<()> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if !is_toggle_action(&args) {
        bail!("Usage: cf-toggle-helper toggle");
    }
    let manager = DnsManager::default();
    manager.set_enabled(!manager.is_enabled())
}

#[cfg(test)]
mod tests {
    use super::is_toggle_action;

    #[test]
    fn only_the_fixed_toggle_subcommand_is_accepted() {
        assert!(is_toggle_action(&["toggle".to_string()]));
        assert!(!is_toggle_action(&[]));
        assert!(!is_toggle_action(&["enable".to_string()]));
        assert!(!is_toggle_action(&[
            "toggle".to_string(),
            "extra".to_string()
        ]));
    }
}
