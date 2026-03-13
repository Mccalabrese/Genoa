//! Arch Linux Production Installer
//!
//! A comprehensive system provisioning tool written in Rust.
//! Designed to take a fresh Arch Linux installation (base + git) and transform it 
//! into a fully configured, multi-session Wayland environment (Niri, Hyprland, Sway).
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
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::io::Write;

// --- Enums for Hardware Detection ---
#[derive(Debug, PartialEq)]
enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Unknown,
}

// --- Packages ---
// Const for auditing and immutability

const RUST_APPS: &[&str] = &[
    // My custom toolchain
    "waybar-switcher", "waybar-weather", "sway-workspace", "update-check",
    "cloudflare-toggle", "wallpaper-manager", "kb-launcher", "updater",
    "sidebar", "rfkill-manager", "clip-manager", "emoji-picker",
    "radio-menu", "waybar-finance", "cal-tui", "battery-daemon",
];

// Hardware Specific: NVIDIA
const NVIDIA_PACKAGES: &[&str] = &[
    "nvidia-dkms", "nvidia-prime", "nvidia-settings", "libva-nvidia-driver",
];

// Hardware Specific: AMD
const AMD_PACKAGES: &[&str] = &[
    "vulkan-radeon", "libva-mesa-driver", "xf86-video-amdgpu",
];

// AUR
const AUR_PACKAGES: &[&str] = &[
    "zoom", "slack-desktop", "ledger-live-bin", 
    "visual-studio-code-bin", "pinta", "ttf-victor-mono", "pear-desktop-bin", 
    "librewolf-bin",
];
// ---------- Main Execution ------_-------

