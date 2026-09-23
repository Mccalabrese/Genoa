//! Wallpaper Indexer Daemon (wp-daemon)
//!
//! A background service that monitors the wallpaper directory.
//! 1. Scans for new images recursively.
//! 2. Generates thumbnails in parallel (using Rayon) to offload CPU work.
//! 3. Maintains a JSON cache for the selection tool to read instantly.
//! 4. Uses `notify` to watch for filesystem changes in real-time.

use anyhow::{Context, Result};
use image::{ImageFormat, ImageReader, Limits, imageops::FilterType};
use notify::{RecursiveMode, Watcher};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant, UNIX_EPOCH};
use tempfile::Builder as TempFileBuilder;
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
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DECODE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 8_192;
const MAX_WALLPAPERS: usize = 2_000;
const MAX_WALLPAPER_DEPTH: usize = 8;
const MAX_THUMBNAIL_WORKERS: usize = 2;
const WATCH_DEBOUNCE: Duration = Duration::from_millis(750);

#[derive(Debug, Clone)]
struct SourceImage {
    path: PathBuf,
    thumb_path: PathBuf,
}

fn is_supported_image(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp")
    )
}

/// Encodes a digest without depending on a digest crate's formatting traits.
/// sha2 0.11 returns a generic fixed-size array, which intentionally does not
/// promise `LowerHex` support.
fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len().saturating_mul(2));
    for &byte in bytes {
        encoded.push(HEX[usize::from(byte >> 4)] as char);
        encoded.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    encoded
}

fn thumbnail_cache_name(path: &Path, metadata: &fs::Metadata) -> Option<String> {
    let canonical_path = fs::canonicalize(path).ok()?;
    let modified = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;

    let mut hasher = Sha256::new();
    hasher.update(canonical_path.as_os_str().as_bytes());
    hasher.update([0]);
    hasher.update(metadata.len().to_le_bytes());
    hasher.update(modified.as_secs().to_le_bytes());
    hasher.update(modified.subsec_nanos().to_le_bytes());
    let digest = hasher.finalize();
    Some(format!("{}.png", lower_hex(digest.as_ref())))
}

fn source_image(path: PathBuf, thumb_dir: &Path) -> Option<SourceImage> {
    if !is_supported_image(&path) {
        return None;
    }

    let metadata = fs::metadata(&path).ok()?;
    if metadata.len() > MAX_SOURCE_BYTES {
        return None;
    }

    let thumb_name = thumbnail_cache_name(&path, &metadata)?;
    Some(SourceImage {
        path,
        thumb_path: thumb_dir.join(thumb_name),
    })
}

/// Generates a thumbnail for a given image if it doesn't exist.
/// Returns the path to the thumbnail.
fn ensure_thumbnail(source: &SourceImage, thumb_dir: &Path) -> Option<PathBuf> {
    let original_path = &source.path;
    let thumb_path = &source.thumb_path;
    // Cache Hit: If thumbnail exists, skip processing to save CPU/Battery.
    if fs::metadata(thumb_path).is_ok_and(|metadata| metadata.is_file()) {
        return Some(thumb_path.clone());
    }

    // Cache miss: decode only approved formats within hard resource limits.
    let mut reader = ImageReader::open(original_path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    // Resize using Nearest Neighbor for speed, or Lanczos3 for quality.
    // Nearest is chosen here for performance on large directories.
    let thumb = img.resize(THUMB_WIDTH, u32::MAX, FilterType::Nearest);
    let temp_file = TempFileBuilder::new()
        .prefix(".thumbnail-")
        .suffix(".png")
        .tempfile_in(thumb_dir)
        .ok()?;
    if let Err(e) = thumb.save_with_format(temp_file.path(), ImageFormat::Png) {
        eprintln!("Failed to save thumb for {:?}: {}", original_path, e);
        return None;
    }
    if temp_file.as_file().sync_all().is_err() {
        return None;
    }
    temp_file.persist(thumb_path).ok()?;
    Some(thumb_path.clone())
}

fn write_cache_atomically(cache_file: &Path, wallpapers: &[Wallpaper]) -> Result<()> {
    let parent = cache_file
        .parent()
        .context("Wallpaper cache file must have a parent directory")?;
    fs::create_dir_all(parent)?;

    let json = serde_json::to_vec(wallpapers)?;
    let mut temp_file = TempFileBuilder::new()
        .prefix(".wallpapers-")
        .suffix(".json")
        .tempfile_in(parent)?;
    temp_file.write_all(&json)?;
    temp_file.as_file().sync_all()?;
    temp_file
        .persist(cache_file)
        .map_err(|error| error.error)
        .context("Failed to atomically replace wallpaper cache")?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
/// The core indexing logic.
/// 1. Walks the directory.
/// 2. Limits sources to safe, static image formats and bounded resources.
/// 3. Generates thumbnails with bounded parallelism.
/// 4. Writes the master JSON index.
fn scan_and_update_cache(wall_dir: &Path, cache_file: &Path) -> Result<()> {
    let home = dirs::home_dir().context("Failed to get $HOME")?;
    let thumb_dir = home.join(".cache/wallpaper_thumbs");
    fs::create_dir_all(&thumb_dir)?;
    println!("Scanning wallpapers in {:?}...", wall_dir);
    // Collect only a bounded set of supported static images. We do not follow
    // links and cap depth to keep a hostile or accidental directory tree from
    // turning a background index into an unbounded traversal.
    let mut entries: Vec<SourceImage> = WalkDir::new(wall_dir)
        .follow_links(false)
        .max_depth(MAX_WALLPAPER_DEPTH)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| source_image(entry.into_path(), &thumb_dir))
        .take(MAX_WALLPAPERS + 1)
        .collect();
    entries.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    if entries.len() > MAX_WALLPAPERS {
        entries.truncate(MAX_WALLPAPERS);
        eprintln!("Wallpaper limit reached ({MAX_WALLPAPERS}); skipping remaining images.");
    }

    // Decoding is CPU- and memory-intensive. Limit work to two images at once
    // instead of borrowing every core from the interactive desktop.
    let thumbnail_pool = rayon::ThreadPoolBuilder::new()
        .num_threads(MAX_THUMBNAIL_WORKERS)
        .build()
        .context("Failed to create bounded thumbnail worker pool")?;
    let wallpapers: Vec<Wallpaper> = thumbnail_pool.install(|| {
        entries
            .par_iter()
            .filter_map(|source| {
                let thumb = ensure_thumbnail(source, &thumb_dir)?;
                Some(Wallpaper {
                    name: source.path.file_stem()?.to_string_lossy().to_string(),
                    path: source.path.clone(),
                    thumb_path: thumb,
                })
            })
            .collect()
    });
    write_cache_atomically(cache_file, &wallpapers)?;
    //Garbage Collection
    // Remove thumbnails for wallpapers that no longer exist.
    let good_thumbs: HashSet<PathBuf> = wallpapers.into_iter().map(|w| w.thumb_path).collect();
    for entry in fs::read_dir(&thumb_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let thumb_path = entry.path();
            if !good_thumbs.contains(&thumb_path) {
                let _ = fs::remove_file(thumb_path);
            }
        }
    }
    println!("Cache update. Found {} wallpapers.", good_thumbs.len());
    Ok(())
}

