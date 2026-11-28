//! Wallpaper Executor (wp-apply)
//!
//! A specialized utility responsible for the side-effects of changing the desktop background.
//! It abstracts away the differences between Wayland compositors (Hyprland, Sway, Niri)
//! so the selection tool doesn't need to know the implementation details.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct WallpaperManagerConfig {
    swww_params: Vec<String>,  // Transition effects for swww
    swaybg_cache_file: String, // Where Sway stores its current state
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

    let config_str = fs::read_to_string(&config_path).with_context(|| {
        format!(
            "Failed to read config file from path: {}",
            config_path.display()
        )
    })?;

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
fn apply_awww_wallpaper(
    selected_file: &Path,
    monitor: &str,
    namespace: &str,
    swww_params: &[String],
) -> Result<()> {
    println!("Applying wallpaper via awww (namespace: {})...", namespace);
    // Clean up incompatible daemons
    pkill("mpvpaper");
    pkill("swaybg");
    let status = Command::new("awww")
        .arg("img")
        .arg("--namespace")
        .arg(namespace)
        .arg("--outputs")
        .arg(monitor)
        .arg(selected_file)
        .args(swww_params)
        .status()
        .context("awww img command failed")?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("awww img exited with {status}")
    }
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

fn apply_wallpaper(
    selected_file: &Path,
    compositor: &str,
    monitor: &str,
    config: &WallpaperManagerConfig,
) -> Result<()> {
    match compositor {
        "niri" => apply_awww_wallpaper(selected_file, monitor, "niri", &config.swww_params),
        "sway" => apply_sway_wallpaper(selected_file, monitor, &config.swaybg_cache_file),
        _ => anyhow::bail!("Compositor argument '{compositor}' is not recognized"),
    }
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

    if monitor == "--all" {
        anyhow::bail!("an explicit output is required");
    }
    apply_wallpaper(&wallpaper_path, compositor, monitor, &config)?;

    Ok(())
}
