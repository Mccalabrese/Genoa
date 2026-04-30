use std::path::Path;

use crate::CmdExecutor;

const TURING_IDS: &[&str] = &[
    "0x1e02", "0x1e04", "0x1e07", "0x1e30", // Titan RTX, 2080 Ti, Quadro...
    "0x1f02", "0x1f06", "0x1f08", "0x1f82", // 2070, 2060, 1650 (TU106)...
    "0x2182", "0x2184", "0x2187", "0x2188", // 1660 Ti, 1660, 1650 Super, 1650...
    "0x2191", "0x21d1", // GTX 1650 Mobile variants..."0x1e02", "0x1e04", "0x1e07", "0x1e30",
];

// --- Enums for Hardware Detection ---
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum NvidiaArch {
    Modern,
    Turing,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GpuVendor {
    Unknown,
    Intel,
    Amd,
    Nvidia(NvidiaArch),
}

/// Parses `lspci` output to identify GPU vendor IDs.
/// 10de = NVIDIA, 1002 = AMD, 8086 = Intel.
pub fn detect_gpu(sys: &impl CmdExecutor) -> GpuVendor {
    let Ok(entries) = sys.list_dir_file_names(Path::new("/sys/bus/pci/devices")) else {
        eprintln!("⚠️ Failed to read PCI devices. Defaulting to Unknown");
        return GpuVendor::Unknown;
    };
    let mut gpus = Vec::new();
    let base_dir = Path::new("/sys/bus/pci/devices");
    for entry in entries.into_iter() {
        let path = base_dir.join(&entry);
        let Ok(class_hex) = sys.read_file_to_string(&path.join("class")) else {
            continue;
        };
        let Ok(vendor_hex) = sys.read_file_to_string(&path.join("vendor")) else {
            continue;
        };
        let Ok(device_hex) = sys.read_file_to_string(&path.join("device")) else {
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
pub fn find_igpu(sys: &impl CmdExecutor) -> Option<(String, String)> {
    let Ok(mut entries) = sys.list_dir_file_names(Path::new("/sys/class/drm")) else {
        eprintln!("⚠️ Failed to read /sys/class/drm. Cannot detect iGPU.");
        return None;
    };
    entries.sort();
    let base_dir = Path::new("/sys/class/drm");
    let mut amd_card: Option<String> = None;
    for entry in entries.into_iter() {
        let path = base_dir.join(&entry);
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.starts_with("card") || name.contains("-") {
            continue;
        } // We only care about card* entries and want to ignore cables
        let vendor_path = path.join("device/vendor");
        let Ok(vendor_hex) = sys.read_file_to_string(&vendor_path) else {
            continue;
        };
        let card_path = format!("/dev/dri/{}", name);
        match vendor_hex.trim() {
            "0x8086" => return Some((card_path, "intel".to_string())),
            "0x1002" => {
                if amd_card.is_none() {
                    amd_card = Some(card_path);
                }
            }
            _ => continue,
        }
    }
    amd_card.map(|card| (card, "amd".to_string()))
}

/// 1. Check if user is on old drivers and ignoring updates in their pacman conf.
/// 2. If they are installingg from scratch, just install AUR nvidia-580-dkms which supports Turing and older cards on newer kernels.
/// 3. For users on old drivers, halt&warn, execute removing ignore line from pacman conf, pacman
///    -Rdd old drivers, install mainline kernel, install AUR drivers, run mkinicpio and
///    grub-mkconfig if user is on grub, and force reboot to load the new drivers safely.
pub fn setup_turing_gpu(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    let pacman_conf = Path::new("/etc/pacman.conf");
    let pac_conf_content = sys.read_file_to_string(pacman_conf)?;
    let drivers_installed = sys.run_cmd("pacman", &["-Q", "nvidia-580xx-dkms"]).is_ok();
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
            continue;
        }
        if trimmed.to_lowercase() == "#[multilib]" {
            *line = "[multilib]".to_string();
            inside_multilib = true;
        } else if inside_multilib
            && trimmed.starts_with("#Include")
            && trimmed.contains("mirrorlist")
        {
            *line = "Include = /etc/pacman.d/mirrorlist".to_string();
            inside_multilib = false;
        }
    }
    let modified = sys.install_string_to_root_file(pacman_conf, &lines.join("\n"), "644")?;
    if modified {
        sys.run_cmd("sudo", &["pacman", "-Sy"])?;
    }
    if is_legacy_nvidia || !drivers_installed {
        let _ = sys.run_cmd_ignore_err(
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
        let _ = sys.run_cmd_ignore_err(
            "sudo",
            &["pacman", "-Rdd", "--noconfirm", "lib32-nvidia-utils"],
        ); // Remove 32-bit drivers if present
        let _ = sys.run_cmd_ignore_err("sudo", &["pacman", "-Rdd", "--noconfirm", "libxnvctrl"]);
        sys.run_cmd(
            "sudo",
            &["pacman", "-S", "--noconfirm", "linux", "linux-headers"],
        )?; // Ensure mainline kernel is installed
    }
    if is_legacy_nvidia || !drivers_installed {
        println!("   👉 Installing legacy NVIDIA drivers from AUR...");
        sys.run_cmd(
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
        let _ = sys.run_cmd_ignore_err("yay", &["-S", "--noconfirm", "lib32-nvidia-580xx-utils"]); // Install 32-bit
    }
    if is_legacy_nvidia || !drivers_installed {
        sys.run_cmd("sudo", &["mkinitcpio", "-P"])?; // Regenerate initramfs
        if sys.path_exists(Path::new("/boot/grub/grub.cfg")) {
            let _ = sys.run_cmd_ignore_err("sudo", &["grub-mkconfig", "-o", "/boot/grub/grub.cfg"]); // Regenerate GRUB config if GRUB is present
        }
        let _ = sys.run_cmd_ignore_err("sudo", &["reboot"]); // Reboot to load new drivers safely
        std::process::exit(0); // In case reboot command fails, we still want to exit to prevent further issues
    }
    Ok(())
}

/// Generates the sway-hybrid wrapper script with DYNAMIC paths.
pub fn create_sway_hybrid_script(sys: &impl CmdExecutor) -> Result<bool, std::io::Error> {
    println!("   🔧 Generating dynamic Sway-Hybrid wrapper...");
    // 1. Find the iGPU
    let script_content = match find_igpu(sys) {
        Some((card_path, vendor)) => {
            println!("      👉 iGPU Found: {} ({})", card_path, vendor);
            let vulkan_driver = if vendor == "amd" {
                "radeon_icd.x86_64.json"
            } else {
                "intel_icd.x86_64.json"
            };
            format!(
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
            )
        }
        None => {
            println!("   ⚠️  Could not detect iGPU. Leaving kernel defaults.");
            r#"#!/bin/sh
# --- Auto-Generated by Rust Installer ---
# iGPU not detected; do not force a device. Let the kernel choose.

# Launch Sway
exec sway
"#
            .to_string()
        }
    };
    //Idempotency Check: If the file already exists with the same content, skip writing
    let wrapper_path = Path::new("/usr/local/bin/sway-hybrid");
    let modified = sys.install_string_to_root_file(wrapper_path, &script_content, "755")?;
    Ok(modified)
}

/// Applies specific fixes for NVIDIA on Wayland.
/// 1. Sets kernel parameters (`nvidia_drm.modeset=1`).
/// 2. Creates modprobe rules to fix suspend/resume.
/// 3. Rebuilds initramfs via `mkinitcpio`.
///
/// Security Note: Uses a secure temp file pattern for writing to /etc/.
/// NOW SMART: Differentiates between Turing (Legacy) and Modern (Ampere/Ada) cards.
pub fn apply_nvidia_configs(
    arch: &NvidiaArch,
    sys: &impl CmdExecutor,
) -> Result<(), std::io::Error> {
    println!("    Applying Nvidia Configs...");
    let is_turing = *arch == NvidiaArch::Turing;
    let mut requires_rebuild = false;
    if is_turing {
        println!("    ℹ️  Configuring for Turing Architecture (GTX 16xx / RTX 20xx)...");
    } else {
        println!("    ℹ️  Configuring for Modern NVIDIA Architecture...");
    }
    // --- 1. MODPROBE CONFIGURATION ---
    // Turing (GTX 16xx/20xx): Needs Firmware=0 to prevent hanging on suspend with legacy drivers.
    // Modern (RTX 30xx/40xx): Needs Firmware=1 (Default/GSP) for proper power management.
    let firmware_val = if is_turing { "0" } else { "1" };
    let modprobe_content = format!(
        "options nvidia NVreg_EnableGpuFirmware={} NVreg_DynamicPowerManagement=0x02 NVreg_EnableS0ixPowerManagement=1\noptions nvidia_drm modeset=1 fbdev=1\n",
        firmware_val
    );
    requires_rebuild |= sys.install_string_to_root_file(
        Path::new("/etc/modprobe.d/nvidia.conf"),
        &modprobe_content,
        "644",
    )?;
    requires_rebuild |= sys.install_string_to_root_file(
        Path::new("/etc/modprobe.d/99-nvidia-uvm-blacklist.conf"),
        "blacklist nvidia_uvm\n",
        "644",
    )?;
    // --- 2. UDEV RULES (Common) ---
    // Keeps the dGPU 'auto' suspended when not in use.
    requires_rebuild |= sys.install_string_to_root_file(
        Path::new("/etc/udev/rules.d/90-nvidia-pm.rules"),
        "SUBSYSTEM==\"pci\", ATTR{vendor}==\"0x10de\", ATTR{power/control}=\"auto\"\n",
        "644",
    )?;
    // --- 4. MKINITCPIO CONFIGURATION ---
    // Newer cards often need early KMS loading for external display hotplug wakeup.
    // We only enforce this for non-turing, though it doesn't hurt turing.
    if !is_turing {
        requires_rebuild |= ensure_nvidia_modules_in_initcpio(sys)?;
    }
    create_sway_hybrid_script(sys)?;
    println!("    🏗️  Rebuilding Initramfs...");
    if requires_rebuild {
        sys.run_cmd("sudo", &["mkinitcpio", "-P"])?;
    } else {
        println!("    ✅ No changes to initramfs configuration. Skipping rebuild.");
    }
    Ok(())
}

/// Helper: Safely adds nvidia modules to mkinitcpio.conf if missing.
/// Handles the request: "-added nvidia to modules in mkinitcpio"
pub fn ensure_nvidia_modules_in_initcpio(sys: &impl CmdExecutor) -> Result<bool, std::io::Error> {
    println!("    🔧 Checking mkinitcpio modules for Modern NVIDIA support...");
    let config_path = Path::new("/etc/mkinitcpio.conf");
    let content = sys.read_file_to_string(config_path)?;

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
    let modified = sys.install_string_to_root_file(config_path, &new_content, "644")?;
    Ok(modified)
}

//-------- Unit Tests -------------------
//---------------------------------------
//

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_env::MockEnv;

    fn seed_pci_device(
        env: &MockEnv,
        bdf: &str,
        class_hex: &str,
        vendor_hex: &str,
        device_hex: &str,
    ) {
        env.mock_files.borrow_mut().insert(
            format!("/sys/bus/pci/devices/{}/class", bdf),
            class_hex.to_string(),
        );
        env.mock_files.borrow_mut().insert(
            format!("/sys/bus/pci/devices/{}/vendor", bdf),
            vendor_hex.to_string(),
        );
        env.mock_files.borrow_mut().insert(
            format!("/sys/bus/pci/devices/{}/device", bdf),
            device_hex.to_string(),
        );
    }

    #[test]
    fn test_detect_gpu() {
        let env = MockEnv::default();
        seed_pci_device(&env, "0000:00:02.0", "0x030000", "0x8086", "0x1234"); // Intel iGPU
        let result = detect_gpu(&env);
        assert_eq!(result, GpuVendor::Intel);
        seed_pci_device(&env, "0000:01:00.0", "0x030000", "0x10de", "0x2204"); // NVIDIA dGPU
        let result = detect_gpu(&env);
        assert_eq!(result, GpuVendor::Nvidia(NvidiaArch::Modern));
        seed_pci_device(&env, "0000:01:00.0", "0x030000", "0x10de", "0x1e02"); // Turing dGPU
        let result = detect_gpu(&env);
        assert_eq!(result, GpuVendor::Nvidia(NvidiaArch::Turing));
    }

    #[test]
    fn test_detect_gpu_matrix() {
        let cases = vec![
            // class, vendor, device, expected
            ("0x030000", "0x8086", "0x1234", GpuVendor::Intel),
            ("0x030000", "0x1002", "0x73bf", GpuVendor::Amd),
            (
                "0x030000",
                "0x10de",
                "0x1f06",
                GpuVendor::Nvidia(NvidiaArch::Turing),
            ),
            (
                "0x038000",
                "0x10de",
                "0x2684",
                GpuVendor::Nvidia(NvidiaArch::Modern),
            ),
            ("0x020000", "0x10de", "0x2204", GpuVendor::Unknown),
            ("0x030000", "0x1234", "0x5678", GpuVendor::Unknown),
        ];

        for (class_hex, vendor_hex, device_hex, expected) in cases {
            let env = MockEnv::default();
            seed_pci_device(&env, "0000:01:00.0", class_hex, vendor_hex, device_hex);
            assert_eq!(detect_gpu(&env), expected);
        }
    }

    #[test]
    fn test_detect_gpu_priority_prefers_nvidia() {
        let env = MockEnv::default();
        seed_pci_device(&env, "0000:00:02.0", "0x030000", "0x8086", "0x1234");
        seed_pci_device(&env, "0000:03:00.0", "0x030000", "0x1002", "0x73bf");
        seed_pci_device(&env, "0000:01:00.0", "0x030000", "0x10de", "0x2204");

        assert_eq!(detect_gpu(&env), GpuVendor::Nvidia(NvidiaArch::Modern));
    }
    #[test]
    fn test_find_igpu_intel() {
        let env = MockEnv::default();
        // Simulate an Intel iGPU at /dev/dri/card0
        env.mock_files.borrow_mut().insert(
            "/sys/class/drm/card0/device/vendor".to_string(),
            "0x8086".to_string(),
        );
        let result = find_igpu(&env);
        assert_eq!(
            result,
            Some(("/dev/dri/card0".to_string(), "intel".to_string()))
        );
    }
    #[test]
    fn test_find_igpu_amd() {
        let env = MockEnv::default();
        // Simulate an AMD iGPU at /dev/dri/card0
        env.mock_files.borrow_mut().insert(
            "/sys/class/drm/card0/device/vendor".to_string(),
            "0x1002".to_string(),
        );
        let result = find_igpu(&env);
        assert_eq!(
            result,
            Some(("/dev/dri/card0".to_string(), "amd".to_string()))
        );
    }
    #[test]
    fn test_find_igpu_none() {
        let env = MockEnv::default();
        // No valid iGPU entries
        env.mock_files.borrow_mut().insert(
            "/sys/class/drm/card0/device/vendor".to_string(),
            "0x1234".to_string(),
        );
        let result = find_igpu(&env);
        assert_eq!(result, None);
    }
}
