use colored::*;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const NEW_REPO_DIR: &str = "Genoa";
const LEGACY_REPO_DIR: &str = "rust-wayland-power";

/// Silently transitions existing users from the legacy hardcoded paths
/// to the new dynamic, config-driven architecture.
pub fn migrate_legacy_users(home: &Path) {
    let old_repo = home.join(LEGACY_REPO_DIR);
    let new_repo = home.join(NEW_REPO_DIR);

    // If the old repo exists, we have a legacy user who needs rescuing
    if old_repo.exists() {
        println!(
            "\n{}",
            "🔄 Legacy installation detected. Silently migrating system...".magenta()
        );

        // 1. Move the physical folder to the new name
        // (This is safe because this binary is currently running from ~/.cargo/bin/)
        if !new_repo.exists()
            && let Err(e) = fs::rename(&old_repo, &new_repo)
        {
            eprintln!("   ⚠️ Failed to rename repository folder: {}", e);
            return; // Abort migration, let them safely remain on the old folder for now
        }

        let active_repo = if new_repo.exists() {
            &new_repo
        } else {
            &old_repo
        };

        // 2. Preserve transport (SSH vs HTTPS) and only swap repo path.
        if let Ok(output) = Command::new("git")
            .current_dir(active_repo)
            .args(["remote", "get-url", "origin"])
            .output()
        {
            if output.status.success() {
                let current_origin = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let migrated_origin = current_origin
                    .replace(
                        "Mccalabrese/rust-wayland-power.git",
                        "Mccalabrese/Genoa.git",
                    )
                    .replace("Mccalabrese/rust-wayland-power", "Mccalabrese/Genoa");

                if migrated_origin != current_origin {
                    let _ = Command::new("git")
                        .current_dir(active_repo)
                        .args(["remote", "set-url", "origin", migrated_origin.as_str()])
                        .status();
                }
            } else {
                eprintln!("   ⚠️ Failed to read current Git origin URL.");
            }
        } else {
            eprintln!("   ⚠️ Failed to execute git while migrating origin URL.");
        }

        // 3. Generate the new config.toml and burn the new path into it
        let _ = write_repo_root(active_repo);

        println!("   ✅ Migration complete. Welcome to the new architecture.");
    }
}

/// Writes the repository root path to the user's config file for dynamic access by other tools.
pub fn write_repo_root(repo_root: &Path) -> Result<(), std::io::Error> {
    let home = dirs::home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
    })?;
    let config_path = home.join(".config/rust-dotfiles/config.toml");
    let repo_root_str = repo_root
        .to_str()
        .ok_or_else(|| std::io::Error::other("Invalid repo root path"))?;
    let config_str = fs::read_to_string(&config_path)?;
    let updated_toml = upsert_repo_root_in_config(&config_str, repo_root_str)?;
    if updated_toml != config_str {
        fs::write(&config_path, updated_toml)?;
    }
    Ok(())
}

/// will insert or update the `root = "path"` line in the [repo] section of the config.toml content
/// using toml_edit.
pub fn upsert_repo_root_in_config(
    content: &str,
    repo_root: &str,
) -> Result<String, std::io::Error> {
    let mut doc = match content.parse::<toml_edit::DocumentMut>() {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!(
                "   ❌  Failed to parse config.toml. Please check your config syntax. Error: {}",
                e
            );
            return Err(std::io::Error::other("Failed to parse config.toml"));
        }
    };
    doc.entry("repo").or_insert(toml_edit::table())["root"] = toml_edit::value(repo_root);
    Ok(doc.to_string())
}

/// Reads a package list from a text file (one package per line).
/// Ignores empty lines and comments starting with '#'.
pub fn load_packages_from_file(filename: &str, repo_root: &Path) -> std::io::Result<Vec<String>> {
    let path = repo_root.join(filename);

    let content = fs::read_to_string(&path)?;
    println!("   ✅ Loaded package list from '{}'.", filename);
    Ok(parse_package_list(&content))
}

