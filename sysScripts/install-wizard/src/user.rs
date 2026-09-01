use crate::helpers::{create_symlink, expected_binary_names};
use crate::traits::CmdExecutor;
use colored::*;
use inquire::Text;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const GEOCLUE_CONF_PATH: &str = "/etc/geoclue/geoclue.conf";
const BEACONDB_GEOLOCATE_URL: &str = "https://api.beacondb.net/v1/geolocate";

const RETIRED_TOOL_SOURCES: &[&str] = &[
    "sysScripts/sidebar/build.rs",
    "sysScripts/sidebar/src/calendar_query.c",
];

/// Removes source files intentionally retired by tool migrations.
///
/// Cargo can clear compiled output, but a stale untracked `build.rs` is still
/// discovered automatically and can break the next build.
pub fn remove_retired_tool_sources(repo_root: &Path) -> Result<(), std::io::Error> {
    for source in RETIRED_TOOL_SOURCES {
        let path = repo_root.join(source);
        if path.is_file() || path.is_symlink() {
            fs::remove_file(&path)?;
            println!("   🧹 Removed retired source: {}", source);
        }
    }

    Ok(())
}

pub fn setup_librewolf(sys: &impl CmdExecutor, home: &Path) -> Result<(), std::io::Error> {
    println!("   🐺 Configuring LibreWolf for Human Beings...");

    let wolf_dir = home.join(".librewolf");
    let override_file = wolf_dir.join("librewolf.overrides.cfg");

    // Ensure directory exists
    sys.create_dir_all(&wolf_dir)?;

    // The "Student-Friendly" Config
    let config_content = r#"
        defaultPref("network.captive-portal-service.enabled", true);
        defaultPref("privacy.resistFingerprinting.letterboxing", false);
        defaultPref("privacy.resistFingerprinting", false);
        defaultPref("webgl.disabled", false);
        defaultPref("privacy.clearOnShutdown.history", false);
        defaultPref("privacy.clearOnShutdown.cookies", false);
    "#;

    // Write it
    let override_path = override_file
        .to_str()
        .ok_or_else(|| std::io::Error::other("Invalid LibreWolf override path"))?;
    sys.write_string_to_file(override_path, config_content)?;
    // Set as Default Browser (XDG)
    println!("   👉 Setting LibreWolf as default browser...");
    let mimes = [
        "text/html",
        "x-scheme-handler/http",
        "x-scheme-handler/https",
    ];

    for mime in mimes {
        let _ = sys.run_cmd_ignore_err("xdg-mime", &["default", "librewolf.desktop", mime]);
    }
    let _ = sys.run_cmd_ignore_err(
        "xdg-settings",
        &["set", "default-web-browser", "librewolf.desktop"],
    );
    Ok(())
}
///I templated my waybar configs to allow gitignore of my personalization.
///This unpacks them if they don't already exist.
pub fn setup_waybar_configs(sys: &impl CmdExecutor, home: &Path) {
    let waybar_dir = home.join(".config/waybar");
    let configs = vec!["swayConfig.jsonc", "niriConfig.jsonc"];

    for config in configs {
        let template = waybar_dir.join(format!("{}.template", config));
        let target = waybar_dir.join(config);

        let template_exists = sys.path_exists(&template);
        let target_exists = sys.path_exists(&target);
        if template_exists && !target_exists {
            match sys.read_file_to_string(&template) {
                Ok(content) => {
                    let Some(target_path) = target.to_str() else {
                        println!("   ⚠️  Failed to create {}: invalid path", config);
                        continue;
                    };
                    match sys.write_string_to_file(target_path, &content) {
                        Ok(()) => println!("   ✅ Created {} from template", config),
                        Err(e) => println!("   ⚠️  Failed to create {}: {}", config, e),
                    }
                }
                Err(e) => println!("   ⚠️  Failed to read {}: {}", config, e),
            }
        } else if target_exists {
            println!("   ℹ️  {} already exists", config);
        }
    }
}
/// Interactive wizard to generate the local `config.toml`.
/// Validates input to prevent injection attacks before writing to system files (like /etc/geoclue).
pub fn setup_secrets_and_geoclue(
    sys: &impl CmdExecutor,
    home: &Path,
) -> Result<(), std::io::Error> {
    let config_dir = home.join(".config/rust-dotfiles");
    let config_path = config_dir.join("config.toml");
    // Logic to handle if 'rust-dotfiles' exists as a file instead of a directory
    if sys.path_exists(&config_dir) {
        if !sys.path_is_dir(&config_dir) {
            println!("   ⚠️  Found a file blocking config directory. Backing it up...");
            let backup = PathBuf::from(format!("{}.bak", config_dir.display()));
            sys.rename_path(&config_dir, &backup)?;
            sys.create_dir_all(&config_dir)?;
        }
    } else {
        sys.create_dir_all(&config_dir)?;
    }

    let config_path_str = config_path
        .to_str()
        .ok_or_else(|| std::io::Error::other("Invalid config path"))?;

    if !sys.path_exists(&config_path) {
        println!(
            "   🧙 We need to generate your central config.toml and configure Location Services."
        );
        let finnhub_api = Text::new(
            "Enter Finnhub.io API Key (get one by making a free account at finnhub.io/register):",
        )
        .prompt()
        .unwrap_or("YOUR_FINNHUB_KEY_HERE".to_string());
        let template = render_config_template(&finnhub_api);
        sys.write_string_to_file(config_path_str, &template)?;
        let _ = sys.run_cmd_ignore_err("chmod", &["600", config_path_str]);
        println!("  ✅ Config generated securely at {:?}", config_path);
    } else {
        let contents = sys.read_file_to_string(&config_path)?;
        if contents.contains("YOUR_FINNHUB_KEY") {
            let finnhub_api = Text::new("Enter Finnhub.io API Key (get one by making a free account at finnhub.io/register):").prompt().unwrap_or("YOUR_FINNHUB_KEY_HERE".to_string());
            if let Some(updated) = update_config_placeholders(&contents, &finnhub_api) {
                sys.write_string_to_file(config_path_str, &updated)?;
                let _ = sys.run_cmd_ignore_err("chmod", &["600", config_path_str]);
            }
        }
    }

    configure_geoclue_for_beacondb(sys)?;

    let wallpaper_path = home.join("Pictures/Wallpapers");
    if !sys.path_exists(&wallpaper_path) {
        println!(
            "   🖼️  Creating wallpaper directory at {:?}",
            wallpaper_path
        );
        sys.create_dir_all(&wallpaper_path)?;
    }
    Ok(())
}

