use crate::traits::CmdExecutor;
use colored::*;
use inquire::Confirm;
use std::path::Path;

const CLEPSYDRE_PACKAGE_NAME: &str = "clepsydre-git-r.head-1-x86_64.pkg.tar.zst";
const CLEPSYDRE_PACKAGE_ID: &str = "clepsydre-git";
const CLEPSYDRE_PACKAGE_URL: &str = "https://github.com/Mccalabrese/Genoa/releases/download/v0.1.0/clepsydre-git-r.head-1-x86_64.pkg.tar.zst";
const CLEPSYDRE_PACKAGE_SHA256: &str =
    "fb17aa2066ec7d3a2e9ebb7b066b4547c9a22ab76e687ad45e9cc64541369852";
const YAY_AUR_URL: &str = "https://aur.archlinux.org/yay.git";
const YAY_PINNED_REF: &str = "refs/genoa/pinned-yay";
// This full commit is part of the signed Genoa release. Update it only after
// reviewing a new Yay revision, then ship the new installer in a signed tag.
const YAY_AUR_COMMIT: &str = "cb43f84828ab4f9700f7c6f9c6d7a923d4cfaff0";

/// Ensures that every requested repository package is installed.
///
/// The updater performs the system-wide package upgrade before invoking the
/// installer, so this phase only needs to install missing release dependencies.
/// Filtering installed targets prevents pacman from emitting one warning per
/// already-current package during ordinary refreshes.
pub fn install_pacman_packages(
    sys: &impl CmdExecutor,
    packages: &[&str],
) -> Result<(), std::io::Error> {
    let missing_packages: Vec<&str> = packages
        .iter()
        .copied()
        .filter(|package| !sys.is_package_installed(package))
        .collect();
    if missing_packages.is_empty() {
        return Ok(());
    }
    let mut args = vec!["pacman", "-S", "--needed", "--noconfirm"];
    args.extend(&missing_packages);
    if let Err(e) = sys.run_cmd("sudo", &args) {
        eprintln!(
            "{}",
            format!(
                "❌ Failed to install packages: {}",
                missing_packages.join(", ")
            )
            .red()
        );
        return Err(e);
    }
    println!("   ✅ Installed packages: {}", missing_packages.join(", "));
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

/// Bootstraps a reviewed, pinned Yay revision when it is not yet installed.
///
/// The user must opt in because building an AUR package executes its PKGBUILD
/// as the current user and may subsequently request sudo for installation.
pub fn install_aur_packages(
    sys: &impl CmdExecutor,
    home: &Path,
    aur_packages: &[&str],
) -> Result<(), std::io::Error> {
    let missing_packages: Vec<&str> = aur_packages
        .iter()
        .copied()
        .filter(|package| !sys.is_package_installed(package))
        .collect();
    if missing_packages.is_empty() {
        return Ok(());
    }
    if !sys.command_exists("yay") && !bootstrap_pinned_yay(sys, home)? {
        println!("   ⏭️  AUR package synchronization skipped.");
        return Ok(());
    }

    let mut args = vec!["-S", "--needed", "--noconfirm"];
    args.extend(missing_packages);
    if sys.run_cmd("yay", &args).is_err() {
        eprintln!("{}", "⚠️  AUR Warning.".yellow());
    }
    Ok(())
}

fn bootstrap_pinned_yay(sys: &impl CmdExecutor, home: &Path) -> Result<bool, std::io::Error> {
    println!("   🔐 Yay is not installed and must be built from the AUR.");
    println!("      Source: {YAY_AUR_URL}");
    println!("      Pinned revision: {YAY_AUR_COMMIT}");
    println!("      Building an AUR package runs its PKGBUILD as your user.");

    let approved = Confirm::new("Build this reviewed Yay revision and continue?")
        .with_default(false)
        .prompt()
        .unwrap_or(false);
    if !approved {
        return Ok(false);
    }

    println!("   ⬇️  Bootstrapping pinned Yay revision...");
    checkout_and_install_pinned_yay(sys, home)?;
    println!("   ✅ Installed pinned Yay revision.");
    Ok(true)
}

fn checkout_and_install_pinned_yay(
    sys: &impl CmdExecutor,
    home: &Path,
) -> Result<(), std::io::Error> {
    let cache_dir = home.join(".cache/genoa");
    let clone_path = sys.create_private_temp_dir(&cache_dir, "yay-bootstrap-")?;
    let clone_dest = clone_path
        .to_str()
        .ok_or_else(|| std::io::Error::other("Invalid Yay checkout path"))?;
    let pinned_refspec = format!("{YAY_AUR_COMMIT}:{YAY_PINNED_REF}");

    let result = (|| {
        sys.run_cmd("git", &["init", "--quiet", clone_dest])?;
        sys.run_cmd_in_dir(
            &clone_path,
            "git",
            &[
                "fetch",
                "--depth=1",
                "--no-tags",
                YAY_AUR_URL,
                &pinned_refspec,
            ],
        )?;
        sys.run_cmd_in_dir(
            &clone_path,
            "git",
            &["checkout", "--detach", YAY_PINNED_REF],
        )?;

        let actual_revision =
            sys.command_output("git", &["-C", clone_dest, "rev-parse", "--verify", "HEAD"])?;
        if actual_revision.trim() != YAY_AUR_COMMIT {
            return Err(std::io::Error::other(format!(
                "Yay checkout verification failed: expected {YAY_AUR_COMMIT}, got {}",
                actual_revision.trim()
            )));
        }

        sys.run_cmd_in_dir(
            &clone_path,
            "makepkg",
            &["--syncdeps", "--install", "--noconfirm"],
        )
    })();

    let cleanup_result = sys.remove_dir_all(&clone_path);
    if let Err(error) = result {
        eprintln!("{}", "❌ Failed to install pinned Yay from AUR.".red());
        return Err(error);
    }
    cleanup_result?;
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
    fn test_install_pacman_packages_skips_installed_targets() {
        let mut env = MockEnv::default();
        env.installed_packages.insert("foo".to_string());
        env.installed_packages.insert("bar".to_string());

        install_pacman_packages(&env, &["foo", "bar"]).unwrap();
        assert!(env.cmd_log.borrow().is_empty());
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

    #[test]
    fn test_install_aur_packages_skips_the_yay_call_when_all_are_installed() {
        let mut env = MockEnv::default();
        env.available_commands.insert("yay".to_string());
        env.installed_packages.insert("pkg-a".to_string());

        install_aur_packages(&env, Path::new("/home/testuser"), &["pkg-a"]).unwrap();
        assert!(env.cmd_log.borrow().is_empty());
    }

    #[test]
    fn test_pinned_yay_fetch_is_shallow_and_verifies_revision_before_building() {
        let env = MockEnv::default();
        let clone_path = "/home/testuser/.cache/genoa/yay-bootstrap-mock-0";
        env.command_outputs.borrow_mut().insert(
            (
                "git".to_string(),
                vec![
                    "-C".to_string(),
                    clone_path.to_string(),
                    "rev-parse".to_string(),
                    "--verify".to_string(),
                    "HEAD".to_string(),
                ],
            ),
            format!("{YAY_AUR_COMMIT}\n"),
        );

        let result = checkout_and_install_pinned_yay(&env, Path::new("/home/testuser"));

        assert!(result.is_ok());
        let log = env.cmd_log.borrow();
        assert_eq!(
            log[0],
            (
                "git".to_string(),
                vec![
                    "init".to_string(),
                    "--quiet".to_string(),
                    clone_path.to_string(),
                ],
            )
        );
        assert_eq!(
            log[1],
            (
                "git".to_string(),
                vec![
                    "fetch".to_string(),
                    "--depth=1".to_string(),
                    "--no-tags".to_string(),
                    YAY_AUR_URL.to_string(),
                    format!("{YAY_AUR_COMMIT}:{YAY_PINNED_REF}"),
                ],
            )
        );
        assert_eq!(
            log[2],
            (
                "git".to_string(),
                vec![
                    "checkout".to_string(),
                    "--detach".to_string(),
                    YAY_PINNED_REF.to_string(),
                ],
            )
        );
        assert_eq!(
            log[3],
            (
                "makepkg".to_string(),
                vec![
                    "--syncdeps".to_string(),
                    "--install".to_string(),
                    "--noconfirm".to_string(),
                ],
            )
        );
    }
}
