//! Wallpaper Executor (wp-apply)
//!
//! A specialized utility responsible for the side-effects of changing the desktop background.
//! It abstracts away the differences between Wayland compositors (Hyprland, Sway, Niri)
//! so the selection tool doesn't need to know the implementation details.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use anyhow::{Context, Result};
use std::fs;
use serde::Deserialize;

/// Resolves shell-style paths (e.g., "~/Pictures") to absolute system paths.
fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct WallpaperManagerConfig {
    swww_params: Vec<String>,        // Transition effects for swww
    swaybg_cache_file: String,       // Where Sway stores its current state
    hyprland_refresh_script: String, // Hook to reload Hyprland colors (e.g., Pywal)
    wallpaper_dir: String,
}

#[derive(Deserialize, Debug)]
struct GlobalConfig {
    wallpaper_manager: WallpaperManagerConfig,
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

// Helper to ensure competing wallpaper daemons are killed before starting a new one.
fn pkill(name: &str) {
    Command::new("pkill").arg("-x").arg(name).status().ok();
}

// --- Compositor Strategies ---

/// Applies wallpaper using `awww` (Solution for Hyprland/Niri).
/// Supports animated transitions and per-monitor namespaces.
fn apply_awww_wallpaper(selected_file: &Path, monitor: &str, namespace: &str, swww_params: &[String]) -> Result<()> {
    println!("Applying wallpaper via awww (namespace: {})...", namespace);
    // Clean up incompatible daemons
    pkill("mpvpaper");
    pkill("swaybg");
    // Ensure the daemon is running in the background
    let _ = Command::new("awww-daemon")
        .arg("--namespace")
        .arg(namespace)
        .arg("--format")
        .arg("argb")
        .spawn();
    // Wait briefly for daemon startup race conditions
    std::thread::sleep(std::time::Duration::from_millis(100));
    // Send the image command
    Command::new("awww")
        .arg("img") 
        .arg("--namespace")
        .arg(namespace)
        .arg("-o")
        .arg(monitor)
        .arg(selected_file)
        .args(swww_params)
        .status()
        .context("awww img command failed")?;
    Ok(())
}
/// Applies wallpaper using `swaybg` (Solution for Sway).
/// Swaybg is static and requires manual process management.
fn apply_sway_wallpaper(selected_file: &Path, monitor: &str, cache_filename: &str) -> Result<()> {
    println!("Applying wallpaper for Sway...");
    // Kill awww as it conflicts with swaybg
    pkill("awww-daemon");
    pkill("hyprpaper");
    Command::new("swaybg")
        .arg("-o")
        .arg(monitor)
        .arg("-i")
        .arg(selected_file)
        .spawn()
        .context("Failed to run swaybg")?;

    // Cache the selection so Sway can restore it on reboot (handled by external startup scripts)
    if let Some(mut cache_path) = dirs::cache_dir() {
        cache_path.push(cache_filename);
        let _ = fs::write(cache_path, selected_file.to_str().unwrap_or(""));
    }

    Ok(())
}

fn main() -> Result<()> {
    let global_config = load_config()?;
    let config = global_config.wallpaper_manager;
    // Parse CLI arguments passed by `wp-select`
    let args: Vec<String> = env::args().collect();
    let wallpaper_path_str = args.get(1).context("Missing wallpaper path")?;
    let compositor = args.get(2).context("Missing compositor name")?;
    let monitor = args.get(3).context("Missing monitor name")?;

    let wallpaper_path = PathBuf::from(wallpaper_path_str);

    // Strategy Pattern: Dispatch based on the detected environment
    match compositor.as_str() {
        "hyprland" => {
            apply_awww_wallpaper(&wallpaper_path, monitor, "hypr", &config.swww_params)?;
            // Trigger hook to update system colors (e.g. Waybar styles)
            let refresh_script = expand_path(&config.hyprland_refresh_script);
            Command::new("bash").arg(refresh_script).status()?;
        }
        "niri" => {
            // Niri uses the same backend (awww) but a isolated namespace
            apply_awww_wallpaper(&wallpaper_path, monitor, "niri", &config.swww_params)?;
        }
        "sway" => {
            apply_sway_wallpaper(&wallpaper_path, monitor, &config.swaybg_cache_file)?;
        }
        _ => anyhow::bail!("Compositor argument '{}' is not recognized.", compositor),
    }

    Ok(())
}
