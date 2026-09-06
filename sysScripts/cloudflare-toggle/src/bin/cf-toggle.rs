//! Toggle dnscrypt-proxy through NetworkManager's DNS configuration.

use anyhow::{Context, Result, bail};
use cloudflare_toggle::DnsManager;
use serde::Deserialize;
use std::env;
use std::fs;
use std::process::Command;

#[derive(Deserialize)]
struct Config {
    bar_process_name: String,
    bar_signal_num: i32,
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

fn run_as_user() -> Result<()> {
    let config = load_config().context("Failed to load config for user")?;
    let self_exe = env::current_exe().context("Failed to get path to own executable")?;
    let status = Command::new("pkexec")
        .arg(self_exe)
        .arg("--toggle")
        .status()
        .context("Failed to run pkexec")?;
    if !status.success() {
        bail!("The DNS toggle was cancelled or failed");
    }

    let signal = 34 + config.bar_signal_num;
    let _ = Command::new("pkill")
        .arg(format!("-{signal}"))
        .arg("-x")
        .arg(&config.bar_process_name)
        .status();
    Ok(())
}

fn set_dns_mode(enabled: bool) -> Result<()> {
    let manager = DnsManager::default();
    manager.set_enabled(enabled)
}

fn toggle_as_root() -> Result<()> {
    let manager = DnsManager::default();
    manager.set_enabled(!manager.is_enabled())
}

fn main() -> Result<()> {
    match env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => run_as_user(),
        [mode] if mode == "--toggle" => toggle_as_root(),
        [mode] if mode == "--enable" => set_dns_mode(true),
        [mode] if mode == "--disable" => set_dns_mode(false),
        _ => bail!("Usage: cf-toggle [--toggle|--enable|--disable]"),
    }
}
