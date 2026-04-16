//! Clipboard Manager (clip-manager)
//!
//! A native Rust interface for `cliphist` (clipboard history manager) using `rofi`.
//!
//! Architecture:
//! 1. **Pipelining:** Replaces complex shell pipelines (`cliphist list | rofi | cliphist decode | wl-copy`)
//!    with safe, type-checked process chaining.
//! 2. **State Loop:** Implements a refresh loop so deleting an item (Ctrl+Del) immediately
//!    re-opens the menu without the app closing.

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::fs;
use std::io::Write;
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

// --- Config Models ---
#[derive(Deserialize, Debug)]
struct ClipConfig {
    rofi_config: String,
    message: String,
}

#[derive(Deserialize, Debug)]
struct GlobalConfig {
    clip_manager: ClipConfig,
}

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

// --- Core Process Wrappers ---

/// Fetches the raw list of clipboard history items.
/// This replaces `cliphist list`.
fn get_cliphist_list() -> Result<String> {
    let output = Command::new("cliphist")
        .arg("list")
        .output()
        .context("Failed to run 'cliphist list'")?;

    if !output.status.success() {
        return Err(anyhow!("cliphist list failed"));
    }

    Ok(String::from_utf8(output.stdout)?)
}

/// Decodes the selected item and copies it to the Wayland clipboard.
/// This manually implements the pipe: `echo "selection" | cliphist decode | wl-copy`.
fn decode_and_copy(selection: &str) -> Result<()> {
    // Spawn `cliphist decode` (The Producer)
    // Pipe both stdin (to feed it the selection) and stdout (to catch the decoded image/text).
    let mut cliphist_child = Command::new("cliphist")
        .arg("decode")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to spawn 'cliphist decode'")?;

    // Feed selection to cliphist
    if let Some(mut stdin) = cliphist_child.stdin.take() {
        stdin
            .write_all(selection.as_bytes())
            .context("Failed to write to cliphist stdin")?;
    }

    // Capture the output stream
    let cliphist_stdout = cliphist_child
        .stdout
        .take()
        .context("Failed to get stdout from cliphist")?;

    // Spawn `wl-copy` (The Consumer)
    // Connect the stdout of `cliphist` directly to the stdin of `wl-copy`.
    // This creates a highly efficient OS-level pipe without buffering data in Rust RAM.
    let wl_copy_status = Command::new("wl-copy")
        .stdin(cliphist_stdout)
        .status()
        .context("Failed to spawn 'wl-copy'")?;

    // Cleanup
    cliphist_child.wait()?;

    if !wl_copy_status.success() {
        return Err(anyhow!("wl-copy failed"));
    }

    Ok(())
}

// --- Modification Actions ---
fn delete_entry(selection: &str) -> Result<()> {
    let mut child = Command::new("cliphist")
        .arg("delete")
        .stdin(Stdio::piped())
        .spawn()
        .context("Failed to spawn 'cliphist delete'")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(selection.as_bytes())
            .context("Failed to write to cliphist stdin")?;
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(anyhow!("cliphist delete failed"));
    }

    Ok(())
}

fn wipe_history() -> Result<()> {
    let status = Command::new("cliphist")
        .arg("wipe")
        .status()
        .context("Failed to run 'cliphist wipe'")?;

    if !status.success() {
        return Err(anyhow!("cliphist wipe failed"));
    }

    Ok(())
}

// --- UI Logic ---

/// Launches Rofi with custom keybindings.
/// Returns the Exit Code (to detect special actions) and the selected string.
fn show_rofi(list: &str, config: &ClipConfig) -> Result<(i32, String)> {
    let rofi_config_path = expand_path(&config.rofi_config);

    let mut child = Command::new("rofi")
        .arg("-i")
        .arg("-dmenu")
        // Bind custom keys for actions
        .arg("-kb-custom-1")
        .arg("Control+Delete") // Exit Code 10
        .arg("-kb-custom-2")
        .arg("Alt+Delete") // Exit Code 11
        .arg("-config")
        .arg(rofi_config_path)
        .arg("-mesg")
        .arg(&config.message)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to spawn rofi")?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(list.as_bytes())?;
    }

    let output = child.wait_with_output()?;
    let selection = String::from_utf8(output.stdout)?.trim().to_string();
    let exit_code = output.status.code().unwrap_or(1); // Default to 1 (Cancel) on failure

    Ok((exit_code, selection))
}

fn main() -> Result<()> {
    // Main Event Loop
    // Allows the menu to persist after performing an action like Delete.
    loop {
        //Refresh data
        let config = load_config()?.clip_manager;
        let history_list = get_cliphist_list()?;

        // User Interaction
        let (exit_code, selection) = show_rofi(&history_list, &config)?;

        // Action Dispatch based on Rofi Exit Code
        match exit_code {
            0 => {
                // Enter: Copy & Exit
                if selection.is_empty() {
                    continue;
                }
                decode_and_copy(&selection)?;
                break;
            }
            1 => break, // 1 = Esc: exit loop
            10 => {
                // 10 = Ctrl+Del: Delete Item
                delete_entry(&selection)?;
                continue; // Re-loop to show updated list 
            }
            11 => {
                // 11 = Alt+Del: Wipe All
                wipe_history()?;
                continue;
            }
            _ => {
                break;
            }
        }
    }

    Ok(())
}
