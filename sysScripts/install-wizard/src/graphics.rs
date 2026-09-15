use std::path::Path;

use crate::CmdExecutor;
use crate::kernel::KernelFlavor;

const TURING_IDS: &[&str] = &[
    "0x1e02", "0x1e04", "0x1e07", "0x1e30", // Titan RTX, 2080 Ti, Quadro...
    "0x1f02", "0x1f06", "0x1f08", "0x1f82", // 2070, 2060, 1650 (TU106)...
    "0x2182", "0x2184", "0x2187", "0x2188", // 1660 Ti, 1660, 1650 Super, 1650...
    "0x2191", "0x21d1", // GTX 1650 Mobile variants..."0x1e02", "0x1e04", "0x1e07", "0x1e30",
];

// Rose-Hulman's Dell Pro Max 16 fleet: RTX PRO 1000 Blackwell paired with the
// Radeon 840M / 860M iGPU. This is deliberately a pair match, rather than a
// broad "all hybrid laptops" policy: Niri's automatic renderer choice is
// already correct on many other NVIDIA laptops.
const RHIT_DELL_NVIDIA_DEVICE: &str = "0x2db8";
const RHIT_DELL_AMD_DEVICE: &str = "0x1114";

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

/// Parses PCI sysfs entries to identify GPU vendor IDs.
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
        if matches!(class_hex.trim(), "0x030000" | "0x030200" | "0x038000") {
            // VGA controller, 3D controller, or display controller.
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

/// Returns the stable iGPU render-node path for the known Dell Pro Max 16
/// hybrid graphics layout. Returning a PCI by-path symlink, rather than a
/// renderD number, keeps the setting valid when DRM node numbering changes.
///
/// This intentionally does not apply to merely any NVIDIA + AMD system. It is
/// a narrowly-scoped workaround for this fleet while Niri selects the NVIDIA
/// card as its default renderer on these machines.
pub fn rhit_dell_pro_max_niri_render_device(sys: &impl CmdExecutor) -> Option<String> {
    let entries = sys
        .list_dir_file_names(Path::new("/sys/bus/pci/devices"))
        .ok()?;
    let base_dir = Path::new("/sys/bus/pci/devices");
    let mut has_rtx_pro_1000 = false;
    let mut amd_igpu_bdf = None;

    for entry in entries {
        let path = base_dir.join(&entry);
        let vendor = sys.read_file_to_string(&path.join("vendor")).ok()?;
        let device = sys.read_file_to_string(&path.join("device")).ok()?;
        match (vendor.trim(), device.trim()) {
            ("0x10de", RHIT_DELL_NVIDIA_DEVICE) => has_rtx_pro_1000 = true,
            ("0x1002", RHIT_DELL_AMD_DEVICE) => amd_igpu_bdf = Some(entry),
            _ => {}
        }
    }

    has_rtx_pro_1000.then(|| {
        let bdf = amd_igpu_bdf?;
        Some(format!("/dev/dri/by-path/pci-{}-render", bdf))
    })?
}

/// Writes the opt-in marker consumed by the Niri session launcher. The marker
/// contains only the iGPU's stable render-node path; it does not disable or
/// otherwise restrict the NVIDIA GPU for CUDA or external displays.
pub fn configure_rhit_dell_pro_max_niri(sys: &impl CmdExecutor) -> Result<bool, std::io::Error> {
    let Some(render_device) = rhit_dell_pro_max_niri_render_device(sys) else {
        return Ok(false);
    };

    println!("    🔧 Applying Dell Pro Max 16 Niri iGPU renderer workaround...");
    sys.create_root_dir_all(Path::new("/etc/genoa"))?;
    sys.install_string_to_root_file(
        Path::new("/etc/genoa/niri-render-drm-device"),
        &(render_device + "\n"),
        "644",
    )
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
/// 2. If they are installing from scratch, install the AUR nvidia-580-dkms driver
///    against the kernel profile selected by the user.
/// 3. For users on old drivers, halt&warn, execute removing ignore line from pacman conf, pacman
///    -Rdd old drivers, install the selected kernel, install AUR drivers, run mkinitcpio and
///    grub-mkconfig if user is on grub, and force reboot to load the new drivers safely.
pub fn setup_turing_gpu(
    kernel_flavor: KernelFlavor,
    sys: &impl CmdExecutor,
) -> Result<(), std::io::Error> {
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
        let mut kernel_args = vec!["pacman", "-S", "--needed", "--noconfirm"];
        kernel_args.extend(kernel_flavor.kernel_packages());
        sys.run_cmd("sudo", &kernel_args)?; // Ensure the selected kernel and its DKMS headers are installed
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
/// 1. Sets the kernel parameter (`nvidia-drm.modeset=1`) when kernel-install
///    manages `/etc/kernel/cmdline`.
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
        requires_rebuild |= ensure_nvidia_drm_modeset_in_kernel_cmdline(sys)?;
    }
    create_sway_hybrid_script(sys)?;
    println!("    🏗️  Rebuilding Initramfs...");
    if requires_rebuild {
        sys.run_cmd("sudo", &["mkinitcpio", "-P"])?;
    }
    Ok(())
}

/// Adds the NVIDIA DRM modeset flag to kernel-install's managed command line.
///
/// Some installations use GRUB instead and do not have `/etc/kernel/cmdline`;
/// leave those boot configurations untouched. The modprobe setting above still
/// supplies modeset for them.
pub fn ensure_nvidia_drm_modeset_in_kernel_cmdline(
    sys: &impl CmdExecutor,
) -> Result<bool, std::io::Error> {
    const CMDLINE_PATH: &str = "/etc/kernel/cmdline";
    const MODESET_FLAG: &str = "nvidia-drm.modeset=1";

    let path = Path::new(CMDLINE_PATH);
    if !sys.path_exists(path) {
        println!("    ℹ️  No /etc/kernel/cmdline; leaving bootloader command line unchanged.");
        return Ok(false);
    }

    println!("    🔧 Checking kernel command line for NVIDIA DRM modeset...");
    let content = sys.read_file_to_string(path)?;
    if content
        .split_whitespace()
        .any(|argument| argument == MODESET_FLAG || argument == "nvidia_drm.modeset=1")
    {
        return Ok(false);
    }

    let updated = match content.trim() {
        "" => format!("{MODESET_FLAG}\n"),
        cmdline => format!("{cmdline} {MODESET_FLAG}\n"),
    };
    sys.install_string_to_root_file(path, &updated, "644")
}

/// Helper: Safely adds nvidia modules to mkinitcpio.conf if missing.
/// Handles the request: "-added nvidia to modules in mkinitcpio"
pub fn ensure_nvidia_modules_in_initcpio(sys: &impl CmdExecutor) -> Result<bool, std::io::Error> {
    println!("    🔧 Checking mkinitcpio modules for Modern NVIDIA support...");
    let config_path = Path::new("/etc/mkinitcpio.conf");
    let content = sys.read_file_to_string(config_path)?;

    let mut new_content = content
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
    if content.ends_with('\n') && !new_content.ends_with('\n') {
        new_content.push('\n');
    }
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
            (
                "0x030200",
                "0x10de",
                "0x2db8",
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
    fn test_rhit_dell_pro_max_niri_render_device() {
        let env = MockEnv::default();
        seed_pci_device(&env, "0000:c6:00.0", "0x030200", "0x10de", "0x2db8");
        seed_pci_device(&env, "0000:c7:00.0", "0x038000", "0x1002", "0x1114");

        assert_eq!(
            rhit_dell_pro_max_niri_render_device(&env),
            Some("/dev/dri/by-path/pci-0000:c7:00.0-render".to_string())
        );
    }

    #[test]
    fn test_rhit_dell_pro_max_niri_workaround_requires_both_gpus() {
        let env = MockEnv::default();
        seed_pci_device(&env, "0000:c7:00.0", "0x038000", "0x1002", "0x1114");

        assert_eq!(rhit_dell_pro_max_niri_render_device(&env), None);
        assert!(!configure_rhit_dell_pro_max_niri(&env).expect("configuration should succeed"));
        assert!(
            !env.mock_files
                .borrow()
                .contains_key("/etc/genoa/niri-render-drm-device")
        );
    }

    #[test]
    fn test_configure_rhit_dell_pro_max_niri_writes_stable_render_path() {
        let env = MockEnv::default();
        seed_pci_device(&env, "0000:c6:00.0", "0x030200", "0x10de", "0x2db8");
        seed_pci_device(&env, "0000:c7:00.0", "0x038000", "0x1002", "0x1114");

        assert!(configure_rhit_dell_pro_max_niri(&env).expect("configuration should succeed"));
        assert_eq!(
            env.mock_files
                .borrow()
                .get("/etc/genoa/niri-render-drm-device"),
            Some(&"/dev/dri/by-path/pci-0000:c7:00.0-render\n".to_string())
        );
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

    #[test]
    fn test_find_igpu_ignores_non_card_entries() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/sys/class/drm/card0-DP-1/device/vendor".to_string(),
            "0x8086".to_string(),
        );
        env.mock_files.borrow_mut().insert(
            "/sys/class/drm/card1/device/vendor".to_string(),
            "0x1002".to_string(),
        );
        let result = find_igpu(&env);
        assert_eq!(
            result,
            Some(("/dev/dri/card1".to_string(), "amd".to_string()))
        );
    }

    #[test]
    fn test_create_sway_hybrid_script_intel() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/sys/class/drm/card0/device/vendor".to_string(),
            "0x8086".to_string(),
        );
        let modified = create_sway_hybrid_script(&env).expect("script creation failed");
        assert!(modified);

        let binding = env.mock_files.borrow();
        let content = binding
            .get("/usr/local/bin/sway-hybrid")
            .expect("script missing");
        assert!(content.contains("WLR_DRM_DEVICES=/dev/dri/card0"));
        assert!(content.contains("intel_icd.x86_64.json"));
    }

    #[test]
    fn test_create_sway_hybrid_script_no_igpu() {
        let env = MockEnv::default();
        let modified = create_sway_hybrid_script(&env).expect("script creation failed");
        assert!(modified);

        let binding = env.mock_files.borrow();
        let content = binding
            .get("/usr/local/bin/sway-hybrid")
            .expect("script missing");
        assert!(content.contains("iGPU not detected"));
        assert!(!content.contains("WLR_DRM_DEVICES"));
    }

    #[test]
    fn test_ensure_nvidia_modules_in_initcpio_adds_missing() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/mkinitcpio.conf".to_string(),
            "MODULES=(i915)\n".to_string(),
        );
        let modified = ensure_nvidia_modules_in_initcpio(&env).expect("initcpio update failed");
        assert!(modified);
        let binding = env.mock_files.borrow();
        let updated = binding.get("/etc/mkinitcpio.conf").unwrap();
        for module in ["nvidia", "nvidia_modeset", "nvidia_uvm", "nvidia_drm"] {
            assert!(updated.contains(module));
        }
    }

    #[test]
    fn test_ensure_nvidia_modules_in_initcpio_noop() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/mkinitcpio.conf".to_string(),
            "MODULES=(nvidia nvidia_modeset nvidia_uvm nvidia_drm)\n".to_string(),
        );
        let modified = ensure_nvidia_modules_in_initcpio(&env).expect("initcpio update failed");
        assert!(!modified);
        assert!(env.cmd_log.borrow().is_empty());
    }

    #[test]
    fn test_kernel_cmdline_adds_nvidia_drm_modeset_once() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/kernel/cmdline".to_string(),
            "root=UUID=example rw quiet\n".to_string(),
        );

        assert!(ensure_nvidia_drm_modeset_in_kernel_cmdline(&env).unwrap());
        let updated = env
            .mock_files
            .borrow()
            .get("/etc/kernel/cmdline")
            .unwrap()
            .clone();
        assert_eq!(updated, "root=UUID=example rw quiet nvidia-drm.modeset=1\n");
        assert!(!ensure_nvidia_drm_modeset_in_kernel_cmdline(&env).unwrap());
    }

    #[test]
    fn test_kernel_cmdline_is_unchanged_when_not_managed() {
        let env = MockEnv::default();
        assert!(!ensure_nvidia_drm_modeset_in_kernel_cmdline(&env).unwrap());
        assert!(env.cmd_log.borrow().is_empty());
    }

    #[test]
    fn test_setup_turing_gpu_enables_multilib() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/pacman.conf".to_string(),
            "[options]\nHoldPkg = pacman\n#[multilib]\n#Include = /etc/pacman.d/mirrorlist\n"
                .to_string(),
        );
        let result = setup_turing_gpu(KernelFlavor::Mainline, &env);
        assert!(result.is_ok());
        let binding = env.mock_files.borrow();
        let updated = binding.get("/etc/pacman.conf").unwrap();
        assert!(updated.contains("[multilib]"));
        assert!(updated.contains("Include = /etc/pacman.d/mirrorlist"));

        let log = env.cmd_log.borrow();
        assert!(log.iter().any(|entry| {
            entry.0 == "sudo"
                && entry.1
                    == ["pacman", "-Sy"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>()
        }));
    }
}
