//! Radio Menu (radio-menu)
//!
//! A minimal internet radio player interface using `rofi` as the frontend and `mpv` as the backend.
//!
//! Features:
//! 1. **Search:** Queries the Community Radio Browser API (radio-browser.info).
//! 2. **Favorites:** Persists preferred stations to a JSON file.
//! 3. **Playback:** Spawns a detached `mpv` process to stream audio.
//! 4. **Menu Navigation:** Implements a loop-based state machine to handle "Back", "Search", and "Home".

use anyhow::{Context, Result, anyhow};
use notify_rust::Notification;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

// --- Constants ---
// Single Source of Truth for UI elements ensures consistency across re-renders.
const ICON_STOP: &str = "⏹ Stop Radio";
const ICON_SEARCH: &str = "🔍 Search Online...";
const PREFIX_FAV: &str = "⭐ ";
const ICON_REDO: &str = "🔄 Try Again";

const RESULT_LIMIT: usize = 15; // API limit to keep the UI snappy

// Rofi UI Hints (displayed in menu)
const SEARCH_PROMPT: &str = "Type to search station name...";
const HOME_HINT: &str = "<b>Enter:</b> Play  |  <b>Ctrl+R:</b> Remove Favorite";
const SEARCH_HINT: &str =
    "<b>Enter:</b> Play  |  <b>Ctrl+S:</b> Save to Favorites  |  <b>Esc:</b> Back to Search";

/// Enumerates the possible outcomes of a user action in the menu loop.
enum Action {
    Exit,     // Close the app (e.g. after playing a station)
    Refresh,  // Reload data (e.g. after deleting a favorite)
    Continue, // No-op
}

fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    PathBuf::from(path)
}

// --- Data Models ---
#[derive(Deserialize, Serialize, Debug, Clone)]
struct Station {
    name: String,
    url_resolved: String, //The actual stream URL
    tags: String,
    stationuuid: String, //Station ID for de-duplication
}

#[derive(Deserialize, Debug)]
struct RadioConfig {
    rofi_config: String,
    message: String,
}

#[derive(Deserialize, Debug)]
struct GlobalConfig {
    radio_menu: RadioConfig,
}

fn get_config_path() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(".config/rust-dotfiles/config.toml")
}

fn get_favorites_path() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(".config/rust-dotfiles/radio_favorites.json")
}

fn load_config() -> Result<GlobalConfig> {
    let path = get_config_path();
    let content = fs::read_to_string(&path).context("Failed to read config.toml")?;
    let config: GlobalConfig = toml::from_str(&content).context("Failed to parse config.toml")?;
    Ok(config)
}

// --- Network Logic ---

/// Queries the Radio Browser API.
/// Uses a blocking client because the UI (Rofi) cannot display results until the search completes anyway.
fn search_stations(query: &str) -> Result<Vec<Station>> {
    let url = format!(
        "https://de1.api.radio-browser.info/json/stations/byname/{}",
        query
    );
    let response = reqwest::blocking::get(&url)?.json::<Vec<Station>>()?;
    Ok(response.into_iter().take(RESULT_LIMIT).collect())
}

// --- Persistence Logic ---
fn load_favorites() -> Result<Vec<Station>> {
    let path = get_favorites_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let data = fs::read_to_string(path)?;
    let stations: Vec<Station> = serde_json::from_str(&data).unwrap_or_default();
    Ok(stations)
}

fn save_favorite(station: Station) -> Result<()> {
    let path = get_favorites_path();
    let mut favorites = load_favorites()?;
    // Prevent duplicates by UUID
    if !favorites
        .iter()
        .any(|s| s.stationuuid == station.stationuuid)
    {
        favorites.push(station);
        let json = serde_json::to_string_pretty(&favorites)?;
        fs::write(path, json)?;
    }
    Ok(())
}

fn remove_favorite(station_name: &str) -> Result<()> {
    let path = get_favorites_path();
    let mut favorites = load_favorites()?;
    favorites.retain(|s| s.name != station_name);
    let json = serde_json::to_string_pretty(&favorites)?;
    fs::write(path, json)?;
    Ok(())
}

// --- Player Logic ---

/// Kills any existing background player instance to prevent audio overlap.
fn stop_radio() {
    let _ = Command::new("pkill").arg("-x").arg("mpv").status();
}

/// Spawns a detached mpv process to stream the audio.
fn play_station(station_name: &str, url: &str) -> Result<()> {
    stop_radio(); // Enforce single-instance playback

    Command::new("mpv")
        .arg("--no-video")
        .arg(format!("--force-media-title={}", station_name))
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Failed to spawn mpv")?;

    let _ = Notification::new()
        .summary("Radio Playing")
        .body(station_name)
        .icon("media-playback-start")
        .show();

    Ok(())
}

// --- UI Logic (Rofi Wrapper) ---

