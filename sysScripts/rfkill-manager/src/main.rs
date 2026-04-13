//! Rfkill Manager (rfkill-manager)
//! Zero-Config Version
//!
//! Responsibilities:
//! 1. Check/Toggle Airplane Mode via `rfkill`.
//! 2. Output simple JSON for bars (class: "on"/"off").
//! 3. Send system notification on toggle.
//! 4. Signal Waybar (SIGRTMIN+10) to update immediately.

use anyhow::{Context, Result, anyhow};
use notify_rust::Notification;
use serde_json::json;
use std::env;
use std::process::Command;

// --- HARDCODED DEFAULTS ---
// No need to configure these. They are standard.
const WAYBAR_SIGNAL: i32 = 10;
const NOTIFICATION_ICON: &str = "airplane-mode-symbolic"; // Uses system theme icon

// --- System Logic ---

/// Queries `rfkill`. Returns true if Airplane Mode is ON (Soft blocked).
fn is_blocked() -> Result<bool> {
    let output = Command::new("rfkill")
        .arg("list")
        .arg("all")
        .output()
        .context("Failed to run 'rfkill list'")?;

    if !output.status.success() {
        return Err(anyhow!(
            "rfkill failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.contains("Soft blocked: yes"))
}

// --- Modes ---

fn run_status() -> Result<()> {
    let blocked = is_blocked().unwrap_or(false);

    // Simple output. The Sidebar/Waybar handles the visuals via CSS classes (.on / .off)
    let class = if blocked { "on" } else { "off" };
    let text = if blocked { "✈" } else { "" };
    let tooltip = if blocked {
        "Airplane Mode: Active"
    } else {
        "Airplane Mode: Inactive"
    };

    println!(
        "{}",
        json!({
            "text": text,
            "class": class,
            "tooltip": tooltip
        })
    );
    Ok(())
}

fn run_toggle() -> Result<()> {
    let blocked = is_blocked().context("Failed to check state")?;
    let (action, body) = if blocked {
        ("unblock", "Airplane Mode: OFF")
    } else {
        ("block", "Airplane Mode: ON")
    };

    // 1. Execute
    let status = Command::new("rfkill").arg(action).arg("all").status()?;
    if !status.success() {
        return Err(anyhow!("Failed to {}", action));
    }

    // 2. Notify
    let _ = Notification::new()
        .summary("Network Manager")
        .body(body)
        .icon(NOTIFICATION_ICON)
        .show();

    // 3. Signal Waybar (Harmless if Waybar isn't running)
    // Refreshes the icon instantly without waiting for poll interval
    let sig_rtmin = 34;
    let signal = sig_rtmin + WAYBAR_SIGNAL;
    let _ = Command::new("pkill")
        .arg(format!("-{}", signal))
        .arg("-x")
        .arg("waybar")
        .status();

    Ok(())
}

// --- Main ---

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("--status") => run_status(),
        Some("--toggle") | None => {
            if let Err(e) = run_toggle() {
                eprintln!("Error: {}", e);
                let _ = Notification::new()
                    .summary("Error")
                    .body(&e.to_string())
                    .show();
            }
            Ok(())
        }
        _ => {
            println!("Usage: rfkill-manager [--status | --toggle]");
            Ok(())
        }
    }
}
