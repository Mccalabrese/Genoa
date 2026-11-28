use std::{
    env,
    fs::OpenOptions,
    io::{Cursor, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use image::{ImageReader, Limits};
use serde::Deserialize;
use uzers::{get_current_uid, get_user_by_uid, os::unix::UserExt};

const MAX_CACHE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_THUMBNAIL_BYTES: u64 = 8 * 1024 * 1024;
const MAX_WALLPAPER_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DECODE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 8_192;
const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;
const LOCK_WALLPAPER_FILE: &str = "lock-wallpaper";

/// A wallpaper and its thumbnail supplied by Genoa's wallpaper indexer.
#[derive(Clone, Debug, Deserialize)]
pub struct Wallpaper {
    /// User-facing file name.
    pub name: String,
    /// Full path of the source image.
    pub path: PathBuf,
    /// Full path of the generated thumbnail.
    pub thumb_path: PathBuf,
}

/// Reads the thumbnail cache produced by `wp-daemon`.
///
/// # Errors
///
/// Returns an error when the cache is absent, unsafe, too large, or malformed.
pub fn load_wallpapers() -> Result<Vec<Wallpaper>, String> {
    let cache = current_user_home()?.join(".cache/wallpapers.json");
    let bytes = read_owned_regular_file(&cache, MAX_CACHE_BYTES)?;
    let mut wallpapers = serde_json::from_slice::<Vec<Wallpaper>>(&bytes)
        .map_err(|error| format!("could not parse {}: {error}", cache.display()))?;
    wallpapers.retain(|wallpaper| {
        !wallpaper.name.is_empty()
            && wallpaper.name.chars().count() <= 160
            && wallpaper.path.is_absolute()
            && wallpaper.thumb_path.is_absolute()
    });
    wallpapers.sort_by_cached_key(|wallpaper| wallpaper.name.to_lowercase());
    Ok(wallpapers)
}

/// Reads a validated thumbnail for display by GTK.
///
/// # Errors
///
/// Returns an error when the thumbnail is not a small, user-owned regular file.
pub fn thumbnail_bytes(wallpaper: &Wallpaper) -> Result<Vec<u8>, String> {
    read_owned_regular_file(&wallpaper.thumb_path, MAX_THUMBNAIL_BYTES)
}

/// Persists the image that Genoa Lock should use on its next invocation.
///
/// # Errors
///
/// Returns an error when the image is unsafe, cannot be decoded within the
/// resource limits, or the private settings file cannot be atomically replaced.
pub fn save_lock_wallpaper(path: &Path) -> Result<(), String> {
    validate_wallpaper(path)?;
    let path_text = path
        .to_str()
        .ok_or_else(|| "the wallpaper path is not valid UTF-8".to_string())?;
    let config = lock_wallpaper_config_path()?;
    let directory = config
        .parent()
        .ok_or_else(|| "the lock-wallpaper settings path has no parent directory".to_string())?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not secure {}: {error}", directory.display()))?;

    let mut temporary = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| format!("could not create a temporary wallpaper setting: {error}"))?;
    temporary
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("could not secure the temporary wallpaper setting: {error}"))?;
    temporary
        .write_all(path_text.as_bytes())
        .and_then(|()| temporary.write_all(b"\n"))
        .map_err(|error| format!("could not write the lock wallpaper setting: {error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("could not save the lock wallpaper setting: {error}"))?;
    temporary.persist(&config).map_err(|error| {
        format!(
            "could not replace {} with the new lock wallpaper: {}",
            config.display(),
            error.error
        )
    })?;
    Ok(())
}

/// Returns the configured lock wallpaper when its setting is readable.
#[must_use]
pub fn configured_lock_wallpaper() -> Option<PathBuf> {
    let config = lock_wallpaper_config_path().ok()?;
    let bytes = read_owned_regular_file(&config, 4_096).ok()?;
    let value = std::str::from_utf8(&bytes).ok()?.trim();
    let path = PathBuf::from(value);
    path.is_absolute().then_some(path)
}

/// Verifies that a source image is safe and bounded before passing it to a
/// wallpaper backend or saving it as the lock-screen selection.
///
/// # Errors
///
/// Returns an error when the file is unsafe, too large, or not a supported
/// image within Genoa's decode limits.
pub fn validate_wallpaper(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("the wallpaper path must be absolute".to_string());
    }
    let bytes = read_owned_regular_file(path, MAX_WALLPAPER_BYTES)?;
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| format!("could not identify {}: {error}", path.display()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| format!("could not decode {} safely: {error}", path.display()))?;
    if u64::from(image.width()) * u64::from(image.height()) > MAX_IMAGE_PIXELS {
        return Err(format!(
            "{} exceeds Genoa's 16-megapixel wallpaper limit",
            path.display()
        ));
    }
    Ok(())
}

fn lock_wallpaper_config_path() -> Result<PathBuf, String> {
    Ok(config_home()?.join("genoa").join(LOCK_WALLPAPER_FILE))
}

fn config_home() -> Result<PathBuf, String> {
    match env::var_os("XDG_CONFIG_HOME") {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => Ok(current_user_home()?.join(".config")),
    }
}

fn current_user_home() -> Result<PathBuf, String> {
    get_user_by_uid(get_current_uid())
        .map(|user| user.home_dir().to_path_buf())
        .ok_or_else(|| "could not determine the current user's home directory".to_string())
}

fn read_owned_regular_file(path: &Path, maximum_bytes: u64) -> Result<Vec<u8>, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file()
        || metadata.uid() != get_current_uid()
        || metadata.permissions().mode() & 0o022 != 0
        || metadata.len() > maximum_bytes
    {
        return Err(format!("{} is not a safe Genoa data file", path.display()));
    }
    let capacity =
        usize::try_from(metadata.len()).map_err(|_| format!("{} is too large", path.display()))?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum_bytes {
        return Err(format!("{} is too large", path.display()));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use image::{DynamicImage, ImageFormat};

    use super::{MAX_WALLPAPER_BYTES, validate_wallpaper};

    #[test]
    fn validates_a_safe_png_and_rejects_a_symlink() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let wallpaper = directory.path().join("wallpaper.png");
        DynamicImage::new_rgba8(64, 32).save_with_format(&wallpaper, ImageFormat::Png)?;
        let link = directory.path().join("wallpaper-link.png");
        symlink(&wallpaper, &link)?;

        assert!(validate_wallpaper(&wallpaper).is_ok());
        assert!(validate_wallpaper(&link).is_err());
        Ok(())
    }

    #[test]
    fn rejects_a_wallpaper_over_the_compressed_size_limit() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let wallpaper = directory.path().join("oversized.png");
        let file = fs::File::create(&wallpaper)?;
        file.set_len(MAX_WALLPAPER_BYTES + 1)?;

        assert!(validate_wallpaper(&wallpaper).is_err());
        Ok(())
    }
}