fn render_config_template(finnhub_api: &str) -> String {
    include_str!("../../../.config/rust-dotfiles/config.toml.template")
        .replace("YOUR_FINNHUB_KEY_HERE", finnhub_api)
}

fn update_config_placeholders(contents: &str, finnhub_api: &str) -> Option<String> {
    let mut modified = false;
    let legacy_weather_block = "# -------------------------------\n# [waybar_weather]\n# Settings for our weather module\n# -------------------------------\n[waybar_weather]\nowm_api_key = \"YOUR_SECRET_OWM_KEY_HERE\"\n";
    let mut contents = contents.to_string();
    if contents.contains(legacy_weather_block) {
        contents = contents.replace(legacy_weather_block, "");
        modified = true;
    }

    let mut lines: Vec<String> = contents.lines().map(|s| s.to_string()).collect();
    for line in &mut lines {
        if line.contains("owm_api_key") || line.contains("YOUR_SECRET_OWM_KEY") {
            *line = String::new();
            modified = true;
        } else if line.contains("YOUR_FINNHUB_KEY") {
            *line = line.replace("YOUR_FINNHUB_KEY_HERE", finnhub_api);
            modified = true;
        }
    }
    if modified {
        let updated = lines
            .into_iter()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        Some(updated + "\n")
    } else {
        None
    }
}