// ---------- Main Execution -----------------
// ---------- Main Execution -----------------
fn main() {
    // 🚨 PREVENT FATAL ROOT EXECUTION 🚨
    // If run with sudo, home_dir() points to /root, which breaks dotfiles and cargo paths.
    if std::env::var("USER").unwrap_or_default() == "root" || std::env::var("SUDO_USER").is_ok() {
        eprintln!("{}", "❌ CRITICAL ERROR: Do not run this script as root or with sudo!".red().bold());
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
        if !status.success() { eprintln!("{}", "❌ Sudo required.".red()); std::process::exit(1); }
    } else {
        // ==========================================
        //  FULL INSTALL MODE (Fresh Install Only)
        // ==========================================
        println!("{}", "🚀 Starting Rust Wayland Power Installation...".green().bold());
        
        let status = Command::new("sudo").arg("-v").status().expect("Failed to sudo");
        if !status.success() { std::process::exit(1); }

        println!("\n{}", "⚔️  Resolving Audio Conflicts (Removing jack2)...".yellow());
        let _ = Command::new("sudo").args(["pacman", "-Rdd", "--noconfirm", "jack2"])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();

        // GPU Drivers Checkpoint & Exit Logic
        let state_file = dirs::home_dir().unwrap().join(".cache/rust_installer_drivers_done");

        if state_file.exists() {
            println!("\n{}", "✅ Drivers already installed (Checkpoint found). Skipping to prevent crash.".green());
        } else {
            println!("\n{}", "🔍 Detecting GPU Hardware & Installing Base Drivers...".blue().bold());
            let gpu = detect_gpu();
            match gpu {
                GpuVendor::Nvidia => {
                    println!("   👉 NVIDIA Detected.");
                    if is_turing_gpu() { install_nvidia_legacy_580(); } 
                    else { install_pacman_packages(NVIDIA_PACKAGES); }
                },
                GpuVendor::Amd => {
                    println!("   👉 AMD Detected.");
                    install_pacman_packages(AMD_PACKAGES);
                },
                GpuVendor::Intel => println!("   👉 Intel Detected (Drivers in common)."),
                GpuVendor::Unknown => println!("   ⚠️  No dedicated GPU detected."),
            }

            let is_gui = std::env::var("WAYLAND_DISPLAY").is_ok() || std::env::var("DISPLAY").is_ok();
            
            if is_gui {
                println!("\n{}", "⚠️  GRAPHICS DRIVERS INSTALLED".yellow().bold());
                println!("We must reboot to load the new kernel modules safely.");
                
                if let Ok(mut file) = fs::File::create(&state_file) {
                    writeln!(file, "Drivers installed successfully.").unwrap();
                }
                
                println!("{}", "✅ Checkpoint saved. Please REBOOT and RUN THIS SCRIPT AGAIN.".green().bold());
                let should_reboot = inquire::Confirm::new("Reboot now?").with_default(true).prompt().unwrap_or(true);
                if should_reboot { let _ = Command::new("sudo").arg("reboot").status(); }
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
    let mut common_pkgs = load_packages_from_file("pkglist.txt");

    let ignored_pkgs = get_ignored_packages();
    common_pkgs.retain(|pkg| !ignored_pkgs.contains(pkg));

    if common_pkgs.is_empty() {
         println!("   ⚠️  No packages found in pkglist.txt.");
    } else {
         let pkg_refs: Vec<&str> = common_pkgs.iter().map(|s| s.as_str()).collect();
         install_pacman_packages(&pkg_refs);
    }

    if !AUR_PACKAGES.is_empty() {
        println!("\n{}", "📦 Syncing AUR Packages...".blue().bold());
        install_aur_packages();
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

    println!("\n{}", "⚙️  Applying System Configurations...".blue().bold());
    optimize_pacman_config(); 

    // 3. Hardware Enforcement
    let current_gpu = detect_gpu();
    let is_nvidia = current_gpu == GpuVendor::Nvidia;
    
    if is_nvidia {
        if is_turing_gpu() {
            enforce_turing_kernel(); // Nuke mainline, install LTS
        }
        apply_nvidia_configs(); 
    }

    enforce_session_order(is_nvidia);

    // 4. Finalize
    if !refresh_mode {
        // --- FRESH INSTALL ONLY ---
        println!("\n{}", "🔗 Linking Config Files...".blue().bold());
        link_dotfiles_and_copy_resources();

        configure_system();
        setup_librewolf();
        setup_waybar_configs();
        setup_secrets_and_geoclue();
        finalize_setup(); // Neovim/Tmux plugins
        setup_battery_daemon();
        print_logo();
        println!("\n{}", "✅ Installation Complete! Please Reboot.".green().bold());
    } else {
        // --- UPDATE MODE ---
        print_logo();
        println!("\n{}", "✅ System Synced & Configs Refreshed Successfully.".green().bold());
    }
}
// --- Helper functions ---

/// Reads a package list from a text file (one package per line).
/// Ignores empty lines and comments starting with '#'.
fn load_packages_from_file(filename: &str) -> Vec<String> {
    let repo_root = get_repo_root();
    let path = repo_root.join(filename);

    if path.exists() {
        let content = fs::read_to_string(&path).unwrap_or_default();
        return content
            .lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| line.to_string())
            .collect();
    }
    
    eprintln!("⚠️ Could not find package list at: {}", path.display());
    vec![]
}
/// Parses `lspci` output to identify GPU vendor IDs.
/// 10de = NVIDIA, 1002 = AMD, 8086 = Intel.
fn detect_gpu() -> GpuVendor {
    let output = Command::new("lspci")
        .arg("-n") //Numeric ID's, string parsing maybe isn't reliable according to the Goog.
        .output();

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout).to_lowercase();
            if stdout.contains("10de:") { return GpuVendor::Nvidia; }
            if stdout.contains("1002:") { return GpuVendor::Amd; }
            if stdout.contains("8086:") { return GpuVendor::Intel; }
            GpuVendor::Unknown
        },
        Err(_) => {
            println!("   ⚠️  lspci failed. Skipping auto-detection.");
            GpuVendor::Unknown
        }
    }
}

/// Scans /sys/class/drm to find the integrated GPU (Intel or AMD).
/// Returns a tuple: (Card Path, Vendor Type "intel"|"amd")
fn find_igpu() -> Option<(String, String)> {
    let drm_dir = Path::new("/sys/class/drm");
    
    if let Ok(entries) = fs::read_dir(drm_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path.file_name().unwrap().to_str().unwrap();

            // We only care about "card0", "card1", etc. (Not renderD128)
            if file_name.starts_with("card") && !file_name.contains("-") {
                // Read the vendor ID
                let vendor_path = path.join("device/vendor");
                if let Ok(vendor_hex) = fs::read_to_string(&vendor_path) {
                    let vendor = vendor_hex.trim(); // e.g. "0x8086"

                    // Check for Intel (0x8086)
                    if vendor == "0x8086" {
                        return Some((format!("/dev/dri/{}", file_name), "intel".to_string()));
                    }
                    // Check for AMD (0x1002)
                    if vendor == "0x1002" {
                        return Some((format!("/dev/dri/{}", file_name), "amd".to_string()));
                    }
                }
            }
        }
    }
    None
}

