//! Cloudflare DNS Toggler (cf-toggle)
//!
//! A secure wrapper for toggling system-level DNS-over-HTTPS settings.
//!
//! Architecture:
//! 1. **User Mode:** When run by a normal user (e.g., clicking Waybar), it detects the current state 
//!    and re-executes *itself* using `pkexec` to gain root privileges.
//! 2. **Root Mode:** When executed with root privileges (via pkexec), it modifies `/etc/resolv.conf`
//!    and manages the `systemd` service.
//!
//! This design avoids needing `sudo` in scripts or storing passwords.

use std::env;
use std::fs;
use std::process::Command;
use anyhow::{Context, Result};
use serde::Deserialize; 

// --- Configuration ---
// Deserialize the full config struct even if we don't use all fields in this binary,
// ensuring we validate the schema correctness early.
#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct Config {
    // JSON Output fields (Used by cf-status)
    text_on: String,
    class_on: String,
    text_off: String,
    class_off: String,
    // Logic fields (Used by cf-toggle)
    resolv_content_on: String,   // e.g. "nameserver 127.0.0.1"
    resolv_content_off: String,  // e.g. "nameserver 1.1.1.1"
    bar_process_name: String,    // "waybar"
    bar_signal_num: i32,         // Signal offset
    service_name: String,
}

#[derive(Deserialize, Debug)]
struct GlobalConfig {
    cloudflare_toggle: Config,
}

fn load_config() -> Result<GlobalConfig> {
    let config_path = dirs::home_dir()
        .context("Cannot find home dir")?
        .join(".config/rust-dotfiles/config.toml");
    let config_str = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file from path: {}", config_path.display()))?;
    let config: GlobalConfig = toml::from_str(&config_str)
        .context("Failed to parse config.toml. Check for syntax errors.")?;
    Ok(config)
}

// --- User Mode (Phase 1) ---

/// The entry point for the standard user.
/// Determines the desired state change and requests Root access to perform it.
fn run_as_user() -> Result<()> {
    let config = load_config()
        .context("Failed to load config for user")?
        .cloudflare_toggle;

    // Check current service status to toggle it
    let is_running = Command::new("systemctl")
        .arg("is-active")
        .arg(&config.service_name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    
    let mode = if is_running { "--stop" } else { "--start" };
    let content_on = &config.resolv_content_on;
    let content_off = &config.resolv_content_off;
    // Self-Reference: Find where this binary lives so we can execute it as root
    let self_exe = env::current_exe()
        .context("Failed to get path to own executable")?;

    // Privilege Escalation
    // We pass the config values as arguments to the root process so the root process
    // doesn't have to try and locate/read the user's home directory config file.
    let status = Command::new("pkexec")
        .arg(self_exe)
        .arg(mode)
        .arg(&config.service_name)
        .arg(content_on)
        .arg(content_off)
        .status()
        .context("Failed to run pkexec")?;

    // Signal Waybar to refresh status immediately on success
    if status.success() {
        let sig_base = 34;
        let signal = sig_base + config.bar_signal_num;
        let _ = Command::new("pkill")
            .arg(format!("-{}", signal))
            .arg("-x")
            .arg(&config.bar_process_name)
            .status();
    }
    Ok(())
}

// --- Root Mode (Phase 2) ---

/// The privileged worker.
/// This function only runs when `pkexec` invokes this binary.
/// It has permission to write to /etc/ and control systemd.
fn run_as_root(mode: &str, service_name: &str, content_on: &str, content_off: &str) -> Result<()> {
    if mode == "--start" {
        // Enable service
        Command::new("systemctl")
            .arg("enable")
            .arg("--now")
            .arg(service_name)
            .status()?
            .success()
            .then_some(())
            .context("Failed to start systemctl service")?;

        // Overwrite DNS
        fs::write("/etc/resolv.conf", content_on)
            .context("Failed to write /etc/resolv.conf")?;

    } else if mode == "--stop" {
        // Disable Service
        Command::new("systemctl")
            .arg("disable")
            .arg("--now")
            .arg(service_name)
            .status()?
            .success()
            .then_some(())
            .context("Failed to stop systemctl service")?;
        
        // Restore DNS
        fs::write("/etc/resolv.conf", content_off)
            .context("Failed to write /etc/resolv.conf")?;
    }
    Ok(())
}

// --- Main Dispatcher ---
fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    // Detect Mode based on arguments
    // Argument order from run_as_user:
    // [0]: binary_path
    // [1]: mode (--start / --stop)
    // [2]: service_name
    // [3]: content_on
    // [4]: content_off

    if args.len() > 1 {
        let mode = &args[1];

        // Safety check: verify we have enough arguments
        if args.len() < 5 {
            eprintln!("Internal Error: Missing arguments for root mode.");
            // We return Ok here to avoid panic, but the operation fails silently.
            return Ok(());
        }

        let service_name = &args[2];
        let content_on = &args[3];
        let content_off = &args[4];

        run_as_root(mode, service_name, content_on, content_off)
    } else {
        // No arguments? We are the user clicking the button.
        run_as_user()
    }
}
