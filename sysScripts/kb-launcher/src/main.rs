//! Keyboard Cheat Sheet Launcher (kb-launcher)
//!
//! A utility to display markdown/text cheat sheets in a floating terminal window.
//!
//! Workflow:
//! 1. Reads a list of "Sheets" (Name -> File Path) from `config.toml`.
//! 2. Uses `rofi` to present a selection menu to the user.
//! 3. Resolves the target file path (expanding `~`).
//! 4. Detects the current compositor (Hyprland/Sway/Niri) to apply specific window rules (floating/size).
//! 5. Launches the user's preferred terminal running a pager (e.g., `bat` or `less`) to view the file.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;
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

// --- Configuration Models ---
#[derive(Deserialize, Debug)]
struct Sheet {
    name: String, // Display name in Rofi (e.g., "Vim Keys")
    file: String, // Path to file (e.g., "~/docs/vim.md")
    compositor: Option<String>,
}

#[derive(Deserialize, Debug)]
struct CompositorArgs {
    // Window rules arguments vary by terminal emulator and compositor combination.
    // We store them as lists of strings in the config.
    sway: Vec<String>,
    niri: Vec<String>,
    default: Vec<String>,
}

#[derive(Deserialize, Debug)]
struct Global {
    terminal: String, // e.g., "ghostty"
    pager: String,    // e.g., "bat" or "less"
}

#[derive(Deserialize, Debug)]
struct KbLauncherConfig {
    compositor_args: CompositorArgs,
    sheet: Vec<Sheet>,
}

#[derive(Deserialize, Debug)]
struct GlobalConfig {
    global: Global,
    kb_launcher: KbLauncherConfig,
}

/// Loads the centralized configuration file.
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
    let config: GlobalConfig =
        toml::from_str(&config_str).context("Failed to parse config file")?;
    Ok(config)
}

/// Detects the active Wayland compositor via environment variables.
fn get_compositor() -> String {
    if env::var("NIRI_SOCKET").is_ok() {
        return "niri".to_string();
    }
    if env::var("SWAYSOCK").is_ok() {
        return "sway".to_string();
    }
    if let Ok(d) = env::var("XDG_CURRENT_DESKTOP") {
        let d = d.to_lowercase();
        if d.contains("niri") {
            return "niri".to_string();
        }
        if d.contains("sway") {
            return "sway".to_string();
        }
    }
    "unknown".to_string()
}

// --- UI Logic ---

/// Spawns Rofi to let the user select a sheet.
/// Returns the name of the selected sheet.
fn show_rofi_menu(sheets: &[&Sheet]) -> Result<String> {
    // Build the input string (newline separated names)
    let menu_string = sheets
        .iter()
        .map(|s| s.name.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    // Spawn Rofi
    let mut child = Command::new("rofi")
        .arg("-dmenu")
        .arg("-i")
        .arg("-p")
        .arg("View Cheat Sheet:")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("Failed to spawn rofi. Is it installed and in your $PATH?")?;
    // Pipe data into Rofi
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(menu_string.as_bytes())
            .context("Failed to write to rofi stdin")?;
    }
    // Capture selection
    let output = child
        .wait_with_output()
        .context("Failed to wait for rofi to exit")?;
    if !output.status.success() {
        // Non-zero exit code usually means the user pressed Esc
        anyhow::bail!("No selection made in rofi.");
    }
    let choice =
        String::from_utf8(output.stdout).context("Failed to parse rofi output as UTF-8")?;
    Ok(choice.trim().to_string())
}

// --- Main Execution ---
fn main() -> Result<()> {
    // Setup
    let global_config = load_config()?;
    let global_conf = global_config.global;
    let kb_config = global_config.kb_launcher;
    let compositor = get_compositor();

    let available_sheets: Vec<&Sheet> = kb_config
        .sheet
        .iter()
        .filter(|s| s.compositor.is_none() || s.compositor.as_deref() == Some(compositor.as_str()))
        .collect();

    if available_sheets.is_empty() {
        anyhow::bail!("No cheat sheets available for the current compositor");
    }
    // User Selection
    let chosen_sheet_name = show_rofi_menu(&available_sheets)?;
    // Resolve File
    let chosen_sheet = available_sheets
        .into_iter()
        .find(|s| s.name == chosen_sheet_name)
        .context("Failed to find chosen sheet")?;

    let sheet_path = expand_path(&chosen_sheet.file);

    // Environment specific args
    // Inject specific arguments (like `--title=float_me`) so the window manager
    // knows to float this specific terminal window.
    let compositor_args = match compositor.as_str() {
        "sway" => &kb_config.compositor_args.sway,
        "niri" => &kb_config.compositor_args.niri,
        _ => &kb_config.compositor_args.default,
    };
    // Command Construction
    // Build a shell command that:
    // a. Runs the pager (bat/less) on the file.
    // b. Prints a "Press key to close" prompt.
    // c. Waits for user input (read -n 1) so the terminal doesn't close immediately.
    let inner_cmd = format!(
        "{} '{}'; printf %s 'Press any key to close...'; read -n 1 -s -r",
        global_conf.pager,
        sheet_path.display()
    );
    //Execution
    Command::new(&global_conf.terminal)
        .args(compositor_args)
        .arg("-e")
        .arg("sh")
        .arg("-c")
        .arg(&inner_cmd)
        .spawn()
        .context(format!(
            "Failed to spawn terminal: {}",
            global_conf.terminal
        ))?;
    Ok(())
}
