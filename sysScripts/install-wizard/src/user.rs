use crate::helpers::{create_symlink, expected_binary_names};
use crate::traits::CmdExecutor;
use colored::*;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const GEOCLUE_CONF_PATH: &str = "/etc/geoclue/geoclue.conf";
const BEACONDB_GEOLOCATE_URL: &str = "https://api.beacondb.net/v1/geolocate";
const SYSTEM_RUSTUP: &str = "/usr/bin/rustup";
const CONFIG_TEMPLATE: &str = include_str!("../../../.config/rust-dotfiles/config.toml.template");

const RETIRED_TOOL_SOURCES: &[&str] = &[
    "sysScripts/sidebar/build.rs",
    "sysScripts/sidebar/src/calendar_query.c",
];

const GTKLOCK_CONFIG_MIGRATIONS: &[(&str, &[(&str, &str)])] = &[
    (
        ".config/sway/swayidle.sh",
        &[
            (
                "flock -n /tmp/gtklock.lock gtklock -d",
                "/usr/local/bin/genoa-lock",
            ),
            (
                "before-sleep 'sh -c \"flock -n /tmp/gtklock.lock gtklock &\"'",
                "before-sleep '/usr/local/bin/genoa-lock'",
            ),
        ],
    ),
    (
        ".config/sway/UserConfigs/UserKeybinds.conf",
        &[
            (
                "exec sh -c 'flock -n /tmp/gtklock.lock gtklock -d; systemctl suspend'",
                "exec /usr/local/bin/genoa-lock --suspend-after-lock",
            ),
            (
                "exec sh -c 'flock -n /tmp/gtklock.lock gtklock -d'",
                "exec /usr/local/bin/genoa-lock",
            ),
        ],
    ),
    (
        ".config/niri/config.kdl",
        &[
            (
                "spawn-sh \"flock -n /tmp/gtklock.lock gtklock -d; systemctl suspend\"",
                "spawn \"/usr/local/bin/genoa-lock\" \"--suspend-after-lock\"",
            ),
            (
                "spawn-sh \"flock -n /tmp/gtklock.lock gtklock -d\"",
                "spawn \"/usr/local/bin/genoa-lock\"",
            ),
            (
                "before-sleep 'sh -c \\\"flock -n /tmp/gtklock.lock gtklock &\\\"'",
                "before-sleep '/usr/local/bin/genoa-lock'",
            ),
            (
                "flock -n /tmp/gtklock.lock gtklock -d",
                "/usr/local/bin/genoa-lock",
            ),
        ],
    ),
];

const IDLE_SERVICE_CONFIG_MIGRATIONS: &[(&str, &[(&str, &str)])] = &[
    (
        ".config/niri/config.kdl",
        &[(
            r#"spawn-sh-at-startup "swayidle -w timeout 200 'brightnessctl set 0%' resume 'brightnessctl set 60%' timeout 600 '/usr/local/bin/genoa-lock' timeout 630 'NIRI_SOCKET=\"$NIRI_SOCKET\" niri msg action power-off-monitors' resume 'NIRI_SOCKET=\"$NIRI_SOCKET\" niri msg action power-on-monitors' timeout 1200 'systemctl suspend' before-sleep '/usr/local/bin/genoa-lock'""#,
            r#"spawn-sh-at-startup "/usr/bin/dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP NIRI_SOCKET; /usr/bin/systemctl --user import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP NIRI_SOCKET; /usr/bin/systemctl --user restart genoa-niri-idle.service""#,
        )],
    ),
    (
        ".config/sway/UserConfigs/Startup_Apps.conf",
        &[
            (
                "dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP",
                "dbus-update-activation-environment --systemd WAYLAND_DISPLAY XDG_CURRENT_DESKTOP SWAYSOCK",
            ),
            (
                "systemctl --user import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP",
                "systemctl --user import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP SWAYSOCK",
            ),
            (
                "exec_always ~/.config/sway/swayidle.sh",
                "exec_always systemctl --user restart genoa-sway-idle.service",
            ),
        ],
    ),
];