fn configure_geoclue_for_beacondb(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    println!("   🌍 Configuring Geoclue for BeaconDB...");

    let gc_path = Path::new(GEOCLUE_CONF_PATH);
    let content = match sys.read_file_to_string(gc_path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };

    let Some(updated) = update_geoclue_config(&content) else {
        println!("   ℹ️  Geoclue already has a functional Wi-Fi geolocation config.");
        return Ok(());
    };

    sys.install_string_to_root_file(gc_path, &updated, "644")?;
    println!("   ✅ BeaconDB Geoclue configuration applied.");
    Ok(())
}

fn update_geoclue_config(contents: &str) -> Option<String> {
    let mut lines: Vec<String> = contents.lines().map(|line| line.to_string()).collect();
    let mut wifi_section_start = None;
    let mut wifi_section_end = lines.len();

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if trimmed.eq_ignore_ascii_case("[wifi]") {
                wifi_section_start = Some(index);
                continue;
            }

            if wifi_section_start.is_some() {
                wifi_section_end = index;
                break;
            }
        }
    }

    let Some(start_index) = wifi_section_start else {
        lines.push("[wifi]".to_string());
        lines.push("enable = true".to_string());
        lines.push(format!("url = {}", BEACONDB_GEOLOCATE_URL));
        return Some(lines.join("\n") + "\n");
    };

    let mut enable_line_index = None;
    let mut url_line_index = None;
    let mut enable_value = None;
    let mut url_value = None;
    let mut commented_google_url_line_index = None;

    for (index, line) in lines
        .iter()
        .enumerate()
        .take(wifi_section_end)
        .skip(start_index + 1)
    {
        let trimmed = line.trim_start();

        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('#') || trimmed.starts_with(';') {
            if let Some((comment_key, comment_value)) = parse_commented_assignment(trimmed)
                && comment_key.eq_ignore_ascii_case("url")
                && is_google_url(&comment_value)
                && !is_placeholder_google_url(&comment_value)
            {
                commented_google_url_line_index = Some(index);
            }
            continue;
        }

        if let Some((key, value)) = trimmed.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');

            if key.eq_ignore_ascii_case("enable") {
                enable_line_index = Some(index);
                enable_value = Some(value.eq_ignore_ascii_case("true"));
            } else if key.eq_ignore_ascii_case("url") {
                url_line_index = Some(index);
                url_value = Some(value.to_string());
            }
        }
    }

    // If there is an active (uncommented) valid personal Google API key:
    if let Some(ref val) = url_value
        && is_google_url(val)
        && !is_placeholder_google_url(val)
    {
        // If wifi is explicitly disabled, leave their key and leave wifi disabled.
        if matches!(enable_value, Some(false)) {
            return None;
        }
        // If it's active and enabled, leave it alone.
        return None;
    }

    if let Some(index) = commented_google_url_line_index {
        lines[index] = format!("url = {}", BEACONDB_GEOLOCATE_URL);
        upsert_enable_true(&mut lines, start_index, wifi_section_end, enable_line_index);
        return Some(lines.join("\n") + "\n");
    }

    // Otherwise (commented out URL, placeholder URL, or missing URL), apply BeaconDB and enable wifi.
    let enable_line = "enable = true".to_string();
    let url_line = format!("url = {}", BEACONDB_GEOLOCATE_URL);

    match (enable_line_index, url_line_index) {
        (Some(enable_index), Some(url_index)) => {
            lines[enable_index] = enable_line;
            lines[url_index] = url_line;
        }
        (Some(enable_index), None) => {
            lines[enable_index] = enable_line;
            lines.insert(enable_index + 1, url_line);
        }
        (None, Some(url_index)) => {
            lines[url_index] = url_line;
            lines.insert(start_index + 1, enable_line);
        }
        (None, None) => {
            lines.insert(start_index + 1, enable_line);
            lines.insert(start_index + 2, url_line);
        }
    }

    Some(lines.join("\n") + "\n")
}