fn is_relevant_event(event: &notify::Event) -> bool {
    matches!(
        event.kind,
        notify::EventKind::Create(_) | notify::EventKind::Modify(_) | notify::EventKind::Remove(_)
    )
}

fn wait_for_debounced_change(
    rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
) -> Option<()> {
    loop {
        match rx.recv().ok()? {
            Ok(event) if is_relevant_event(&event) => break,
            Ok(_) => {}
            Err(error) => eprintln!("Watch error {error:?}"),
        }
    }

    let mut deadline = Instant::now() + WATCH_DEBOUNCE;
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(event)) if is_relevant_event(&event) => {
                deadline = Instant::now() + WATCH_DEBOUNCE
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => eprintln!("Watch error {error:?}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Some(()),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
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
    while wait_for_debounced_change(&rx).is_some() {
        println!("Relevant wallpaper changes detected. Refreshing cache...");
        if let Err(error) = scan_and_update_cache(&wall_dir, &cache_file) {
            eprintln!("Error updating cache: {error}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_static_supported_image_extensions_are_indexed() {
        assert!(is_supported_image(Path::new("wallpaper.JPEG")));
        assert!(is_supported_image(Path::new("wallpaper.webp")));
        assert!(!is_supported_image(Path::new("video.webm")));
        assert!(!is_supported_image(Path::new("archive.zip")));
    }

    #[test]
    fn lower_hex_encodes_each_digest_byte() {
        assert_eq!(lower_hex(&[0x00, 0xab, 0xff]), "00abff");
    }

    #[test]
    fn thumbnail_names_include_path_and_source_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first.png");
        let second = temp.path().join("second.png");
        fs::write(&first, b"same contents").unwrap();
        fs::write(&second, b"same contents").unwrap();

        let first_name = thumbnail_cache_name(&first, &fs::metadata(&first).unwrap()).unwrap();
        let second_name = thumbnail_cache_name(&second, &fs::metadata(&second).unwrap()).unwrap();
        assert_ne!(first_name, second_name);
        assert!(first_name.ends_with(".png"));
    }

    #[test]
    fn source_images_reject_oversized_files_before_decode() {
        let temp = tempfile::tempdir().unwrap();
        let image_path = temp.path().join("large.png");
        let file = File::create(&image_path).unwrap();
        file.set_len(MAX_SOURCE_BYTES + 1).unwrap();

        assert!(source_image(image_path, temp.path()).is_none());
    }

    #[test]
    fn thumbnail_generation_uses_the_metadata_keyed_cache_path() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("source.png");
        let thumb_dir = temp.path().join("thumbs");
        fs::create_dir(&thumb_dir).unwrap();
        image::DynamicImage::new_rgba8(32, 16)
            .save_with_format(&source_path, ImageFormat::Png)
            .unwrap();

        let source = source_image(source_path, &thumb_dir).unwrap();
        let thumb = ensure_thumbnail(&source, &thumb_dir).unwrap();
        assert_eq!(thumb, source.thumb_path);
        assert!(thumb.is_file());
        assert_eq!(ensure_thumbnail(&source, &thumb_dir), Some(thumb));
    }

    #[test]
    fn cache_replacement_leaves_valid_json() {
        let temp = tempfile::tempdir().unwrap();
        let cache_path = temp.path().join("wallpapers.json");
        fs::write(&cache_path, "not json").unwrap();
        let wallpapers = vec![Wallpaper {
            name: "Sunset".to_string(),
            path: PathBuf::from("/wallpapers/sunset.jpg"),
            thumb_path: PathBuf::from("/cache/sunset.png"),
        }];

        write_cache_atomically(&cache_path, &wallpapers).unwrap();
        let parsed: Vec<Wallpaper> =
            serde_json::from_slice(&fs::read(&cache_path).unwrap()).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].name, "Sunset");
    }
}