/// checks lspci to see if the card is Turing architecture (GTX 16xx / RTX 20xx)
/// These cards require the 580 driver to sleep correctly.
fn is_turing_gpu() -> bool {
    let output = Command::new("lspci").arg("-v").output();
    
    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            // Check for specific Turing identifiers
            // 1650, 1660, 2060, 2070, 2080 (and Super/Ti variants)
            let is_16_series = stdout.contains("GeForce GTX 16");
            let is_20_series = stdout.contains("GeForce RTX 20");
            
            if is_16_series || is_20_series {
                return true;
            }
            false
        },
        Err(_) => false,
    }
}

/// Installs the specific 580.119.02 driver from Arch Archive and locks it.
fn install_nvidia_legacy_580() {
    println!("\n{}", "🛑 Turing GPU Detected (GTX 16xx / RTX 20xx)".yellow().bold());
    println!("   The latest NVIDIA drivers (590+) break power management on this card.");
    println!("   Downgrading to version 580.119.02 for battery life safety...");

    // 1. Install specific versions via URL
    // We include lib32 variants assuming multilib is enabled (standard for gaming)
    let packages = vec![
        "https://archive.archlinux.org/packages/n/nvidia-dkms/nvidia-dkms-580.119.02-1-x86_64.pkg.tar.zst",
        "https://archive.archlinux.org/packages/n/nvidia-utils/nvidia-utils-580.119.02-1-x86_64.pkg.tar.zst",
        "https://archive.archlinux.org/packages/n/nvidia-settings/nvidia-settings-580.119.02-1-x86_64.pkg.tar.zst"
    ];

    let mut args = vec!["-U", "--noconfirm"];
    args.extend(packages);

    let status = Command::new("sudo")
        .arg("pacman")
        .args(&args)
        .status()
        .unwrap_or_else(|_| {
            eprintln!("❌ pacman failed to install legacy drivers.");
            std::process::exit(1);
        });

    if !status.success() {
        eprintln!("{}", "❌ Critical Error: Failed to install legacy NVIDIA drivers.".red());
        std::process::exit(1);
    }

    // 2. Pin the version in pacman.conf
    println!("   🔒 Pinning NVIDIA drivers in /etc/pacman.conf...");
    let pacman_conf = "/etc/pacman.conf";
    let ignore_line = "IgnorePkg = nvidia-dkms nvidia-utils lib32-nvidia-utils nvidia-settings linux linux-headers linux-lts linux-lts-headers";
    
    // Check if IgnorePkg is already active
    let content = fs::read_to_string(pacman_conf).unwrap_or_default();
    
    if !content.contains("nvidia-dkms") {
        // We look for the [options] header and insert IgnorePkg below it
        // Or simply uncomment the existing IgnorePkg line if standard arch config
        // Simplest robust method: Append to [options]
        
        let sed_cmd = format!("/^\\[options\\]/a {}", ignore_line);
        let _ = Command::new("sudo")
            .args(["sed", "-i", &sed_cmd, pacman_conf])
            .status();
            
        println!("   ✅ Drivers pinned. System updates will skip NVIDIA.");
    }
}