fn is_google_url(value: &str) -> bool {
    let value = value.trim();
    value.contains("googleapis.com") || value.contains("geolocation/v1/geolocate")
}

fn is_placeholder_google_url(value: &str) -> bool {
    let value = value.trim();
    is_google_url(value) && value.contains("YOUR_KEY")
}

fn parse_commented_assignment(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    let body = trimmed
        .strip_prefix('#')
        .or_else(|| trimmed.strip_prefix(';'))?
        .trim_start();
    let (key, value) = body.split_once('=')?;
    Some((
        key.trim().to_string(),
        value.trim().trim_matches('"').to_string(),
    ))
}

fn upsert_enable_true(
    lines: &mut Vec<String>,
    wifi_section_start: usize,
    wifi_section_end: usize,
    enable_line_index: Option<usize>,
) {
    if let Some(index) = enable_line_index {
        lines[index] = "enable = true".to_string();
        return;
    }

    let insert_index = wifi_section_end.max(wifi_section_start + 1);
    lines.insert(insert_index, "enable = true".to_string());
}

/// Builds custom Rust apps using native caching.
/// If source files haven't changed, this takes milliseconds.
pub fn build_custom_apps(
    sys: &impl CmdExecutor,
    home: &Path,
    repo_root: &Path,
) -> Result<(), std::io::Error> {
    let sys_scripts_dir = repo_root.join("sysScripts");

    // Ensure ~/.cargo/bin exists
    let cargo_bin_dir = home.join(".cargo/bin");

    fs::create_dir_all(&cargo_bin_dir)?;

    if let Ok(entries) = fs::read_dir(&sys_scripts_dir) {
        for entry in entries.flatten() {
            let app_path = entry.path();
            if app_path.is_dir() && app_path.join("Cargo.toml").exists() {
                let app_name = match app_path.file_name().and_then(|n| n.to_str()) {
                    Some(name) => name,
                    None => {
                        println!("   ⚠️  Skipping app with invalid name at {:?}", app_path);
                        continue;
                    }
                };
                //let app_name = app_path.file_name().unwrap().to_str().unwrap();
                if sys
                    .run_cmd_in_dir(&app_path, "cargo", &["build", "--release", "-q"])
                    .is_ok()
                {
                    let release_dir = app_path.join("target/release");
                    let expected_bins = expected_binary_names(&app_path, app_name);

                    if let Ok(bin_entries) = fs::read_dir(&release_dir) {
                        for bin_entry in bin_entries.flatten() {
                            let bin_path = bin_entry.path();
                            if !bin_path.is_file() {
                                continue;
                            }

                            // On Linux, real executables have at least one execute bit set.
                            let is_executable = fs::metadata(&bin_path)
                                .map(|m| m.permissions().mode() & 0o111 != 0)
                                .unwrap_or(false);
                            if !is_executable {
                                continue;
                            }

                            // Ignore hidden entries and extension-based artifacts.
                            let filename = match bin_path.file_name() {
                                Some(name) => name.to_string_lossy().to_string(),
                                None => continue,
                            };
                            if filename.starts_with('.') || bin_path.extension().is_some() {
                                continue;
                            }
                            if !expected_bins.contains(&filename) {
                                continue;
                            }

                            let target_bin = cargo_bin_dir.join(&filename);
                            let compiled_time = fs::metadata(&bin_path).and_then(|m| m.modified());
                            let target_time = fs::metadata(&target_bin).and_then(|m| m.modified());
                            let target_exists = target_bin.exists();
                            let should_update = match (compiled_time, target_time) {
                                (Ok(c_time), Ok(t_time)) => c_time > t_time,
                                (_, Err(_)) => true,
                                _ => false,
                            };
                            if should_update {
                                if target_bin.exists() {
                                    let _ = fs::remove_file(&target_bin);
                                }
                                match fs::copy(&bin_path, &target_bin) {
                                    Ok(_) => {
                                        println!("   ✅ Synced binary: {}", filename);
                                    }
                                    Err(e) => {
                                        eprintln!("   ❌ Failed to sync {}: {}", filename, e);
                                        return Err(std::io::Error::other(format!(
                                            "Failed to sync {}: {}",
                                            filename, e
                                        )));
                                    }
                                }
                            }
                            if !should_update && target_exists {
                                println!("   ✅  {} is already up to date.", filename);
                            }
                        }
                    }
                } else {
                    println!("      ❌ Failed to build {}", app_name);
                    return Err(std::io::Error::other(format!(
                        "Failed to build {}",
                        app_name
                    )));
                }
            }
        }
    }
    Ok(())
}

