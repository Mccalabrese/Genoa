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

#[derive(Deserialize)]
struct SwayMonitor {
    name: String,
    active: bool,
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

fn active_monitors(compositor: &str) -> Result<Vec<String>> {
    match compositor {
        "sway" => {
            let output = Command::new("swaymsg")
                .args(["-t", "get_outputs"])
                .output()
                .context("could not query Sway outputs")?;
            if !output.status.success() {
                anyhow::bail!("swaymsg failed while listing outputs");
            }
            let monitors = serde_json::from_slice::<Vec<SwayMonitor>>(&output.stdout)
                .context("could not parse Sway output list")?;
            Ok(monitors
                .into_iter()
                .filter(|monitor| monitor.active)
                .map(|monitor| monitor.name)
                .collect())
        }
        "niri" => {
            let output = Command::new("awww")
                .args(["query", "--namespace", "niri"])
                .output()
                .context("could not query Niri outputs")?;
            if !output.status.success() {
                anyhow::bail!("awww failed while listing Niri outputs");
            }
            Ok(String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.split_once(':').map(|(_, monitor)| monitor.trim()))
                .filter(|monitor| !monitor.is_empty())
                .map(ToOwned::to_owned)
                .collect())
        }
        _ => anyhow::bail!("Compositor argument '{compositor}' is not recognized"),
    }
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

    let monitors = if monitor == "--all" {
        let monitors = active_monitors(compositor)?;
        if monitors.is_empty() {
            anyhow::bail!("No active outputs were reported by {compositor}");
        }
        monitors
    } else {
        vec![monitor.to_string()]
    };
    for monitor in monitors {
        apply_wallpaper(&wallpaper_path, compositor, &monitor, &config)?;
    }

    Ok(())
}