/// Generates the sway-hybrid wrapper script with DYNAMIC paths.
fn create_sway_hybrid_script() {
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
    let script_content = format!(r#"#!/bin/sh
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

    let wrapper_path = "/usr/local/bin/sway-hybrid";
    let local_tmp = "./sway-hybrid-tmp";
    
    // 4. Write to local temp file first (Safe)
    if let Err(e) = fs::write(local_tmp, script_content) {
        eprintln!("   ❌ Failed to write temp file: {}", e);
        return;
    }

    // 5. Use sudo to install it to /usr/local/bin with +x permissions
    let status = Command::new("sudo")
        .args(["install", "-m", "755", "-o", "root", "-g", "root", local_tmp, wrapper_path])
        .status();

    if status.is_ok() && status.unwrap().success() {
        println!("   ✅ Created {}", wrapper_path);
        let _ = fs::remove_file(local_tmp); // Cleanup
    } else {
        eprintln!("   ❌ Failed to install sway-hybrid script.");
    }
}
//-------- Main Steps ------
fn setup_librewolf() {
    println!("   🐺 Configuring LibreWolf for Human Beings...");
    
    let home = dirs::home_dir().unwrap();
    let wolf_dir = home.join(".librewolf");
    let override_file = wolf_dir.join("librewolf.overrides.cfg");

    // Ensure directory exists
    let _ = fs::create_dir_all(&wolf_dir);

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
    if let Err(e) = fs::write(&override_file, config_content) {
        eprintln!("   ⚠️ Failed to write LibreWolf config: {}", e);
    } else {
        println!("   ✅ LibreWolf overrides applied (WiFi & Canvas fixed).");
    }
    // Set as Default Browser (XDG)
    println!("   👉 Setting LibreWolf as default browser...");
    
    let _ = Command::new("xdg-settings")
        .args(["set", "default-web-browser", "librewolf.desktop"])
        .status();

    let _ = Command::new("xdg-mime")
        .args(["default", "librewolf.desktop", "x-scheme-handler/http"])
        .status();

    let _ = Command::new("xdg-mime")
        .args(["default", "librewolf.desktop", "x-scheme-handler/https"])
        .status();
}

/// installs packages via pacman with --needed and --noconfirm
fn install_pacman_packages(packages: &[&str]) {
    if packages.is_empty() { return; }
    let mut args = vec!["-S", "--needed", "--noconfirm"];
    args.extend(packages);
    let status = Command::new("sudo")
        .arg("pacman")
        .args(&args)
        .status()
        .unwrap_or_else(|_| {
            eprintln!("❌ pacman not found or failed to execute.");
            std::process::exit(1);
        });

    // UPDATE: Make failure fatal!
    if !status.success() { 
        eprintln!("{}", "❌ Critical Error: Pacman failed to install packages.".red());
        std::process::exit(1); 
    }
}
/// Bootstraps 'yay' from the AUR git repo if not present.
/// This allows the script to run on a truly clean Arch install.
fn install_aur_packages() {
    let yay_check = Command::new("which").arg("yay").output();
    
    if yay_check.is_err() || !yay_check.unwrap().status.success() {
        println!("   ⬇️  Bootstrapping 'yay'...");
        let home = dirs::home_dir().unwrap_or_else(|| {
             eprintln!("⚠️ Could not determine home directory. Using /tmp as fallback.");
             PathBuf::from("/tmp")
        });        
        let clone_path = home.join("yay-clone");

        if clone_path.exists() { let _ = fs::remove_dir_all(&clone_path); }

        let _ = Command::new("git").args(["clone", "https://aur.archlinux.org/yay.git", clone_path.to_str().unwrap()]).status();
        let status = Command::new("makepkg").arg("-si").arg("--noconfirm").current_dir(&clone_path).status();

        if status.is_err() || !status.unwrap().success() {
            println!("{}", "❌ Failed to bootstrap yay.".red());
            return;
        }
        let _ = fs::remove_dir_all(&clone_path);
    }

    let mut args = vec!["-S", "--needed", "--noconfirm"];
    args.extend(AUR_PACKAGES);
    let status = Command::new("yay")
        .args(&args)
        .status()
        .unwrap_or_else(|_| {
            eprintln!("❌ Failed to run yay");
            std::process::exit(1);
        });
    if !status.success() { eprintln!("{}", "⚠️  AUR Warning.".yellow()); }
}
/// Configures critical system services.
/// 1. Disables systemd-resolved (we use Cloudflared/Dnsmasq).
/// 2. Configures `greetd` (tuigreet) as the display manager.
/// 3. Sets `KillUserProcesses=yes` to prevent lingering sessions.
fn configure_system() {
    // --- 1. SANITIZE MKINITCPIO (Fix Archinstall 2025 Bug) ---
    // This protects NVIDIA users from the 'o"' corruption crash.
    println!("   🧹 Checking mkinitcpio.conf for corruption...");
    let mkinit_path = "/etc/mkinitcpio.conf";
    
    // 1. Check if the file specifically ends with the garbage (ignoring whitespace)
    // We read it first to be safe, rather than firing sed blindly.
    if let Ok(content) = fs::read_to_string(mkinit_path) {
        let trimmed = content.trim(); // Removes trailing \n
        if trimmed.ends_with("o\"") || trimmed.ends_with("o”") {
            println!("   ⚠️  Corruption detected at end of file. Cleaning up...");
            
            // 2. Safe Delete: Only delete the last line ($) if it matches the pattern
            // usage: sed -i '${/^o"$/d}' filename
            let _ = Command::new("sudo")
                .args(["sed", "-i", "${/^o\"$/d}", mkinit_path])
                .status();
                
            // Extra safety: Removing the smart-quote variation just in case
            let _ = Command::new("sudo")
                .args(["sed", "-i", "${/^o”$/d}", mkinit_path])
                .status();
        }
    }
    run_cmd("sudo", &["systemctl", "enable", "geoclue.service"]);
    run_cmd("sudo", &["systemctl", "enable", "bluetooth.service"]);
    run_cmd("sudo", &["systemctl", "enable", "bolt.service"]);

    // Prevent Pacman from eating the entire hard drive over time
    println!("   🧹 Enabling automated Pacman cache cleanup...");
    run_cmd("sudo", &["systemctl", "enable", "--now", "paccache.timer"]);

    // --- CLOUDFLARED CONFIGURATION ---
    println!("   🔧 Configuring dnscrypt-proxy (DNS Proxy)...");
    
    // 1. Ensure package is installed (failsafe)
    let _ = Command::new("sudo").args(["pacman", "-S", "--needed", "--noconfirm", "dnscrypt-proxy"]).status();

    // 2. Configure TOML to use Cloudflare
    let dns_conf = "/etc/dnscrypt-proxy/dnscrypt-proxy.toml";
    if Path::new(dns_conf).exists() {
        // Uncomment server_names = ['cloudflare']
        let _ = Command::new("sudo")
            .args(["sed", "-i", "s/^# server_names = \\['cloudflare'\\]/server_names = ['cloudflare']/", dns_conf])
            .status();
        // Ensure it listens on localhost (usually default, but good to ensure)
        let _ = Command::new("sudo")
            .args(["sed", "-i", "s/^listen_addresses = \\['127.0.0.1:53'\\]/listen_addresses = ['127.0.0.1:53', '[::1]:53']/", dns_conf])
            .status();
    }

    // 3. Enable the service
    run_cmd("sudo", &["systemctl", "enable", "--now", "dnscrypt-proxy"]);

    // 4. Clean up old Cloudflared artifacts if they exist
    let _ = Command::new("sudo").args(["systemctl", "disable", "--now", "cloudflared-dns"]).status();
    let _ = Command::new("sudo").args(["rm", "-f", "/etc/systemd/system/cloudflared-dns.service"]).status();
    let _ = Command::new("sudo").args(["systemctl", "daemon-reload"]).status();

    // --- ENVIRONMENT & LOGIND ---
    println!("    🔧 Configuring Session Environment (PATH)...");
    let env_dir = dirs::home_dir().unwrap().join(".config/environment.d");
    let env_file = env_dir.join("99-cargo-path.conf");

    if fs::create_dir_all(&env_dir).is_ok() {
        let content = "PATH=$HOME/.cargo/bin:$PATH\n";
        let _ = fs::write(&env_file, content);
    }
    
    println!("    🔧 Configuring Logind...");
    let logind_conf = "/etc/systemd/logind.conf";
    run_cmd("sudo", &["sed", "-i", "s/#KillUserProcesses=no/KillUserProcesses=yes/", logind_conf]);
    run_cmd("sudo", &["sed", "-i", "s/KillUserProcesses=no/KillUserProcesses=yes/", logind_conf]);

    println!("    🔧 Configuring Greetd...");
    let greetd_config = r#"
[terminal]
vt = 1
[default_session]
command = "tuigreet --time --remember --sessions /usr/share/wayland-sessions:/usr/share/xsessions"
user = "greeter"
"#;
    let _ = fs::write("./greetd_config.toml", greetd_config);
    run_cmd("sudo", &["mv", "./greetd_config.toml", "/etc/greetd/config.toml"]);
    let _ = Command::new("sudo").args(["systemctl", "disable", "gdm", "sddm", "lightdm"]).status();
    run_cmd("sudo", &["systemctl", "enable", "--force", "greetd.service"]);
    
    println!("    🔧 Setting Shell to Zsh...");
    let user = std::env::var("USER").unwrap_or_else(|_| "root".to_string());
    let _ = Command::new("sudo").args(["chsh", "-s", "/usr/bin/zsh", &user]).output();
    
    println!("    ✨ Setting up Tmux Plugin Manager...");
    let tpm_dir = dirs::home_dir().unwrap().join(".tmux/plugins/tpm");
    if !tpm_dir.exists() {
        let _ = Command::new("git").args(["clone", "https://github.com/tmux-plugins/tpm", tpm_dir.to_str().unwrap()]).status();
    }
}

fn run_cmd(cmd: &str, args: &[&str]) {
    let status = Command::new(cmd).args(args).status();
    match status {
        Ok(s) if s.success() => {}, // All good
        _ => {
            eprintln!("❌ Critical Error: Failed to run {} {:?}", cmd, args);
            std::process::exit(1);
        }
    }
}
/// Gleans pacman.conf to remove unwanted sessions and prevent future installs.
/// Gnome installs a lot of sessions we don't need, this keeps the list clean.
fn optimize_pacman_config() {
    println!("   🔧 Optimizing pacman.conf & Cleaning Sessions...");
    
    let sessions_to_remove = vec![
        "/usr/share/wayland-sessions/gnome.desktop",
        "/usr/share/wayland-sessions/gnome-classic.desktop",
        "/usr/share/wayland-sessions/gnome-classic-wayland.desktop",
        "/usr/share/wayland-sessions/hyprland-uwsm.desktop"
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
fn apply_nvidia_configs() {
    println!("    Applying Nvidia Configs...");

    let is_turing = is_turing_gpu();
    
    if is_turing {
        println!("    ℹ️  Configuring for Turing Architecture (GTX 16xx / RTX 20xx)...");
    } else {
        println!("    ℹ️  Configuring for Modern NVIDIA Architecture...");
    }

    // Helper closure: Write to local dir (safe) then install
    let install_securely = |content: &str, dest: &str| {
        let filename = Path::new(dest).file_name().unwrap().to_str().unwrap();
        let local_tmp = format!("./{}", filename);

        if let Err(e) = fs::write(&local_tmp, content) {
            eprintln!("❌ Failed to write local file {}: {}", local_tmp, e);
            std::process::exit(1);
        }

        // Use 'install' to copy with root:root ownership and 644 permissions
        let status = Command::new("sudo")
            .args(["install", "-m", "644", "-o", "root", "-g", "root", &local_tmp, dest])
            .status();

        match status {
            Ok(s) if s.success() => {
                 let _ = fs::remove_file(&local_tmp); // Cleanup
            },
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

    install_securely(
        &modprobe_content,
        "/etc/modprobe.d/nvidia.conf"
    );

    install_securely(
        "blacklist nvidia_uvm\n",
        "/etc/modprobe.d/99-nvidia-uvm-blacklist.conf"
    );

    // --- 2. UDEV RULES (Common) ---
    // Keeps the dGPU 'auto' suspended when not in use.
    install_securely(
        "SUBSYSTEM==\"pci\", ATTR{vendor}==\"0x10de\", ATTR{power/control}=\"auto\"\n",
        "/etc/udev/rules.d/90-nvidia-pm.rules"
    );

    // --- 3. GRUB Configuration (Common) ---
    let grub_path = "/etc/default/grub";
    println!("    🔧 Checking GRUB for NVIDIA modeset...");
    let content = fs::read_to_string(grub_path).unwrap_or_default();

    if !content.contains("nvidia_drm.modeset=1") {
        println!("    👉 Adding nvidia_drm.modeset=1 to GRUB...");
        let status = Command::new("sudo")
            .args([
                "sed", "-i",
                "s/GRUB_CMDLINE_LINUX_DEFAULT=\"[^\"]*/& nvidia_drm.modeset=1/",
                grub_path
            ])
            .status()
            .expect("Failed to patch GRUB");

        if !status.success() {
             println!("    ⚠️  Failed to patch GRUB. Please manually add nvidia_drm.modeset=1");
        }
    }

    // --- 4. MKINITCPIO CONFIGURATION ---
    // Newer cards often need early KMS loading for external display hotplug wakeup.
    // We only enforce this for non-turing, though it doesn't hurt turing.
    if !is_turing {
        ensure_nvidia_modules_in_initcpio();
    }

    create_sway_hybrid_script();

    println!("    🏗️  Rebuilding Initramfs & GRUB...");
    let _ = Command::new("sudo").args(["mkinitcpio", "-P"]).status();
    let _ = Command::new("sudo").args(["grub-mkconfig", "-o", "/boot/grub/grub.cfg"]).status();
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
                "sed", "-i",
                "s/^MODULES=(\\(.*\\))/MODULES=(\\1 nvidia nvidia_modeset nvidia_uvm nvidia_drm)/",
                config_path
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
    let finnhub_api = Text::new("Enter Finnhub.io API Key (get one by making a free account at finnhub.io/register):").prompt().unwrap_or_else(|e| { eprintln!("❌ Error: {}", e); std::process::exit(1); });
    
    // SECURE FIX: Validation logic for keys to prevent injection
    let google_geo_api = Text::new("Enter Google Geolocation API Key for Geoclue(get one at console.cloud.google.com/apis/library/geocoding-backend.googleapis.com):").prompt().unwrap_or_else(|e| { eprintln!("❌ Error: {}", e); std::process::exit(1); });
    
    // --- GEOCLUE CONFIGURATION ---
    if !google_geo_api.is_empty() {
        println!("   🌍 Configuring Geoclue...");
        let gc_path = "/etc/geoclue/geoclue.conf";

        // 1. Ensure the wifi source is enabled (uncomment 'enable=true')
        // We use a loose match to catch ';enable=true' or '#enable=true'
        let _ = Command::new("sudo").args(["sed", "-i", "s/^.*enable=true/enable=true/", gc_path]).status();

        // 2. Inject the Key
        // We look for the placeholder URL provided by the package and replace it.
        // The default line usually looks like:
        // #url=https://www.googleapis.com/geolocation/v1/geolocate?key=YOUR_KEY
        
        // We construct a regex-like sed command to find the googleapis line (commented or not) 
        // and replace the WHOLE line with our active key.
        let new_url = format!("url=https://www.googleapis.com/geolocation/v1/geolocate?key={}", google_geo_api);
        
        // This sed command finds any line containing "googleapis.com" and replaces the entire line.
        let status = Command::new("sudo")
            .args(["sed", "-i", &format!("s|^.*googleapis.com.*|{}|", new_url), gc_path])
            .status();

        match status {
             Ok(s) if s.success() => {
                 let _ = Command::new("sudo").args(["systemctl", "restart", "geoclue.service"]).output();
                 println!("   ✅ Geoclue Configured");
             },
             _ => eprintln!("   ❌ Failed to patch geoclue config."),
        }
    } else {
        println!("   ⚠️  No Google API Key provided. Location services may fail.");
    }
    let term_choice = Select::new("Preferred Terminal:", vec!["ghostty", "alacritty", "kitty"]).prompt().unwrap_or("ghostty");
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
hyprland_refresh_script = "~/.config/hypr/scripts/Refresh.sh"
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
hyprland_config = "~/.config/waybar/hyprConfig.jsonc"
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
hyprland = ["--title=KeybindCheatSheetApp"]
sway = ["--title=KeybindCheatSheetApp"]
niri = ["--title=KeybindCheatSheet"]
default = []
[[kb_launcher.sheet]]
name = "Niri"
file = "~/.config/niri/keybinds_niri.txt"

[[kb_launcher.sheet]]
name = "Sway"
file = "~/.config/sway/keybinds_sway.txt"

[[kb_launcher.sheet]]
name = "Hyprland"
file = "~/.config/hypr/keybinds_hypr.txt"

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
    fs::write(&config_path, config_content).expect("Failed to write config.toml");
    println!("   ✅ Config generated at {:?}", config_path);
}
/// Builds custom Rust apps using native caching. 
/// If source files haven't changed, this takes milliseconds.
fn build_custom_apps() {
    let repo_root = get_repo_root();
    let sys_scripts_dir = repo_root.join("sysScripts");
    
    // Ensure ~/.cargo/bin exists
    let cargo_bin_dir = dirs::home_dir().unwrap().join(".cargo/bin");
    let _ = fs::create_dir_all(&cargo_bin_dir);

    for app in RUST_APPS {
        let app_path = sys_scripts_dir.join(app);
        if app_path.exists() {
            // Use standard build to leverage local target/ cache!
            let status = Command::new("cargo")
                .args(["build", "--release"])
                .current_dir(&app_path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();

            match status {
                Ok(s) if s.success() => {
                    let compiled_bin = app_path.join("target/release").join(app);
                    let target_bin = cargo_bin_dir.join(app);
                    
                    if compiled_bin.exists() {
                        // CRITICAL: Delete the old binary first to prevent "Text file busy" 
                        // errors if the app (like your sidebar) is currently running in the background.
                        if target_bin.exists() {
                            let _ = fs::remove_file(&target_bin);
                        }
                        
                        // Copy the cached or newly compiled binary into your PATH
                        match fs::copy(&compiled_bin, &target_bin) {
                            Ok(_) => println!("     ✅ {}", app),
                            Err(e) => println!("     ❌ Failed to copy {}: {}", app, e),
                        }
                    }
                },
                _ => println!("     ❌ Failed to build {}", app),
            }
        } else {
            println!("     ⚠️  Missing directory for {}", app);
        }
    }
}
/// Renames session files to enforce a specific order in Greetd/Tuigreet.
/// Strategy: Move standard files (e.g. hyprland.desktop) to custom numbered files (30-hyprland.desktop).
/// This prevents Pacman from deleting our custom config during updates while NoExtract is active.
fn enforce_session_order(is_nvidia: bool) {
    println!("   🔧 Enforcing Session Order (Renaming .desktop files)...");
    
    let sessions_dir = "/usr/share/wayland-sessions";
    
    // Tuple: (Original Name, Safe Custom Name, Display Name)
    let updates = vec![
        ("niri.desktop", "10-niri.desktop", "1. Niri"),
        ("sway.desktop", "20-sway.desktop", "2. Sway (Battery)"),
        ("hyprland.desktop", "30-hyprland.desktop", "3. Hyprland"),
        ("gnome.desktop", "40-gnome.desktop", "4. Gnome"),
        ("gnome-wayland.desktop", "40-gnome-wayland.desktop", "4. Gnome-wayland"), // Handle Arch variation
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
                .args(["sed", "-i", "s|^Exec=.*|Exec=/usr/local/bin/sway-hybrid|", sway_session])
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
        (".tmux.conf", ".tmux.conf"), (".profile", ".profile"), (".zshrc", ".zshrc"),
        (".config/waybar", ".config/waybar"), (".config/sway", ".config/sway"),
        (".config/hypr", ".config/hypr"), (".config/niri", ".config/niri"),
        (".config/rofi", ".config/rofi"),
        (".config/ghostty", ".config/ghostty"), (".config/fastfetch", ".config/fastfetch"),
        (".config/gtk-3.0", ".config/gtk-3.0"), (".config/gtk-4.0", ".config/gtk-4.0"),
        (".config/environment.d", ".config/environment.d"), (".config/mako", ".config/mako"),
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
        println!("   ℹ️  Neovim config found. Skipping to preserve your setup. If you would like my setup just copy ~/rust-wayland-power/.config/nvim to ~/.config/nvim");
        println!("      (Note: The 'Neovim' cheat sheet in kb-launcher may not work)");
    } else {
        println!("   ✨ Installing LazyVim Config...");
        let nvim_src = repo_root.join(".config/nvim");
        create_symlink(&nvim_src, &nvim_dest);
    }
    // Link TLP
    let tlp_src = repo_root.join("tlp.conf");
    let _ = Command::new("sudo").args(["ln", "-sf", tlp_src.to_str().unwrap(), "/etc/tlp.conf"]).status();
    let _ = Command::new("sudo").args(["systemctl", "enable", "tlp.service"]).output();

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
    if let Some(parent) = dest.parent() { let _ = fs::create_dir_all(parent); }
    if dest.is_symlink() { let _ = fs::remove_file(dest); }
    #[cfg(unix)]
    std::os::unix::fs::symlink(src, dest).unwrap_or_else(|_| eprintln!("Failed to link {:?}", dest));
}
/// Runs post-install hooks to set up themes and plugins.
/// This ensures the user doesn't see "broken" visuals on first launch.
fn finalize_setup() {
    println!("\n{}", "✨ Finalizing Setup (Themes & Plugins)...".blue().bold());
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
            _ => println!("   ⚠️  Tmux plugin install failed (You can press Prefix + I inside Tmux)"),
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
        .args(["pacman", "-S", "--needed", "--noconfirm", "linux-lts", "linux-lts-headers"])
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
        .parent().expect("Could not find sysScripts parent")
        .parent().expect("Could not find repo root parent")
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
fn setup_battery_daemon() {
    println!("   🔋 Configuring Battery Safety Daemon...");
    
    let home = std::env::var("HOME").expect("HOME environment variable not set");
    let systemd_user_dir = std::path::Path::new(&home).join(".config/systemd/user");

    // Make sure the ~/.config/systemd/user/ folder actually exists
    let _ = std::fs::create_dir_all(&systemd_user_dir);

    // Grab the .service file from the repo and put it in the systemd folder
    let current_dir = std::env::current_dir().expect("Could not get current dir");
    let service_src = current_dir.join("../battery-daemon/battery-daemon.service");
    let service_dest = systemd_user_dir.join("battery-daemon.service");

    if let Err(e) = std::fs::copy(&service_src, &service_dest) {
        eprintln!("   ⚠️ Failed to copy battery-daemon.service: {}", e);
    } else {
        // Reload systemd so it sees the new file
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        
        // Enable it for future boots AND start it right now
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "enable", "--now", "battery-daemon.service"])
            .status();
            
        println!("   ✅ Battery Daemon activated.");
    }
}
fn print_logo() {
println!(r#"
                                                                                                    
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
                                               *++++* "#);
}