///Walks through dotfiles in repo and symlinks them to home directory.
pub fn link_dotfiles_and_copy_resources(sys: &impl CmdExecutor, home: &Path, repo_root: &Path) {
    let links = vec![
        (".tmux.conf", ".tmux.conf"),
        (".profile", ".profile"),
        (".zshrc", ".zshrc"),
        (".config/waybar", ".config/waybar"),
        (".config/sway", ".config/sway"),
        (".config/hypr", ".config/hypr"),
        (".config/niri", ".config/niri"),
        (".config/rofi", ".config/rofi"),
        (".config/ghostty", ".config/ghostty"),
        (".config/fastfetch", ".config/fastfetch"),
        (".config/gtk-3.0", ".config/gtk-3.0"),
        (".config/gtk-4.0", ".config/gtk-4.0"),
        (".config/environment.d", ".config/environment.d"),
        (".config/mako", ".config/mako"),
        (".config/xdg-desktop-portal", ".config/xdg-desktop-portal"),
    ];

    for (src, dest) in links {
        let src_path = repo_root.join(src);
        let dest_path = home.join(dest);
        create_symlink(&src_path, &dest_path);
    }
    // --- SPECIAL HANDLING FOR NEOVIM ---
    // We only install this if the user has NO config, to avoid angering Vim power users.
    let nvim_dest = home.join(".config/nvim");
    if nvim_dest.exists() {
        println!(
            "   ℹ️  Neovim config found. Skipping to preserve your setup. If you would like my setup, copy {}/.config/nvim to ~/.config/nvim",
            repo_root.display()
        );
        println!("      (Note: The 'Neovim' cheat sheet in kb-launcher may not work)");
    } else {
        println!("   ✨ Installing LazyVim Config...");
        let nvim_src = repo_root.join(".config/nvim");
        create_symlink(&nvim_src, &nvim_dest);
    }

    // Copy Wallpapers
    println!("   🖼️  Seeding default wallpapers...");
    let wallpaper_src = repo_root.join("wallpapers");
    let wallpaper_dest = home.join("Pictures/Wallpapers");

    if wallpaper_src.exists() {
        if let Ok(entries) = fs::read_dir(&wallpaper_src) {
            fs::create_dir_all(&wallpaper_dest).unwrap_or_else(|e| {
                eprintln!("❌ Failed to create wallpaper destination dir: {}", e);
                std::process::exit(1);
            });
            for entry in entries.flatten() {
                let file_name = entry.file_name();
                let dest_path = wallpaper_dest.join(&file_name);
                if !dest_path.exists() {
                    let _ = fs::copy(entry.path(), dest_path);
                }
            }
            println!("   ✅ Copied wallpapers to ~/Pictures/Wallpapers");
        }
    } else {
        println!("   ⚠️  'wallpapers' directory not found in repo root.");
    }
    println!("   🏠 Updating User Directories (XDG)...");
    // This regenerates ~/.config/user-dirs.dirs and ~/.config/gtk-3.0/bookmarks
    // ensuring they point to the *current* user's home, not Michael's.
    let _ = sys.run_cmd_ignore_err("xdg-user-dirs-update", &[]);
}

