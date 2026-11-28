//! Arch Linux Production Installer
//!
//! A comprehensive system provisioning tool written in Rust.
//! Designed to take a fresh Arch Linux installation (base + git) and transform it
//! into a fully configured, multi-session Wayland environment (Niri, Sway).
//!
//! Core Responsibilities:
//! 1. **Hardware Detection:** Automatically identifies GPU vendors (NVIDIA/AMD/Intel)
//!    via `lspci` and installs the appropriate drivers/VAAPI packages.
//! 2. **Package Management:** Orchestrates `pacman` (official repo) and `yay` (AUR) installations.
//! 3. **Security Hardening:** Configures UFW, Polkit, and secure directory permissions.
//! 4. **Config Deployment:** Links dotfiles and generates machine-specific secrets (API keys)
//!    securely without storing them in git.
//! 5. **Safety:** Implements "Fail Fast" logic—if a critical step fails, the installer halts immediately.

use colored::*;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

mod graphics;
mod helpers;
mod kernel;
mod live_env;
#[cfg(test)]
mod mock_env;
mod session;
mod traits;
mod update;
mod user;

use crate::graphics::{
    GpuVendor, NvidiaArch, apply_nvidia_configs, configure_rhit_dell_pro_max_niri, detect_gpu,
    setup_turing_gpu,
};
use crate::helpers::{
    load_effective_packages, migrate_legacy_users, read_repo_root_from_config,
    repair_repo_symlink_targets, resolve_repo_root, write_repo_root,
};
use crate::kernel::{
    KernelFlavor, finalize_lts_uki, modern_nvidia_packages, persist_kernel_flavor, prepare_lts_uki,
    prompt_for_kernel_flavor, read_kernel_flavor, resign_lts_uki_if_needed,
};
use crate::live_env::LiveEnv;
use crate::session::{
    configure_dns, configure_printing_services, configure_quiet_boot, configure_system,
    configure_tlp, enforce_session_order, migrate_mkinitcpio_to_systemd_initramfs,
    sanitize_mkinitcpio,
};
use crate::traits::CmdExecutor;
use crate::update::{
    get_ignored_packages, install_aur_packages, install_clepsydre_package, install_pacman_packages,
    optimize_pacman_config,
};
use crate::user::{
    build_custom_apps, finalize_setup, link_dotfiles_and_copy_resources,
    migrate_gtklock_lock_commands, migrate_idle_service_commands,
    patch_waybar_known_genoa_commands, remove_retired_gtklock_config, remove_retired_tool_sources,
    setup_librewolf, setup_papers, setup_secrets_and_geoclue, setup_waybar_configs,
};

// Hardware Specific: AMD
const AMD_PACKAGES: &[&str] = &["vulkan-radeon", "libva-mesa-driver", "xf86-video-amdgpu"];

// AUR
const AUR_PACKAGES: &[&str] = &[
    "zoom",
    "slack-desktop",
    "ledger-live-bin",
    "visual-studio-code-bin",
    "pinta",
    "ttf-victor-mono",
    "pear-desktop-bin",
];

// The installer reaches sudo after running several helpers. Restrict command
// lookup at process entry so none of those helpers—or sudo itself—can be
// shadowed by an executable in a user-writable PATH directory.
const TRUSTED_SYSTEM_PATH: &str = "/usr/bin:/bin";
const SYSTEM_RUSTUP: &str = "/usr/bin/rustup";
const SYSTEMD_INHIBIT: &str = "/usr/bin/systemd-inhibit";
const IDLE_INHIBITED_ARGUMENT: &str = "--genoa-idle-inhibited";
const CF_TOGGLE_HELPER_SOURCE: &str =
    "sysScripts/cloudflare-toggle/target/release/cf-toggle-helper";
const CF_TOGGLE_HELPER_PATH: &str = "/usr/libexec/genoa/cf-toggle-helper";
const CF_TOGGLE_POLICY_PATH: &str =
    "/usr/share/polkit-1/actions/io.github.genoa.cloudflare-toggle.policy";
const CF_TOGGLE_POLICY: &str =
    include_str!("../../cloudflare-toggle/io.github.genoa.cloudflare-toggle.policy");
const GENOA_LOCK_SOURCE: &str = "sysScripts/genoa-lock/target/release/genoa-lock";
const GENOA_LOCK_PATH: &str = "/usr/local/bin/genoa-lock";
const GENOA_IDLE_SUSPEND_SOURCE: &str = "sysScripts/genoa-lock/target/release/genoa-idle-suspend";
const GENOA_IDLE_SUSPEND_PATH: &str = "/usr/local/bin/genoa-idle-suspend";
const GENOA_LOCK_PAM_HELPER_SOURCE: &str = "sysScripts/genoa-lock/target/release/genoa-lock-pam";
const GENOA_LOCK_PAM_HELPER_PATH: &str = "/usr/local/lib/genoa/genoa-lock-pam";
const GENOA_LOCK_PAM_PATH: &str = "/etc/pam.d/genoa-lock";
const GENOA_LOCK_PAM_POLICY: &str =
    "auth include system-local-login\naccount include system-local-login\n";
const RETIRED_GTKLOCK_PACKAGES: &[&str] = &["gtklock", "gtklock-runshell-module"];
const IDLE_SERVICE_UNITS: &[(&str, &str)] = &[
    (
        "genoa-niri-idle.service",
        include_str!("../../idle-manager/genoa-niri-idle.service"),
    ),
    (
        "genoa-sway-idle.service",
        include_str!("../../idle-manager/genoa-sway-idle.service"),
    ),
];

fn restrict_command_path() {
    // SAFETY: called by main before any threads or child processes exist; see
    // Rust 2024's environment-mutation safety requirement.
    unsafe { std::env::set_var("PATH", TRUSTED_SYSTEM_PATH) };
}

fn update_suspend_protection_enabled(home: &Path) -> bool {
    let path = home.join(".config/genoa/idle.toml");
    let Ok(contents) = fs::read_to_string(path) else {
        return true;
    };
    let Ok(document) = contents.parse::<toml_edit::DocumentMut>() else {
        return true;
    };
    document
        .get("inhibit_suspend_while_updating")
        .and_then(toml_edit::Item::as_bool)
        .unwrap_or(true)
}

