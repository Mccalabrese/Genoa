//! Read-only Waybar status for the managed dnscrypt-proxy toggle.

use anyhow::{Context, Result};
use cloudflare_toggle::dnscrypt_proxy_is_active;
use serde::Deserialize;
use serde_json::json;
use std::fs;

#[derive(Deserialize)]
struct Config {
    text_on: String,
    class_on: String,
    text_off: String,
    class_off: String,
}

#[derive(Deserialize)]
struct GlobalConfig {
    cloudflare_toggle: Config,
}

fn load_config() -> Result<Config> {
    let config_path = dirs::home_dir()
        .context("Cannot find home dir")?
        .join(".config/rust-dotfiles/config.toml");
    let config_str = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read {}", config_path.display()))?;
    Ok(toml::from_str::<GlobalConfig>(&config_str)
        .context("Failed to parse config.toml. Check for syntax errors.")?
        .cloudflare_toggle)
}

fn main() -> Result<()> {
    let config = load_config().ok();
    let enabled = dnscrypt_proxy_is_active();
    let (text, class, tooltip) = if enabled {
        (
            config.as_ref().map_or("ON", |c| &c.text_on),
            config.as_ref().map_or("on", |c| &c.class_on),
            "Cloudflare DNS: active (managed by NetworkManager)",
        )
    } else {
        (
            config.as_ref().map_or("OFF", |c| &c.text_off),
            config.as_ref().map_or("off", |c| &c.class_off),
            "Cloudflare DNS: inactive (direct Cloudflare DNS via NetworkManager)",
        )
    };
    println!(
        "{}",
        json!({ "text": text, "class": class, "tooltip": tooltip })
    );
    Ok(())
}
