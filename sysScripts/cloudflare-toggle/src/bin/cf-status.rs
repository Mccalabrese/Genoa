//! Cloudflare Status Monitor (cf-status)
//!
//! A read-only utility to poll the status of the Cloudflare DNS service.
//! Used by Waybar's `custom/script` module to display the current state.

use std::fs;
use std::process::Command;
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct Config {
    text_on: String,
    class_on: String,
    text_off: String,
    class_off: String,
    resolv_content_on: String,
    resolv_content_off: String,
    bar_process_name: String,
    bar_signal_num: i32,
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

fn main() -> Result<()> {
    let config = load_config().map(|gc| gc.cloudflare_toggle);
    
    // 1. Check Service State
    // systemctl is-active returns "active" (exit code 0) or "inactive" (exit code 3/4).
    let service_active = Command::new("systemctl")
        .arg("is-active")
        .arg(config.as_ref().map_or("dnscrypt-proxy", |c| &c.service_name))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    // 2. Read DNS Configuration
    // We display the actual content of resolv.conf in the tooltip for verification.
    let resolv_conf = fs::read_to_string("/etc/resolv.conf")
        .unwrap_or_else(|_| "Error reading /etc/resolv.conf".to_string());

    // 3. Determine UI State
    let (text, class, tooltip) = if service_active {
        (
            config.as_ref().map_or("ON", |c| &c.text_on),
            config.as_ref().map_or("on", |c| &c.class_on),
            format!("Cloudflared:Running\nresolv.conf: {}", resolv_conf.trim())
        )
    } else {
        (
            config.as_ref().map_or("OFF", |c| &c.text_off),
            config.as_ref().map_or("off", |c| &c.class_off),
            format!("Cloudflared: Stopped\nresolv.conf: {}", resolv_conf.trim())
        )
    };
    // 4. Output JSON
    println!("{}", json!({
        "text": text,
        "class": class,
        "tooltip": tooltip
    }));
    Ok(())
}