/// Combines release-owned defaults with the user's never-overwritten package
/// additions. The local file is deliberately outside the Genoa checkout so a
/// release update can never discard it.
pub fn load_effective_packages(repo_root: &Path, home: &Path) -> std::io::Result<Vec<String>> {
    let mut packages = load_packages_from_file("pkglist.txt", repo_root)?;
    let local_path = home.join(".config/genoa/pkglist.local");
    match fs::read_to_string(&local_path) {
        Ok(content) => {
            let local_packages = parse_package_list(&content);
            println!(
                "   ✅ Loaded {} local package override(s) from {}.",
                local_packages.len(),
                local_path.display()
            );
            packages.extend(local_packages);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let mut seen = HashSet::new();
    packages.retain(|package| seen.insert(package.clone()));
    Ok(packages)
}

fn parse_package_list(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(String::from)
        .collect()
}

pub fn resolve_repo_root(home: &Path) -> Result<PathBuf, std::io::Error> {
    if let Ok(env_path) = std::env::var("REPO_ROOT") {
        let path = PathBuf::from(env_path);
        if path.exists() {
            return Ok(path);
        }
    }

    if let Some(path) = read_repo_root_from_config(home)
        && path.exists()
    {
        return Ok(path);
    }

    if let Ok(path) = get_repo_root()
        && path.exists()
    {
        return Ok(path);
    }

    let preferred = home.join(NEW_REPO_DIR);
    if preferred.exists() {
        return Ok(preferred);
    }

    let legacy = home.join(LEGACY_REPO_DIR);
    if legacy.exists() {
        return Ok(legacy);
    }

    Err(std::io::Error::other(
        "Repository root could not be resolved",
    ))
}

pub fn read_repo_root_from_config(home: &Path) -> Option<PathBuf> {
    let config_path = home.join(".config/rust-dotfiles/config.toml");
    let contents = fs::read_to_string(config_path).ok()?;
    parse_repo_root_from_config(&contents, home)
}

fn parse_repo_root_from_config(contents: &str, home: &Path) -> Option<PathBuf> {
    let mut in_repo_section = false;
    for line in contents.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_repo_section = trimmed == "[repo]";
            continue;
        }

        if !in_repo_section {
            continue;
        }

        let normalized = trimmed.trim_start_matches('#').trim_start();
        if !normalized.starts_with("root") {
            continue;
        }

        let (_, rhs) = normalized.split_once('=')?;
        let value = rhs.trim().trim_matches('"').trim_matches('\'');
        if value.is_empty() {
            return None;
        }

        if let Some(stripped) = value.strip_prefix("~/") {
            return Some(home.join(stripped));
        }

        return Some(PathBuf::from(value));
    }

    None
}

/// Reliably finds the root of the dotfiles repository regardless of where the binary is executed.
pub fn get_repo_root() -> Result<PathBuf, std::io::Error> {
    // Prefer deriving the repo from the current working directory so this works for
    // both `cargo run` and installed binaries invoked from the repo.
    let cwd = std::env::current_dir()?;

    for ancestor in cwd.ancestors() {
        if ancestor
            .join("sysScripts/install-wizard/Cargo.toml")
            .exists()
        {
            return Ok(ancestor.to_path_buf());
        }

        if ancestor.file_name().and_then(|n| n.to_str()) == Some("install-wizard")
            && ancestor.join("Cargo.toml").exists()
            && let Some(sys_scripts) = ancestor.parent()
            && sys_scripts.file_name().and_then(|n| n.to_str()) == Some("sysScripts")
            && let Some(repo_root) = sys_scripts.parent()
        {
            return Ok(repo_root.to_path_buf());
        }
    }

    Err(std::io::Error::other(
        "Could not determine repository root from current directory",
    ))
}

/// During updates, only repair symlinks that were previously managed by this repo.
/// Never rewrite regular files/directories in the user's config.
pub fn repair_repo_symlink_targets(
    home: &Path,
    previous_repo_root: Option<&Path>,
    active_repo_root: &Path,
) {
    let managed_links = [
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
        (".config/nvim", ".config/nvim"),
    ];

    for (src_rel, dest_rel) in managed_links {
        let expected_target = active_repo_root.join(src_rel);
        let dest = home.join(dest_rel);
        maybe_repair_symlink(home, &dest, src_rel, previous_repo_root, &expected_target);
    }
}

