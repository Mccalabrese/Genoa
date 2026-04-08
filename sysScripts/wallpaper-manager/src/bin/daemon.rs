//! Wallpaper Indexer Daemon (wp-daemon)
//!
//! A background service that monitors the wallpaper directory.
//! 1. Scans for new images recursively.
//! 2. Generates thumbnails in parallel (using Rayon) to offload CPU work.
//! 3. Maintains a JSON cache for the selection tool to read instantly.
//! 4. Uses `notify` to watch for filesystem changes in real-time.

use anyhow::{Context, Result};
use image::imageops::FilterType;
use notify::{RecursiveMode, Watcher};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use walkdir::WalkDir;

fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    PathBuf::from(path)
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct WallpaperManagerConfig {
    wallpaper_dir: String,
    swww_params: Vec<String>,
    swaybg_cache_file: String,
    cache_file: String,
    rofi_config_path: String,
    rofi_theme_override: String,
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
#[derive(Serialize, Deserialize, Debug, Clone)]
struct Wallpaper {
    name: String,
    path: PathBuf,
    thumb_path: PathBuf,
}
const THUMB_WIDTH: u32 = 500;

/// Generates a thumbnail for a given image if it doesn't exist.
/// Returns the path to the thumbnail.
fn ensure_thumbnail(original_path: &Path, thumb_dir: &Path) -> Option<PathBuf> {
    let file_name = original_path.file_name()?;
    let thumb_path = thumb_dir.join(file_name);
    // Cache Hit: If thumbnail exists, skip processing to save CPU/Battery.
    if thumb_path.exists() {
        return Some(thumb_path);
    }
    // Cache Miss: Generate thumbnail
    let img = match image::open(original_path) {
        Ok(img) => img,
        Err(_) => return None, // Skip unreadable/corrupt files
    };
    // Resize using Nearest Neighbor for speed, or Lanczos3 for quality.
    // Nearest is chosen here for performance on large directories.
    let thumb = img.resize(THUMB_WIDTH, u32::MAX, FilterType::Nearest);
    if let Err(e) = thumb.save(&thumb_path) {
        eprintln!("Failed to save thumb for {:?}: {}", original_path, e);
        return None;
    }
    Some(thumb_path)
}
/// The core indexing logic.
/// 1. Walks the directory.
/// 2. Filters video files.
/// 3. Generates thumbnails in parallel.
/// 4. Writes the master JSON index.
fn scan_and_update_cache(wall_dir: &Path, cache_file: &Path) -> Result<()> {
    let home = dirs::home_dir().context("Failed to get $HOME")?;
    let thumb_dir = home.join(".cache/wallpaper_thumbs");
    fs::create_dir_all(&thumb_dir)?;
    println!("Scanning wallpapers in {:?}...", wall_dir);
    // Collect all files (Sequential I/O)
    let entries: Vec<PathBuf> = WalkDir::new(wall_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .map(|e| e.path().to_path_buf())
        .collect();
    // Process Images (Parallel CPU)
    // Rayon (.par_iter) distributes image resizing across all available CPU cores.
    let wallpapers: Vec<Wallpaper> = entries
        .par_iter()
        .filter_map(|path| {
            // Skip video wallpapers (mp4, mkv) as image crate cannot handle them
            if let Some(ext) = path.extension() {
                let ext_str = ext.to_string_lossy().to_lowercase();
                if ext_str == "mp4" || ext_str == "webm" || ext_str == "mkv" {
                    return None;
                }
            }
            let thumb = ensure_thumbnail(path, &thumb_dir)?;
            Some(Wallpaper {
                name: path.file_stem()?.to_string_lossy().to_string(),
                path: path.clone(),
                thumb_path: thumb,
            })
        })
        .collect();
    // Update Cache File
    let json = serde_json::to_string(&wallpapers)?;
    fs::write(cache_file, json).context("Failed to write cache file")?;
    //Garbage Collection
    // Remove thumbnails for wallpapers that no longer exist.
    let good_thumbs: HashSet<PathBuf> = wallpapers.into_iter().map(|w| w.thumb_path).collect();
    for entry in fs::read_dir(&thumb_dir)? {
        let entry = entry?;
        let thumb_path = entry.path();
        if !good_thumbs.contains(&thumb_path) {
            println!("Garbage collecting old thumb: {:?}", thumb_path);
            let _ = fs::remove_file(thumb_path);
        }
    }
    println!("Cache update. Found {} wallpapers.", good_thumbs.len());
    Ok(())
}
fn main() -> Result<()> {
    let global_config = load_config()?;
    let config = global_config.wallpaper_manager;
    let wall_dir = expand_path(&config.wallpaper_dir);
    let cache_file = expand_path(&config.cache_file);
    if !wall_dir.exists() {
        anyhow::bail!("Wallpaper directory does not exist: {:?}", wall_dir);
    }
    //Initial scan on startup
    if let Err(e) = scan_and_update_cache(&wall_dir, &cache_file) {
        eprintln!("Initial scan failed: {}", e);
    }
    // Real-time Filesystem Watcher
    // Uses inotify (Linux) to trigger updates immediately when files are added/removed.
    let (tx, rx) = channel();
    let mut watcher = notify::recommended_watcher(tx)?;
    watcher.watch(&wall_dir, RecursiveMode::Recursive)?;
    println!("Daemon started. Watching {:?}...", wall_dir);
    // Event Loop
    for res in rx {
        match res {
            Ok(event) => {
                // FILTER: Ignore access events, metadata changes, or other noise.
                // We only care if a file was created, modified (content), or removed.
                use notify::EventKind;
                match event.kind {
                    EventKind::Create(_)
                    | EventKind::Modify(notify::event::ModifyKind::Data(_))
                    | EventKind::Remove(_) => {
                        println!(
                            "Relevant change detected ({:?}). Refreshing cache...",
                            event.kind
                        );
                        // Debounce? (Optional optimization, but this filter usually fixes the loop)
                        if let Err(e) = scan_and_update_cache(&wall_dir, &cache_file) {
                            eprintln!("Error updating cache: {}", e);
                        }
                    }
                    _ => {} // Ignore everything else (Access, Chmod, etc.)
                }
            }
            Err(e) => eprintln!("Watch error {:?}", e),
        }
    }
    Ok(())
}