fn run_with_suspend_protection(home: &Path, arguments: &[String]) -> Option<i32> {
    if arguments
        .iter()
        .any(|argument| argument == IDLE_INHIBITED_ARGUMENT)
        || !update_suspend_protection_enabled(home)
    {
        return None;
    }
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            eprintln!("   ⚠️  Could not enable suspend protection: {error}");
            return None;
        }
    };
    let mut command = Command::new(SYSTEMD_INHIBIT);
    command.args([
        "--what=sleep",
        "--mode=block",
        "--who=Genoa",
        "--why=Genoa is updating the system",
        "--",
    ]);
    command
        .arg(executable)
        .args(arguments.iter().skip(1))
        .arg(IDLE_INHIBITED_ARGUMENT);
    match command.status() {
        Ok(status) => Some(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("   ⚠️  Could not enable suspend protection: {error}");
            None
        }
    }
}

/// Deploys the only privileged portion of the DNS toggle outside the
/// user-writable Cargo bin directory, then ties pkexec to that path and its
/// single permitted argument.
fn install_cloudflare_toggle_helper(
    sys: &impl CmdExecutor,
    repo_root: &Path,
) -> Result<(), std::io::Error> {
    let source = repo_root.join(CF_TOGGLE_HELPER_SOURCE);
    if !sys.path_exists(&source) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "Cloudflare toggle helper was not built: {}",
                source.display()
            ),
        ));
    }

    sys.create_root_dir_all(Path::new("/usr/libexec/genoa"))?;
    sys.install_file_to_root(&source, Path::new(CF_TOGGLE_HELPER_PATH), "755")?;
    sys.create_root_dir_all(Path::new("/usr/share/polkit-1/actions"))?;
    sys.install_string_to_root_file(Path::new(CF_TOGGLE_POLICY_PATH), CF_TOGGLE_POLICY, "644")?;
    Ok(())
}

/// Installs the experimental locker at a fixed, root-owned path.  Every
/// desktop integration invokes this exact path so a user-writable Cargo bin
/// directory can never decide what locks the session.
fn install_genoa_lock(sys: &impl CmdExecutor, repo_root: &Path) -> Result<(), std::io::Error> {
    let source = repo_root.join(GENOA_LOCK_SOURCE);
    let idle_suspend_source = repo_root.join(GENOA_IDLE_SUSPEND_SOURCE);
    let helper_source = repo_root.join(GENOA_LOCK_PAM_HELPER_SOURCE);
    if !sys.path_exists(&source) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Genoa Lock was not built: {}", source.display()),
        ));
    }
    if !sys.path_exists(&helper_source) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "Genoa Lock PAM helper was not built: {}",
                helper_source.display()
            ),
        ));
    }
    if !sys.path_exists(&idle_suspend_source) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!(
                "Genoa idle suspend helper was not built: {}",
                idle_suspend_source.display()
            ),
        ));
    }

    sys.create_root_dir_all(Path::new("/usr/local/bin"))?;
    sys.install_file_to_root(&source, Path::new(GENOA_LOCK_PATH), "755")?;
    sys.install_file_to_root(
        &idle_suspend_source,
        Path::new(GENOA_IDLE_SUSPEND_PATH),
        "755",
    )?;
    sys.create_root_dir_all(Path::new("/usr/local/lib/genoa"))?;
    sys.install_file_to_root(&helper_source, Path::new(GENOA_LOCK_PAM_HELPER_PATH), "755")?;
    sys.create_root_dir_all(Path::new("/etc/pam.d"))?;
    sys.install_string_to_root_file(Path::new(GENOA_LOCK_PAM_PATH), GENOA_LOCK_PAM_POLICY, "644")?;
    Ok(())
}

/// Deploys the compositor-specific idle units. Startup configuration selects
/// exactly one unit and imports that compositor's Wayland socket first.
fn setup_idle_services(home: &Path, sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    let systemd_user_dir = home.join(".config/systemd/user");
    sys.create_dir_all(&systemd_user_dir)?;

    let mut changed = false;
    for (name, contents) in IDLE_SERVICE_UNITS {
        let destination = systemd_user_dir.join(name);
        if sys.read_file_to_string(&destination).ok().as_deref() != Some(*contents) {
            let destination = destination
                .to_str()
                .ok_or_else(|| std::io::Error::other("idle service path is not valid UTF-8"))?;
            sys.write_string_to_file(destination, contents)?;
            changed = true;
        }
    }

    if changed {
        sys.run_cmd("systemctl", &["--user", "daemon-reload"])?;
    }
    Ok(())
}

/// Remove the retired locker only after the new root-owned locker is present
/// and every managed active configuration has been migrated.  A custom config
/// that still invokes gtklock leaves the old package in place rather than
/// risking a machine without a working screen lock.
fn retire_gtklock(sys: &impl CmdExecutor, home: &Path) -> Result<(), std::io::Error> {
    if !migrate_gtklock_lock_commands(sys, home)? {
        eprintln!(
            "   ⚠️  Gtklock is still referenced by a custom active config; leaving it installed."
        );
        return Ok(());
    }

    let installed: Vec<&str> = RETIRED_GTKLOCK_PACKAGES
        .iter()
        .copied()
        .filter(|package| sys.is_package_installed(package))
        .collect();
    if !installed.is_empty() {
        let mut args = vec!["pacman", "-R", "--noconfirm"];
        args.extend(installed);
        sys.run_cmd("sudo", &args)?;
    }

    remove_retired_gtklock_config(sys, home)?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct RunOptions {
    refresh_mode: bool,
    sync_packages: bool,
    release_root: bool,
}

/// On the first legacy refresh, the just-built updater can create its isolated
/// signed-release keyring in the same terminal. Release-owned refreshes and
/// ordinary installs intentionally never invoke this compatibility handoff.
fn offer_signed_release_migration(home: &Path, repo_root: &Path, options: &RunOptions) {
    if !options.refresh_mode || options.release_root {
        return;
    }
    if !repo_root
        .join("sysScripts/updater/assets/genoa-pubkey.asc")
        .is_file()
    {
        return;
    }

    let updater = home.join(".cargo/bin/updater");
    if !updater.is_file() {
        return;
    }

    println!("\n🔐 Completing the signed-release updater migration...");
    match Command::new(&updater)
        .arg("--initialize-release-trust")
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!(
            "   ⚠️  Signed-release verification was not initialized ({status}). You can retry from the next system update."
        ),
        Err(error) => eprintln!(
            "   ⚠️  Could not start the rebuilt updater: {error}. You can retry from the next system update."
        ),
    }
}

