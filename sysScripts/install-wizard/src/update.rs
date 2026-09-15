use crate::traits::CmdExecutor;
use colored::*;
use std::path::Path;

const CLEPSYDRE_PACKAGE_NAME: &str = "clepsydre-git-r.head-1-x86_64.pkg.tar.zst";
const CLEPSYDRE_PACKAGE_ID: &str = "clepsydre-git";
const CLEPSYDRE_PACKAGE_URL: &str = "https://github.com/Mccalabrese/Genoa/releases/download/v0.1.0/clepsydre-git-r.head-1-x86_64.pkg.tar.zst";
const CLEPSYDRE_PACKAGE_SHA256: &str =
    "fb17aa2066ec7d3a2e9ebb7b066b4547c9a22ab76e687ad45e9cc64541369852";

/// installs packages via pacman with --needed and --noconfirm
pub fn install_pacman_packages(
    sys: &impl CmdExecutor,
    packages: &[&str],
) -> Result<(), std::io::Error> {
    if packages.is_empty() {
        return Ok(());
    }
    let mut args = vec!["pacman", "-S", "--needed", "--noconfirm"];
    args.extend(packages);
    if let Err(e) = sys.run_cmd("sudo", &args) {
        eprintln!(
            "{}",
            format!("❌ Failed to install packages: {}", packages.join(", ")).red()
        );
        return Err(e);
    }
    println!("   ✅ Installed packages: {}", packages.join(", "));
    Ok(())
}

/// Downloads and installs the packaged clepsydre dependency required by the sidebar.
///
/// The package is kept outside the repository because it is a locally built, WIP
/// dependency rather than a package currently available in the configured repos.
pub fn install_clepsydre_package(
    sys: &impl CmdExecutor,
    home: &Path,
) -> Result<(), std::io::Error> {
    if sys.is_package_installed(CLEPSYDRE_PACKAGE_ID) {
        return Ok(());
    }

    let cache_dir = home.join(".cache/genoa");
    sys.create_dir_all(&cache_dir)?;

    let package_path = cache_dir.join(CLEPSYDRE_PACKAGE_NAME);
    let checksum_path = cache_dir.join("clepsydre-git-r.head-1-x86_64.pkg.tar.zst.sha256");
    let package_path_str = package_path
        .to_str()
        .ok_or_else(|| std::io::Error::other("Invalid clepsydre package path"))?;
    let checksum_path_str = checksum_path
        .to_str()
        .ok_or_else(|| std::io::Error::other("Invalid clepsydre checksum path"))?;

    println!("   ⬇️  Downloading clepsydre dependency...");
    sys.run_cmd(
        "curl",
        &[
            "--fail",
            "--location",
            "--retry",
            "3",
            "--retry-delay",
            "2",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--output",
            package_path_str,
            CLEPSYDRE_PACKAGE_URL,
        ],
    )?;

    sys.write_string_to_file(
        checksum_path_str,
        &format!("{}  {}\n", CLEPSYDRE_PACKAGE_SHA256, package_path_str),
    )?;
    sys.run_cmd("sha256sum", &["--check", checksum_path_str])?;

    println!("   📦 Installing clepsydre dependency...");
    let install_result = sys.run_cmd(
        "sudo",
        &["pacman", "-U", "--needed", "--noconfirm", package_path_str],
    );

    if install_result.is_ok() {
        let _ = sys.run_cmd_ignore_err("rm", &["-f", package_path_str, checksum_path_str]);
        println!("   ✅ Installed clepsydre dependency.");
    }

    install_result
}

/// Bootstraps 'yay' from the AUR git repo if not present.
/// This allows the script to run on a truly clean Arch install.
pub fn install_aur_packages(
    sys: &impl CmdExecutor,
    home: &Path,
    aur_packages: &[&str],
) -> Result<(), std::io::Error> {
    if aur_packages.is_empty() {
        return Ok(());
    }
    if !sys.command_exists("yay") {
        println!("   ⬇️  Bootstrapping 'yay'...");
        let clone_path = home.join("yay-clone");

        if sys.path_exists(&clone_path) {
            let _ = sys.remove_dir_all(&clone_path);
        }

        let clone_dest = clone_path
            .to_str()
            .ok_or_else(|| std::io::Error::other("Invalid clone path"))?;
        sys.run_cmd(
            "git",
            &["clone", "https://aur.archlinux.org/yay.git", clone_dest],
        )?;

        if let Err(e) = sys.run_cmd_in_dir(&clone_path, "makepkg", &["-si", "--noconfirm"]) {
            let _ = sys.remove_dir_all(&clone_path);
            eprintln!("{}", "❌ Failed to install yay from AUR.".red());
            return Err(e);
        }
        sys.remove_dir_all(&clone_path)?;
    }

    let mut args = vec!["-S", "--needed", "--noconfirm"];
    args.extend(aur_packages);
    if sys.run_cmd("yay", &args).is_err() {
        eprintln!("{}", "⚠️  AUR Warning.".yellow());
    }
    Ok(())
}

