use crate::app::Config;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, DirBuilder, File, Permissions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tempfile::Builder as TempFileBuilder;

// Struct to parse the central TOML
#[derive(Deserialize)]
struct GlobalConfig {
    waybar_finance: Option<FinanceConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum StockConfig {
    Legacy(Option<Vec<String>>),
    V2(Vec<StockStruct>),
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq)]
pub struct StockStruct {
    pub symbol: String,
    #[serde(default = "set_sidebar_default")]
    pub sidebar: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ParsedConfig {
    api_key: String,
    stocks: Option<StockConfig>,
}

#[derive(Deserialize)]
struct FinanceConfig {
    api_key: String,
    stocks: Option<StockConfig>,
}

fn set_sidebar_default() -> bool {
    true
}
/// Resolves the XDG-compliant configuration path.
/// Usually ~/.config/waybar-finance/config.json on Linux.
pub fn get_config_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("Could not find config directory")?;
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
        if let Ok(parsed) = serde_json::from_str::<ParsedConfig>(&content) {
            let unified_stocks: Vec<StockStruct> = match parsed.stocks {
                Some(StockConfig::Legacy(stocks)) => stocks
                    .unwrap_or_default()
                    .into_iter()
                    .map(|s| StockStruct {
                        symbol: s,
                        sidebar: true,
                    })
                    .collect(),
                Some(StockConfig::V2(stocks)) => stocks,
                _ => {
                    vec![
                        StockStruct {
                            symbol: "SPY".into(),
                            sidebar: true,
                        },
                        StockStruct {
                            symbol: "QQQ".into(),
                            sidebar: true,
                        },
                        StockStruct {
                            symbol: "BTC-USD".into(),
                            sidebar: true,
                        },
                    ]
                }
            };
            return Ok(Config {
                api_key: Some(parsed.api_key),
                stocks: unified_stocks,
            });
        }
    }

    // 2. Try Central TOML (Installer provided)
    if let Some(central_path) = get_central_config_path()
        && central_path.exists()
        && let Ok(content) = fs::read_to_string(&central_path)
        && let Ok(global) = toml::from_str::<GlobalConfig>(&content)
        && let Some(finance) = global.waybar_finance
    {
        let unified_stocks: Vec<StockStruct> = match finance.stocks {
            Some(StockConfig::Legacy(stocks)) => stocks
                .unwrap_or_default()
                .into_iter()
                .map(|s| StockStruct {
                    symbol: s,
                    sidebar: true,
                })
                .collect(),
            Some(StockConfig::V2(stocks)) => stocks,
            _ => {
                vec![
                    StockStruct {
                        symbol: "SPY".into(),
                        sidebar: true,
                    },
                    StockStruct {
                        symbol: "QQQ".into(),
                        sidebar: true,
                    },
                    StockStruct {
                        symbol: "BTC-USD".into(),
                        sidebar: true,
                    },
                ]
            }
        };
        return Ok(Config {
            api_key: Some(finance.api_key),
            stocks: unified_stocks,
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
        create_private_config_dir(parent)?;
    }
    write_private_config(&config_path, &json)?;
    Ok(())
}

fn create_private_config_dir(path: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder
        .recursive(true)
        .mode(0o700)
        .create(path)
        .context("Failed to create private config directory")
}

/// Writes a configuration containing the Finnhub API key without an insecure
/// creation window, then atomically replaces the previous complete file.
fn write_private_config(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("Configuration path does not have a parent directory")?;
    let mut temp_file = TempFileBuilder::new()
        .prefix(".config.json.")
        .permissions(Permissions::from_mode(0o600))
        .tempfile_in(parent)
        .context("Failed to create private temporary config")?;
    temp_file
        .write_all(content.as_bytes())
        .context("Failed to write private temporary config")?;
    temp_file
        .as_file()
        .sync_all()
        .context("Failed to flush private temporary config")?;
    temp_file
        .persist(path)
        .map_err(|error| error.error)
        .context("Failed to atomically replace config file")?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .context("Failed to flush config directory")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_config_replacement_preserves_strict_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let config_dir = temp.path().join("waybar-finance");
        let config_path = config_dir.join("config.json");

        create_private_config_dir(&config_dir).unwrap();
        write_private_config(&config_path, "old key").unwrap();
        write_private_config(&config_path, "new key").unwrap();

        assert_eq!(fs::read_to_string(&config_path).unwrap(), "new key");
        assert_eq!(
            fs::metadata(&config_dir).unwrap().permissions().mode() & 0o077,
            0
        );
        assert_eq!(
            fs::metadata(&config_path).unwrap().permissions().mode() & 0o077,
            0
        );
    }
}