const GEOCLUE_AGENT: &str = "/usr/lib/geoclue-2.0/demos/agent";
const AUTOMATIC_TIMEZONE_MIGRATION_MARKER: &str = ".automatic-timezone-v1";
const LOCATION_STARTUP_MIGRATIONS: &[(&str, &str, &str)] = &[
    (
        ".config/niri/config.kdl",
        "spawn-at-startup \"/usr/lib/gsd-datetime\"",
        "spawn-at-startup \"/usr/lib/geoclue-2.0/demos/agent\"",
    ),
    (
        ".config/sway/UserConfigs/Startup_Apps.conf",
        "exec /usr/lib/gsd-datetime",
        "exec /usr/lib/geoclue-2.0/demos/agent",
    ),
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

/// Rewrites Genoa's former gtklock launch sites without overwriting unrelated
/// user changes. Returns false when a non-commented gtklock reference remains,
/// which tells the caller to preserve the old package rather than leave the
/// user with no working locker.
pub fn migrate_gtklock_lock_commands(
    sys: &impl CmdExecutor,
    home: &Path,
) -> Result<bool, std::io::Error> {
    let mut safe_to_retire = true;

    for (relative_path, replacements) in GTKLOCK_CONFIG_MIGRATIONS {
        let path = home.join(relative_path);
        if !sys.path_exists(&path) {
            continue;
        }

        let original = sys.read_file_to_string(&path)?;
        let mut updated = original.clone();
        for (from, to) in *replacements {
            updated = updated.replace(from, to);
        }
        if updated != original {
            let path = path
                .to_str()
                .ok_or_else(|| std::io::Error::other("Gtklock config path is not valid UTF-8"))?;
            sys.write_string_to_file(path, &updated)?;
        }
        if has_active_gtklock_reference(&updated) {
            eprintln!(
                "   ⚠️  Active gtklock command remains in {}; manual migration is required.",
                path.display()
            );
            safe_to_retire = false;
        }
    }

    Ok(safe_to_retire)
}

/// Moves only Genoa's known direct swayidle startup commands to the managed
/// service. Other user idle commands remain untouched.
pub fn migrate_idle_service_commands(
    sys: &impl CmdExecutor,
    home: &Path,
) -> Result<(), std::io::Error> {
    for (relative_path, replacements) in IDLE_SERVICE_CONFIG_MIGRATIONS {
        let path = home.join(relative_path);
        if !sys.path_exists(&path) {
            continue;
        }

        let original = sys.read_file_to_string(&path)?;
        let mut updated = original.clone();
        for (from, to) in *replacements {
            updated = updated.replace(from, to);
        }
        if updated != original {
            let path = path.to_str().ok_or_else(|| {
                std::io::Error::other("idle service config path is not valid UTF-8")
            })?;
            sys.write_string_to_file(path, &updated)?;
        }
    }
    Ok(())
}

/// Adds the GeoClue authorization agent beside Genoa's GNOME Date & Time
/// service without replacing a user's compositor configuration. The agent is
/// already present in current Niri configs; this migration repairs older Niri
/// files and brings Sway to feature parity.
pub fn migrate_location_service_startup(
    sys: &impl CmdExecutor,
    home: &Path,
) -> Result<(), std::io::Error> {
    for (relative_path, datetime_startup, agent_startup) in LOCATION_STARTUP_MIGRATIONS {
        let path = home.join(relative_path);
        if !sys.path_exists(&path) {
            continue;
        }

        let original = sys.read_file_to_string(&path)?;
        let Some(updated) =
            insert_startup_command_after(&original, datetime_startup, agent_startup, GEOCLUE_AGENT)
        else {
            continue;
        };

        let path_string = path
            .to_str()
            .ok_or_else(|| std::io::Error::other("Location config path is not valid UTF-8"))?;
        sys.write_string_to_file(path_string, &updated)?;
    }

    Ok(())
}

fn insert_startup_command_after(
    contents: &str,
    datetime_startup: &str,
    agent_startup: &str,
    agent_path: &str,
) -> Option<String> {
    if has_active_command(contents, agent_path) {
        return None;
    }

    let mut updated = String::with_capacity(contents.len() + agent_startup.len() + 1);
    let mut inserted = false;

    for line in contents.split_inclusive('\n') {
        let line_without_newline = line.strip_suffix('\n').unwrap_or(line);
        updated.push_str(line);

        if !inserted && line_without_newline.trim() == datetime_startup {
            if !line.ends_with('\n') {
                updated.push('\n');
            }
            let indent_len = line_without_newline.len() - line_without_newline.trim_start().len();
            updated.push_str(&line_without_newline[..indent_len]);
            updated.push_str(agent_startup);
            updated.push('\n');
            inserted = true;
        }
    }

    inserted.then_some(updated)
}

fn has_active_command(contents: &str, command: &str) -> bool {
    contents.lines().any(|line| {
        let line = line.trim_start();
        !line.starts_with('#') && !line.starts_with("//") && line.contains(command)
    })
}

/// Deletes only the retired gtklock configuration directory, after the caller
/// has installed Genoa Lock and removed every active gtklock launch path.
pub fn remove_retired_gtklock_config(
    sys: &impl CmdExecutor,
    home: &Path,
) -> Result<(), std::io::Error> {
    let config_dir = home.join(".config/gtklock");
    if sys.path_is_dir(&config_dir) {
        sys.remove_dir_all(&config_dir)?;
    }
    Ok(())
}

fn has_active_gtklock_reference(contents: &str) -> bool {
    contents.lines().any(|line| {
        let line = line.trim_start();
        !line.starts_with('#') && !line.starts_with("//") && line.contains("gtklock")
    })
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

/// Sets GNOME Papers as the default PDF viewer for a fresh Genoa installation.
pub fn setup_papers(sys: &impl CmdExecutor) {
    println!("   📄 Setting GNOME Papers as the default PDF viewer...");
    for mime in ["application/pdf", "application/x-pdf"] {
        let _ = sys.run_cmd_ignore_err("xdg-mime", &["default", "org.gnome.Papers.desktop", mime]);
    }
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
        }
    }
}
/// Generates the local `config.toml` and configures location services.
pub fn setup_secrets_and_geoclue(
    sys: &impl CmdExecutor,
    home: &Path,
) -> Result<(), std::io::Error> {
    let config_dir = home.join(".config/rust-dotfiles");
    let config_path = config_dir.join("config.toml");
    // Logic to handle if 'rust-dotfiles' exists as a file instead of a directory
    if sys.path_exists(&config_dir) && !sys.path_is_dir(&config_dir) {
        println!("   ⚠️  Found a file blocking config directory. Backing it up...");
        let backup = PathBuf::from(format!("{}.bak", config_dir.display()));
        sys.rename_path(&config_dir, &backup)?;
    }
    sys.ensure_private_dir(&config_dir)?;

    if !sys.path_exists(&config_path) {
        println!("   🧙 Generating your central config.toml and configuring Location Services.");
        sys.write_private_string_to_file(&config_path, CONFIG_TEMPLATE)?;
        println!("  ✅ Config generated securely at {:?}", config_path);
    } else {
        let needs_repair = sys.private_file_needs_repair(&config_path)?;
        let contents = sys.read_file_to_string(&config_path)?;
        match contents.parse::<toml_edit::DocumentMut>() {
            Ok(mut doc) => {
                if migrate_config_document(&mut doc) {
                    sys.write_private_string_to_file(&config_path, &doc.to_string())?;
                } else if needs_repair {
                    sys.write_private_string_to_file(&config_path, &contents)?;
                }
            }
            Err(error) => {
                eprintln!(
                    "   ⚠️  config.toml is not valid TOML; skipping config migration: {error}"
                );
                if needs_repair {
                    sys.write_private_string_to_file(&config_path, &contents)?;
                }
            }
        }
    }

    migrate_local_finance_config(sys, &home.join(".config/waybar-finance/config.json"))?;

    configure_geoclue_for_beacondb(sys)?;
    configure_automatic_timezone(sys, &config_dir);
    migrate_location_service_startup(sys, home)?;

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

/// Sets Genoa's automatic-timezone default once, then records the migration
/// privately so later user choices are never overwritten by an update.
fn configure_automatic_timezone(sys: &impl CmdExecutor, config_dir: &Path) {
    let marker = config_dir.join(AUTOMATIC_TIMEZONE_MIGRATION_MARKER);
    if sys.path_exists(&marker) {
        return;
    }
    if !sys.command_exists("gsettings") {
        return;
    }

    let automatic_timezone_enabled = sys
        .run_cmd(
            "gsettings",
            &[
                "set",
                "org.gnome.desktop.datetime",
                "automatic-timezone",
                "true",
            ],
        )
        .is_ok();
    let location_enabled = sys
        .run_cmd(
            "gsettings",
            &["set", "org.gnome.system.location", "enabled", "true"],
        )
        .is_ok();

    if !automatic_timezone_enabled || !location_enabled {
        eprintln!(
            "   ⚠️  Could not enable automatic timezone and location access in this session."
        );
        return;
    }

    if let Err(error) = sys.write_private_string_to_file(&marker, "enabled\n") {
        eprintln!(
            "   ⚠️  Automatic timezone is enabled, but its migration marker could not be saved: {error}"
        );
    } else {
        println!("   ✅ Automatic timezone and location access enabled.");
    }
}

/// Removes the retired Finnhub key from the app-local watchlist while retaining
/// the user's stocks, then repairs private file permissions when needed.
fn migrate_local_finance_config(
    sys: &impl CmdExecutor,
    config_path: &Path,
) -> Result<(), std::io::Error> {
    if !sys.path_exists(config_path) {
        return Ok(());
    }
    let parent = config_path
        .parent()
        .ok_or_else(|| std::io::Error::other("Finance config path has no parent directory"))?;
    sys.ensure_private_dir(parent)?;

    let needs_repair = sys.private_file_needs_repair(config_path)?;
    let content = sys.read_file_to_string(config_path)?;
    let migrated = serde_json::from_str::<serde_json::Value>(&content)
        .ok()
        .and_then(|mut config| {
            config
                .as_object_mut()
                .and_then(|object| object.remove("api_key"))?;
            serde_json::to_string_pretty(&config).ok()
        });

    if let Some(content) = migrated {
        sys.write_private_string_to_file(config_path, &content)?;
    } else if needs_repair {
        sys.write_private_string_to_file(config_path, &content)?;
    }
    Ok(())
}

/// Applies configuration migrations to an already parsed document.
fn migrate_config_document(doc: &mut toml_edit::DocumentMut) -> bool {
    let mut modified = false;

    let has_legacy_weather_key = doc
        .get("waybar_weather")
        .and_then(toml_edit::Item::as_table_like)
        .and_then(|table| table.get("owm_api_key"))
        .and_then(toml_edit::Item::as_str)
        .is_some_and(|key| key.contains("YOUR_SECRET_OWM_KEY"));
    if has_legacy_weather_key {
        doc.remove("waybar_weather");
        modified = true;
    }

    let removed_legacy_finnhub_key = doc
        .get_mut("waybar_finance")
        .and_then(toml_edit::Item::as_table_like_mut)
        .and_then(|table| table.remove("api_key"))
        .is_some();
    modified |= removed_legacy_finnhub_key;

    modified
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
                    .run_cmd_in_dir(
                        &app_path,
                        SYSTEM_RUSTUP,
                        &["run", "stable", "cargo", "build", "--release", "-q"],
                    )
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
pub fn link_dotfiles_and_copy_resources(
    sys: &impl CmdExecutor,
    home: &Path,
    repo_root: &Path,
) -> Result<(), std::io::Error> {
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
        create_symlink(&src_path, &dest_path)?;
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
        create_symlink(&nvim_src, &nvim_dest)?;
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
    Ok(())
}

/// Surgical rewrite of Genoa-owned Waybar commands in ModulesCustom.
///
/// Users are free to keep their own modules and commands. These replacements
/// are deliberately limited to the exact legacy command values Genoa shipped.
pub fn patch_waybar_known_genoa_commands(sys: &impl CmdExecutor, home: &Path) {
    let modules_path = home.join(".config/waybar/ModulesCustom");
    let Ok(content) = sys.read_file_to_string(&modules_path) else {
        return;
    };

    let mut updated = content.clone();
    let mut changed = false;
    if let Some(patched) = update_sidebar_toggle_path(&updated) {
        updated = patched;
        changed = true;
    }
    if let Some(patched) = update_waybar_lock_command(&updated) {
        updated = patched;
        changed = true;
    }
    if !changed {
        return;
    }

    let Some(modules_path_str) = modules_path.to_str() else {
        return;
    };

    match sys.write_string_to_file(modules_path_str, &updated) {
        Ok(()) => println!(
            "   ✅ Updated known Genoa Waybar commands in {}",
            modules_path.display()
        ),
        Err(e) => eprintln!(
            "   ⚠️ Failed to update Waybar sidebar_toggle path in {}: {}",
            modules_path.display(),
            e
        ),
    }
}

fn update_waybar_lock_command(content: &str) -> Option<String> {
    const LEGACY_COMMAND: &str =
        "pidof hyprlock >/dev/null || (hyprlock & sleep 0.5); systemctl suspend";
    const GENOA_COMMAND: &str = "/usr/local/bin/genoa-lock --suspend-after-lock";

    let entry_start = content.find("\"custom/lock\"")?;
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
    let block = &content[block_start..=block_end];
    if !block.contains(LEGACY_COMMAND) {
        return None;
    }

    let updated_block = block.replacen(LEGACY_COMMAND, GENOA_COMMAND, 1);
    let mut updated = String::with_capacity(content.len() - block.len() + updated_block.len());
    updated.push_str(&content[..block_start]);
    updated.push_str(&updated_block);
    updated.push_str(&content[block_end + 1..]);
    Some(updated)
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
            configure_neovim_health(sys);
        } else {
            println!("   ⚠️  Neovim setup skipped (will run on first launch)");
        }
    }
}