/// Surgical rewrite: only updates the sidebar_toggle on-click path in ModulesCustom.
pub fn patch_waybar_sidebar_toggle_path(sys: &impl CmdExecutor, home: &Path) {
    let modules_path = home.join(".config/waybar/ModulesCustom");
    let Ok(content) = sys.read_file_to_string(&modules_path) else {
        return;
    };

    let Some(updated) = update_sidebar_toggle_path(&content) else {
        return;
    };

    let Some(modules_path_str) = modules_path.to_str() else {
        return;
    };

    match sys.write_string_to_file(modules_path_str, &updated) {
        Ok(()) => println!(
            "   ✅ Updated Waybar sidebar_toggle path in {}",
            modules_path.display()
        ),
        Err(e) => eprintln!(
            "   ⚠️ Failed to update Waybar sidebar_toggle path in {}: {}",
            modules_path.display(),
            e
        ),
    }
}

fn update_sidebar_toggle_path(content: &str) -> Option<String> {
    let entry_start = content.find("\"custom/sidebar_toggle\"")?;
    let open_brace_rel = content[entry_start..].find('{')?;

    let block_start = entry_start + open_brace_rel;
    let mut depth = 0;
    let mut block_end = None;

    for (offset, ch) in content[block_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    block_end = Some(block_start + offset);
                    break;
                }
            }
            _ => {}
        }
    }

    let block_end = block_end?;

    let old_path = "$HOME/rust-wayland-power/sidebar-toggle";
    let new_path = "$HOME/Genoa/sidebar-toggle";
    let block = &content[block_start..=block_end];
    if !block.contains(old_path) {
        return None;
    }

    let updated_block = block.replacen(old_path, new_path, 1);
    let mut updated = String::with_capacity(content.len() - block.len() + updated_block.len());
    updated.push_str(&content[..block_start]);
    updated.push_str(&updated_block);
    updated.push_str(&content[block_end + 1..]);

    Some(updated)
}

