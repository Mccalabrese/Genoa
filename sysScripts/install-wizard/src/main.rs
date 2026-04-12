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
use inquire::Text;
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::NamedTempFile;

const TURING_IDS: &[&str] = &[
    "0x1e02", "0x1e04", "0x1e07", "0x1e30", // Titan RTX, 2080 Ti, Quadro...
    "0x1f02", "0x1f06", "0x1f08", "0x1f82", // 2070, 2060, 1650 (TU106)...
    "0x2182", "0x2184", "0x2187", "0x2188", // 1660 Ti, 1660, 1650 Super, 1650...
    "0x2191", "0x21d1", // GTX 1650 Mobile variants..."0x1e02", "0x1e04", "0x1e07", "0x1e30",
];

// --- Enums for Hardware Detection ---
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum NvidiaArch {
    Modern,
    Turing,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GpuVendor {
    Unknown,
    Intel,
    Amd,
    Nvidia(NvidiaArch),
}

// Hardware Specific: NVIDIA
const NVIDIA_PACKAGES: &[&str] = &[
    "nvidia-dkms",
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
    "librewolf-bin",
];

const NEW_REPO_DIR: &str = "Genoa";
const LEGACY_REPO_DIR: &str = "rust-wayland-power";
// ---------- Main Execution ------_-------

// ---------- Main Execution -----------------
// ---------- Main Execution -----------------
fn main() {
    let home = dirs::home_dir().unwrap_or_else(|| {
        eprintln!(
            "{}",
            "❌ Critical Error: Could not determine home directory.".red()
        );
        std::process::exit(1);
    });
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
    let refresh_mode = args.contains(&"--refresh-configs".to_string());

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
            let gpu = detect_gpu();
            match gpu {
                GpuVendor::Nvidia(NvidiaArch::Turing) => {
                    println!("   👉 NVIDIA Turing Detected (GTX 16xx / RTX 20xx).");
                    if let Err(e) = setup_turing_gpu() {
                        eprintln!("   ❌ Failed to install legacy NVIDIA drivers: {}", e);
                        std::process::exit(1);
                    }
                }
                GpuVendor::Nvidia(NvidiaArch::Modern) => {
                    println!("   👉 Modern NVIDIA Detected (RTX 30xx/40xx).");
                    if let Err(e) = install_pacman_packages(NVIDIA_PACKAGES) {
                        eprintln!("   ❌ Failed to install NVIDIA drivers: {}", e);
                        std::process::exit(1);
                    }
                }
                GpuVendor::Amd => {
                    println!("   👉 AMD Detected.");
                    if let Err(e) = install_pacman_packages(AMD_PACKAGES) {
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

    let ignored_pkgs = get_ignored_packages();
    common_pkgs.retain(|pkg| !ignored_pkgs.contains(pkg));

    if common_pkgs.is_empty() {
        println!("   ⚠️  No packages found in pkglist.txt.");
    } else {
        let pkg_refs: Vec<&str> = common_pkgs.iter().map(|s| s.as_str()).collect();
        if let Err(e) = install_pacman_packages(&pkg_refs) {
            eprintln!("   ❌ Failed to install standard packages: {}", e);
            std::process::exit(1);
        };
    }

    if !AUR_PACKAGES.is_empty() {
        println!("\n{}", "📦 Syncing AUR Packages...".blue().bold());
        if let Err(e) = install_aur_packages(&home) {
            eprintln!("   ❌ Failed to install AUR packages: {}", e);
        };
    }

    // 2. Re-compile Rust Apps (Ensures updates to your tools are applied)
    println!("\n{}", "🦀 Syncing Custom Rust Apps...".blue().bold());
    // GUARANTEE Rust toolchain is loaded and set to stable (fixes GUI launcher bug)
    let _ = Command::new("rustup")
        .args(["default", "stable"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if let Err(e) = build_custom_apps(&home, &repo_root) {
        println!("   ⚠️  Failed to build custom Rust apps: {}", e);
    };

    let update_only_mode = has_existing_install && !refresh_mode;
    if update_only_mode {
        println!(
            "\n{}",
            "ℹ️  Existing install detected. Limiting this run to pkglist + sysScripts sync."
                .yellow()
                .bold()
        );
    } else {
        println!(
            "\n{}",
            "⚙️  Applying System Configurations...".blue().bold()
        );
        if let Err(e) = optimize_pacman_config() {
            eprintln!("   ❌ Failed to optimize pacman configuration: {}", e);
        }

        // 3. Hardware Enforcement
        let current_gpu = detect_gpu();

        let is_nvidia = if let GpuVendor::Nvidia(arch) = current_gpu {
            if arch == NvidiaArch::Turing
                && let Err(e) = setup_turing_gpu()
            {
                eprintln!("   ❌ Failed to set up Turing NVIDIA drivers: {}", e);
                std::process::exit(1);
            }
            if let Err(e) = apply_nvidia_configs(&arch) {
                eprintln!("   ❌ Failed to apply NVIDIA configurations: {}", e);
                std::process::exit(1);
            }
            true
        } else {
            false
        };

        enforce_session_order(is_nvidia);

        // 4. Check or install battery-daemon
        if let Err(e) = setup_battery_daemon(&home) {
            eprintln!("   ❌ Failed to set up battery-daemon: {}", e);
        }
    }

    // 5. Finalize
    if !refresh_mode {
        if has_existing_install {
            // --- UPDATE MODE (safe for personal configs) ---
            println!("\n{}", "🔧 Repairing managed symlink targets...".blue().bold());
            repair_repo_symlink_targets(&home, previous_repo_root.as_deref(), &repo_root);
            if let Err(e) = write_repo_root(&repo_root) {
                eprintln!("   ⚠️ Failed to write repository root to config: {}", e);
            }

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
            link_dotfiles_and_copy_resources(&home, &repo_root);

            if let Err(e) = configure_system(&home) {
                eprintln!("   ❌ Failed to configure system services: {}", e);
                std::process::exit(1);
            }

            if let Err(e) = setup_librewolf(&home) {
                eprintln!("   ⚠️ Failed to configure LibreWolf: {}", e);
            }
            setup_waybar_configs(&home);
            if let Err(e) = setup_secrets_and_geoclue(&home) {
                eprintln!("   ⚠️ Failed to set up secrets and geoclue: {}", e);
            }
            if let Err(e) = write_repo_root(&repo_root) {
                eprintln!("   ⚠️ Failed to write repository root to config: {}", e);
            }
            finalize_setup(&home); // Neovim/Tmux plugins

            print_logo();
            println!(
                "\n{}",
                "✅ Installation Complete! Please Reboot.".green().bold()
            );
        }
    } else {
        // --- REFRESH MODE (Updater) ---
        println!("\n{}", "🔧 Repairing managed symlink targets...".blue().bold());
        repair_repo_symlink_targets(&home, previous_repo_root.as_deref(), &repo_root);
        if let Err(e) = write_repo_root(&repo_root) {
            eprintln!("   ⚠️ Failed to write repository root to config: {}", e);
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

// --- Helper functions ---
/// Silently transitions existing users from the legacy hardcoded paths
/// to the new dynamic, config-driven architecture.
fn migrate_legacy_users(home: &Path) {
    let old_repo = home.join(LEGACY_REPO_DIR);
    let new_repo = home.join(NEW_REPO_DIR);

    // If the old repo exists, we have a legacy user who needs rescuing
    if old_repo.exists() {
        println!("\n{}", "🔄 Legacy installation detected. Silently migrating system...".magenta());

        // 1. Move the physical folder to the new name
        // (This is safe because this binary is currently running from ~/.cargo/bin/)
        if !new_repo.exists() {
            if let Err(e) = fs::rename(&old_repo, &new_repo) {
                eprintln!("   ⚠️ Failed to rename repository folder: {}", e);
                return; // Abort migration, let them safely remain on the old folder for now
            }
        }

        let active_repo = if new_repo.exists() { &new_repo } else { &old_repo };

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

fn write_repo_root(repo_root: &Path) -> Result<(), std::io::Error> {
    let home = dirs::home_dir().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found"))?;
    let config_path = home.join(".config/rust-dotfiles/config.toml");
    let repo_root_str = repo_root.to_str().ok_or_else(|| std::io::Error::other("Invalid repo root path"))?;
    let config_str = fs::read_to_string(&config_path)?;
    let updated_toml = upsert_repo_root_in_config(&config_str, repo_root_str);
    if updated_toml != config_str {
        fs::write(&config_path, updated_toml)?;
    }
    Ok(())
}

fn upsert_repo_root_in_config(content: &str, repo_root: &str) -> String {
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let escaped_root = repo_root.replace('\\', "\\\\").replace('"', "\\\"");
    let root_line = format!("root = \"{}\"", escaped_root);

    if let Some(repo_idx) = lines.iter().position(|l| l.trim() == "[repo]") {
        let mut section_end = lines.len();
        for (idx, line) in lines.iter().enumerate().skip(repo_idx + 1) {
            let trimmed = line.trim();
            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                section_end = idx;
                break;
            }
        }

        let mut root_idx = None;
        for (idx, line) in lines.iter().enumerate().take(section_end).skip(repo_idx + 1) {
            let normalized = line.trim_start().trim_start_matches('#').trim_start();
            if normalized.starts_with("root") && normalized.contains('=') {
                root_idx = Some(idx);
                break;
            }
        }

        if let Some(idx) = root_idx {
            lines[idx] = root_line;
        } else {
            lines.insert(repo_idx + 1, root_line);
        }
    } else {
        if !lines.is_empty() && !lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("[repo]".to_string());
        lines.push(root_line);
    }

    let mut updated = lines.join("\n");
    if !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated
}

/// Reads a package list from a text file (one package per line).
/// Ignores empty lines and comments starting with '#'.
fn load_packages_from_file(filename: &str, repo_root: &Path) -> std::io::Result<Vec<String>> {
    let path = repo_root.join(filename);

    let content = fs::read_to_string(&path)?;
    println!("   ✅ Loaded package list from '{}'.", filename);
    Ok(content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(String::from)
        .collect::<Vec<String>>())
}

/// Parses `lspci` output to identify GPU vendor IDs.
/// 10de = NVIDIA, 1002 = AMD, 8086 = Intel.
fn detect_gpu() -> GpuVendor {
    let Ok(entries) = std::fs::read_dir("/sys/bus/pci/devices") else {
        eprintln!("⚠️ Failed to read PCI devices. Defaulting to Unknown");
        return GpuVendor::Unknown;
    };
    let mut gpus = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let class_path = path.join("class");
        let vendor_path = path.join("vendor");
        let Ok(class_hex) = fs::read_to_string(&class_path) else {
            continue;
        };
        let Ok(vendor_hex) = fs::read_to_string(&vendor_path) else {
            continue;
        };
        let Ok(device_hex) = fs::read_to_string(path.join("device")) else {
            continue;
        };
        if class_hex.trim() == "0x030000" || class_hex.trim() == "0x038000" {
            // VGA Controller
            match vendor_hex.trim() {
                "0x10de" => {
                    let dev = device_hex.trim();
                    if TURING_IDS.contains(&dev)
                        || dev.starts_with("0x1e")
                        || dev.starts_with("0x1f")
                        || dev.starts_with("0x21")
                    {
                        gpus.push(GpuVendor::Nvidia(NvidiaArch::Turing));
                    } else {
                        gpus.push(GpuVendor::Nvidia(NvidiaArch::Modern));
                    }
                }
                "0x1002" => gpus.push(GpuVendor::Amd),
                "0x8086" => gpus.push(GpuVendor::Intel),
                _ => continue,
            }
        }
    }
    gpus.into_iter().max().unwrap_or(GpuVendor::Unknown) // If multiple GPUs, we prioritize NVIDIA > AMD > Intel
}

/// Scans /sys/class/drm to find the integrated GPU (Intel or AMD).
/// Returns a tuple: (Card Path, Vendor Type "intel"|"amd")
fn find_igpu() -> Option<(String, String)> {
    let entries = std::fs::read_dir("/sys/class/drm").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("card") || name.contains("-") {
            continue;
        } // We only care about card* entries and want to ignore cables
        let device_path = path.join("device");
        let vendor_path = path.join("device/vendor");
        let Ok(symlink_target) = fs::read_link(&device_path) else {
            continue;
        };
        let Some(link_str) = symlink_target.to_str() else {
            continue;
        };
        if !link_str.contains("0000:00:") {
            continue;
        } // iGPU's addresses only
        let Ok(vendor_hex) = fs::read_to_string(&vendor_path) else {
            continue;
        };
        match vendor_hex.trim() {
            "0x8086" => return Some((format!("/dev/dri/{}", name), "intel".to_string())),
            "0x1002" => return Some((format!("/dev/dri/{}", name), "amd".to_string())),
            _ => continue,
        }
    }
    None
}

/// 1. Check if user is on old drivers and ignoring updates in their pacman conf.
/// 2. If they are installingg from scratch, just install AUR nvidia-580-dkms which supports Turing and older cards on newer kernels.
/// 3. For users on old drivers, halt&warn, execute removing ignore line from pacman conf, pacman
///    -Rdd old drivers, install mainline kernel, install AUR drivers, run mkinicpio and
///    grub-mkconfig if user is on grub, and force reboot to load the new drivers safely.
fn setup_turing_gpu() -> Result<(), std::io::Error> {
    let pacman_conf = "/etc/pacman.conf";
    let pac_conf_content = fs::read_to_string(pacman_conf)?;
    let drivers_installed = Command::new("pacman")
        .args(["-Q", "nvidia-580xx-dkms"])
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    let is_legacy_nvidia = pac_conf_content.lines().any(|line| {
        let trimmed = line.trim_start();
        !trimmed.starts_with('#')
            && trimmed.starts_with("IgnorePkg")
            && (trimmed.contains("nvidia") || trimmed.contains("nvidia-dkms"))
    });
    if is_legacy_nvidia
        && !inquire::Confirm::new("⚠️  Legacy NVIDIA configuration detected. We need to migrate you to the new AUR drivers to restore mainline kernel support. This will rebuild your drivers and reboot your computer. Proceed?").with_default(true).prompt().unwrap_or(false) {        
            std::process::exit(1);
        }
    let mut config_modified = false;

    let mut inside_multilib = false;
    let mut lines: Vec<String> = pac_conf_content.lines().map(|s| s.to_string()).collect();
    for line in &mut lines {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#')
            && trimmed.starts_with("IgnorePkg")
            && (trimmed.contains("nvidia") || trimmed.contains("nvidia-dkms"))
        {
            *line = line
                .replace("lib32-nvidia-utils", "")
                .replace("nvidia-settings", "")
                .replace("nvidia-utils", "")
                .replace("nvidia-dkms", "")
                .replace("nvidia", "");
            config_modified = true;
            continue;
        }
        if trimmed.to_lowercase() == "#[multilib]" {
            *line = "[multilib]".to_string();
            config_modified = true;
            inside_multilib = true;
        } else if inside_multilib
            && trimmed.starts_with("#Include")
            && trimmed.contains("mirrorlist")
        {
            *line = "Include = /etc/pacman.d/mirrorlist".to_string();
            config_modified = true;
            inside_multilib = false;
        }
    }
    if config_modified {
        let mut temp_file = NamedTempFile::new()?;
        write!(temp_file, "{}", lines.join("\n"))?;
        run_cmd(
            "sudo",
            &[
                "install",
                "-m",
                "644",
                "-o",
                "root",
                "-g",
                "root",
                temp_file.path().to_str().unwrap(),
                pacman_conf,
            ],
        )?;
        run_cmd("sudo", &["pacman", "-Sy"])?;
    }
    if is_legacy_nvidia || !drivers_installed {
        let _ = run_cmd(
            "sudo",
            &[
                "pacman",
                "-Rdd",
                "--noconfirm",
                "nvidia-dkms",
                "nvidia-utils",
                "nvidia-settings",
            ],
        );
        let _ = run_cmd(
            "sudo",
            &["pacman", "-Rdd", "--noconfirm", "lib32-nvidia-utils"],
        ); // Remove 32-bit drivers if present
        let _ = run_cmd("sudo", &["pacman", "-Rdd", "--noconfirm", "libxnvctrl"]);
        run_cmd(
            "sudo",
            &["pacman", "-S", "--noconfirm", "linux", "linux-headers"],
        )?; // Ensure mainline kernel is installed
    }
    if is_legacy_nvidia || !drivers_installed {
        println!("   👉 Installing legacy NVIDIA drivers from AUR...");
        run_cmd(
            "yay",
            &[
                "-S",
                "--noconfirm",
                "nvidia-580xx-dkms",
                "nvidia-580xx-utils",
                "nvidia-580xx-settings",
                "libva-nvidia-driver",
            ],
        )?;
        let _ = run_cmd("yay", &["-S", "--noconfirm", "lib32-nvidia-580xx-utils"]); // Install 32-bit
    }
    if is_legacy_nvidia || !drivers_installed {
        run_cmd("sudo", &["mkinitcpio", "-P"])?; // Regenerate initramfs
        if Path::new("/boot/grub/grub.cfg").exists() {
            let _ = run_cmd("sudo", &["grub-mkconfig", "-o", "/boot/grub/grub.cfg"]); // Regenerate GRUB config if GRUB is present
        }
        let _ = run_cmd("sudo", &["reboot"]); // Reboot to load new drivers safely
        std::process::exit(0); // In case reboot command fails, we still want to exit to prevent further issues
    }
    Ok(())
}

/// Generates the sway-hybrid wrapper script with DYNAMIC paths.
fn create_sway_hybrid_script() -> Result<bool, std::io::Error> {
    println!("   🔧 Generating dynamic Sway-Hybrid wrapper...");

    // 1. Find the iGPU
    let (card_path, vendor) = match find_igpu() {
        Some(tuple) => tuple,
        None => {
            println!("   ⚠️  Could not detect iGPU. Defaulting to /dev/dri/card1 (Risky!)");
            ("/dev/dri/card1".to_string(), "intel".to_string())
        }
    };

    println!("      👉 iGPU Found: {} ({})", card_path, vendor);

    // 2. Determine Vulkan JSON path based on vendor
    let vulkan_driver = if vendor == "amd" {
        "radeon_icd.x86_64.json"
    } else {
        "intel_icd.x86_64.json"
    };

    // 3. Write the Script
    let script_content = format!(
        r#"#!/bin/sh
# --- Auto-Generated by Rust Installer ---
# Forces Sway to run on the iGPU ({vendor}) while keeping NVIDIA available for suspend.

# 1. Force OpenGL (Xwayland/X11 apps) to use Mesa
export __GLX_VENDOR_LIBRARY_NAME=mesa

# 2. Force Vulkan to use the iGPU
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/{vulkan}

# 3. Force EGL (Wayland apps) to use Mesa
export __EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json

# 4. The Critical Fix: Tell Sway (wlroots) explicitly which card to drive
export WLR_DRM_DEVICES={card}

# Launch Sway
exec sway
"#,
        vendor = vendor,
        vulkan = vulkan_driver,
        card = card_path
    );

    //Idempotency Check: If the file already exists with the same content, skip writing
    let wrapper_path = "/usr/local/bin/sway-hybrid";
    if fs::read_to_string(wrapper_path)
        .is_ok_and(|current_content| current_content == script_content)
    {
        println!("   ✅ Sway-Hybrid script is already up to date. No changes made.");
        return Ok(false);
    }

    // 4. Write to a secure temp file first (prevents partial writes to /usr/local/bin)
    let mut local_tmp = NamedTempFile::new()?;
    local_tmp.write_all(script_content.as_bytes())?;

    // 5. Use sudo to install it to /usr/local/bin with +x permissions
    let status = Command::new("sudo")
        .arg("install")
        .arg("-m")
        .arg("755")
        .arg("-o")
        .arg("root")
        .arg("-g")
        .arg("root")
        .arg(local_tmp.path())
        .arg(wrapper_path)
        .status()?;

    if !status.success() {
        eprintln!("{}", "❌ Failed to install sway-hybrid script.".red());
        return Err(std::io::Error::other("Failed to install sway-hybrid"));
    }
    Ok(true)
}
//-------- Main Steps ------
fn setup_librewolf(home: &Path) -> Result<(), std::io::Error> {
    println!("   🐺 Configuring LibreWolf for Human Beings...");

    let wolf_dir = home.join(".librewolf");
    let override_file = wolf_dir.join("librewolf.overrides.cfg");

    // Ensure directory exists
    fs::create_dir_all(&wolf_dir)?;

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
    fs::write(&override_file, config_content)?;
    // Set as Default Browser (XDG)
    println!("   👉 Setting LibreWolf as default browser...");
    let mimes = [
        "text/html",
        "x-scheme-handler/http",
        "x-scheme-handler/https",
    ];

    for mime in mimes {
        let _ = Command::new("xdg-mime")
            .args(["default", "librewolf.desktop", mime])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = Command::new("xdg-settings")
        .args(["set", "default-web-browser", "librewolf.desktop"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(())
}

/// installs packages via pacman with --needed and --noconfirm
fn install_pacman_packages(packages: &[&str]) -> Result<(), std::io::Error> {
    if packages.is_empty() {
        return Ok(());
    }
    let mut args = vec!["-S", "--needed", "--noconfirm"];
    args.extend(packages);
    let status = Command::new("sudo").arg("pacman").args(&args).status()?;
    if !status.success() {
        eprintln!(
            "{}",
            format!("❌ Failed to install packages: {}", packages.join(", ")).red()
        );
        return Err(std::io::Error::other("Failed to install packages"));
    }
    println!("   ✅ Installed packages: {}", packages.join(", "));
    Ok(())
}

/// Bootstraps 'yay' from the AUR git repo if not present.
/// This allows the script to run on a truly clean Arch install.
fn install_aur_packages(home: &Path) -> Result<(), std::io::Error> {
    if !Command::new("which")
        .arg("yay")
        .status()
        .is_ok_and(|s| s.success())
    {
        println!("   ⬇️  Bootstrapping 'yay'...");
        let clone_path = home.join("yay-clone");

        if clone_path.exists() {
            let _ = fs::remove_dir_all(&clone_path);
        }

        Command::new("git")
            .arg("clone")
            .arg("https://aur.archlinux.org/yay.git")
            .arg(&clone_path)
            .status()?;

        let status = Command::new("makepkg")
            .arg("-si")
            .arg("--noconfirm")
            .current_dir(&clone_path)
            .status()?;

        fs::remove_dir_all(&clone_path)?;

        if !status.success() {
            eprintln!("{}", "❌ Failed to install yay from AUR.".red());
            return Err(std::io::Error::other("Failed to install yay"));
        }
    }

    let mut args = vec!["-S", "--needed", "--noconfirm"];
    args.extend(AUR_PACKAGES);
    let status = Command::new("yay").args(&args).status()?;

    if !status.success() {
        eprintln!("{}", "⚠️  AUR Warning.".yellow());
    }
    Ok(())
}

/// Configures essential system services and settings, including mkinitcpio sanitation, enabling
/// geoclue/bluetooth/bolt, enabling Pacman cache cleanup, setting up the session environment, and
/// configuring logind and greetd. This function is idempotent and can be safely run multiple times
/// without causing issues.
fn configure_system(home: &Path) -> Result<(), std::io::Error> {
    sanitize_mkinitcpio()?;
    run_cmd("sudo", &["systemctl", "enable", "geoclue.service"])?;
    run_cmd("sudo", &["systemctl", "enable", "bluetooth.service"])?;
    run_cmd("sudo", &["systemctl", "enable", "bolt.service"])?;
    configure_dns()?;
    // Prevent Pacman from eating the entire hard drive over time
    println!("   🧹 Enabling automated Pacman cache cleanup...");
    run_cmd("sudo", &["systemctl", "enable", "--now", "paccache.timer"])?;

    // --- ENVIRONMENT & LOGIND ---
    println!("    🔧 Configuring Session Environment (PATH)...");
    let env_dir = home.join(".config/environment.d");
    let env_file = env_dir.join("99-cargo-path.conf");

    fs::create_dir_all(&env_dir)?;
    let content = "PATH=$HOME/.cargo/bin:$PATH\n";
    fs::write(&env_file, content)?;

    configure_logind()?;
    configure_greetd()?;
    configure_shell(home)?;
    Ok(())
}

/// Cleans up the `mkinitcpio.conf` file to fix the known Archinstall 2025 bug that appends 'o"' to
/// the end of the file,
fn sanitize_mkinitcpio() -> Result<(), std::io::Error> {
    // --- SANITIZE MKINITCPIO (Fix Archinstall 2025 Bug) ---
    // This protects NVIDIA users from the 'o"' corruption crash.
    println!("   🧹 Checking mkinitcpio.conf for corruption...");
    let mkinit_path = "/etc/mkinitcpio.conf";

    // Check if the file specifically ends with the garbage (ignoring whitespace)
    // We read it first to be safe, rather than firing sed blindly.
    if let Ok(content) = fs::read_to_string(mkinit_path) {
        let trimmed = content.trim(); // Removes trailing \n
        if trimmed.ends_with("o\"") || trimmed.ends_with("o”") {
            println!("   ⚠️  Corruption detected at end of file. Cleaning up...");
            let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
            let mut last_line = lines.pop().unwrap_or_default();
            if last_line.trim_end().ends_with("o\"") || last_line.trim_end().ends_with("o”") {
                // Remove the offending characters
                last_line = last_line.trim_end_matches(['o', '"', '”']).to_string();
                if !last_line.is_empty() {
                    lines.push(last_line);
                }
            } else {
                // If the last line doesn't match, we put it back (defensive)
                lines.push(last_line);
            }
            let mut temp_file = NamedTempFile::new()?;
            writeln!(temp_file, "{}", lines.join("\n"))?;
            let status = Command::new("sudo")
                .arg("install")
                .arg("-m")
                .arg("644")
                .arg("-o")
                .arg("root")
                .arg("-g")
                .arg("root")
                .arg(temp_file.path())
                .arg(mkinit_path)
                .status()?;
            if !status.success() {
                eprintln!("{}", "❌ Failed to sanitize mkinitcpio.conf.".red());
                return Err(std::io::Error::other("Failed to sanitize mkinitcpio.conf"));
            }
        }
    }
    Ok(())
}

///Configures dnscrypt-proxy to use Cloudflare's DNS servers for enhanced privacy and security.
fn configure_dns() -> Result<(), std::io::Error> {
    // --- DNS Crypt Proxy CONFIGURATION ---
    println!("   🔧 Configuring dnscrypt-proxy (DNS Proxy)...");

    // 1. Ensure package is installed (failsafe)
    let status = Command::new("sudo")
        .args(["pacman", "-S", "--needed", "--noconfirm", "dnscrypt-proxy"])
        .status()?;
    if !status.success() {
        eprintln!(
            "{}",
            "❌ Failed to install dnscrypt-proxy. DNS configuration aborted.".red()
        );
        return Err(std::io::Error::other("Failed to install dnscrypt-proxy"));
    }
    // 2. Configure TOML to use Cloudflare
    let dns_conf = "/etc/dnscrypt-proxy/dnscrypt-proxy.toml";
    let content = fs::read_to_string(dns_conf)?;
    let mut modified = false;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    for line in &mut lines {
        let normalized = line.trim_start().trim_start_matches('#').trim_start();
        if normalized.starts_with("server_names =") && normalized.contains("cloudflare") {
            if line == "server_names = ['cloudflare']" {
                continue; // Already correct
            }
            *line = "server_names = ['cloudflare']".to_string();
            modified = true;
        } else if normalized.starts_with("listen_addresses =")
            && normalized.contains("127.0.0.1:53")
        {
            if line == "listen_addresses = ['127.0.0.1:53', '[::1]:53']" {
                continue; // Already correct
            }
            *line = "listen_addresses = ['127.0.0.1:53', '[::1]:53']".to_string();
            modified = true;
        }
    }
    if modified {
        let mut temp_file = NamedTempFile::new()?;
        writeln!(temp_file, "{}", lines.join("\n"))?;
        let status = Command::new("sudo")
            .arg("install")
            .arg("-m")
            .arg("644")
            .arg("-o")
            .arg("root")
            .arg("-g")
            .arg("root")
            .arg(temp_file.path())
            .arg(dns_conf)
            .status()?;
        if !status.success() {
            eprintln!(
                "{}",
                "❌ Failed to update dnscrypt-proxy.toml with Cloudflare.".red()
            );
            return Err(std::io::Error::other(
                "Failed to update dnscrypt-proxy.toml",
            ));
        }
    }
    // 3. Enable the service
    run_cmd("sudo", &["systemctl", "enable", "--now", "dnscrypt-proxy"])?;

    // 4. Clean up old Cloudflared artifacts if they exist
    Command::new("sudo")
        .args(["systemctl", "disable", "--now", "cloudflared-dns"])
        .status()?;
    Command::new("sudo")
        .args(["rm", "-f", "/etc/systemd/system/cloudflared-dns.service"])
        .status()?;
    Command::new("sudo")
        .args(["systemctl", "daemon-reload"])
        .status()?;
    Ok(())
}

///Configures the user's shell to Zsh and sets up Tmux Plugin Manager for enhanced terminal
///experience.
fn configure_shell(home: &Path) -> Result<(), std::io::Error> {
    println!("    🔧 Setting Shell to Zsh...");
    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    Command::new("sudo")
        .args(["chsh", "-s", "/usr/bin/zsh", &user])
        .output()?;

    println!("    ✨ Setting up Tmux Plugin Manager...");
    let tpm_dir = home.join(".tmux/plugins/tpm");
    if !tpm_dir.exists() {
        Command::new("git")
            .arg("clone")
            .arg("https://github.com/tmux-plugins/tpm")
            .arg(tpm_dir)
            .status()?;
    }
    Ok(())
}

///Configures systemd-logind to ensure that user processes are killed on logout, preventing
///lingering sessions and resource leaks.
fn configure_logind() -> Result<(), std::io::Error> {
    println!("    🔧 Configuring Logind...");
    let logind_conf = "/etc/systemd/logind.conf";
    let content = fs::read_to_string(logind_conf)?;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut found = false;
    let mut modified = false;
    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("KillUserProcesses=") || trimmed.starts_with("#KillUserProcesses=") {
            if trimmed == "KillUserProcesses=yes" {
                println!("   ✅ KillUserProcesses is already set to yes.");
                found = true;
                break;
            }
            found = true;
            modified = true;
            *line = "KillUserProcesses=yes".to_string();
            break;
        }
    }
    if !found {
        // If the setting is not found, we add it under the [Login] section
        let login_section = lines.iter().position(|l| l.trim() == "[Login]");
        if let Some(idx) = login_section {
            lines.insert(idx + 1, "KillUserProcesses=yes".to_string());
        } else {
            // If [Login] section doesn't exist, append it at the end
            lines.push("[Login]".to_string());
            lines.push("KillUserProcesses=yes".to_string());
        }
        modified = true;
    }
    if modified {
        let mut temp_file = NamedTempFile::new()?;
        writeln!(temp_file, "{}", lines.join("\n"))?;
        let status = Command::new("sudo")
            .arg("install")
            .arg("-m")
            .arg("644")
            .arg("-o")
            .arg("root")
            .arg("-g")
            .arg("root")
            .arg(temp_file.path())
            .arg(logind_conf)
            .status()?;
        if !status.success() {
            eprintln!(
                "{}",
                "❌ Failed to update logind.conf with KillUserProcesses.".red()
            );
            return Err(std::io::Error::other("Failed to update logind.conf"));
        }
    }
    Ok(())
}

/// Configures Greetd with a custom tuigreet session and disables other DMs.
fn configure_greetd() -> Result<(), std::io::Error> {
    println!("    🔧 Configuring Greetd...");
    let greetd_config = r#"
[terminal]
vt = 1
[default_session]
command = "tuigreet --time --remember --sessions /usr/share/wayland-sessions:/usr/share/xsessions"
user = "greeter"
"#;
    tempfile::NamedTempFile::new()
        .and_then(|mut temp_file| {
            temp_file.write_all(greetd_config.as_bytes())?;
            Command::new("sudo")
                .arg("install")
                .arg("-m")
                .arg("644")
                .arg("-o")
                .arg("root")
                .arg("-g")
                .arg("root")
                .arg(temp_file.path())
                .arg("/etc/greetd/config.toml")
                .status()
        })
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other("Failed to install greetd config"))
            }
        })?;
    Command::new("sudo")
        .args(["systemctl", "disable", "gdm", "sddm", "lightdm"])
        .status()?;
    run_cmd(
        "sudo",
        &["systemctl", "enable", "--force", "greetd.service"],
    )?;
    Ok(())
}

/// Helper to run a command and check for success, returning an error if it fails.
fn run_cmd(cmd: &str, args: &[&str]) -> Result<(), std::io::Error> {
    let status = Command::new(cmd).args(args).status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "Command '{}' with args {:?} failed",
            cmd, args
        )));
    }
    Ok(())
}

/// Gleans pacman.conf to remove unwanted sessions and prevent future installs.
/// Gnome installs a lot of sessions we don't need, this keeps the list clean.
fn optimize_pacman_config() -> Result<(), std::io::Error> {
    println!("   🔧 Optimizing pacman.conf & Cleaning Sessions...");

    let sessions_to_remove = vec![
        "/usr/share/wayland-sessions/gnome-classic.desktop",
        "/usr/share/wayland-sessions/gnome-classic-wayland.desktop",
    ];

    for session in sessions_to_remove {
        Command::new("sudo").args(["rm", "-f", session]).output()?;
    }

    let pacman_conf = "/etc/pacman.conf";
    let content = fs::read_to_string(pacman_conf)?;

    if content.contains("NoExtract = usr/share/wayland-sessions/niri.desktop") {
        //println!("   👉 Injecting NoExtract rules into [options]...");
        println!("   👉 Removing old NoExtract rules to allow session updates...");
        let temp_content = content
            .lines()
            .filter(|line| {
                !line
                    .trim_start()
                    .starts_with("NoExtract = usr/share/wayland-sessions/")
            })
            .collect::<Vec<&str>>()
            .join("\n");
        let mut temp_file = NamedTempFile::new()?;
        writeln!(temp_file, "{}", temp_content)?;
        let status = Command::new("sudo")
            .arg("install")
            .arg("-m")
            .arg("644")
            .arg("-o")
            .arg("root")
            .arg("-g")
            .arg("root")
            .arg(temp_file.path())
            .arg(pacman_conf)
            .status()?;
        if !status.success() {
            eprintln!(
                "{}",
                "❌ Failed to update pacman.conf for session optimization.".red()
            );
            return Err(std::io::Error::other("Failed to update pacman.conf"));
        }
    }
    Ok(())
}

/// Applies specific fixes for NVIDIA on Wayland.
/// 1. Sets kernel parameters (`nvidia_drm.modeset=1`).
/// 2. Creates modprobe rules to fix suspend/resume.
/// 3. Rebuilds initramfs via `mkinitcpio`.
///
/// Security Note: Uses a secure temp file pattern for writing to /etc/.
/// NOW SMART: Differentiates between Turing (Legacy) and Modern (Ampere/Ada) cards.
fn apply_nvidia_configs(arch: &NvidiaArch) -> Result<(), std::io::Error> {
    println!("    Applying Nvidia Configs...");

    let is_turing = *arch == NvidiaArch::Turing;
    let mut requires_rebuild = false;

    if is_turing {
        println!("    ℹ️  Configuring for Turing Architecture (GTX 16xx / RTX 20xx)...");
    } else {
        println!("    ℹ️  Configuring for Modern NVIDIA Architecture...");
    }

    // Helper closure: Write to local dir (safe) then install
    let install_securely = |content: &str, dest: &str| -> Result<bool, std::io::Error> {
        if let Ok(existing) = fs::read_to_string(dest)
            && existing == content
        {
            println!("   ✅ {} is already up to date.", dest);
            return Ok(false); // No changes made
        }
        //let local_tmp = format!("./{}", filename);
        let mut temp_file = NamedTempFile::new()?;
        temp_file.write_all(content.as_bytes())?;
        // Use 'install' to copy with root:root ownership and 644 permissions
        let status = Command::new("sudo")
            .args([
                "install",
                "-m",
                "644",
                "-o",
                "root",
                "-g",
                "root",
                temp_file.path().to_str().unwrap(),
                dest,
            ])
            .status()?;
        if !status.success() {
            eprintln!("❌ Failed to install file to {}.", dest);
            return Err(std::io::Error::other(format!(
                "Failed to install file to {}",
                dest
            )));
        }
        Ok(true) // Changes were made
    };

    // --- 1. MODPROBE CONFIGURATION ---
    // Turing (GTX 16xx/20xx): Needs Firmware=0 to prevent hanging on suspend with legacy drivers.
    // Modern (RTX 30xx/40xx): Needs Firmware=1 (Default/GSP) for proper power management.
    let firmware_val = if is_turing { "0" } else { "1" };

    let modprobe_content = format!(
        "options nvidia NVreg_EnableGpuFirmware={} NVreg_DynamicPowerManagement=0x02 NVreg_EnableS0ixPowerManagement=1\noptions nvidia_drm modeset=1 fbdev=1\n",
        firmware_val
    );

    requires_rebuild |= install_securely(&modprobe_content, "/etc/modprobe.d/nvidia.conf")?;

    requires_rebuild |= install_securely(
        "blacklist nvidia_uvm\n",
        "/etc/modprobe.d/99-nvidia-uvm-blacklist.conf",
    )?;

    // --- 2. UDEV RULES (Common) ---
    // Keeps the dGPU 'auto' suspended when not in use.
    requires_rebuild |= install_securely(
        "SUBSYSTEM==\"pci\", ATTR{vendor}==\"0x10de\", ATTR{power/control}=\"auto\"\n",
        "/etc/udev/rules.d/90-nvidia-pm.rules",
    )?;

    // --- 4. MKINITCPIO CONFIGURATION ---
    // Newer cards often need early KMS loading for external display hotplug wakeup.
    // We only enforce this for non-turing, though it doesn't hurt turing.
    if !is_turing {
        requires_rebuild |= ensure_nvidia_modules_in_initcpio()?;
    }

    create_sway_hybrid_script()?;

    println!("    🏗️  Rebuilding Initramfs...");
    if requires_rebuild {
        Command::new("sudo").args(["mkinitcpio", "-P"]).status()?;
    } else {
        println!("    ✅ No changes to initramfs configuration. Skipping rebuild.");
    }
    Ok(())
}

/// Helper: Safely adds nvidia modules to mkinitcpio.conf if missing.
/// Handles the request: "-added nvidia to modules in mkinitcpio"
fn ensure_nvidia_modules_in_initcpio() -> Result<bool, std::io::Error> {
    println!("    🔧 Checking mkinitcpio modules for Modern NVIDIA support...");
    let config_path = "/etc/mkinitcpio.conf";
    let content = fs::read_to_string(config_path)?;

    let new_content = content
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("MODULES=") {
                let start = trimmed.find('(').unwrap_or(0);
                let end = trimmed.find(')').unwrap_or(trimmed.len());
                if start < end {
                    let inner = &trimmed[start + 1..end];
                    let mut modules: Vec<&str> = inner.split_whitespace().collect();

                    for req in ["nvidia", "nvidia_modeset", "nvidia_uvm", "nvidia_drm"] {
                        if !modules.contains(&req) {
                            modules.push(req);
                        }
                    }
                    return format!("MODULES=({})", modules.join(" "));
                }
            }
            line.to_string()
        })
        .collect::<Vec<String>>()
        .join("\n");
    if new_content == content.trim_end() {
        return Ok(false); // No changes needed
    }
    let mut temp_file = NamedTempFile::new()?;
    writeln!(temp_file, "{}", new_content)?;
    let status = Command::new("sudo")
        .arg("install")
        .arg("-m")
        .arg("644")
        .arg("-o")
        .arg("root")
        .arg("-g")
        .arg("root")
        .arg(temp_file.path())
        .arg(config_path)
        .status()?;
    if status.success() {
        println!("    ✅ Added nvidia modules to Initramfs config.");
        Ok(true)
    } else {
        eprintln!("    ⚠️  Failed to update mkinitcpio.conf.");
        Err(std::io::Error::other("Failed to update mkinitcpio.conf"))
    }
}
///I templated my waybar configs to allow gitignore of my personalization.
///This unpacks them if they don't already exist.
fn setup_waybar_configs(home: &Path) {
    let waybar_dir = home.join(".config/waybar");
    let configs = vec!["swayConfig.jsonc", "niriConfig.jsonc"];

    for config in configs {
        let template = waybar_dir.join(format!("{}.template", config));
        let target = waybar_dir.join(config);

        if template.exists() && !target.exists() {
            match fs::copy(&template, &target) {
                Ok(_) => println!("   ✅ Created {} from template", config),
                Err(e) => println!("   ⚠️  Failed to create {}: {}", config, e),
            }
        } else if target.exists() {
            println!("   ℹ️  {} already exists", config);
        }
    }
}
/// Interactive wizard to generate the local `config.toml`.
/// Validates input to prevent injection attacks before writing to system files (like /etc/geoclue).
fn setup_secrets_and_geoclue(home: &Path) -> Result<(), std::io::Error> {
    let config_dir = home.join(".config/rust-dotfiles");
    let config_path = config_dir.join("config.toml");
    // Logic to handle if 'rust-dotfiles' exists as a file instead of a directory
    if config_dir.exists() {
        if !config_dir.is_dir() {
            println!("   ⚠️  Found a file blocking config directory. Backing it up...");
            let backup = format!("{}.bak", config_dir.display());
            std::fs::rename(&config_dir, &backup)?;
            std::fs::create_dir_all(&config_dir)?;
        }
    } else {
        std::fs::create_dir_all(&config_dir)?;
    }

    if !config_path.exists() {
        println!(
            "   🧙 We need to generate your central config.toml and configure Location Services."
        );
        let weather_api = Text::new("Enter OpenWeatherMap API Key (get one by making a free account at https://home.openweathermap.org/users/sign_up):").prompt().unwrap_or("YOUR_SECRET_OWM_KEY_HERE".to_string());
        let finnhub_api = Text::new(
            "Enter Finnhub.io API Key (get one by making a free account at finnhub.io/register):",
        )
        .prompt()
        .unwrap_or("YOUR_FINNHUB_KEY_HERE".to_string());
        let template = include_str!("../../../.config/rust-dotfiles/config.toml.template")
            .replace("YOUR_SECRET_OWM_KEY_HERE", &weather_api)
            .replace("YOUR_FINNHUB_KEY_HERE", &finnhub_api);
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true).mode(0o600);
        match options.open(&config_path) {
            Ok(mut file) => {
                file.write_all(template.as_bytes())
                    .expect("Failed to write secure config.toml");
                println!("  ✅ Config generated securely at {:?}", config_path);
            }
            Err(e) => {
                eprintln!("❌ Failed to securely open config.toml: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        let contents = fs::read_to_string(&config_path)?;
        if contents.contains("YOUR_SECRET_OWM_KEY") || contents.contains("YOUR_FINNHUB_KEY") {
            let weather_api = Text::new("Enter OpenWeatherMap API Key (get one by making a free account at https://home.openweathermap.org/users/sign_up):").prompt().unwrap_or("YOUR_SECRET_OWM_KEY_HERE".to_string());
            let finnhub_api = Text::new("Enter Finnhub.io API Key (get one by making a free account at finnhub.io/register):").prompt().unwrap_or("YOUR_FINNHUB_KEY_HERE".to_string());
            let mut modified = false;
            let mut lines: Vec<String> = contents.lines().map(|s| s.to_string()).collect();
            for line in &mut lines {
                if line.contains("YOUR_SECRET_OWM_KEY") || line.contains("YOUR_FINNHUB_KEY") {
                    *line = line
                        .replace("YOUR_SECRET_OWM_KEY_HERE", &weather_api)
                        .replace("YOUR_FINNHUB_KEY_HERE", &finnhub_api);
                    modified = true;
                }
            }
            if modified {
                let mut temp_file = NamedTempFile::new()?;
                write!(temp_file, "{}", lines.join("\n"))?;
                let status = Command::new("sudo")
                    .arg("install")
                    .arg("-m")
                    .arg("600")
                    .arg("-o")
                    .arg(std::env::var("USER").unwrap_or_else(|_| "root".to_string()))
                    .arg("-g")
                    .arg(std::env::var("USER").unwrap_or_else(|_| "root".to_string()))
                    .arg(temp_file.path())
                    .arg(&config_path)
                    .status()?;
                if !status.success() {
                    eprintln!("{}", "❌ Failed to update config.toml with API keys.".red());
                    return Err(std::io::Error::other("Failed to update config.toml"));
                }
            }
        }
    }
    configure_geoclue()?;
    let wallpaper_path = home.join("Pictures/Wallpapers");
    if !wallpaper_path.exists() {
        println!(
            "   🖼️  Creating wallpaper directory at {:?}",
            wallpaper_path
        );
        fs::create_dir_all(&wallpaper_path)?;
    }
    Ok(())
}

fn configure_geoclue() -> Result<(), std::io::Error> {
    println!("   🌍 Configuring Geoclue...");
    let gc_path = "/etc/geoclue/geoclue.conf";
    let google_geo_api = Text::new("Enter Google Geolocation API Key for Geoclue(get one at console.cloud.google.com/apis/library/geocoding-backend.googleapis.com):").prompt().unwrap_or_default();
    if google_geo_api.is_empty() {
        println!("   ⚠️  No API key entered. Skipping Geoclue configuration.");
        return Ok(());
    }

    let mut modified = false;
    let new_url = format!(
        "url=https://www.googleapis.com/geolocation/v1/geolocate?key={}",
        google_geo_api
    );
    let content = fs::read_to_string(gc_path)?;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    for line in &mut lines {
        let normalized = line
            .trim_start()
            .trim_start_matches(['#', ';'])
            .trim_start();
        if normalized.starts_with("enable=") {
            if normalized == "enable=true" {
                println!("   ✅ Geoclue is already enabled.");
                continue; // Already enabled
            }
            *line = "enable=true".to_string();
            modified = true;
        } else if normalized.contains("googleapis.com") && normalized != new_url {
            *line = new_url.clone();
            modified = true;
        } else if normalized.starts_with("method=") && !normalized.contains("gmaps") {
            *line = "method=gmaps".to_string();
            modified = true;
        }
    }
    if !modified {
        println!("   ⚠️  No changes needed for geoclue.conf. It may already be configured.");
        return Ok(());
    }
    let mut temp_file = NamedTempFile::new()?;
    writeln!(temp_file, "{}", lines.join("\n"))?;
    let status = Command::new("sudo")
        .arg("install")
        .arg("-m")
        .arg("644")
        .arg("-o")
        .arg("root")
        .arg("-g")
        .arg("root")
        .arg(temp_file.path())
        .arg(gc_path)
        .status()?;
    if !status.success() {
        eprintln!(
            "{}",
            "   ❌ Failed to update geoclue.conf with API key.".red()
        );
        return Err(std::io::Error::other("Failed to update geoclue.conf"));
    }
    Ok(())
}

/// Helper to parse `cargo metadata` and extract the expected binary names for a given app.
/// Parses the JSON in a way that explicitly returns the app name if the parsing fails or the
/// expected fields are missing
fn expected_binary_names(app_path: &Path, app_name: &str) -> HashSet<String> {
    let mut expected = HashSet::new();
    let err_closure = |detail: &str| {
        eprintln!(
            "   ⚠️  Warning: {} for {}. Falling back to single binary assumption.",
            detail, app_name
        );
        HashSet::from([app_name.to_string()])
    };
    let metadata = match Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
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

/// Builds custom Rust apps using native caching.
/// If source files haven't changed, this takes milliseconds.
fn build_custom_apps(home: &Path, repo_root: &Path) -> Result<(), std::io::Error> {
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
                let status = Command::new("cargo")
                    .args(["build", "--release", "-q"])
                    .current_dir(&app_path)
                    .status();

                if status.is_ok_and(|s| s.success()) {
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
/// Renames session files to enforce a specific order in Greetd/Tuigreet.
/// Strategy: Move standard files (e.g. niri.desktop) to custom numbered files (10-niri.desktop).
/// This prevents Pacman from deleting our custom config during updates while NoExtract is active.
fn enforce_session_order(is_nvidia: bool) {
    println!("   🔧 Enforcing Session Order (Renaming .desktop files)...");

    let sessions_dir = "/usr/share/wayland-sessions";

    // Tuple: (Original Name, Safe Custom Name, Display Name)
    let updates = vec![
        ("niri.desktop", "10-niri.desktop", "1. Niri"),
        ("sway.desktop", "20-sway.desktop", "2. Sway (Battery)"),
        ("gnome.desktop", "40-gnome.desktop", "3. Gnome"),
        (
            "gnome-wayland.desktop",
            "40-gnome-wayland.desktop",
            "3. Gnome-wayland",
        ), // Handle Arch variation
    ];

    for (std_name, custom_name, display_name) in updates {
        let std_path = format!("{}/{}", sessions_dir, std_name);
        let custom_path = format!("{}/{}", sessions_dir, custom_name);

        // 1. If the standard file exists (fresh install or update), STEAL IT.
        // We move it to the custom path so Pacman doesn't own it anymore.
        if Path::new(&std_path).exists() {
            println!("      Moving {} -> {}", std_name, custom_name);
            let _ = Command::new("sudo")
                .args(["mv", "-f", &std_path, &custom_path])
                .status();
        }

        // 2. Patch the Name inside the CUSTOM file (if it exists)
        if Path::new(&custom_path).exists() {
            let sed_cmd = format!("s/^Name=.*/Name={}/", display_name);
            let _ = Command::new("sudo")
                .args(["sed", "-i", &sed_cmd, &custom_path])
                .status();
        }
    }

    let sway_session = "/usr/share/wayland-sessions/20-sway.desktop";

    if Path::new(sway_session).exists() {
        if is_nvidia {
            println!("   🔧 Pointing Sway (Battery) to NVIDIA hybrid wrapper...");
            // Replace Exec=sway with Exec=/usr/local/bin/sway-hybrid
            let _ = Command::new("sudo")
                .args([
                    "sed",
                    "-i",
                    "s|^Exec=.*|Exec=/usr/local/bin/sway-hybrid|",
                    sway_session,
                ])
                .status();
        } else {
            println!(" 🔧 Ensuring Sway uses native launch (Non-NVIDIA)...");
            // Standardize back to native sway
            let _ = Command::new("sudo")
                .args(["sed", "-i", "s|^Exec=.*|Exec=sway|", sway_session])
                .status();
            //Clean up wwrapper script if it exists from a previous hardware config
            let _ = Command::new("sudo")
                .args(["rm", "-f", "/usr/local/bin/sway-hybrid"])
                .status();
        }
    }
}

///Walks through dotfiles in repo and symlinks them to home directory.
fn link_dotfiles_and_copy_resources(home: &Path, repo_root: &Path) {
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
    // Link TLP
    let tlp_src = repo_root.join("tlp.conf");
    let _ = Command::new("sudo")
        .args(["ln", "-sf", tlp_src.to_str().unwrap(), "/etc/tlp.conf"])
        .status();
    let _ = Command::new("sudo")
        .args(["systemctl", "enable", "tlp.service"])
        .output();

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
    let _ = Command::new("xdg-user-dirs-update").status();
}
///Helper to create symlinks, backing up existing files if needed.
fn create_symlink(src: &Path, dest: &Path) {
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

/// During updates, only repair symlinks that were previously managed by this repo.
/// Never rewrite regular files/directories in the user's config.
fn repair_repo_symlink_targets(home: &Path, previous_repo_root: Option<&Path>, active_repo_root: &Path) {
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

fn maybe_repair_symlink(
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

    if fs::remove_file(dest).is_ok()
        && std::os::unix::fs::symlink(expected_target, dest).is_ok()
    {
        println!(
            "   ✅ Repaired symlink: {} -> {}",
            dest.display(),
            expected_target.display()
        );
    }
}
/// Runs post-install hooks to set up themes and plugins.
/// This ensures the user doesn't see "broken" visuals on first launch.
fn finalize_setup(home: &Path) {
    println!(
        "\n{}",
        "✨ Finalizing Setup (Themes & Plugins)...".blue().bold()
    );

    // 1. Install Tmux Plugins (Fixes the Green Bar)
    let tpm_script = home.join(".tmux/plugins/tpm/bin/install_plugins");
    if tpm_script.exists() {
        println!("   📦 Installing Tmux Plugins (Headless)...");
        // We capture output to avoid spamming the user's terminal unless it fails
        let status = Command::new(&tpm_script)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        match status {
            Ok(s) if s.success() => println!("   ✅ Tmux Plugins Installed"),
            _ => {
                println!("   ⚠️  Tmux plugin install failed (You can press Prefix + I inside Tmux)")
            }
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
        let status = Command::new("nvim")
            .args(["--headless", "+Lazy! sync", "+qa"])
            .stdout(Stdio::null()) // Neovim is noisy, silence it
            .stderr(Stdio::null())
            .status();

        match status {
            Ok(s) if s.success() => println!("   ✅ Neovim Plugins Synced"),
            _ => println!("   ⚠️  Neovim setup skipped (will run on first launch)"),
        }
    }
}
/// Reliably finds the root of the dotfiles repository regardless of where the binary is executed.
fn get_repo_root() -> Result<PathBuf, std::io::Error> {
    // Prefer deriving the repo from the current working directory so this works for
    // both `cargo run` and installed binaries invoked from the repo.
    let cwd = std::env::current_dir()?;

    for ancestor in cwd.ancestors() {
        if ancestor.join("sysScripts/install-wizard/Cargo.toml").exists() {
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

    Err(std::io::Error::other("Could not determine repository root from current directory"))
}

fn read_repo_root_from_config(home: &Path) -> Option<PathBuf> {
    let config_path = home.join(".config/rust-dotfiles/config.toml");
    let contents = fs::read_to_string(config_path).ok()?;

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

fn resolve_repo_root(home: &Path) -> Result<PathBuf, std::io::Error> {
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

    Err(std::io::Error::other("Repository root could not be resolved"))
}
/// Reads /etc/pacman.conf and extracts any packages listed in IgnorePkg.
fn get_ignored_packages() -> Vec<String> {
    let mut ignored = Vec::new();
    if let Ok(content) = fs::read_to_string("/etc/pacman.conf") {
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
    }
    ignored
}
// Installs the battery life warning and exectes systemctl poweroff to protect battery
/// Installs the battery life warning and exectes systemctl poweroff to protect battery
fn setup_battery_daemon(home: &Path) -> Result<(), std::io::Error> {
    println!("   🔋 Configuring Battery Safety Daemon...");

    configure_upower()?;

    let systemd_user_dir = home.join(".config/systemd/user");
    let service_dest = systemd_user_dir.join("battery-daemon.service");

    println!("   🔋 Setting up Battery Safety Daemon...");

    // Make sure the ~/.config/systemd/user/ folder actually exists
    std::fs::create_dir_all(&systemd_user_dir)?;
    let service_content = include_str!("../../battery-daemon/battery-daemon.service");
    let existing_content = std::fs::read_to_string(&service_dest).unwrap_or_default();

    if existing_content != service_content {
        println!("   ✅ Battery daemon already configured. Skipping systemd setup.");

        std::fs::write(&service_dest, service_content)?;

        let status = std::process::Command::new("systemctl")
            .arg("--user")
            .arg("daemon-reload")
            .status()?;
        if !status.success() {
            eprintln!("   ❌ Failed to reload systemd daemon for battery service.");
            return Err(std::io::Error::other("Failed to reload systemd daemon"));
        }
    } else {
        println!("   ✅ Battery daemon already configured. Skipping systemd setup.");
    }
    let status = std::process::Command::new("systemctl")
        .arg("--user")
        .arg("enable")
        .arg("--now")
        .arg("battery-daemon.service")
        .status()?;
    if !status.success() {
        eprintln!("   ❌ Failed to enable/start battery daemon service.");
        return Err(std::io::Error::other(
            "Failed to enable/start battery daemon",
        ));
    }

    println!("   ✅ Battery Daemon ready.");

    Ok(())
}

fn configure_upower() -> Result<(), std::io::Error> {
    println!("🔋 Enforcing UPower Critical Shutdown at 5%...");

    let upower_conf = "/etc/UPower/UPower.conf";
    let file_content = fs::read_to_string(upower_conf)?;
    let mut needs_update = false;
    let mut lines: Vec<String> = file_content.lines().map(|s| s.to_string()).collect();

    for line in &mut lines {
        let normalized = line.trim_start().trim_start_matches('#').trim_start();
        if normalized.starts_with("PercentageAction=") && !line.starts_with("PercentageAction=5.0")
        {
            needs_update = true;
            *line = "PercentageAction=5.0".to_string();
        } else if normalized.starts_with("CriticalPowerAction=")
            && !line.starts_with("CriticalPowerAction=PowerOff")
        {
            needs_update = true;
            *line = "CriticalPowerAction=PowerOff".to_string();
        }
    }
    if !needs_update {
        println!("⚡ UPower already configured for critical shutdown. Skipping.");
        return Ok(());
    }
    let mut temp_upower_file = NamedTempFile::new()?;
    writeln!(temp_upower_file, "{}", lines.join("\n"))?;
    let status = Command::new("sudo")
        .arg("install")
        .arg("-m")
        .arg("644")
        .arg("-o")
        .arg("root")
        .arg("-g")
        .arg("root")
        .arg(temp_upower_file.path())
        .arg(upower_conf)
        .status()?;
    if !status.success() {
        eprintln!(
            "{}",
            "❌ Failed to update script with new poweroff battery features".red()
        );
        return Err(std::io::Error::other("Failed to update upower config"));
    }
    // restarting to apply changes
    run_cmd("sudo", &["systemctl", "restart", "upower.service"])?;

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