/// Installs the Rust tooling and Tree-sitter parsers required by this Neovim configuration.
/// Lazy.nvim is synchronized first so the `:TSInstall` command is available in headless Neovim.
fn configure_neovim_health(sys: &impl CmdExecutor) {
    println!("   🦀 Installing Rust analyzer and Clippy...");
    if sys
        .run_cmd(
            SYSTEM_RUSTUP,
            &["component", "add", "rust-analyzer", "clippy"],
        )
        .is_ok()
    {
        println!("   ✅ Rust analyzer and Clippy ready");
    } else {
        println!("   ⚠️  Rust component setup failed");
    }

    println!("   🌳 Installing Neovim Tree-sitter parsers...");
    if sys
        .run_cmd(
            "nvim",
            &[
                "--headless",
                "+TSInstall css latex norg scss svelte typst vue",
                "+qa",
            ],
        )
        .is_ok()
    {
        println!("   ✅ Neovim Tree-sitter parsers installed");
    } else {
        println!("   ⚠️  Tree-sitter parser setup failed (run :TSInstall manually in Neovim)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_env::MockEnv;
    use std::path::Path;

    #[test]
    fn location_startup_migration_adds_the_agent_without_touching_other_lines() {
        let env = MockEnv::default();
        let home = Path::new("/home/testuser");
        let niri_path = home.join(".config/niri/config.kdl");
        let sway_path = home.join(".config/sway/UserConfigs/Startup_Apps.conf");
        env.mock_files.borrow_mut().insert(
            niri_path.to_string_lossy().to_string(),
            "spawn-at-startup \"/usr/lib/gsd-datetime\"\nspawn-at-startup \"nm-applet\"\n"
                .to_string(),
        );
        env.mock_files.borrow_mut().insert(
            sway_path.to_string_lossy().to_string(),
            "# startup\nexec /usr/lib/gsd-datetime\nexec nm-applet\n".to_string(),
        );

        migrate_location_service_startup(&env, home).unwrap();

        assert_eq!(
            env.read_file_to_string(&niri_path).unwrap(),
            "spawn-at-startup \"/usr/lib/gsd-datetime\"\nspawn-at-startup \"/usr/lib/geoclue-2.0/demos/agent\"\nspawn-at-startup \"nm-applet\"\n"
        );
        assert_eq!(
            env.read_file_to_string(&sway_path).unwrap(),
            "# startup\nexec /usr/lib/gsd-datetime\nexec /usr/lib/geoclue-2.0/demos/agent\nexec nm-applet\n"
        );
    }

    #[test]
    fn location_startup_migration_preserves_an_existing_agent() {
        let env = MockEnv::default();
        let home = Path::new("/home/testuser");
        let niri_path = home.join(".config/niri/config.kdl");
        let original = "spawn-at-startup \"/usr/lib/gsd-datetime\"\nspawn-at-startup \"/usr/lib/geoclue-2.0/demos/agent\"\n";
        env.mock_files.borrow_mut().insert(
            niri_path.to_string_lossy().to_string(),
            original.to_string(),
        );

        migrate_location_service_startup(&env, home).unwrap();

        assert_eq!(env.read_file_to_string(&niri_path).unwrap(), original);
    }

    #[test]
    fn idle_service_migration_rewrites_only_known_niri_startup() {
        let env = MockEnv::default();
        let home = Path::new("/home/testuser");
        let niri_path = home.join(".config/niri/config.kdl");
        let legacy = IDLE_SERVICE_CONFIG_MIGRATIONS[0].1[0].0;
        let managed = IDLE_SERVICE_CONFIG_MIGRATIONS[0].1[0].1;
        env.mock_files.borrow_mut().insert(
            niri_path.to_string_lossy().to_string(),
            format!("{legacy}\ncustom line\n"),
        );

        migrate_idle_service_commands(&env, home).unwrap();

        assert_eq!(
            env.read_file_to_string(&niri_path).unwrap(),
            format!("{managed}\ncustom line\n")
        );
    }

    #[test]
    fn automatic_timezone_migration_enables_both_settings_once() {
        let mut env = MockEnv::default();
        env.available_commands.insert("gsettings".to_string());
        let config_dir = Path::new("/home/testuser/.config/rust-dotfiles");

        configure_automatic_timezone(&env, config_dir);
        configure_automatic_timezone(&env, config_dir);

        assert_eq!(
            *env.cmd_log.borrow(),
            vec![
                (
                    "gsettings".to_string(),
                    vec![
                        "set".to_string(),
                        "org.gnome.desktop.datetime".to_string(),
                        "automatic-timezone".to_string(),
                        "true".to_string(),
                    ],
                ),
                (
                    "gsettings".to_string(),
                    vec![
                        "set".to_string(),
                        "org.gnome.system.location".to_string(),
                        "enabled".to_string(),
                        "true".to_string(),
                    ],
                ),
            ]
        );
        assert_eq!(
            env.read_file_to_string(&config_dir.join(AUTOMATIC_TIMEZONE_MIGRATION_MARKER))
                .unwrap(),
            "enabled\n"
        );
    }

    #[test]
    fn automatic_timezone_migration_retries_when_gsettings_is_unavailable() {
        let env = MockEnv::default();
        let config_dir = Path::new("/home/testuser/.config/rust-dotfiles");

        configure_automatic_timezone(&env, config_dir);

        assert!(env.cmd_log.borrow().is_empty());
        assert!(!env.path_exists(&config_dir.join(AUTOMATIC_TIMEZONE_MIGRATION_MARKER)));
    }

    #[test]
    fn gtklock_migration_preserves_lock_before_suspend_order() {
        let env = MockEnv::default();
        let home = Path::new("/home/testuser");
        let niri_path = home.join(".config/niri/config.kdl");
        env.mock_files.borrow_mut().insert(
            niri_path.to_string_lossy().to_string(),
            "spawn-sh \"flock -n /tmp/gtklock.lock gtklock -d; systemctl suspend\"\n".to_string(),
        );

        assert!(migrate_gtklock_lock_commands(&env, home).unwrap());

        let migrated = env
            .mock_files
            .borrow()
            .get(&niri_path.to_string_lossy().to_string())
            .cloned()
            .unwrap();
        assert_eq!(
            migrated,
            "spawn \"/usr/local/bin/genoa-lock\" \"--suspend-after-lock\"\n"
        );
    }

    #[test]
    fn gtklock_migration_keeps_the_old_locker_for_unknown_active_commands() {
        let env = MockEnv::default();
        let home = Path::new("/home/testuser");
        let sway_path = home.join(".config/sway/swayidle.sh");
        env.mock_files.borrow_mut().insert(
            sway_path.to_string_lossy().to_string(),
            "timeout 30 'gtklock --custom-flag'\n".to_string(),
        );

        assert!(!migrate_gtklock_lock_commands(&env, home).unwrap());
    }

    #[test]
    fn idle_service_migration_rewrites_only_known_niri_startup() {
        let env = MockEnv::default();
        let home = Path::new("/home/testuser");
        let niri_path = home.join(".config/niri/config.kdl");
        let (legacy, managed) = IDLE_SERVICE_CONFIG_MIGRATIONS[0].1[0];
        env.mock_files.borrow_mut().insert(
            niri_path.to_string_lossy().to_string(),
            format!("{legacy}\ncustom line\n"),
        );

        migrate_idle_service_commands(&env, home).unwrap();

        assert_eq!(
            env.mock_files
                .borrow()
                .get(&niri_path.to_string_lossy().to_string()),
            Some(&format!("{managed}\ncustom line\n"))
        );
    }

    #[test]
    fn retired_gtklock_config_is_removed_only_when_present() {
        let env = MockEnv::default();
        let home = Path::new("/home/testuser");
        let config_dir = home.join(".config/gtklock");
        env.mock_dirs
            .borrow_mut()
            .insert(config_dir.to_string_lossy().to_string());

        remove_retired_gtklock_config(&env, home).unwrap();

        assert!(!env.path_exists(&config_dir));
    }

    #[test]
    fn config_document_migration_removes_legacy_finnhub_key_but_preserves_stocks() {
        let original = r#"[waybar_finance]
api_key = "existing-key"
stocks = ["SPY", "QQQ"]

[other]
setting = "preserved"
"#;
        let mut document = original.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(migrate_config_document(&mut document));

        let updated = document.to_string();
        assert!(!updated.contains("api_key"));
        assert!(updated.contains("stocks = [\"SPY\", \"QQQ\"]"));
        assert!(updated.contains("[other]"));
        assert!(updated.contains("setting = \"preserved\""));
    }

    #[test]
    fn local_finance_config_migration_removes_the_retired_api_key() {
        let env = MockEnv::default();
        let config_path = Path::new("/home/student/.config/waybar-finance/config.json");
        env.mock_files.borrow_mut().insert(
            config_path.display().to_string(),
            r#"{
  "api_key": "existing-key",
  "stocks": ["SPY", "QQQ"]
}"#
            .to_string(),
        );

        migrate_local_finance_config(&env, config_path).unwrap();

        let updated = env.read_file_to_string(config_path).unwrap();
        assert!(!updated.contains("api_key"));
        assert!(updated.contains("SPY"));
        assert!(updated.contains("QQQ"));
    }

    #[test]
    fn test_configure_neovim_health_installs_rust_and_treesitter_dependencies() {
        let env = MockEnv::default();

        configure_neovim_health(&env);

        assert_eq!(
            *env.cmd_log.borrow(),
            vec![
                (
                    "/usr/bin/rustup".to_string(),
                    vec![
                        "component".to_string(),
                        "add".to_string(),
                        "rust-analyzer".to_string(),
                        "clippy".to_string(),
                    ],
                ),
                (
                    "nvim".to_string(),
                    vec![
                        "--headless".to_string(),
                        "+TSInstall css latex norg scss svelte typst vue".to_string(),
                        "+qa".to_string(),
                    ],
                ),
            ]
        );
    }

    #[test]
    fn test_setup_papers_sets_pdf_associations() {
        let env = MockEnv::default();

        setup_papers(&env);

        assert_eq!(
            *env.cmd_log.borrow(),
            vec![
                (
                    "xdg-mime".to_string(),
                    vec![
                        "default".to_string(),
                        "org.gnome.Papers.desktop".to_string(),
                        "application/pdf".to_string(),
                    ],
                ),
                (
                    "xdg-mime".to_string(),
                    vec![
                        "default".to_string(),
                        "org.gnome.Papers.desktop".to_string(),
                        "application/x-pdf".to_string(),
                    ],
                ),
            ]
        );
    }

    #[test]
    fn legacy_weather_and_finnhub_key_migrate_together() {
        let original = r#"# -------------------------------
# [waybar_weather]
# Settings for our weather module
# -------------------------------
[waybar_weather]
owm_api_key = "YOUR_SECRET_OWM_KEY_HERE"

[waybar_finance]
api_key = "already-configured"
"#;
        let mut document = original.parse::<toml_edit::DocumentMut>().unwrap();
        assert!(migrate_config_document(&mut document));

        let updated = document.to_string();
        assert!(!updated.contains("waybar_weather"));
        assert!(!updated.contains("YOUR_SECRET_OWM_KEY"));
        assert!(!updated.contains("api_key"));
    }

    #[test]
    fn bundled_config_template_has_no_finnhub_placeholder() {
        let document = CONFIG_TEMPLATE.parse::<toml_edit::DocumentMut>().unwrap();

        assert!(
            document
                .get("waybar_finance")
                .and_then(toml_edit::Item::as_table_like)
                .and_then(|table| table.get("api_key"))
                .is_none()
        );
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
    fn update_waybar_lock_command_retires_the_known_hyprlock_suspend_chain() {
        let original = r#"{
    "custom/lock": {
        "on-click": "pidof hyprlock >/dev/null || (hyprlock & sleep 0.5); systemctl suspend"
    }
}
"#;

        let updated = update_waybar_lock_command(original).expect("no update");
        assert!(updated.contains("/usr/local/bin/genoa-lock --suspend-after-lock"));
        assert!(!updated.contains("hyprlock"));
    }

    #[test]
    fn update_waybar_lock_command_leaves_custom_commands_alone() {
        let original = r#"{
    "custom/lock": {
        "on-click": "my-lock-command"
    }
}
"#;

        assert!(update_waybar_lock_command(original).is_none());
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
