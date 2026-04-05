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
use inquire::{Select, Text};
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
// ---------- Main Execution ------_-------

// ---------- Main Execution -----------------
// ---------- Main Execution -----------------
fn main() {
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
        let _ = Command::new("sudo")
            .args(["pacman", "-Rdd", "--noconfirm", "jack2"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        // GPU Drivers Checkpoint & Exit Logic
        let state_file = dirs::home_dir()
            .unwrap()
            .join(".cache/rust_installer_drivers_done");

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
                    if let Err(e) = install_nvidia_legacy_580() {
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
    let mut common_pkgs = match load_packages_from_file("pkglist.txt") {
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
        if let Err(e) = install_aur_packages() {
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
    build_custom_apps();

    println!(
        "\n{}",
        "⚙️  Applying System Configurations...".blue().bold()
    );
    optimize_pacman_config();

    // 3. Hardware Enforcement
    let current_gpu = detect_gpu();

    let is_nvidia = if let GpuVendor::Nvidia(arch) = current_gpu {
        if arch == NvidiaArch::Turing {
            enforce_turing_kernel(); // Nuke mainline, install LTS
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
    setup_battery_daemon();

    // 5. Finalize
    if !refresh_mode {
        // --- FRESH INSTALL ONLY ---
        println!("\n{}", "🔗 Linking Config Files...".blue().bold());
        link_dotfiles_and_copy_resources();

        if let Err(e) = configure_system() {
            eprintln!("   ❌ Failed to configure system services: {}", e);
        }

        if let Err(e) = setup_librewolf() {
            eprintln!("   ❌ Failed to configure LibreWolf: {}", e);
        }
        setup_waybar_configs();
        setup_secrets_and_geoclue();
        finalize_setup(); // Neovim/Tmux plugins

        print_logo();
        println!(
            "\n{}",
            "✅ Installation Complete! Please Reboot.".green().bold()
        );
    } else {
        // --- UPDATE MODE ---
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

/// Reads a package list from a text file (one package per line).
/// Ignores empty lines and comments starting with '#'.
fn load_packages_from_file(filename: &str) -> std::io::Result<Vec<String>> {
    let repo_root = get_repo_root();
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

/// Installs the specific 580.119.02 driver from Arch Archive and locks it.
fn install_nvidia_legacy_580() -> Result<(), std::io::Error> {
    println!(
        "\n{}",
        "🛑 Turing GPU Detected (GTX 16xx / RTX 20xx)"
            .yellow()
            .bold()
    );
    println!("   The latest NVIDIA drivers (590+) break power management on this card.");
    println!("   Downgrading to version 580.119.02 for battery life safety...");

    // 1. Install specific versions via URL
    // We include lib32 variants assuming multilib is enabled (standard for gaming)
    let packages = vec![
        "https://archive.archlinux.org/packages/n/nvidia-dkms/nvidia-dkms-580.119.02-1-x86_64.pkg.tar.zst",
        "https://archive.archlinux.org/packages/n/nvidia-utils/nvidia-utils-580.119.02-1-x86_64.pkg.tar.zst",
        "https://archive.archlinux.org/packages/n/nvidia-settings/nvidia-settings-580.119.02-1-x86_64.pkg.tar.zst",
        "https://archive.archlinux.org/packages/l/lib32-nvidia-utils/lib32-nvidia-utils-580.119.02-1-x86_64.pkg.tar.zst",
    ];

    let mut args = vec!["-U", "--noconfirm"];
    args.extend(packages);

    let status = Command::new("sudo").arg("pacman").args(&args).status()?;

    if !status.success() {
        eprintln!(
            "{}",
            "❌ Critical Error: Failed to install legacy NVIDIA drivers.".red()
        );
        return Err(std::io::Error::other("Failed to install NVIDIA drivers"));
    }

    // 2. Pin the version in pacman.conf
    println!("   🔒 Pinning NVIDIA drivers in /etc/pacman.conf...");
    let pacman_conf = "/etc/pacman.conf";
    let ignore_line = "IgnorePkg = nvidia-dkms nvidia-utils lib32-nvidia-utils nvidia-settings";

    // Check if IgnorePkg is already active
    let content = fs::read_to_string(pacman_conf).unwrap_or_default();

    if !content.contains(ignore_line) {
        // We look for the [options] header and insert IgnorePkg below it
        // Or simply uncomment the existing IgnorePkg line if standard arch config
        // Simplest robust method: Append to [options]

        let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        let opts = lines.iter().position(|line| line.starts_with("[options]"));
        let patched_content = if let Some(idx) = opts {
            lines.insert(idx + 1, ignore_line.to_string());
            lines.join("\n")
        } else {
            // If [options] not found, append at the end (unlikely)
            format!("{}\n\n[options]\n{}", content, ignore_line)
        };
        let mut temp_file = NamedTempFile::new()?;
        temp_file.write_all(patched_content.as_bytes())?;
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
                "❌ Failed to update pacman.conf with IgnorePkg.".red()
            );
            return Err(std::io::Error::other("Failed to update pacman.conf"));
        }
    }
    println!("   ✅ Drivers pinned. System updates will skip NVIDIA.");
    Ok(())
}

/// Generates the sway-hybrid wrapper script with DYNAMIC paths.
fn create_sway_hybrid_script() -> Result<(), std::io::Error> {
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
    if let Ok(current_content) = fs::read_to_string(wrapper_path)
        && current_content == script_content
    {
        println!("   ✅ Sway-Hybrid script is already up to date. No changes made.");
        return Ok(());
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
    Ok(())
}
//-------- Main Steps ------
fn setup_librewolf() -> Result<(), std::io::Error> {
    println!("   🐺 Configuring LibreWolf for Human Beings...");

    let home = dirs::home_dir()
        .ok_or_else(|| std::io::Error::other("Could not determine home directory"))?;
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

    Command::new("xdg-settings")
        .args(["set", "default-web-browser", "librewolf.desktop"])
        .status()?;

    Command::new("xdg-mime")
        .args(["default", "librewolf.desktop", "x-scheme-handler/http"])
        .status()?;

    Command::new("xdg-mime")
        .args(["default", "librewolf.desktop", "x-scheme-handler/https"])
        .status()?;
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
fn install_aur_packages() -> Result<(), std::io::Error> {
    if !Command::new("which")
        .arg("yay")
        .status()
        .is_ok_and(|s| s.success())
    {
        println!("   ⬇️  Bootstrapping 'yay'...");
        let home = dirs::home_dir()
            .ok_or_else(|| std::io::Error::other("Could not determine home directory"))?;
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
fn configure_system() -> Result<(), std::io::Error> {
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
    let home = dirs::home_dir()
        .ok_or_else(|| std::io::Error::other("Could not determine home directory"))?;
    let env_dir = home.join(".config/environment.d");
    let env_file = env_dir.join("99-cargo-path.conf");

    fs::create_dir_all(&env_dir)?;
    let content = "PATH=$HOME/.cargo/bin:$PATH\n";
    fs::write(&env_file, content)?;

    configure_logind()?;
    configure_greetd()?;
    configure_shell()?;
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
            temp_file.write_all(lines.join("\n").as_bytes())?;
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
    Command::new("sudo")
        .args(["pacman", "-S", "--needed", "--noconfirm", "dnscrypt-proxy"])
        .status()?;

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
        temp_file.write_all(lines.join("\n").as_bytes())?;
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
fn configure_shell() -> Result<(), std::io::Error> {
    println!("    🔧 Setting Shell to Zsh...");
    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    Command::new("sudo")
        .args(["chsh", "-s", "/usr/bin/zsh", &user])
        .output()?;

    println!("    ✨ Setting up Tmux Plugin Manager...");
    let home = dirs::home_dir()
        .ok_or_else(|| std::io::Error::other("Could not determine home directory"))?;
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
        temp_file.write_all(lines.join("\n").as_bytes())?;
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
fn optimize_pacman_config() {
    println!("   🔧 Optimizing pacman.conf & Cleaning Sessions...");

    let sessions_to_remove = vec![
        "/usr/share/wayland-sessions/gnome.desktop",
        "/usr/share/wayland-sessions/gnome-classic.desktop",
        "/usr/share/wayland-sessions/gnome-classic-wayland.desktop",
    ];

    for session in sessions_to_remove {
        let _ = Command::new("sudo").args(["rm", "-f", session]).output();
    }

    let pacman_conf = "/etc/pacman.conf";
    let content = fs::read_to_string(pacman_conf).unwrap_or_default();

    if content.contains("NoExtract = usr/share/wayland-sessions/niri.desktop") {
        //println!("   👉 Injecting NoExtract rules into [options]...");
        println!("   👉 Removing old NoExtract rules to allow session updates...");
        // Sed to delete lines containing "wayland-sessions"
        let _ = Command::new("sudo")
            .args(["sed", "-i", "/wayland-sessions/d", pacman_conf])
            .status();
    }
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

    if is_turing {
        println!("    ℹ️  Configuring for Turing Architecture (GTX 16xx / RTX 20xx)...");
    } else {
        println!("    ℹ️  Configuring for Modern NVIDIA Architecture...");
    }

    // Helper closure: Write to local dir (safe) then install
    let install_securely = |content: &str, dest: &str| {
        let Some(filename) = Path::new(dest).file_name().and_then(|n| n.to_str()) else {
            eprintln!("❌ Invalid destination path: {}", dest);
            return;
        };
        let local_tmp = format!("./{}", filename);

        if let Err(e) = fs::write(&local_tmp, content) {
            eprintln!("❌ Failed to write local file {}: {}", local_tmp, e);
            return;
        }

        // Use 'install' to copy with root:root ownership and 644 permissions
        let status = Command::new("sudo")
            .args([
                "install", "-m", "644", "-o", "root", "-g", "root", &local_tmp, dest,
            ])
            .status();

        match status {
            Ok(s) if s.success() => {
                let _ = fs::remove_file(&local_tmp); // Cleanup
            }
            _ => {
                eprintln!("⚠️  Failed to install {} to {}", local_tmp, dest);
            }
        }
    };

    // --- 1. MODPROBE CONFIGURATION ---
    // Turing (GTX 16xx/20xx): Needs Firmware=0 to prevent hanging on suspend with legacy drivers.
    // Modern (RTX 30xx/40xx): Needs Firmware=1 (Default/GSP) for proper power management.
    let firmware_val = if is_turing { "0" } else { "1" };

    let modprobe_content = format!(
        "options nvidia NVreg_EnableGpuFirmware={} NVreg_DynamicPowerManagement=0x02 NVreg_EnableS0ixPowerManagement=1\n",
        firmware_val
    );

    install_securely(&modprobe_content, "/etc/modprobe.d/nvidia.conf");

    install_securely(
        "blacklist nvidia_uvm\n",
        "/etc/modprobe.d/99-nvidia-uvm-blacklist.conf",
    );

    // --- 2. UDEV RULES (Common) ---
    // Keeps the dGPU 'auto' suspended when not in use.
    install_securely(
        "SUBSYSTEM==\"pci\", ATTR{vendor}==\"0x10de\", ATTR{power/control}=\"auto\"\n",
        "/etc/udev/rules.d/90-nvidia-pm.rules",
    );

    // --- 3. GRUB Configuration (Common) ---
    let grub_path = "/etc/default/grub";
    println!("    🔧 Checking GRUB for NVIDIA modeset...");
    let content = fs::read_to_string(grub_path).unwrap_or_default();

    if !content.contains("nvidia_drm.modeset=1") {
        println!("    👉 Adding nvidia_drm.modeset=1 to GRUB...");
        let result = Command::new("sudo")
            .args([
                "sed",
                "-i",
                "s/GRUB_CMDLINE_LINUX_DEFAULT=\"[^\"]*/& nvidia_drm.modeset=1/",
                grub_path,
            ])
            .status();
        match result {
            Ok(status) if status.success() => {
                println!("    ✅ GRUB configured for NVIDIA modeset.");
            }
            Ok(status) => {
                println!(
                    "    ⚠️  Failed to patch GRUB (exit code: {}. Please manually add nvidia_drm.modeset=1)",
                    status
                );
            }
            Err(e) => {
                eprintln!("    ❌ Failed to run sed for GRUB configuration: {}", e);
            }
        }
    }

    // --- 4. MKINITCPIO CONFIGURATION ---
    // Newer cards often need early KMS loading for external display hotplug wakeup.
    // We only enforce this for non-turing, though it doesn't hurt turing.
    if !is_turing {
        ensure_nvidia_modules_in_initcpio();
    }

    create_sway_hybrid_script()?;

    println!("    🏗️  Rebuilding Initramfs & GRUB...");
    let _ = Command::new("sudo").args(["mkinitcpio", "-P"]).status();
    let _ = Command::new("sudo")
        .args(["grub-mkconfig", "-o", "/boot/grub/grub.cfg"])
        .status();
    Ok(())
}

/// Helper: Safely adds nvidia modules to mkinitcpio.conf if missing.
/// Handles the request: "-added nvidia to modules in mkinitcpio"
fn ensure_nvidia_modules_in_initcpio() {
    println!("    🔧 Checking mkinitcpio modules for Modern NVIDIA support...");
    let config_path = "/etc/mkinitcpio.conf";

    let content = fs::read_to_string(config_path).unwrap_or_default();

    // We check if 'nvidia' is already in the file to avoid duplicates
    if !content.contains("nvidia ") && !content.contains("(nvidia)") {
        println!("    👉 Injecting nvidia modules into mkinitcpio.conf...");

        // Sed magic:
        // Finds the line starting with MODULES=(...
        // Replaces the closing parenthesis ')' with ' nvidia nvidia_modeset nvidia_uvm nvidia_drm)'
        let status = Command::new("sudo")
            .args([
                "sed",
                "-i",
                "s/^MODULES=(\\(.*\\))/MODULES=(\\1 nvidia nvidia_modeset nvidia_uvm nvidia_drm)/",
                config_path,
            ])
            .status()
            .unwrap_or_else(|_| {
                eprintln!("❌ sed not found");
                std::process::exit(1);
            });

        if status.success() {
            println!("    ✅ Added nvidia modules to Initramfs config.");
        } else {
            eprintln!("    ⚠️  Failed to update mkinitcpio.conf.");
        }
    } else {
        println!("    ℹ️  Nvidia modules already present in mkinitcpio.");
    }
}
///I templated my waybar configs to allow gitignore of my personalization.
///This unpacks them if they don't already exist.
fn setup_waybar_configs() {
    let home = dirs::home_dir().unwrap_or_else(|| {
        eprintln!("⚠️ Could not determine home directory. Using /tmp as fallback.");
        PathBuf::from("/tmp")
    });
    let waybar_dir = home.join(".config/waybar");
    let configs = vec!["hyprConfig.jsonc", "swayConfig.jsonc", "niriConfig.jsonc"];

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
fn setup_secrets_and_geoclue() {
    let home = dirs::home_dir().unwrap_or_else(|| {
        eprintln!("⚠️ Could not determine home directory. Using /tmp as fallback.");
        PathBuf::from("/tmp")
    });
    let config_dir = home.join(".config/rust-dotfiles");
    let config_path = config_dir.join("config.toml");

    let wallpaper_path = home.join("Pictures/Wallpapers");
    fs::create_dir_all(&wallpaper_path).expect("Failed to create wallpaper dir");

    println!("   🧙 We need to generate your central config.toml and configure Location Services.");

    let weather_api = Text::new("Enter OpenWeatherMap API Key (get one by making a free account at https://home.openweathermap.org/users/sign_up):").prompt().unwrap_or_else(|e| { eprintln!("❌ Error: {}", e); std::process::exit(1); });
    let finnhub_api = Text::new(
        "Enter Finnhub.io API Key (get one by making a free account at finnhub.io/register):",
    )
    .prompt()
    .unwrap_or_else(|e| {
        eprintln!("❌ Error: {}", e);
        std::process::exit(1);
    });

    // SECURE FIX: Validation logic for keys to prevent injection
    let google_geo_api = Text::new("Enter Google Geolocation API Key for Geoclue(get one at console.cloud.google.com/apis/library/geocoding-backend.googleapis.com):").prompt().unwrap_or_else(|e| { eprintln!("❌ Error: {}", e); std::process::exit(1); });

    // --- GEOCLUE CONFIGURATION ---
    if !google_geo_api.is_empty() {
        println!("   🌍 Configuring Geoclue...");
        let gc_path = "/etc/geoclue/geoclue.conf";

        // 1. Ensure the wifi source is enabled (uncomment 'enable=true')
        // We use a loose match to catch ';enable=true' or '#enable=true'
        let _ = Command::new("sudo")
            .args(["sed", "-i", "s/^.*enable=true/enable=true/", gc_path])
            .status();

        // 2. Inject the Key
        // We look for the placeholder URL provided by the package and replace it.
        // The default line usually looks like:
        // #url=https://www.googleapis.com/geolocation/v1/geolocate?key=YOUR_KEY

        // We construct a regex-like sed command to find the googleapis line (commented or not)
        // and replace the WHOLE line with our active key.
        let new_url = format!(
            "url=https://www.googleapis.com/geolocation/v1/geolocate?key={}",
            google_geo_api
        );

        // This sed command finds any line containing "googleapis.com" and replaces the entire line.
        let status = Command::new("sudo")
            .args([
                "sed",
                "-i",
                &format!("s|^.*googleapis.com.*|{}|", new_url),
                gc_path,
            ])
            .status();

        match status {
            Ok(s) if s.success() => {
                let _ = Command::new("sudo")
                    .args(["systemctl", "restart", "geoclue.service"])
                    .output();
                println!("   ✅ Geoclue Configured");
            }
            _ => eprintln!("   ❌ Failed to patch geoclue config."),
        }
    } else {
        println!("   ⚠️  No Google API Key provided. Location services may fail.");
    }
    let term_choice = Select::new("Preferred Terminal:", vec!["ghostty", "alacritty", "kitty"])
        .prompt()
        .unwrap_or("ghostty");
    if config_path.exists() {
        println!("   ℹ️  config.toml already exists. Skipping write.");
        return;
    }

    let config_content = format!(
        r#"[global]
pager = "bat --paging=always --style=plain"
terminal = "{}"

[waybar_weather]
owm_api_key = "{}"

[waybar_finance]
api_key = "{}"
stocks = ["SPY", "QQQ", "NVDA"]

[wallpaper_manager]
wallpaper_dir = "{}/Pictures/Wallpapers"
swww_params = ["--transition-fps", "60", "--transition-type", "any", "--transition-duration", "2"]
swaybg_cache_file = "swaybg_last_wallpaper"
cache_file = "~/.cache/wallpapers.json"
rofi_config_path = "~/.config/rofi/config-wallpaper.rasi"
rofi_theme_override = "element-icon {{ size: 20%; }}"

[update_check]
command_string = "nm-online -q -t 5 && (checkupdates; yay -Qua) || true"
cache_file = "~/.cache/update-check.json"
stale_icon = "⚠"
error_icon = "!"

[updater]
update_command = ["yay", "-Syu"]
icon_success = "/usr/share/icons/Adwaita/48x48/status/software-update-available.png"
icon_error = "/usr/share/icons/Adwaita/48x48/status/dialog-error.png"
window_title = "System Update"

[waybar_switcher]
target_file = "/tmp/waybar-config.jsonc"
niri_config = "~/.config/waybar/niriConfig.jsonc"
sway_config = "~/.config/waybar/swayConfig.jsonc"

[cloudflare_toggle]
text_on = "󰅟"
class_on = "on"
text_off = "⚠︎"
class_off = "off"
service_name = "dnscrypt-proxy"
resolv_content_on = "nameserver 127.0.0.1"
resolv_content_off = "nameserver 1.1.1.1\nnameserver 1.0.0.1"
bar_process_name = "waybar"
bar_signal_num = 10

[clip_manager]
rofi_config = "~/.config/rofi/config-clipboard.rasi"
message = "CTRL+DEL = Delete Entry | ALT+DEL = Wipe History"

[emoji_picker]
rofi_config = "~/.config/rofi/config-emoji.rasi"
message = "Search Emojis (Name or Keyword)"

[radio_menu]
rofi_config = "~/.config/rofi/config-radio.rasi"
message = "Radio Menu"

[kb_launcher.compositor_args]
sway = ["--title=KeybindCheatSheetApp"]
niri = ["--title=KeybindCheatSheet"]
default = []

[[kb_launcher.sheet]]
name = "Niri"
file = "~/.config/niri/keybinds_niri.txt"
compositor = "niri"

[[kb_launcher.sheet]]
name = "Sway"
file = "~/.config/sway/keybinds_sway.txt"
compositor = "sway"

[[kb_launcher.sheet]]
name = "Neovim"
file = "~/.config/nvim/keybinds_nvim.txt"
"#,
        term_choice,
        weather_api,
        finnhub_api,
        home.display()
    );

    // Logic to handle if 'rust-dotfiles' exists as a file instead of a directory
    if config_dir.exists() {
        if !config_dir.is_dir() {
            println!("   ⚠️  Found a file blocking config directory. Backing it up...");
            let backup = format!("{}.bak", config_dir.display());
            std::fs::rename(&config_dir, &backup).expect("Failed to move blocking file");
            std::fs::create_dir_all(&config_dir).expect("Failed to create config dir");
        }
    } else {
        std::fs::create_dir_all(&config_dir).expect("Failed to create config dir");
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true).mode(0o600);
    match options.open(&config_path) {
        Ok(mut file) => {
            file.write_all(config_content.as_bytes())
                .expect("Failed to write secure config.toml");
            println!("  ✅ Config generated securely at {:?}", config_path);
        }
        Err(e) => {
            eprintln!("❌ Failed to securely open config.toml: {}", e);
            std::process::exit(1);
        }
    }
}
fn expected_binary_names(app_path: &Path, app_name: &str) -> HashSet<String> {
    let mut expected = HashSet::new();

    let metadata = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(app_path)
        .output();

    if let Ok(output) = metadata
        && output.status.success()
        && let Ok(json) = serde_json::from_slice::<Value>(&output.stdout)
        && let Some(packages) = json.get("packages").and_then(|v| v.as_array())
    {
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
    }

    // Safe fallback so single-bin crates still update even if metadata fails.
    if expected.is_empty() {
        expected.insert(app_name.to_string());
    }

    expected
}

/// Builds custom Rust apps using native caching.
/// If source files haven't changed, this takes milliseconds.
fn build_custom_apps() {
    let repo_root = get_repo_root();
    let sys_scripts_dir = repo_root.join("sysScripts");

    // Ensure ~/.cargo/bin exists
    let cargo_bin_dir = dirs::home_dir().unwrap().join(".cargo/bin");
    let _ = fs::create_dir_all(&cargo_bin_dir);

    if let Ok(entries) = fs::read_dir(&sys_scripts_dir) {
        for entry in entries.flatten() {
            let app_path = entry.path();
            if app_path.is_dir() && app_path.join("Cargo.toml").exists() {
                let app_name = app_path.file_name().unwrap().to_str().unwrap();
                let status = Command::new("cargo")
                    .args(["build", "--release", "-q"])
                    .current_dir(&app_path)
                    .status();

                if status.is_ok() && status.unwrap().success() {
                    let release_dir = app_path.join("target/release");
                    let expected_bins = expected_binary_names(&app_path, app_name);
                    let mut synced_any = false;

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
                                (Ok(_), Err(_)) => true,
                                _ => false,
                            };

                            if should_update {
                                if target_bin.exists() {
                                    let _ = fs::remove_file(&target_bin);
                                }
                                if fs::copy(&bin_path, &target_bin).is_ok() {
                                    println!("       ✅ Synced binary: {}", filename);
                                    synced_any = true;
                                }
                            }
                        }
                    }

                    if !synced_any {
                        println!("       ✔ {} is already up to date", app_name);
                    }
                } else {
                    println!("      ❌ Failed to build {}", app_name);
                }
            }
        }
    }
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
fn link_dotfiles_and_copy_resources() {
    let home = dirs::home_dir().unwrap_or_else(|| {
        eprintln!("⚠️ Could not determine home directory. Using /tmp as fallback.");
        PathBuf::from("/tmp")
    });
    let repo_root = get_repo_root();

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
            "   ℹ️  Neovim config found. Skipping to preserve your setup. If you would like my setup just copy ~/rust-wayland-power/.config/nvim to ~/.config/nvim"
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
/// Runs post-install hooks to set up themes and plugins.
/// This ensures the user doesn't see "broken" visuals on first launch.
fn finalize_setup() {
    println!(
        "\n{}",
        "✨ Finalizing Setup (Themes & Plugins)...".blue().bold()
    );
    let home = dirs::home_dir().unwrap();

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
/// Specifically enforces the LTS kernel for Turing GPUs to protect the 580 driver.
/// It will install linux-lts and actively remove the mainline linux kernel.
fn enforce_turing_kernel() {
    println!("   🛡️  Turing GPU: Verifying LTS Kernel environment...");

    // 1. Ensure LTS is installed
    let _ = Command::new("sudo")
        .args([
            "pacman",
            "-S",
            "--needed",
            "--noconfirm",
            "linux-lts",
            "linux-lts-headers",
        ])
        .status();

    // 2. Check if mainline is installed
    let check_mainline = Command::new("pacman")
        .args(["-Q", "linux"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    if check_mainline.is_ok() && check_mainline.unwrap().success() {
        println!("   🗑️  Turing GPU: Nuking mainline kernel to protect sleep states...");
        // Remove mainline and skip dependency checks
        let _ = Command::new("sudo")
            .args(["pacman", "-Rdd", "--noconfirm", "linux", "linux-headers"])
            .status();

        println!("   🏗️  Rebuilding GRUB for LTS Kernel...");
        let _ = Command::new("sudo")
            .args(["grub-mkconfig", "-o", "/boot/grub/grub.cfg"])
            .status();
    }
}
/// Reliably finds the root of the dotfiles repository regardless of where the binary is executed.
fn get_repo_root() -> PathBuf {
    // This macro captures the absolute path of the 'install-wizard' folder AT COMPILE TIME.
    // e.g., "/home/michael/path/to/rust-wayland-power/sysScripts/install-wizard"
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    // Navigate up two levels: install-wizard -> sysScripts -> repo_root
    Path::new(manifest_dir)
        .parent()
        .expect("Could not find sysScripts parent")
        .parent()
        .expect("Could not find repo root parent")
        .to_path_buf()
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
fn setup_battery_daemon() {
    println!("   🔋 Configuring Battery Safety Daemon...");

    let home = std::env::var("HOME").expect("HOME environment variable not set");
    let systemd_user_dir = std::path::Path::new(&home).join(".config/systemd/user");
    let service_dest = systemd_user_dir.join("battery-daemon.service");

    if service_dest.exists() {
        println!("   ✅ Battery daemon already configured. Skipping systemd setup.");
        return;
    }
    println!("   🔋 Setting up Battery Safety Daemon for the first time...");

    // Make sure the ~/.config/systemd/user/ folder actually exists
    let _ = std::fs::create_dir_all(&systemd_user_dir);

    let service_content = include_str!("../../battery-daemon/battery-daemon.service");

    if let Err(e) = std::fs::write(&service_dest, service_content) {
        eprintln!("   ⚠️ Failed to write battery-daemon.service: {}", e);
        return;
    }

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "battery-daemon.service"])
        .status();

    println!("   ✅ Battery Daemon ready.");
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
