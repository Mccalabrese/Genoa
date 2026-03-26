use std::fs;
use std::path::PathBuf;
use anyhow::{Result, Context};
use serde::Deserialize;
use crate::app::Config;

// Struct to parse the central TOML
#[derive(Deserialize)]
struct GlobalConfig {
    waybar_finance: Option<FinanceConfig>,
}

#[derive(Deserialize)]
struct FinanceConfig {
    api_key: String,
    stocks: Option<Vec<String>>,
}

/// Resolves the XDG-compliant configuration path.
/// Usually ~/.config/waybar-finance/config.json on Linux.
pub fn get_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .context("Could not find config directory")?;
    Ok(config_dir.join("waybar-finance/config.json"))
}
///fallback to config for rust-dotfiles
pub fn get_central_config_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config/rust-dotfiles/config.toml"))
}
/// Loads the configuration from disk.
/// Returns a default configuration if the file does not exist.
pub fn load_config(path: &PathBuf) -> Result<Config> {
    // 1. Try Local JSON first (App specific overrides)
    if path.exists() {
        let content = fs::read_to_string(path).context("Failed to read config file")?;
        if let Ok(config) = serde_json::from_str::<Config>(&content)
            && config.api_key.is_some() {
                return Ok(config);
            }
    }

    // 2. Try Central TOML (Installer provided)
    if let Some(central_path) = get_central_config_path()
        && central_path.exists()
            && let Ok(content) = fs::read_to_string(&central_path)
                && let Ok(global) = toml::from_str::<GlobalConfig>(&content)
                    && let Some(finance) = global.waybar_finance {
                        return Ok(Config {
                            api_key: Some(finance.api_key),
                            stocks: finance.stocks.unwrap_or_else(|| vec![
                                "SPY".into(), "QQQ".into(), "BTC-USD".into()
                            ]),
                        });
                    }

    // 3. Fallback to defaults
    Ok(Config::default())
}
/// Persists the current application state to config.json.
/// This handles creating the directory structure if it doesn't exist (first run).
pub fn save_config(config: &Config) -> Result<()> {
    let config_path = get_config_path()?;
    let json = serde_json::to_string_pretty(config).context("Failed to serialize config")?;
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).context("Failed to create config directory")?;
    }
    fs::write(config_path, json).context("Failed to write config file")?;
    Ok(())
}
