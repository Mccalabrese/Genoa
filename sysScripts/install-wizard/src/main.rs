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
    load_packages_from_file, migrate_legacy_users, read_repo_root_from_config,
    repair_repo_symlink_targets, resolve_repo_root, write_repo_root,
};
use crate::live_env::LiveEnv;
use crate::session::{configure_dns, configure_system, configure_tlp, enforce_session_order};
use crate::traits::CmdExecutor;
use crate::update::{
    get_ignored_packages, install_aur_packages, install_clepsydre_package, install_pacman_packages,
    optimize_pacman_config,
};
use crate::user::{
    build_custom_apps, finalize_setup, link_dotfiles_and_copy_resources,
    patch_waybar_sidebar_toggle_path, remove_retired_tool_sources, setup_librewolf,
    setup_secrets_and_geoclue, setup_waybar_configs,
};

// Hardware Specific: NVIDIA
const NVIDIA_PACKAGES: &[&str] = &[
    // New NVIDIA GPUs, including the RTX generation used in current Dell Pro
    // Max systems, require the open kernel module package.
    "nvidia-open",
    "nvidia-utils",
    "nvidia-prime",
    "nvidia-settings",
    "libva-nvidia-driver",
];

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

#[derive(Debug, PartialEq, Eq)]
struct RunOptions {
    refresh_mode: bool,
    sync_packages: bool,
}

fn parse_run_options(args: &[String]) -> RunOptions {
    let refresh_mode = args.iter().any(|arg| arg == "--refresh-configs");

    // This flag is an updater-only optimization. A fresh install always syncs
    // packages, and refresh mode keeps its legacy behavior unless told to skip.
    let sync_packages = !refresh_mode || !args.iter().any(|arg| arg == "--skip-package-sync");

    RunOptions {
        refresh_mode,
        sync_packages,
    }
}

// ---------- Main Execution ------_-------

fn main() {
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
    let previous_repo_root = read_repo_root_from_config(&home);
    let has_existing_install = home.join(".config/rust-dotfiles/config.toml").exists();

    migrate_legacy_users(&home);

    let repo_root = resolve_repo_root(&home).unwrap_or_else(|e| {
        eprintln!("❌ Error determining repository root: {}", e);
        std::process::exit(1);
    });

    // 0. Parse Arguments
    let args: Vec<String> = std::env::args().collect();
    let run_options = parse_run_options(&args);
    let refresh_mode = run_options.refresh_mode;

    if refresh_mode {
        println!("{}", "🔄 Running in CONFIG REFRESH MODE".magenta().bold());
        let status = Command::new("sudo").arg("-v").status().unwrap();
        if !status.success() {
            eprintln!("{}", "❌ Sudo required.".red());
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
        } else {
            println!("   ✅ No JACK audio server detected. Skipping removal.");
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
                    if let Err(e) = setup_turing_gpu(&live_sys) {
                        eprintln!("   ❌ Failed to install legacy NVIDIA drivers: {}", e);
                        std::process::exit(1);
                    }
                }
                GpuVendor::Nvidia(NvidiaArch::Modern) => {
                    println!("   👉 Modern NVIDIA Detected (RTX 30xx/40xx/Blackwell).");
                    if let Err(e) = install_pacman_packages(&live_sys, NVIDIA_PACKAGES) {
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

        println!("\n{}", "🦀 Setting up Rust (rustup)...".blue().bold());
        let _ = Command::new("rustup").args(["default", "stable"]).status();
    }

    // ==========================================
    //  SHARED LOGIC (Runs on Install AND Refresh)
    // ==========================================

    // 1. Sync Standard & AUR Packages
    if run_options.sync_packages {
        println!("\n{}", "📦 Syncing Standard Packages...".blue().bold());
        let mut common_pkgs = match load_packages_from_file("pkglist.txt", &repo_root) {
            Ok(pkgs) => pkgs,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("   ⚠️  pkglist.txt not found. Skipping package installation.");
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
            println!("   ⚠️  No packages found in pkglist.txt.");
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
            };
        }
    } else {
        println!(
            "\n{}",
            "📦 Skipping package sync (pkglist unchanged).".dimmed()
        );
    }

    println!("\n{}", "🌐 Configuring DNS proxy...".blue().bold());
    if let Err(e) = configure_dns(&live_sys) {
        eprintln!("   ❌ Failed to configure dnscrypt-proxy: {}", e);
        std::process::exit(1);
    }

    // 2. Re-compile Rust Apps (Ensures updates to your tools are applied)
    println!("\n{}", "🦀 Syncing Custom Rust Apps...".blue().bold());
    // GUARANTEE Rust toolchain is loaded and set to stable (fixes GUI launcher bug)
    let _ = Command::new("rustup")
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

    if let Err(e) = build_custom_apps(&live_sys, &home, &repo_root) {
        println!("   ⚠️  Failed to build custom Rust apps: {}", e);
    };

    if let Err(e) = configure_tlp(&live_sys, &repo_root) {
        eprintln!("   ❌ Failed to configure TLP power management: {}", e);
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
        match configure_detected_nvidia(&live_sys) {
            Ok(true) => {
                if let Err(e) = enforce_session_order(&live_sys, true, &repo_root) {
                    eprintln!("   ❌ Failed to refresh NVIDIA session integration: {}", e);
                    std::process::exit(1);
                }
            }
            Ok(false) => {}
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
        let is_nvidia = configure_detected_nvidia(&live_sys).unwrap_or_else(|e| {
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
            println!(
                "\n{}",
                "🔧 Repairing managed symlink targets...".blue().bold()
            );
            repair_repo_symlink_targets(&home, previous_repo_root.as_deref(), &repo_root);
            if let Err(e) = write_repo_root(&repo_root) {
                eprintln!("   ⚠️ Failed to write repository root to config: {}", e);
            }
            patch_waybar_sidebar_toggle_path(&live_sys, &home);

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
            link_dotfiles_and_copy_resources(&live_sys, &home, &repo_root);

            if let Err(e) = configure_system(&live_sys, &home) {
                eprintln!("   ❌ Failed to configure system services: {}", e);
                std::process::exit(1);
            }

            if let Err(e) = setup_librewolf(&live_sys, &home) {
                eprintln!("   ⚠️ Failed to configure LibreWolf: {}", e);
            }
            setup_waybar_configs(&live_sys, &home);
            patch_waybar_sidebar_toggle_path(&live_sys, &home);
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
        println!(
            "\n{}",
            "🔧 Repairing managed symlink targets...".blue().bold()
        );
        repair_repo_symlink_targets(&home, previous_repo_root.as_deref(), &repo_root);
        if let Err(e) = write_repo_root(&repo_root) {
            eprintln!("   ⚠️ Failed to write repository root to config: {}", e);
        }
        patch_waybar_sidebar_toggle_path(&live_sys, &home);
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
fn configure_detected_nvidia(sys: &impl CmdExecutor) -> Result<bool, std::io::Error> {
    let GpuVendor::Nvidia(arch) = detect_gpu(sys) else {
        return Ok(false);
    };

    if arch == NvidiaArch::Turing {
        setup_turing_gpu(sys)?;
    } else {
        // Update runs also repair systems where older GPU detection skipped an
        // NVIDIA 3D controller and therefore never installed its driver.
        install_pacman_packages(sys, NVIDIA_PACKAGES)?;
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
    } else {
        println!("   ✅ Battery daemon already configured. Skipping systemd setup.");
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
        println!("⚡ UPower already configured for critical shutdown. Skipping.");
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
            }
        );
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
            1,
            "Expected exactly one command to be run for UPower configuration"
        );
        assert_eq!(
            log[0],
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