/// Gleans pacman.conf to remove unwanted sessions and prevent future installs.
/// Gnome installs a lot of sessions we don't need, this keeps the list clean.
pub fn optimize_pacman_config(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    println!("   🔧 Optimizing pacman.conf & Cleaning Sessions...");

    let sessions_to_remove = vec![
        "/usr/share/wayland-sessions/gnome-classic.desktop",
        "/usr/share/wayland-sessions/gnome-classic-wayland.desktop",
    ];

    for session in sessions_to_remove {
        let _ = sys.run_cmd_ignore_err("sudo", &["rm", "-f", session]);
    }

    let pacman_conf = Path::new("/etc/pacman.conf");
    let content = sys.read_file_to_string(pacman_conf)?;

    if let Some(updated) = remove_noextract_sessions(&content) {
        //println!("   👉 Injecting NoExtract rules into [options]...");
        println!("   👉 Removing old NoExtract rules to allow session updates...");
        sys.install_string_to_root_file(pacman_conf, &updated, "644")?;
    }
    Ok(())
}

/// Reads /etc/pacman.conf and extracts any packages listed in IgnorePkg.
pub fn get_ignored_packages(sys: &impl CmdExecutor) -> Vec<String> {
    let content = match sys.read_file_to_string(Path::new("/etc/pacman.conf")) {
        Ok(content) => content,
        Err(_) => return Vec::new(),
    };
    parse_ignored_packages(&content)
}

fn remove_noextract_sessions(content: &str) -> Option<String> {
    if !content.contains("NoExtract = usr/share/wayland-sessions/") {
        return None;
    }
    let updated = content
        .lines()
        .filter(|line| {
            !line
                .trim_start()
                .starts_with("NoExtract = usr/share/wayland-sessions/")
        })
        .collect::<Vec<&str>>()
        .join("\n");
    Some(updated.trim_end().to_string() + "\n")
}

fn parse_ignored_packages(content: &str) -> Vec<String> {
    let mut ignored = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("IgnorePkg") {
            // Splits "IgnorePkg = pkg1 pkg2" and grabs the right side
            if let Some(pkgs) = trimmed.split('=').nth(1) {
                for pkg in pkgs.split_whitespace() {
                    ignored.push(pkg.to_string());
                }
            }
        }
    }
    ignored
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_env::MockEnv;
    use std::path::Path;

    #[test]
    fn test_install_pacman_packages_empty() {
        let env = MockEnv::default();
        let result = install_pacman_packages(&env, &[]);
        assert!(result.is_ok());
        assert!(env.cmd_log.borrow().is_empty());
    }

    #[test]
    fn test_install_pacman_packages_runs_command() {
        let env = MockEnv::default();
        let result = install_pacman_packages(&env, &["foo", "bar"]);
        assert!(result.is_ok());
        let log = env.cmd_log.borrow();
        assert_eq!(log.len(), 1);
        assert_eq!(
            log[0],
            (
                "sudo".to_string(),
                vec![
                    "pacman".to_string(),
                    "-S".to_string(),
                    "--needed".to_string(),
                    "--noconfirm".to_string(),
                    "foo".to_string(),
                    "bar".to_string(),
                ]
            )
        );
    }

    #[test]
    fn test_install_clepsydre_package_downloads_verifies_and_installs() {
        let env = MockEnv::default();
        let home = Path::new("/home/testuser");

        let result = install_clepsydre_package(&env, home);

        assert!(result.is_ok());
        let log = env.cmd_log.borrow();
        assert_eq!(log.len(), 4);
        assert_eq!(log[0].0, "curl");
        assert!(log[0].1.contains(&CLEPSYDRE_PACKAGE_URL.to_string()));
        assert_eq!(log[1].0, "sha256sum");
        assert_eq!(log[1].1[0], "--check");
        assert_eq!(log[2].0, "sudo");
        assert_eq!(
            log[2].1[0..4],
            [
                "pacman".to_string(),
                "-U".to_string(),
                "--needed".to_string(),
                "--noconfirm".to_string(),
            ]
        );
        assert_eq!(log[3].0, "rm");
    }

    #[test]
    fn test_install_clepsydre_package_skips_download_when_already_installed() {
        let mut env = MockEnv::default();
        env.installed_packages
            .insert(CLEPSYDRE_PACKAGE_ID.to_string());

        let result = install_clepsydre_package(&env, Path::new("/home/testuser"));

        assert!(result.is_ok());
        assert!(env.cmd_log.borrow().is_empty());
    }

    #[test]
    fn test_get_ignored_packages_parses_values() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/pacman.conf".to_string(),
            "IgnorePkg = foo bar\n#IgnorePkg = baz\nIgnorePkg=qux\n".to_string(),
        );
        let ignored = get_ignored_packages(&env);
        assert_eq!(ignored, vec!["foo", "bar", "qux"]);
    }

    #[test]
    fn test_optimize_pacman_config_removes_noextract() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/pacman.conf".to_string(),
            "[options]\nNoExtract = usr/share/wayland-sessions/niri.desktop\nHoldPkg = pacman\n"
                .to_string(),
        );
        let result = optimize_pacman_config(&env);
        assert!(result.is_ok());
        let binding = env.mock_files.borrow();
        let updated = binding.get("/etc/pacman.conf").unwrap();
        assert!(!updated.contains("NoExtract = usr/share/wayland-sessions"));
    }

    #[test]
    fn test_install_aur_packages_runs_yay_when_present() {
        let mut env = MockEnv::default();
        env.available_commands.insert("yay".to_string());
        let result = install_aur_packages(&env, Path::new("/home/testuser"), &["pkg-a"]);
        assert!(result.is_ok());
        let log = env.cmd_log.borrow();
        assert!(log.iter().any(|entry| {
            entry.0 == "yay"
                && entry.1
                    == ["-S", "--needed", "--noconfirm", "pkg-a"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
        }));
    }
}