/// Wraps Rofi execution to handle custom keybindings (Ctrl+S, Ctrl+R).
/// Returns the Exit Code (indicating which key was pressed) and the Selection String.
fn show_rofi(
    options: &[String],
    prompt: &str,
    config: &RadioConfig,
    custom_message: Option<&str>,
) -> Result<(i32, String)> {
    let input = options.join("\n");
    let rofi_config_path = expand_path(&config.rofi_config);
    let msg = custom_message.unwrap_or(&config.message);

    let mut child = Command::new("rofi")
        .arg("-dmenu")
        .arg("-i")
        .arg("-p")
        .arg(prompt)
        .arg("-config")
        .arg(rofi_config_path)
        .arg("-mesg")
        .arg(msg)
        .arg("-markup-rows")
        // Define custom return codes for keybinds
        .arg("-kb-custom-1")
        .arg("Control+s") // Save
        .arg("-kb-custom-2")
        .arg("Control+r") // remove
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes())?;
    }

    let output = child.wait_with_output()?;

    let code = output.status.code().unwrap_or(1);

    // Rofi uses specific codes: 0=Enter, 1=Esc, 10-28=Custom Keys
    if code != 0 && code != 1 && code != 10 && code != 11 {
        return Err(anyhow!("Rofi failed with exit code: {}", code));
    }

    let selection = String::from_utf8(output.stdout)?.trim().to_string();

    Ok((code, selection))
}

// --- Search Workflow ---

/// Handles the search loop: Prompt -> Fetch -> Display Results -> Handle Action.
/// Returns true if an action was taken (play/save), false if user backed out.
fn search(initial_query: Option<String>, config: &RadioConfig) -> Result<bool> {
    let mut current_query = initial_query;
    loop {
        // 1. Get Query (either passed in or prompted)
        let query = match current_query.take() {
            Some(q) => q,
            None => {
                let (code, q) = show_rofi(&[], "Search Query", config, Some(SEARCH_PROMPT))?;
                if code == 1 || q.is_empty() {
                    return Ok(false);
                }
                q
            }
        };
        // 2. Perform Search
        let results = search_stations(&query)?;

        // 3. Handle No Results
        if results.is_empty() {
            // Show error dialog
            let (retry_code, _) = show_rofi(
                &[ICON_REDO.to_string()],
                "No Results",
                config,
                Some(&format!("No stations found for '<b>{}</b>'", query)),
            )?;

            if retry_code == 1 {
                return Ok(false);
            } // Esc -> Back
            continue; // Retry -> Loop back to search bar
        }

        // 4. Show Results
        let result_names: Vec<String> = results.iter().map(|s| s.name.clone()).collect();
        let (r_code, picked_name) = show_rofi(&result_names, "Results", config, Some(SEARCH_HINT))?;

        if r_code == 1 {
            continue;
        } // Esc -> Back to search input

        //5. Handle Action
        if let Some(station) = results.into_iter().find(|s| s.name == picked_name) {
            if r_code == 10 {
                // Ctrl+S -> Save
                save_favorite(station.clone())?;
                play_station(&station.name, &station.url_resolved)?;
                let _ = Notification::new()
                    .summary("Radio")
                    .body("Station Saved")
                    .show();
                return Ok(true);
            } else if r_code == 0 {
                // Enter -> Play
                play_station(&station.name, &station.url_resolved)?;
                return Ok(true);
            }
        }
    }
}
/// Handles keybind actions on the main menu (Delete Favorite).
fn handle_favorite_actions(clean_name: &str, code: i32, favorites: &[Station]) -> Result<Action> {
    if code == 11 {
        // Ctrl+R: Remove Favorite
        remove_favorite(clean_name)?;
        let _ = Notification::new()
            .summary("Radio")
            .body("Favorite Removed")
            .show();
        Ok(Action::Refresh)
    } else if code == 0 {
        // Enter: Play Favorite
        if let Some(station) = favorites.iter().find(|s| s.name == clean_name) {
            play_station(&station.name, &station.url_resolved)?;
            Ok(Action::Exit)
        } else {
            Ok(Action::Continue)
        }
    } else {
        Ok(Action::Continue)
    }
}
// --- Main Execution ---
fn main() -> Result<()> {
    let global_config = load_config()?;
    let config = global_config.radio_menu;
    let mut menu_options = Vec::with_capacity(20);

    // Main Application Loop
    // Keeps the menu open until the user plays a station or explicitly quits.
    'main_menu: loop {
        let favorites = load_favorites()?;
        // Rebuild Menu Options
        menu_options.clear();
        menu_options.push(ICON_STOP.to_string());
        menu_options.push(ICON_SEARCH.to_string());

        for station in &favorites {
            menu_options.push([PREFIX_FAV, &station.name].concat());
        }

        let (code, selection) = show_rofi(&menu_options, "Radio", &config, Some(HOME_HINT))?;

        if code == 1 {
            break 'main_menu;
        } // Esc -> Quit

        if selection == ICON_STOP {
            stop_radio();
            let _ = Notification::new().summary("Radio").body("Stopped").show();
            break 'main_menu;
        } else if selection == ICON_SEARCH {
            // Enter Search Loop
            if search(None, &config)? {
                break 'main_menu; // If seach ended in playback, exit app
            }
        } else if let Some(clean_name) = selection.strip_prefix(PREFIX_FAV) {
            // Handle Favorites
            let action = handle_favorite_actions(clean_name, code, &favorites)?;
            match action {
                Action::Exit => break 'main_menu,
                Action::Refresh => continue 'main_menu, // Reload list (after delete)
                Action::Continue => continue 'main_menu,
            }
        } else {
            // Handle "Type-to-search" from main menu (User typed a query directly)
            if search(Some(selection), &config)? {
                break 'main_menu;
            }
        }
    }

    Ok(())
}