fn parse_run_options(args: &[String]) -> RunOptions {
    let refresh_mode = args.iter().any(|arg| arg == "--refresh-configs");

    // This flag is an updater-only optimization. A fresh install always syncs
    // packages, and refresh mode keeps its legacy behavior unless told to skip.
    let sync_packages = !refresh_mode || !args.iter().any(|arg| arg == "--skip-package-sync");
    let release_root = args.iter().any(|arg| arg == "--release-root");

    RunOptions {
        refresh_mode,
        sync_packages,
        release_root,
    }
}

// ---------- Main Execution ------_-------

fn main() {
    restrict_command_path();
    let home = dirs::home_dir().unwrap_or_else(|| {
        eprintln!(
            "{}",
            "❌ Critical Error: Could not determine home directory.".red()
        );
        std::process::exit(1);
    });
    let live_sys = LiveEnv;
    // 🚨 PREVENT FATAL ROOT EXECUTION 🚨
    // If run with sudo, home_dir() points to /root, which breaks dotfiles and cargo paths.
    if std::env::var("USER").unwrap_or_default() == "root" || std::env::var("SUDO_USER").is_ok() {
        eprintln!(
            "{}",
            "❌ CRITICAL ERROR: Do not run this script as root or with sudo!"
                .red()
                .bold()
        );
        eprintln!("Please run it as your standard Wayland user.");
        eprintln!("The script is designed to safely elevate privileges internally when needed.");
        std::process::exit(1);
    }
    // 0. Parse Arguments
    let args: Vec<String> = std::env::args().collect();
    if let Some(exit_code) = run_with_suspend_protection(&home, &args) {
        std::process::exit(exit_code);
    }
    let run_options = parse_run_options(&args);
    let refresh_mode = run_options.refresh_mode;
    if run_options.release_root && !refresh_mode {
        eprintln!("❌ --release-root may only be used with --refresh-configs.");
        std::process::exit(1);
    }

    let previous_repo_root = read_repo_root_from_config(&home);
    let has_existing_install = home.join(".config/rust-dotfiles/config.toml").exists();
    if !run_options.release_root {
        migrate_legacy_users(&home);
    }

    let repo_root = resolve_repo_root(&home).unwrap_or_else(|e| {
        eprintln!("❌ Error determining repository root: {}", e);
        std::process::exit(1);
    });
    // Existing Genoa systems retain their current behaviour unless they already
    // opted into the persisted LTS policy. New installs choose below, after
    // sudo is available to write that policy safely.
    let mut kernel_flavor = read_kernel_flavor(&live_sys).unwrap_or(KernelFlavor::Mainline);

    if refresh_mode {
        println!("{}", "🔄 Running in CONFIG REFRESH MODE".magenta().bold());
        let status = Command::new("sudo").arg("-v").status().unwrap();
        if !status.success() {
            eprintln!("{}", "❌ Sudo required.".red());
            std::process::exit(1);
        }
        if let Err(e) = sanitize_mkinitcpio(&live_sys) {
            eprintln!("   ❌ Failed to sanitize mkinitcpio configuration: {}", e);
            std::process::exit(1);
        }
        if let Err(e) = migrate_mkinitcpio_to_systemd_initramfs(&live_sys) {
            eprintln!("   ❌ Failed to migrate the initramfs configuration: {}", e);
            std::process::exit(1);
        }
    } else {
        // ==========================================
        //  FULL INSTALL MODE (Fresh Install Only)
        // ==========================================
        println!(
            "{}",
            "🚀 Starting Rust Wayland Power Installation..."
                .green()
                .bold()
        );

        let status = Command::new("sudo")
            .arg("-v")
            .status()
            .expect("Failed to sudo");
        if !status.success() {
            std::process::exit(1);
        }

        if let Err(e) = sanitize_mkinitcpio(&live_sys) {
            eprintln!("   ❌ Failed to sanitize mkinitcpio configuration: {}", e);
            std::process::exit(1);
        }
        if let Err(e) = migrate_mkinitcpio_to_systemd_initramfs(&live_sys) {
            eprintln!("   ❌ Failed to migrate the initramfs configuration: {}", e);
            std::process::exit(1);
        }

        if !has_existing_install && read_kernel_flavor(&live_sys).is_none() {
            println!("\n{}", "🐧 Choosing kernel policy...".blue().bold());
            kernel_flavor = prompt_for_kernel_flavor();
            if let Err(e) = persist_kernel_flavor(&live_sys, kernel_flavor) {
                eprintln!("   ❌ Failed to save kernel policy: {}", e);
                std::process::exit(1);
            }
        }

        let lts_uki = if !has_existing_install && kernel_flavor == KernelFlavor::Lts {
            println!("\n{}", "🐧 Preparing LTS kernel...".blue().bold());
            match prepare_lts_uki(&live_sys) {
                Ok(uki) => uki,
                Err(e) => {
                    eprintln!("   ❌ Failed to prepare the LTS kernel: {}", e);
                    std::process::exit(1);
                }
            }
        } else {
            None
        };

        println!(
            "\n{}",
            "⚔️  Resolving Audio Conflicts (Removing jack2)...".yellow()
        );

        if Command::new("which")
            .arg("jackd")
            .status()
            .is_ok_and(|s| s.success())
        {
            println!("   👉 Detected 'jackd' in PATH. Removing 'jack2' to prevent conflicts...");
            let _ = Command::new("sudo")
                .args(["pacman", "-Rdd", "--noconfirm", "jack2"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }

        // GPU Drivers Checkpoint & Exit Logic
        let state_file = home.join(".cache/rust_installer_drivers_done");

        if state_file.exists() {
            println!(
                "\n{}",
                "✅ Drivers already installed (Checkpoint found). Skipping to prevent crash."
                    .green()
            );
        } else {
            println!(
                "\n{}",
                "🔍 Detecting GPU Hardware & Installing Base Drivers..."
                    .blue()
                    .bold()
            );
            let gpu = detect_gpu(&live_sys);
            match gpu {
                GpuVendor::Nvidia(NvidiaArch::Turing) => {
                    println!("   👉 NVIDIA Turing Detected (GTX 16xx / RTX 20xx).");
                    if let Err(e) = setup_turing_gpu(kernel_flavor, &live_sys) {
                        eprintln!("   ❌ Failed to install legacy NVIDIA drivers: {}", e);
                        std::process::exit(1);
                    }
                }
                GpuVendor::Nvidia(NvidiaArch::Modern) => {
                    println!("   👉 Modern NVIDIA Detected (RTX 30xx/40xx/Blackwell).");
                    if let Err(e) =
                        install_pacman_packages(&live_sys, modern_nvidia_packages(kernel_flavor))
                    {
                        eprintln!("   ❌ Failed to install NVIDIA drivers: {}", e);
                        std::process::exit(1);
                    }
                    // The GUI checkpoint below exits before the normal hardware
                    // enforcement stage. Apply the boot-critical NVIDIA setup now
                    // so the first installer pass is complete before rebooting.
                    if let Err(e) = apply_nvidia_configs(&NvidiaArch::Modern, &live_sys) {
                        eprintln!("   ❌ Failed to configure NVIDIA power management: {}", e);
                        std::process::exit(1);
                    }
                    if let Err(e) = configure_rhit_dell_pro_max_niri(&live_sys) {
                        eprintln!("   ❌ Failed to configure Niri GPU selection: {}", e);
                        std::process::exit(1);
                    }
                }
                GpuVendor::Amd => {
                    println!("   👉 AMD Detected.");
                    if let Err(e) = install_pacman_packages(&live_sys, AMD_PACKAGES) {
                        eprintln!("   ❌ Failed to install AMD drivers: {}", e);
                        std::process::exit(1);
                    }
                }
                GpuVendor::Intel => println!("   👉 Intel Detected (Drivers in common)."),
                GpuVendor::Unknown => println!("   ⚠️  No dedicated GPU detected."),
            }

            if let Some(uki) = &lts_uki
                && let Err(e) = finalize_lts_uki(&live_sys, uki)
            {
                eprintln!(
                    "   ❌ Failed to validate and select the LTS boot entry: {}",
                    e
                );
                std::process::exit(1);
            }

            let is_gui =
                std::env::var("WAYLAND_DISPLAY").is_ok() || std::env::var("DISPLAY").is_ok();

            if is_gui {
                println!("\n{}", "⚠️  GRAPHICS DRIVERS INSTALLED".yellow().bold());
                println!("We must reboot to load the new kernel modules safely.");

                if let Ok(mut file) = fs::File::create(&state_file) {
                    writeln!(file, "Drivers installed successfully.").unwrap();
                }

                println!(
                    "{}",
                    "✅ Checkpoint saved. Please REBOOT and RUN THIS SCRIPT AGAIN."
                        .green()
                        .bold()
                );
                let should_reboot = inquire::Confirm::new("Reboot now?")
                    .with_default(true)
                    .prompt()
                    .unwrap_or(true);
                if should_reboot {
                    let _ = Command::new("sudo").arg("reboot").status();
                }
                std::process::exit(0);
            }
        }

        // A second installer pass skips the graphics checkpoint, but it still
        // needs to rebuild and select the LTS image prepared above.
        if state_file.exists()
            && let Some(uki) = &lts_uki
            && let Err(e) = finalize_lts_uki(&live_sys, uki)
        {
            eprintln!(
                "   ❌ Failed to validate and select the LTS boot entry: {}",
                e
            );
            std::process::exit(1);
        }

        println!("\n{}", "🦀 Setting up Rust (rustup)...".blue().bold());
        let _ = Command::new(SYSTEM_RUSTUP)
            .args(["default", "stable"])
            .status();
    }

    // ==========================================
    //  SHARED LOGIC (Runs on Install AND Refresh)
    // ==========================================

    // 1. Sync Standard & AUR Packages
    if run_options.sync_packages {
        println!("\n{}", "📦 Syncing Standard Packages...".blue().bold());
        let mut packages_synced = true;
        let mut common_pkgs = match load_effective_packages(&repo_root, &home) {
            Ok(pkgs) => pkgs,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("   ⚠️  Release package list not found. Skipping package installation.");
                Vec::new()
            }
            Err(e) => {
                eprintln!("   ❌ Failed to read pkglist.txt: {}", e);
                std::process::exit(1);
            }
        };

        let ignored_pkgs = get_ignored_packages(&live_sys);
        common_pkgs.retain(|pkg| !ignored_pkgs.contains(pkg));

        if common_pkgs.is_empty() {
            println!("   ⚠️  No packages found in release or local package lists.");
        } else {
            let pkg_refs: Vec<&str> = common_pkgs.iter().map(|s| s.as_str()).collect();
            if let Err(e) = install_pacman_packages(&live_sys, &pkg_refs) {
                eprintln!("   ❌ Failed to install standard packages: {}", e);
                std::process::exit(1);
            };
        }

        if !AUR_PACKAGES.is_empty() {
            println!("\n{}", "📦 Syncing AUR Packages...".blue().bold());
            if let Err(e) = install_aur_packages(&live_sys, &home, AUR_PACKAGES) {
                eprintln!("   ❌ Failed to install AUR packages: {}", e);
                packages_synced = false;
            };
        }

        if packages_synced {
            println!("   ✅ Package synchronization complete.");
        }
    }

    println!("\n{}", "🌐 Configuring DNS proxy...".blue().bold());
    if let Err(e) = configure_dns(&live_sys) {
        eprintln!("   ❌ Failed to configure dnscrypt-proxy: {}", e);
        std::process::exit(1);
    }
    println!("   ✅ DNS proxy ready.");

    println!("\n{}", "🖨️  Configuring printing services...".blue().bold());
    if let Err(e) = configure_printing_services(&live_sys) {
        eprintln!("   ❌ Failed to configure printing services: {}", e);
        std::process::exit(1);
    }
    println!("   ✅ Printing services ready.");

    println!("\n{}", "🔇 Configuring quiet boot...".blue().bold());
    if let Err(e) = configure_quiet_boot(&live_sys) {
        eprintln!("   ❌ Failed to configure quiet boot: {}", e);
        std::process::exit(1);
    }
    if kernel_flavor == KernelFlavor::Lts
        && let Err(e) = resign_lts_uki_if_needed(&live_sys)
    {
        eprintln!("   ❌ Failed to re-sign the LTS boot image: {}", e);
        std::process::exit(1);
    }
    println!("   ✅ Boot configuration ready.");

    // 2. Re-compile Rust Apps (Ensures updates to your tools are applied)
    println!("\n{}", "🦀 Syncing Custom Rust Apps...".blue().bold());
    // GUARANTEE Rust toolchain is loaded and set to stable (fixes GUI launcher bug)
    let _ = Command::new(SYSTEM_RUSTUP)
        .args(["default", "stable"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // Install the sidebar's packaged EDS dependency before compiling the app.
    if let Err(e) = install_clepsydre_package(&live_sys, &home) {
        eprintln!(
            "   ❌ Failed to install the clepsydre dependency required by sidebar: {}",
            e
        );
        std::process::exit(1);
    }

    if let Err(e) = remove_retired_tool_sources(&repo_root) {
        eprintln!("   ❌ Failed to remove retired tool sources: {}", e);
        std::process::exit(1);
    }

    let custom_apps_built = match build_custom_apps(&live_sys, &home, &repo_root) {
        Ok(()) => true,
        Err(e) => {
            println!("   ⚠️  Failed to build custom Rust apps: {}", e);
            false
        }
    };
    // Re-deploy this root-owned helper on every updater pass when its most
    // recently built binary is available. This repairs ownership and traversal
    // mode drift even if an unrelated custom Rust app failed to build.
    let helper_installed = match install_cloudflare_toggle_helper(&live_sys, &repo_root) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("   ⚠️  Failed to install the Cloudflare DNS helper: {e}");
            false
        }
    };
    if custom_apps_built {
        let mut helpers_installed = helper_installed;
        let mut idle_protection_ready = false;
        match install_genoa_lock(&live_sys, &repo_root) {
            Ok(()) => {
                idle_protection_ready = true;
                match retire_gtklock(&live_sys, &home) {
                    Ok(()) => {}
                    Err(e) => {
                        eprintln!("   ⚠️  Failed to retire gtklock safely: {e}");
                        helpers_installed = false;
                    }
                }
            }
            Err(e) => {
                eprintln!("   ⚠️  Failed to install Genoa Lock; keeping gtklock installed: {e}");
                helpers_installed = false;
            }
        }
        if idle_protection_ready {
            match setup_idle_services(&home, &live_sys)
                .and_then(|()| migrate_idle_service_commands(&live_sys, &home))
            {
                Ok(()) => println!("   ✅ Idle resume protection ready."),
                Err(e) => eprintln!("   ⚠️  Failed to deploy idle resume protection: {e}"),
            }
        }
        offer_signed_release_migration(&home, &repo_root, &run_options);
        if helpers_installed {
            println!("   ✅ Custom Rust apps ready.");
        }
    }

    match configure_tlp(&live_sys, &repo_root) {
        Ok(()) => println!("   ✅ TLP power management ready."),
        Err(e) => eprintln!("   ❌ Failed to configure TLP power management: {}", e),
    }

    let update_only_mode = has_existing_install && !refresh_mode;
    if update_only_mode {
        println!(
            "\n{}",
            "ℹ️  Existing install detected. Limiting this run to pkglist + sysScripts sync."
                .yellow()
                .bold()
        );

        // Existing installations normally skip system configuration. NVIDIA
        // power-management repairs are safe and needed to migrate older runs
        // that completed the driver checkpoint without reaching that stage.
        match configure_detected_nvidia(kernel_flavor, &live_sys) {
            Ok(true) => {
                if let Err(e) = enforce_session_order(&live_sys, true, &repo_root) {
                    eprintln!("   ❌ Failed to refresh NVIDIA session integration: {}", e);
                    std::process::exit(1);
                }
                println!("   ✅ NVIDIA integration ready.");
            }
            Ok(false) => println!("   ✅ Hardware integration check complete."),
            Err(e) => {
                eprintln!("   ❌ Failed to configure NVIDIA hardware: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        println!(
            "\n{}",
            "⚙️  Applying System Configurations...".blue().bold()
        );
        if let Err(e) = optimize_pacman_config(&live_sys) {
            eprintln!("   ❌ Failed to optimize pacman configuration: {}", e);
        }

        // 3. Hardware Enforcement
        let is_nvidia = configure_detected_nvidia(kernel_flavor, &live_sys).unwrap_or_else(|e| {
            eprintln!("   ❌ Failed to configure NVIDIA hardware: {}", e);
            std::process::exit(1);
        });

        if let Err(e) = enforce_session_order(&live_sys, is_nvidia, &repo_root) {
            eprintln!("   ❌ Failed to enforce session order: {}", e);
            std::process::exit(1);
        }

        // 4. Check or install battery-daemon
        if let Err(e) = setup_battery_daemon(&home, &live_sys) {
            eprintln!("   ❌ Failed to set up battery-daemon: {}", e);
        }
    }

    // 5. Finalize
    if !refresh_mode {
        if has_existing_install {
            // --- UPDATE MODE (safe for personal configs) ---
            if run_options.release_root {
                println!(
                    "\n{}",
                    "🔒 Verified release refresh: keeping personal dotfile links unchanged."
                        .blue()
                        .bold()
                );
            } else {
                println!(
                    "\n{}",
                    "🔧 Repairing managed symlink targets...".blue().bold()
                );
                repair_repo_symlink_targets(&home, previous_repo_root.as_deref(), &repo_root);
                if let Err(e) = write_repo_root(&repo_root) {
                    eprintln!("   ⚠️ Failed to write repository root to config: {}", e);
                }
            }
            patch_waybar_known_genoa_commands(&live_sys, &home);

            print_logo();
            println!(
                "\n{}",
                "✅ System Synced. Personal configs preserved; managed symlinks repaired."
                    .green()
                    .bold()
            );
        } else {
            // --- FRESH INSTALL ONLY ---
            println!("\n{}", "🔗 Linking Config Files...".blue().bold());
            if let Err(e) = link_dotfiles_and_copy_resources(&live_sys, &home, &repo_root) {
                eprintln!("   ❌ Failed to link configuration files safely: {}", e);
                std::process::exit(1);
            }

            if let Err(e) = configure_system(&live_sys, &home) {
                eprintln!("   ❌ Failed to configure system services: {}", e);
                std::process::exit(1);
            }

            if let Err(e) = setup_librewolf(&live_sys, &home) {
                eprintln!("   ⚠️ Failed to configure LibreWolf: {}", e);
            }
            setup_papers(&live_sys);
            setup_waybar_configs(&live_sys, &home);
            patch_waybar_known_genoa_commands(&live_sys, &home);
            if let Err(e) = setup_secrets_and_geoclue(&live_sys, &home) {
                eprintln!("   ⚠️ Failed to set up secrets and geoclue: {}", e);
            }
            if let Err(e) = write_repo_root(&repo_root) {
                eprintln!("   ⚠️ Failed to write repository root to config: {}", e);
            }
            finalize_setup(&live_sys, &home); // Neovim/Tmux plugins

            print_logo();
            println!(
                "\n{}",
                "✅ Installation Complete! Please Reboot.".green().bold()
            );
        }
    } else {
        // --- REFRESH MODE (Updater) ---
        if run_options.release_root {
            println!(
                "\n{}",
                "🔒 Verified release refresh: preserving personal dotfile links."
                    .blue()
                    .bold()
            );
        } else {
            println!(
                "\n{}",
                "🔧 Repairing managed symlink targets...".blue().bold()
            );
            repair_repo_symlink_targets(&home, previous_repo_root.as_deref(), &repo_root);
            if let Err(e) = write_repo_root(&repo_root) {
                eprintln!("   ⚠️ Failed to write repository root to config: {}", e);
            }
        }
        patch_waybar_known_genoa_commands(&live_sys, &home);
        if let Err(e) = setup_secrets_and_geoclue(&live_sys, &home) {
            eprintln!("   ⚠️ Failed to set up secrets and geoclue: {}", e);
        }

        print_logo();
        println!(
            "\n{}",
            "✅ System Synced & Configs Refreshed Successfully."
                .green()
                .bold()
        );
    }
}

/// Applies the NVIDIA runtime configuration whenever NVIDIA hardware is present.
/// Kept separate from the broader system setup so update-only runs can repair
/// the driver checkpoint safely.
fn configure_detected_nvidia(
    kernel_flavor: KernelFlavor,
    sys: &impl CmdExecutor,
) -> Result<bool, std::io::Error> {
    let GpuVendor::Nvidia(arch) = detect_gpu(sys) else {
        return Ok(false);
    };

    if arch == NvidiaArch::Turing {
        setup_turing_gpu(kernel_flavor, sys)?;
    } else {
        // Update runs also repair systems where older GPU detection skipped an
        // NVIDIA 3D controller and therefore never installed its driver.
        install_pacman_packages(sys, modern_nvidia_packages(kernel_flavor))?;
    }
    apply_nvidia_configs(&arch, sys)?;
    configure_rhit_dell_pro_max_niri(sys)?;
    Ok(true)
}

// Installs the battery life warning and exectes systemctl poweroff to protect battery
/// Installs the battery life warning and exectes systemctl poweroff to protect battery
fn setup_battery_daemon(home: &Path, sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    println!("   🔋 Configuring Battery Safety Daemon...");

    configure_upower(sys)?;

    let systemd_user_dir = home.join(".config/systemd/user");
    let service_dest = systemd_user_dir.join("battery-daemon.service");
    let service_path = Path::new(&service_dest);

    println!("   🔋 Setting up Battery Safety Daemon...");

    // Make sure the ~/.config/systemd/user/ folder actually exists
    sys.create_dir_all(&systemd_user_dir)?;
    let service_content = include_str!("../../battery-daemon/battery-daemon.service");
    let existing_content = sys.read_file_to_string(service_path).unwrap_or_default();

    if existing_content != service_content {
        println!("   📝 Updating battery daemon configuration.");
        sys.write_string_to_file(
            service_dest
                .to_str()
                .ok_or_else(|| std::io::Error::other("invalid"))?,
            service_content,
        )?;
        sys.run_cmd("systemctl", &["--user", "daemon-reload"])?;
    }

    sys.run_cmd(
        "systemctl",
        &["--user", "enable", "--now", "battery-daemon.service"],
    )?;
    println!("   ✅ Battery Daemon ready.");

    Ok(())
}

fn configure_upower(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    println!("🔋 Enforcing UPower Critical Shutdown at 5%...");

    let upower_conf = Path::new("/etc/UPower/UPower.conf");
    let file_content = sys.read_file_to_string(upower_conf)?;
    let mut needs_update = false;
    let mut found_percentage = false;
    let mut found_critical = false;
    let mut lines: Vec<String> = file_content.lines().map(|s| s.to_string()).collect();

    for line in &mut lines {
        let normalized = line.trim_start().trim_start_matches('#').trim_start();
        if normalized.starts_with("PercentageAction=") {
            found_percentage = true;
            if !line.starts_with("PercentageAction=5.0") {
                needs_update = true;
                *line = "PercentageAction=5.0".to_string();
            }
        } else if normalized.starts_with("CriticalPowerAction=") {
            found_critical = true;
            if !line.starts_with("CriticalPowerAction=PowerOff") {
                needs_update = true;
                *line = "CriticalPowerAction=PowerOff".to_string();
            }
        }
    }
    if !found_critical {
        needs_update = true;
        lines.push("CriticalPowerAction=PowerOff".to_string());
    }
    if !found_percentage {
        needs_update = true;
        lines.push("PercentageAction=5.0".to_string());
    }
    if !needs_update {
        return Ok(());
    }

    // sys.write_string_to_file(upower_conf.to_str().unwrap(), &lines.join("\n"))?;

    sys.install_string_to_root_file(upower_conf, &lines.join("\n"), "644")?;

    // restarting to apply changes
    sys.run_cmd("sudo", &["systemctl", "restart", "upower.service"])?;

    Ok(())
}
fn print_logo() {
    println!(
        r#"
                                                                                                    
                                             ++++++++++                                             
                                           ++++++++++++++                                           
                                          ++++++++++++++++                                          
                                         ++++++++++++++++++                                         
                                        ++++++++++++++++++++                                        
                                       +++++++++====+++++++++                                       
                                       ++++++=:......:=++++++                                       
                                      +++++=:..........:=+++++                                      
                                      ++++=..............=++++                                      
                                      +++=.=##=......=##-.=+++                                      
                                     ++++:-%%-.-....-%%:.-:++++                                     
                                     +++=.*%%. *....#%%..*.=+++                                     
                                     +++-.#%%#*%....%%%###.-+++                                     
                                     +++-.#%%%%#....#%%%%#.-+++                                     
                                     +++-.+%%%%*....*%%%%+.-+++                                     
                                      ++=.:#%%#:....:#%%#:.=++                                      
                                      +++..:=+:......:+=:..+++                                      
                                     ++++-................-++++                                     
                                     +++++:..............:+++++                                     
                                    +++++++:............:+++++++                                    
                                   +++++**+:............:+**+++++                                   
                                   ++++****+=::......::=+****++++                                   
                                  +++++*********++++*********+++++                                  
                                  +++++++******************+++++++                                  
                                  ++++++:.-+***************:++++++                                  
                                 +++++++....::--------::***-+++++++                                 
                                 ++++++-................+**==++++++                                 
                                 ++++++:................-***-++++++                                 
                                 ++++++:.................***-++++++                                 
                                 ++++++..................+**=++++++                                 
                                 ++++++..................-***++++++                                 
                                 ++++++...................***++++++                                 
                                  +++++:..................=*++++++                                  
                                  +++++-...................:-+++++                                  
                                  +++++=....................=+++++                                  
                                  ++++++:..................:+++++* 
                                   +++++-..................-+++++                                   
                                    +++++:................:+++++                                    
                                    +++++=................++++++                                    
                                     +++++=..............=+++++                                     
                                      +++++=:..........:=+++++                                      
                                       ++++++-........-++++++                                       
                                        ++++++++=--=++++++++                                        
                                          ++++++++++++++++                                          
                                            ++++++++++++                                            
                                               *++++* "#
    );
}
//----------- Unit Tests ---------------------
//--------------------------------------------
//

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_env::MockEnv;

    fn options(args: &[&str]) -> RunOptions {
        parse_run_options(
            &args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn fresh_installs_always_sync_packages() {
        assert_eq!(
            options(&["install-wizard", "--skip-package-sync"]),
            RunOptions {
                refresh_mode: false,
                sync_packages: true,
                release_root: false,
            }
        );
    }

    #[test]
    fn refresh_mode_preserves_legacy_full_refresh_without_skip_flags() {
        assert_eq!(
            options(&["install-wizard", "--refresh-configs"]),
            RunOptions {
                refresh_mode: true,
                sync_packages: true,
                release_root: false,
            }
        );
    }

    #[test]
    fn refresh_mode_can_skip_only_package_sync() {
        assert_eq!(
            options(&["install-wizard", "--refresh-configs", "--skip-package-sync",]),
            RunOptions {
                refresh_mode: true,
                sync_packages: false,
                release_root: false,
            }
        );
    }

    #[test]
    fn release_refresh_preserves_user_workspace_links() {
        assert_eq!(
            options(&["install-wizard", "--refresh-configs", "--release-root"]),
            RunOptions {
                refresh_mode: true,
                sync_packages: true,
                release_root: true,
            }
        );
    }

    #[test]
    fn cloudflare_helper_deployment_uses_a_root_owned_fixed_path_and_policy() {
        let env = MockEnv::default();
        let repo = Path::new("/repo");
        let source = repo.join(CF_TOGGLE_HELPER_SOURCE);
        env.mock_files.borrow_mut().insert(
            source.to_string_lossy().to_string(),
            "compiled helper".to_string(),
        );

        install_cloudflare_toggle_helper(&env, repo).unwrap();

        let log = env.cmd_log.borrow();
        assert!(log.iter().any(|(command, args)| {
            command == "sudo"
                && args
                    == &vec![
                        "install".to_string(),
                        "-m".to_string(),
                        "755".to_string(),
                        "-o".to_string(),
                        "root".to_string(),
                        "-g".to_string(),
                        "root".to_string(),
                        source.to_string_lossy().to_string(),
                        CF_TOGGLE_HELPER_PATH.to_string(),
                    ]
        }));
        let policy = env.mock_files.borrow();
        let contents = policy.get(CF_TOGGLE_POLICY_PATH).unwrap();
        assert!(contents.contains("org.freedesktop.policykit.exec.path"));
        assert!(contents.contains("org.freedesktop.policykit.exec.argv1\">toggle"));
    }

    #[test]
    fn gtklock_is_retired_only_after_genoa_lock_and_its_config_migration() {
        let mut env = MockEnv::default();
        let repo = Path::new("/repo");
        let home = Path::new("/home/testuser");
        let source = repo.join(GENOA_LOCK_SOURCE);
        let idle_suspend_source = repo.join(GENOA_IDLE_SUSPEND_SOURCE);
        let helper_source = repo.join(GENOA_LOCK_PAM_HELPER_SOURCE);
        let sway_config = home.join(".config/sway/UserConfigs/UserKeybinds.conf");
        let gtklock_config = home.join(".config/gtklock");
        env.mock_files.borrow_mut().insert(
            source.to_string_lossy().to_string(),
            "compiled Genoa Lock".to_string(),
        );
        env.mock_files.borrow_mut().insert(
            idle_suspend_source.to_string_lossy().to_string(),
            "compiled Genoa idle suspend helper".to_string(),
        );
        env.mock_files.borrow_mut().insert(
            helper_source.to_string_lossy().to_string(),
            "compiled Genoa Lock PAM helper".to_string(),
        );
        env.mock_files.borrow_mut().insert(
            sway_config.to_string_lossy().to_string(),
            "bindsym Control+Mod1+l exec sh -c 'flock -n /tmp/gtklock.lock gtklock -d'\n"
                .to_string(),
        );
        env.mock_dirs
            .borrow_mut()
            .insert(gtklock_config.to_string_lossy().to_string());
        env.installed_packages.insert("gtklock".to_string());
        env.installed_packages
            .insert("gtklock-runshell-module".to_string());

        install_genoa_lock(&env, repo).unwrap();
        retire_gtklock(&env, home).unwrap();

        let log = env.cmd_log.borrow();
        assert!(log.iter().any(|(command, args)| {
            command == "sudo"
                && args
                    == &vec![
                        "install".to_string(),
                        "-m".to_string(),
                        "755".to_string(),
                        "-o".to_string(),
                        "root".to_string(),
                        "-g".to_string(),
                        "root".to_string(),
                        source.to_string_lossy().to_string(),
                        GENOA_LOCK_PATH.to_string(),
                    ]
        }));
        assert!(log.iter().any(|(command, args)| {
            command == "sudo"
                && args
                    == &vec![
                        "install".to_string(),
                        "-m".to_string(),
                        "755".to_string(),
                        "-o".to_string(),
                        "root".to_string(),
                        "-g".to_string(),
                        "root".to_string(),
                        idle_suspend_source.to_string_lossy().to_string(),
                        GENOA_IDLE_SUSPEND_PATH.to_string(),
                    ]
        }));
        assert!(log.iter().any(|(command, args)| {
            command == "sudo"
                && args
                    == &vec![
                        "install".to_string(),
                        "-m".to_string(),
                        "755".to_string(),
                        "-o".to_string(),
                        "root".to_string(),
                        "-g".to_string(),
                        "root".to_string(),
                        helper_source.to_string_lossy().to_string(),
                        GENOA_LOCK_PAM_HELPER_PATH.to_string(),
                    ]
        }));
        assert!(log.iter().any(|(command, args)| {
            command == "sudo"
                && args
                    == &vec![
                        "pacman".to_string(),
                        "-R".to_string(),
                        "--noconfirm".to_string(),
                        "gtklock".to_string(),
                        "gtklock-runshell-module".to_string(),
                    ]
        }));
        assert!(
            env.mock_files
                .borrow()
                .get(&sway_config.to_string_lossy().to_string())
                .is_some_and(|contents| contents.contains("/usr/local/bin/genoa-lock"))
        );
        assert!(!env.path_exists(&gtklock_config));
        assert_eq!(
            env.mock_files.borrow().get(GENOA_LOCK_PAM_PATH),
            Some(&GENOA_LOCK_PAM_POLICY.to_string())
        );
    }

    #[test]
    fn setup_idle_services_deploys_the_resume_guard_units() {
        let env = MockEnv::default();
        let home = Path::new("/home/testuser");

        setup_idle_services(home, &env).unwrap();

        let files = env.mock_files.borrow();
        for (name, contents) in IDLE_SERVICE_UNITS {
            let destination = home.join(".config/systemd/user").join(name);
            assert_eq!(
                files.get(&destination.to_string_lossy().to_string()),
                Some(&contents.to_string())
            );
            assert!(
                contents
                    .contains("ExecStartPre=/usr/local/bin/genoa-idle-suspend --record-baseline")
            );
            assert!(contents.contains("ExecStart=%h/.cargo/bin/genoa-idle run"));
        }
        assert!(env.cmd_log.borrow().contains(&(
            "systemctl".to_string(),
            vec!["--user".to_string(), "daemon-reload".to_string()]
        )));
    }

    #[test]
    fn gtklock_is_preserved_when_a_custom_active_command_needs_manual_migration() {
        let mut env = MockEnv::default();
        let home = Path::new("/home/testuser");
        let sway_config = home.join(".config/sway/swayidle.sh");
        let gtklock_config = home.join(".config/gtklock");
        env.mock_files.borrow_mut().insert(
            sway_config.to_string_lossy().to_string(),
            "timeout 30 'gtklock --custom-flag'\n".to_string(),
        );
        env.mock_dirs
            .borrow_mut()
            .insert(gtklock_config.to_string_lossy().to_string());
        env.installed_packages.insert("gtklock".to_string());

        retire_gtklock(&env, home).unwrap();

        assert!(!env.cmd_log.borrow().iter().any(|(command, args)| {
            command == "sudo" && args.first().is_some_and(|argument| argument == "pacman")
        }));
        assert!(env.path_exists(&gtklock_config));
    }

    #[test]
    fn test_setup_battery_daemon() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/home/.config/systemd/user/battery-daemon.service".to_string(),
            "\nolder content\n".to_string(),
        );
        env.mock_files.borrow_mut().insert(
            "/etc/UPower/UPower.conf".to_string(),
            "\nPercentageAction=5.0\nCriticalPowerAction=PowerOff\n".to_string(),
        );
        let result = setup_battery_daemon(std::path::Path::new("/home"), &env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert!(
            log.contains(&(
                "systemctl".to_string(),
                vec!["--user".to_string(), "daemon-reload".to_string()]
            )),
            "Expected daemon-reload to be triggered when service file is updated"
        );
    }
    #[test]
    fn test_setup_battery_daemon_without_update() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/home/.config/systemd/user/battery-daemon.service".to_string(),
            include_str!("../../battery-daemon/battery-daemon.service").to_string(),
        );
        env.mock_files.borrow_mut().insert(
            "/etc/UPower/UPower.conf".to_string(),
            "\nPercentageAction=5.0\nCriticalPowerAction=PowerOff\n".to_string(),
        );
        let result = setup_battery_daemon(std::path::Path::new("/home"), &env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert!(
            !log.contains(&(
                "systemctl".to_string(),
                vec!["--user".to_string(), "daemon-reload".to_string()]
            )),
            "Expected no commands to be triggered when service file is already up to date"
        );
        assert_eq!(
            log.len(),
            1,
            "Expected exactly one command to be run for battery daemon setup when no update is needed"
        );
        assert_eq!(
            log[0],
            (
                "systemctl".to_string(),
                vec![
                    "--user".to_string(),
                    "enable".to_string(),
                    "--now".to_string(),
                    "battery-daemon.service".to_string()
                ]
            )
        );
    }

    #[test]
    fn test_configure_upower() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/UPower/UPower.conf".to_string(),
            "\n#PercentageAction=2.0\nCriticalPowerAction=Hibernate\n".to_string(),
        );
        let result = configure_upower(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        let binding = env.mock_files.borrow();
        let updated_file = binding.get("/etc/UPower/UPower.conf").unwrap();
        assert_eq!(
            updated_file,
            "\nPercentageAction=5.0\nCriticalPowerAction=PowerOff"
        );
        assert_eq!(
            log.len(),
            2,
            "Expected UPower to install the changed config and restart the service"
        );
        assert_eq!(
            log[0],
            (
                "sudo".to_string(),
                vec![
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string(),
                    "-o".to_string(),
                    "root".to_string(),
                    "-g".to_string(),
                    "root".to_string(),
                    "/tmp/mock_file".to_string(),
                    "/etc/UPower/UPower.conf".to_string(),
                ]
            )
        );
        assert_eq!(
            log[1],
            (
                "sudo".to_string(),
                vec![
                    "systemctl".to_string(),
                    "restart".to_string(),
                    "upower.service".to_string()
                ]
            )
        );
    }
    #[test]
    fn test_configure_upower_without_update() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/UPower/UPower.conf".to_string(),
            "\nPercentageAction=5.0\nCriticalPowerAction=PowerOff\n".to_string(),
        );
        let result = configure_upower(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            0,
            "Expected no commands to be run when UPower config is already correct"
        );
    }
}