pub fn maybe_repair_symlink(
    home: &Path,
    dest: &Path,
    src_rel: &str,
    previous_repo_root: Option<&Path>,
    expected_target: &Path,
) {
    let Ok(metadata) = fs::symlink_metadata(dest) else {
        return;
    };

    if !metadata.file_type().is_symlink() {
        return;
    }

    let Ok(link_target_raw) = fs::read_link(dest) else {
        return;
    };

    let resolved_target = if link_target_raw.is_absolute() {
        link_target_raw
    } else {
        match dest.parent() {
            Some(parent) => parent.join(link_target_raw),
            None => return,
        }
    };

    if resolved_target == expected_target {
        return;
    }

    let src_rel_path = Path::new(src_rel);
    let from_previous_root = previous_repo_root
        .map(|root| root.join(src_rel_path) == resolved_target)
        .unwrap_or(false);
    let from_legacy_root = home.join(LEGACY_REPO_DIR).join(src_rel_path) == resolved_target;

    if !from_previous_root && !from_legacy_root {
        return;
    }

    if !expected_target.exists() {
        return;
    }

    if fs::remove_file(dest).is_ok() && std::os::unix::fs::symlink(expected_target, dest).is_ok() {
        println!(
            "   ✅ Repaired symlink: {} -> {}",
            dest.display(),
            expected_target.display()
        );
    }
}

///Helper to create symlinks, backing up existing files if needed.
pub fn create_symlink(src: &Path, dest: &Path) {
    if dest.exists() && !dest.is_symlink() {
        let backup = format!("{}.backup", dest.to_string_lossy());
        let _ = fs::rename(dest, &backup);
    }
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if dest.is_symlink() {
        let _ = fs::remove_file(dest);
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(src, dest)
        .unwrap_or_else(|_| eprintln!("Failed to link {:?}", dest));
}

/// Helper to parse `cargo metadata` and extract the expected binary names for a given app.
/// Parses the JSON in a way that explicitly returns the app name if the parsing fails or the
/// expected fields are missing
pub fn expected_binary_names(app_path: &Path, app_name: &str) -> HashSet<String> {
    let mut expected = HashSet::new();
    let err_closure = |detail: &str| {
        eprintln!(
            "   ⚠️  Warning: {} for {}. Falling back to single binary assumption.",
            detail, app_name
        );
        HashSet::from([app_name.to_string()])
    };
    let metadata = match Command::new("/usr/bin/rustup")
        .args([
            "run",
            "stable",
            "cargo",
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .current_dir(app_path)
        .output()
    {
        Ok(metadata) if metadata.status.success() => metadata,
        _ => return err_closure("Failed to run cargo metadata"),
    };

    let json: Value = match serde_json::from_slice(&metadata.stdout) {
        Ok(json) => json,
        Err(_) => return err_closure("Failed to parse cargo metadata JSON"),
    };
    let packages = match json.get("packages").and_then(|v| v.as_array()) {
        Some(packages) => packages,
        None => {
            return err_closure("Failed to find 'packages' array in cargo metadata");
        }
    };
    for package in packages {
        if let Some(targets) = package.get("targets").and_then(|v| v.as_array()) {
            for target in targets {
                let is_bin = target
                    .get("kind")
                    .and_then(|v| v.as_array())
                    .map(|kinds| kinds.iter().any(|k| k.as_str() == Some("bin")))
                    .unwrap_or(false);

                if is_bin && let Some(name) = target.get("name").and_then(|v| v.as_str()) {
                    expected.insert(name.to_string());
                }
            }
        }
    }
    // Safe fallback so single-bin crates still update even if metadata fails.
    if expected.is_empty() {
        expected.insert(app_name.to_string());
    }

    expected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_repo_root_in_config_inserts_section() {
        let original = "title = 'genoa'\n";
        let updated =
            upsert_repo_root_in_config(original, "/home/testuser/Genoa").expect("upsert failed");
        assert!(updated.contains("[repo]"));
        assert!(updated.contains("root = \"/home/testuser/Genoa\""));
    }

    #[test]
    fn test_parse_repo_root_from_config_with_tilde() {
        let contents = "[repo]\nroot = \"~/Genoa\"\n";
        let home = Path::new("/home/testuser");
        let parsed = parse_repo_root_from_config(contents, home).expect("missing root");
        assert_eq!(parsed, PathBuf::from("/home/testuser/Genoa"));
    }

    #[test]
    fn test_parse_repo_root_from_config_missing_section() {
        let contents = "[other]\nroot = \"/tmp/ignore\"\n";
        let home = Path::new("/home/testuser");
        let parsed = parse_repo_root_from_config(contents, home);
        assert!(parsed.is_none());
    }

    #[test]
    fn package_parser_ignores_comments_and_empty_lines() {
        assert_eq!(
            parse_package_list("base-devel\n# optional\n\n  git  \n"),
            vec!["base-devel", "git"]
        );
    }
}
