//! Emoji Picker Utility (emoji-picker)
//!
//! A Wayland-native utility that allows users to select Unicode emojis via Rofi.
//!
//! Key Features:
//! 1. **Zero-Latency Search:** Pre-generates the entire Unicode dataset in memory.
//! 2. **Hidden Metadata:** Injects invisible Pango markup so users can search by name ("smile")
//!    without cluttering the visual interface with text.
//! 3. **Wayland Integration:** Pipes the result directly to `wl-copy` for immediate pasting.

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::fmt::Write;
use std::fs;
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    PathBuf::from(path)
}

// --- Configuration ---

#[derive(Debug, Deserialize)]
struct EmojiConfig {
    rofi_config: String,
    message: String,
}
#[derive(Debug, Deserialize)]
struct GlobalConfig {
    emoji_picker: EmojiConfig,
}
// Standard TOML loader respecting XDG paths
fn load_config() -> Result<GlobalConfig> {
    let config_path = dirs::home_dir()
        .context("Cannot find home dir")?
        .join(".config/rust-dotfiles/config.toml");
    let config_str = fs::read_to_string(&config_path).with_context(|| {
        format!(
            "Failed to read config file from path {}",
            config_path.display()
        )
    })?;
    let config: GlobalConfig = toml::from_str(&config_str)
        .context("Failed to parse config.toml. Check for syntax errors.")?;
    Ok(config)
}

// --- Core Logic ---

/// Generates the input buffer for Rofi.
///
/// UX Trick: I want users to be able to search for "fire" and see 🔥,
/// but we don't want the word "fire" taking up screen space.
/// We use Pango markup to make the metadata (name, shortcode) strictly invisible
/// (size 1, transparent color), but Rofi's filter engine still sees it.
fn build_emoji_list() -> String {
    // Pre-allocate memory to avoid re-allocations during the loop (approx 60kb data)
    let mut buffer = String::with_capacity(60 * 1024);
    for emoji in emojis::iter() {
        let shortcode = emoji.shortcode().unwrap_or("");
        // Format: <Visible Emoji> <Invisible Keywords>
        let _ = writeln!(
            buffer,
            "{} <span size='1' foreground='#00000000'>{} {}</span>",
            emoji.as_str(),
            emoji.name(),
            shortcode
        );
    }
    buffer
}

/// Spawns the Rofi selector process.
/// Pipes the generated emoji list into Rofi's STDIN.
fn show_rofi(list: &str, config: &EmojiConfig) -> Result<String> {
    let rofi_config_path = expand_path(&config.rofi_config);
    let mut child = Command::new("rofi")
        .arg("-i") // Case insensitive search
        .arg("-dmenu") // Dmenu mode (read stdin)
        .arg("-markup-rows") // Enable Pango markup parsing (for the hidden text hack)
        .arg("-config")
        .arg(rofi_config_path)
        .arg("-mesg")
        .arg(&config.message)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to spawn rofi")?;

    // Write data to pipe
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(list.as_bytes())?;
    }
    let output = child.wait_with_output()?;

    // Code 1 usually means the user pressed Esc (Cancel), which isn't a crash.
    if !output.status.success() && output.status.code() != Some(1) {
        return Err(anyhow!("Rofi failed with an error"));
    }

    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Processing the result.
/// Extracts the actual emoji character from the selected line and copies it to clipboard.
fn parse_and_copy(selection: &str) -> Result<()> {
    // 1. Extract: The string contains "🔥 <span...". I only want the first part.
    let emoji = match selection.split_whitespace().next() {
        Some(emoji_char) => emoji_char,
        None => return Ok(()), // Empty selection
    };

    // 2. Clipboard: Pipe to `wl-copy`.
    // We explicitly set MIME type to UTF-8 text to ensure compatibility across apps.
    let mut child = Command::new("wl-copy")
        .arg("--type")
        .arg("text/plain;charset=utf-8")
        .stdin(Stdio::piped())
        .spawn()
        .context("Failed to spawn 'wl-copy'")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(emoji.as_bytes())?;
    }

    if !child.wait()?.success() {
        return Err(anyhow!("wl-copy failed"));
    }

    Ok(())
}
fn main() -> Result<()> {
    let config = load_config()?.emoji_picker;
    // Generate data
    let emoji_list_string = build_emoji_list();
    // Prompt User
    let selection = show_rofi(&emoji_list_string, &config)?;
    // Execute
    if !selection.is_empty() {
        parse_and_copy(&selection)?;
    }
    Ok(())
}
