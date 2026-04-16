//! Waybar Updates Module (waybar-updates)
//!
//! A lightweight utility to check for system updates (Pacman/Yay) and display the count in Waybar.
//!
//! Design Priorities:
//! 1. **Speed:** Checks must be fast to avoid blocking the bar startup.
//! 2. **Resilience:** If the check fails (e.g., no internet), it falls back to the last known cached count instead of crashing or showing "Error".
//! 3. **Visual Feedback:** Distinct JSON classes ("updates", "synced", "stale", "error") allow CSS styling in Waybar (e.g., turning red if stale).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    PathBuf::from(path)
}

// --- Config Models ---

#[derive(Deserialize, Debug)]
struct UpdateCheckConfig {
    command_string: String, // The shell command to count updates (e.g., "checkupdates | wc -l")
    cache_file: String,     // Path to store the last successful count
    stale_icon: String,     // Icon to append if data is old
    error_icon: String,     // Icon for total failure
}

#[derive(Deserialize, Debug)]
struct GlobalConfig {
    update_check: UpdateCheckConfig,
}

// --- Persistence Model ---
#[derive(Serialize, Deserialize, Debug)]
struct Cache {
    count: usize,
}

fn load_config() -> Result<GlobalConfig> {
    let config_path = dirs::home_dir()
        .context("Cannot find home dir")?
        .join(".config/rust-dotfiles/config.toml");

    let config_str = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config: {}", config_path.display()))?;

    let config: GlobalConfig =
        toml::from_str(&config_str).context("Failed to parse config.toml")?;

    Ok(config)
}

// --- Persistence Logic ---

fn read_cache(cache_path: &Path) -> Result<Cache> {
    let json_data = fs::read_to_string(cache_path).context("Failed to read cache file")?;
    let cache: Cache = serde_json::from_str(&json_data).context("Failed to parse cache JSON")?;
    Ok(cache)
}

fn save_cache(count: usize, cache_path: &Path) -> Result<()> {
    let cache = Cache { count };
    let json_data = serde_json::to_string(&cache)?;
    if let Some(parent) = cache_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(cache_path, json_data).context("Failed to write cache file")?;
    Ok(())
}

// --- Core Logic ---

/// Executes the update check command defined in config.toml.
/// Returns the number of updates found.
fn run_check(command_string: &str) -> Result<usize> {
    let output = Command::new("bash")
        .arg("-c")
        .arg(command_string)
        .output()
        .context(format!("Failed to spawn command: '{}'", command_string))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let count = stdout.trim().lines().count();
    // Exit Code 0: Success.
    if output.status.success() {
        return Ok(count);
    }

    // Exit Code 1: 'checkupdates' returns 1 if NO updates are found (not an error).
    // We handle this edge case specifically.
    if output.status.code() == Some(1) && count == 0 {
        return Ok(0);
    }
    // Any other exit code is a legitimate failure (e.g., DB lock, no network).
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::bail!(
        "Check command failed (exit code: {}):\n{}",
        output.status.code().unwrap_or(-1),
        stderr.trim()
    );
}

// --- Output Formatters (Waybar JSON Protocol) ---

/// Standard success output.
/// Classes: "updates" (if count > 0), "synced" (if 0).
fn print_success_json(count: usize) {
    if count > 0 {
        println!(
            "{}",
            json!({
                "text": count.to_string(),
                "tooltip": format!("{} Updates Available", count),
                "class": "updates"
            })
        );
    } else {
        println!(
            "{}",
            json!({
                "text": "0",
                "tooltip": "System is up to date",
                "class": "synced"
            })
        );
    }
}
/// Fallback output when the check fails but cache exists.
/// Class: "stale". Adds a visual indicator (icon) to the text.
fn print_stale_json(stale_count: usize, config: &UpdateCheckConfig) {
    println!(
        "{}",
        json!({
            "text": format!("{} {}", stale_count, config.stale_icon),
            "tooltip": format!(
                "Update check failed. Showing last known count: {}",
                stale_count
            ),
            "class": "stale"
        })
    );
}
/// Total failure output (Check failed AND Cache missing).
/// Class: "error".
fn print_error_json(config: &UpdateCheckConfig, error_msg: &str) {
    println!(
        "{}",
        json!({
            "text": config.error_icon.clone(),
            "tooltip": format!("Update check failed:\n{}", error_msg),
            "class": "error"
        })
    );
}

fn main() -> Result<()> {
    let config = match load_config() {
        Ok(global_config) => global_config.update_check,
        Err(e) => {
            // Output JSON even on crash so Waybar renders an error icon instead of vanishing
            println!(
                "{}",
                json!({
                    "text": "!",
                    "tooltip": format!("Failed to load config.toml:\n{}", e),
                    "class": "error"
                })
            );
            return Err(e);
        }
    };

    let cache_path = expand_path(&config.cache_file);
    // Strategy: Try Live Check -> Fallback to Cache -> Error
    match run_check(&config.command_string) {
        Ok(count) => {
            // Happy Path: Update cache and display fresh data
            if let Err(e) = save_cache(count, &cache_path) {
                eprintln!("Warning: Failed to save cache: {}", e);
            }
            print_success_json(count);
        }
        Err(check_err) => {
            // Check failed. Attempt recovery via cache.
            eprintln!("Update check failed: {}", check_err); // For debugging
            match read_cache(&cache_path) {
                Ok(cache) => {
                    print_stale_json(cache.count, &config);
                }
                Err(cache_err) => {
                    // Critical Failure
                    let combined_err =
                        format!("Check Error: {}\nCache Error: {}", check_err, cache_err);
                    print_error_json(&config, &combined_err);
                }
            }
        }
    }

    Ok(())
}