/// Runs post-install hooks to set up themes and plugins.
/// This ensures the user doesn't see "broken" visuals on first launch.
pub fn finalize_setup(sys: &impl CmdExecutor, home: &Path) {
    println!(
        "\n{}",
        "✨ Finalizing Setup (Themes & Plugins)...".blue().bold()
    );

    // 1. Install Tmux Plugins (Fixes the Green Bar)
    let tpm_script = home.join(".tmux/plugins/tpm/bin/install_plugins");
    if tpm_script.exists() {
        println!("   📦 Installing Tmux Plugins (Headless)...");
        // We capture output to avoid spamming the user's terminal unless it fails
        if sys
            .run_cmd(
                tpm_script.to_str().unwrap_or("/tmp/tpm-install-plugins"),
                &[],
            )
            .is_ok()
        {
            println!("   ✅ Tmux Plugins Installed");
        } else {
            println!("   ⚠️  Tmux plugin install failed (You can press Prefix + I inside Tmux)");
        }
    }

    // 2. Install Neovim Plugins (Lazy.nvim)
    // Only run if we actually installed the config (check if dest exists)
    let nvim_config = home.join(".config/nvim/init.lua"); // Check for main config file
    if nvim_config.exists() {
        println!("   📦 Bootstrapping Neovim (Lazy.nvim)...");
        // --headless: Don't open a UI
        // "+Lazy! sync": Run the sync command
        // "+qa": Quit All after finishing
        if sys
            .run_cmd("nvim", &["--headless", "+Lazy! sync", "+qa"])
            .is_ok()
        {
            println!("   ✅ Neovim Plugins Synced");
        } else {
            println!("   ⚠️  Neovim setup skipped (will run on first launch)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_env::MockEnv;
    use std::path::Path;

    #[test]
    fn test_update_config_placeholders_replaces_keys() {
        let original = "finnhub = \"YOUR_FINNHUB_KEY_HERE\"\n";
        let updated = update_config_placeholders(original, "fin-key").expect("no update");
        assert!(updated.contains("fin-key"));
    }

    #[test]
    fn test_update_config_placeholders_removes_legacy_weather_section() {
        let original = r#"# -------------------------------
# [waybar_weather]
# Settings for our weather module
# -------------------------------
[waybar_weather]
owm_api_key = "YOUR_SECRET_OWM_KEY_HERE"

[waybar_finance]
api_key = "YOUR_FINNHUB_KEY_HERE"
"#;
        let updated = update_config_placeholders(original, "fin-key").expect("no update");
        assert!(!updated.contains("waybar_weather"));
        assert!(!updated.contains("YOUR_SECRET_OWM_KEY"));
        assert!(updated.contains("fin-key"));
    }

    #[test]
    fn test_update_geoclue_config_keeps_functional_existing_url() {
        let original = r#"[wifi]
enable = true
url = "https://maps.googleapis.com/maps/api/geolocation/v1/geolocate?key=personal"
"#;

        assert!(update_geoclue_config(original).is_none());
    }

    #[test]
    fn test_update_geoclue_config_preserves_google_when_wifi_disabled() {
        let original = r#"[wifi]
enable = false
url = "https://maps.googleapis.com/maps/api/geolocation/v1/geolocate?key=personal"
"#;

        assert!(update_geoclue_config(original).is_none());
    }

    #[test]
    fn test_update_geoclue_config_replaces_commented_google_url() {
        let original = r#"[wifi]
enable = false
# url = "https://maps.googleapis.com/maps/api/geolocation/v1/geolocate?key=personal"
"#;

        let updated = update_geoclue_config(original).expect("expected update");
        assert!(updated.contains(BEACONDB_GEOLOCATE_URL));
        assert!(updated.contains("enable = true"));
        assert!(!updated.contains("maps.googleapis.com"));
    }

    #[test]
    fn test_update_geoclue_config_replaces_dead_mozilla_url() {
        let original = r#"[wifi]
enable = false
url = https://location.services.mozilla.com/v1/geolocate
"#;

        let updated = update_geoclue_config(original).expect("expected update");
        assert!(updated.contains(BEACONDB_GEOLOCATE_URL));
        assert!(updated.contains("enable = true"));
        assert_eq!(
            updated
                .lines()
                .filter(|line| line.trim_start().starts_with("url"))
                .count(),
            1
        );
    }

    #[test]
    fn test_update_geoclue_config_replaces_google_placeholder_template() {
        let original = r#"[wifi]
# url=https://www.googleapis.com/geolocation/v1/geolocate?key=YOUR_KEY
"#;

        let updated = update_geoclue_config(original).expect("expected update");
        assert!(updated.contains(BEACONDB_GEOLOCATE_URL));
        assert!(updated.contains("enable = true"));
        assert_eq!(
            updated
                .lines()
                .filter(|line| line.trim_start().starts_with("url"))
                .count(),
            1
        );
    }

    #[test]
    fn test_update_sidebar_toggle_path_replaces_repo() {
        let original = r#"{
    "custom/sidebar_toggle": {
        "on-click": "$HOME/rust-wayland-power/sidebar-toggle"
    }
}
"#;
        let updated = update_sidebar_toggle_path(original).expect("no update");
        assert!(updated.contains("$HOME/Genoa/sidebar-toggle"));
    }

    #[test]
    fn test_setup_waybar_configs_copies_template() {
        let env = MockEnv::default();
        let home = Path::new("/home/testuser");
        env.mock_files.borrow_mut().insert(
            "/home/testuser/.config/waybar/swayConfig.jsonc.template".to_string(),
            "{\"test\": true}".to_string(),
        );
        setup_waybar_configs(&env, home);
        let binding = env.mock_files.borrow();
        let created = binding
            .get("/home/testuser/.config/waybar/swayConfig.jsonc")
            .unwrap();
        assert_eq!(created, "{\"test\": true}");
    }
}
