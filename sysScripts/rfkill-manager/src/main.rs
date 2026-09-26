//! Rfkill Manager (rfkill-manager)
//! Zero-Config Version
//!
//! Responsibilities:
//! 1. Check/Toggle Airplane Mode via `rfkill`.
//! 2. Output simple JSON for bars (class: "on"/"off").
//! 3. Send system notification on toggle.
//! 4. Signal Waybar (SIGRTMIN+10) to update immediately.

use anyhow::{Context, Result, bail};
use notify_rust::Notification;
use serde::{Deserialize, Serialize};
use std::env;
use std::process::Command;

// --- HARDCODED DEFAULTS ---
// No need to configure these. They are standard.
const WAYBAR_SIGNAL: i32 = 10;
const NOTIFICATION_ICON: &str = "airplane-mode-symbolic"; // Uses system theme icon
const RFKILL_PATH: &str = "/usr/bin/rfkill";

#[derive(Debug, Deserialize)]
struct RfkillResponse {
    #[serde(default)]
    rfkilldevices: Vec<RfkillDevice>,
}

#[derive(Debug, Deserialize)]
struct RfkillDevice {
    soft: String,
}

#[derive(Debug, Serialize)]
struct StatusOutput {
    text: &'static str,
    class: &'static str,
    tooltip: &'static str,
}

// --- System Logic ---

/// Queries rfkill's stable JSON output. Returns true if any radio is soft
/// blocked, which is the state controlled by Airplane Mode.
fn is_blocked() -> Result<bool> {
    let output = Command::new(RFKILL_PATH)
        .args(["--json", "--output", "TYPE,SOFT"])
        .output()
        .context("Failed to run rfkill")?;

    if !output.status.success() {
        bail!(
            "rfkill failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    if !output.stderr.is_empty() {
        bail!(
            "rfkill reported an error: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    blocked_from_json(&output.stdout)
}

fn blocked_from_json(stdout: &[u8]) -> Result<bool> {
    let response: RfkillResponse =
        serde_json::from_slice(stdout).context("rfkill returned invalid JSON")?;
    Ok(response
        .rfkilldevices
        .iter()
        .any(|device| device.soft.eq_ignore_ascii_case("blocked")))
}

fn status_output(blocked: bool) -> StatusOutput {
    if blocked {
        StatusOutput {
            text: "✈",
            class: "on",
            tooltip: "Airplane Mode: Active",
        }
    } else {
        StatusOutput {
            text: "",
            class: "off",
            tooltip: "Airplane Mode: Inactive",
        }
    }
}

fn print_status(blocked: bool) -> Result<()> {
    println!("{}", serde_json::to_string(&status_output(blocked))?);
    Ok(())
}

// --- Modes ---

fn run_status() -> Result<()> {
    print_status(is_blocked()?)
}

fn run_toggle() -> Result<bool> {
    let blocked = is_blocked().context("Failed to check state")?;
    let (action, expected_blocked, body) = if blocked {
        ("unblock", false, "Airplane Mode: OFF")
    } else {
        ("block", true, "Airplane Mode: ON")
    };

    let status = Command::new(RFKILL_PATH)
        .args([action, "all"])
        .status()
        .with_context(|| format!("Failed to run rfkill {action} all"))?;
    if !status.success() {
        bail!("rfkill {action} all failed with {status}");
    }

    let observed_blocked = is_blocked().context("Failed to verify rfkill state after toggle")?;
    if observed_blocked != expected_blocked {
        bail!(
            "rfkill {action} all completed, but Airplane Mode is still {}",
            if observed_blocked {
                "active"
            } else {
                "inactive"
            }
        );
    }

    // Notify only after the command's resulting state has been verified.
    let _ = Notification::new()
        .summary("Network Manager")
        .body(body)
        .icon(NOTIFICATION_ICON)
        .show();

    // Signal Waybar (harmless if Waybar is not running).
    // Refreshes the icon instantly without waiting for poll interval
    let sig_rtmin = 34;
    let signal = sig_rtmin + WAYBAR_SIGNAL;
    let _ = Command::new("pkill")
        .arg(format!("-{}", signal))
        .arg("-x")
        .arg("waybar")
        .status();

    Ok(observed_blocked)
}

// --- Main ---

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("--status") => run_status(),
        Some("--toggle") | None => {
            let blocked = match run_toggle() {
                Ok(blocked) => blocked,
                Err(error) => {
                    let _ = Notification::new()
                        .summary("Airplane Mode Error")
                        .body(&error.to_string())
                        .show();
                    return Err(error);
                }
            };
            print_status(blocked)
        }
        _ => {
            println!("Usage: rfkill-manager [--status | --toggle]");
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_status_detects_a_soft_block() {
        let json = br#"{"rfkilldevices":[{"type":"wlan","soft":"unblocked"},{"type":"bluetooth","soft":"blocked"}]}"#;
        assert!(blocked_from_json(json).unwrap());
    }

    #[test]
    fn json_status_handles_an_empty_device_list() {
        assert!(!blocked_from_json(br#"{"rfkilldevices":[]}"#).unwrap());
    }

    #[test]
    fn malformed_json_is_an_error_not_an_inactive_state() {
        assert!(blocked_from_json(b"not json").is_err());
    }
}
