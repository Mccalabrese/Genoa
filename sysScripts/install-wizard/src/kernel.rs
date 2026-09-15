//! Kernel selection and UKI setup.
//!
//! Genoa deliberately defaults new installs to the LTS kernel. The user's
//! choice is written to `/etc/genoa/kernel-policy`, so later installer passes
//! select matching NVIDIA modules instead of silently changing kernel tracks.

use std::path::{Path, PathBuf};

use crate::traits::CmdExecutor;
use crate::update::install_pacman_packages;

const POLICY_PATH: &str = "/etc/genoa/kernel-policy";
const LTS_PRESET_PATH: &str = "/etc/mkinitcpio.d/linux-lts.preset";
const LTS_PRESET_NAME: &str = "linux-lts";
const LTS_UKI_FILE_NAME: &str = "genoa-linux-lts.efi";
const SBCTL_HOOK_PATH: &str = "/etc/pacman.d/hooks/99-genoa-sign-uki.hook";

// Keep a fully usable mainline fallback. This matters especially for DKMS
// drivers, which need headers for every kernel users might select at boot.
const LTS_KERNEL_PACKAGES: &[&str] = &["linux", "linux-headers", "linux-lts", "linux-lts-headers"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KernelFlavor {
    Lts,
    Mainline,
}

impl KernelFlavor {
    pub fn policy_value(self) -> &'static str {
        match self {
            Self::Lts => "lts",
            Self::Mainline => "mainline",
        }
    }

    pub fn kernel_packages(self) -> &'static [&'static str] {
        match self {
            Self::Lts => LTS_KERNEL_PACKAGES,
            Self::Mainline => &["linux", "linux-headers"],
        }
    }
}

/// The default is LTS. Choosing `false` is the explicit mainline opt-out.
pub fn prompt_for_kernel_flavor() -> KernelFlavor {
    let use_lts = inquire::Confirm::new("Use the recommended LTS kernel? (choose No for mainline)")
        .with_default(true)
        .prompt()
        .unwrap_or(true);

    if use_lts {
        KernelFlavor::Lts
    } else {
        KernelFlavor::Mainline
    }
}

pub fn read_kernel_flavor(sys: &impl CmdExecutor) -> Option<KernelFlavor> {
    match sys.read_file_to_string(Path::new(POLICY_PATH)).ok()?.trim() {
        "lts" => Some(KernelFlavor::Lts),
        "mainline" => Some(KernelFlavor::Mainline),
        _ => None,
    }
}

pub fn persist_kernel_flavor(
    sys: &impl CmdExecutor,
    flavor: KernelFlavor,
) -> Result<(), std::io::Error> {
    sys.create_root_dir_all(Path::new("/etc/genoa"))?;
    sys.install_string_to_root_file(
        Path::new(POLICY_PATH),
        &format!("{}\n", flavor.policy_value()),
        "644",
    )?;
    Ok(())
}

/// The modern NVIDIA packages needed for the selected kernel profile.
///
/// LTS systems retain the mainline module package as well. This keeps the
/// already-installed Arch mainline kernel usable from the boot menu as a real
/// graphical fallback, not just a text-mode recovery path.
pub fn modern_nvidia_packages(flavor: KernelFlavor) -> &'static [&'static str] {
    match flavor {
        KernelFlavor::Lts => &[
            "nvidia-open",
            "nvidia-open-lts",
            "nvidia-utils",
            "nvidia-prime",
            "nvidia-settings",
            "libva-nvidia-driver",
        ],
        KernelFlavor::Mainline => &[
            "nvidia-open",
            "nvidia-utils",
            "nvidia-prime",
            "nvidia-settings",
            "libva-nvidia-driver",
        ],
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LtsUki {
    path: PathBuf,
    entry_id: String,
}

/// Installs LTS and configures its preset to write a dedicated UKI when this
/// system exposes a boot-loader-specification $BOOT partition. GRUB systems
/// still receive LTS, but retain their own menu/default policy.
pub fn prepare_lts_uki(sys: &impl CmdExecutor) -> Result<Option<LtsUki>, std::io::Error> {
    install_pacman_packages(sys, LTS_KERNEL_PACKAGES)?;

    if !sys.command_exists("bootctl") {
        println!("   ℹ️  bootctl is unavailable; installed LTS without changing the boot default.");
        return Ok(None);
    }
    if sys.run_cmd("bootctl", &["is-installed"]).is_err() {
        println!(
            "   ℹ️  systemd-boot is not installed; installed LTS without changing the boot default."
        );
        return Ok(None);
    }

    let boot_path = match sys.command_output("bootctl", &["--print-boot-path"]) {
        Ok(output) => PathBuf::from(output.trim()),
        Err(_) => {
            println!(
                "   ℹ️  No BLS boot partition found; installed LTS without changing the boot default."
            );
            return Ok(None);
        }
    };
    if !boot_path.is_absolute() || boot_path.as_os_str().is_empty() {
        return Err(std::io::Error::other(
            "bootctl returned an invalid boot partition path",
        ));
    }

    let uki_path = boot_path.join("EFI/Linux").join(LTS_UKI_FILE_NAME);
    let preset_path = Path::new(LTS_PRESET_PATH);
    let preset = sys.read_file_to_string(preset_path)?;
    let updated_preset = set_default_uki(&preset, &uki_path)?;
    sys.create_root_dir_all(
        uki_path
            .parent()
            .ok_or_else(|| std::io::Error::other("LTS UKI has no parent directory"))?,
    )?;
    sys.install_string_to_root_file(preset_path, &updated_preset, "644")?;

    Ok(Some(LtsUki {
        path: uki_path,
        entry_id: LTS_UKI_FILE_NAME.to_string(),
    }))
}

/// Builds, signs when necessary, validates, and then selects the LTS UKI.
pub fn finalize_lts_uki(sys: &impl CmdExecutor, uki: &LtsUki) -> Result<(), std::io::Error> {
    println!("   🏗️  Building the LTS Unified Kernel Image...");
    sys.run_cmd("sudo", &["mkinitcpio", "-p", LTS_PRESET_NAME])?;

    if secure_boot_enabled(sys)? {
        if !sys.command_exists("sbctl") {
            return Err(std::io::Error::other(
                "Secure Boot is enabled but sbctl is unavailable; refusing to select an unsigned LTS UKI",
            ));
        }
        let uki_path = uki
            .path
            .to_str()
            .ok_or_else(|| std::io::Error::other("LTS UKI path is not valid UTF-8"))?;
        sys.run_cmd("sudo", &["sbctl", "sign", "--save", uki_path])?;
        sys.run_cmd("sudo", &["sbctl", "verify"])?;
        install_sbctl_update_hook(sys)?;
    }

    let entries = sys.command_output("bootctl", &["list"])?;
    if !entries.contains(&uki.entry_id) {
        return Err(std::io::Error::other(format!(
            "LTS UKI was built but bootctl did not discover entry '{}'",
            uki.entry_id
        )));
    }
    sys.run_cmd("sudo", &["bootctl", "set-default", &uki.entry_id])?;
    println!(
        "   ✅ LTS is now the default boot entry; mainline remains available in the boot menu."
    );
    Ok(())
}

/// Re-sign a rebuilt LTS UKI during later installer passes. The default entry
/// was already validated on the first pass, so this intentionally does not
/// alter boot selection again.
pub fn resign_lts_uki_if_needed(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    let Some(uki) = current_lts_uki(sys)? else {
        return Ok(());
    };
    if secure_boot_enabled(sys)? {
        if !sys.command_exists("sbctl") {
            return Err(std::io::Error::other(
                "Secure Boot is enabled but sbctl is unavailable to re-sign the LTS UKI",
            ));
        }
        let uki_path = uki
            .path
            .to_str()
            .ok_or_else(|| std::io::Error::other("LTS UKI path is not valid UTF-8"))?;
        sys.run_cmd("sudo", &["sbctl", "sign", "--save", uki_path])?;
        sys.run_cmd("sudo", &["sbctl", "verify"])?;
        install_sbctl_update_hook(sys)?;
    }
    Ok(())
}

fn current_lts_uki(sys: &impl CmdExecutor) -> Result<Option<LtsUki>, std::io::Error> {
    if !sys.command_exists("bootctl") {
        return Ok(None);
    }
    let Ok(boot_path) = sys.command_output("bootctl", &["--print-boot-path"]) else {
        return Ok(None);
    };
    let path = PathBuf::from(boot_path.trim())
        .join("EFI/Linux")
        .join(LTS_UKI_FILE_NAME);
    if !sys.path_exists(&path) {
        return Ok(None);
    }
    Ok(Some(LtsUki {
        path,
        entry_id: LTS_UKI_FILE_NAME.to_string(),
    }))
}

fn secure_boot_enabled(sys: &impl CmdExecutor) -> Result<bool, std::io::Error> {
    if !sys.command_exists("bootctl") {
        return Ok(false);
    }
    let status = sys.command_output("bootctl", &["status"])?;
    Ok(status.lines().any(|line| {
        let normalized = line.trim().to_ascii_lowercase();
        normalized.starts_with("secure boot:") && normalized.contains("enabled")
    }))
}

fn install_sbctl_update_hook(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    const HOOK: &str = "[Trigger]\nOperation = Install\nOperation = Upgrade\nType = Package\nTarget = linux\nTarget = linux-lts\nTarget = mkinitcpio\nTarget = nvidia-open\nTarget = nvidia-open-lts\nTarget = nvidia-580xx-dkms\n\n[Action]\nDescription = Re-signing Genoa Unified Kernel Images for Secure Boot...\nWhen = PostTransaction\nExec = /usr/bin/sbctl sign-all\n";
    sys.create_root_dir_all(Path::new("/etc/pacman.d/hooks"))?;
    sys.install_string_to_root_file(Path::new(SBCTL_HOOK_PATH), HOOK, "644")?;
    Ok(())
}

fn set_default_uki(preset: &str, uki_path: &Path) -> Result<String, std::io::Error> {
    let uki_path = uki_path
        .to_str()
        .ok_or_else(|| std::io::Error::other("LTS UKI path is not valid UTF-8"))?;
    let mut found = false;
    let mut lines = Vec::new();
    for line in preset.lines() {
        if line.trim_start().starts_with("default_uki=") {
            if found {
                continue;
            }
            lines.push(format!("default_uki=\"{uki_path}\""));
            found = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !found {
        if !lines.is_empty() && !lines.last().is_some_and(|line| line.is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("default_uki=\"{uki_path}\""));
    }
    Ok(lines.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_env::MockEnv;

    fn mock_output(env: &MockEnv, command: &str, args: &[&str], output: &str) {
        env.command_outputs.borrow_mut().insert(
            (
                command.to_string(),
                args.iter().map(|arg| arg.to_string()).collect(),
            ),
            output.to_string(),
        );
    }

    #[test]
    fn lts_profile_keeps_modules_for_the_mainline_fallback() {
        assert!(modern_nvidia_packages(KernelFlavor::Lts).contains(&"nvidia-open"));
        assert!(modern_nvidia_packages(KernelFlavor::Lts).contains(&"nvidia-open-lts"));
    }

    #[test]
    fn preset_uses_one_discovered_uki_path() {
        let preset = "ALL_kver=\"/boot/vmlinuz-linux-lts\"\ndefault_uki=\"/efi/EFI/Linux/old.efi\"\ndefault_uki=\"/efi/EFI/Linux/duplicate.efi\"\n";
        let updated =
            set_default_uki(preset, Path::new("/boot/EFI/Linux/genoa-linux-lts.efi")).unwrap();
        assert_eq!(updated.matches("default_uki=").count(), 1);
        assert!(updated.contains("/boot/EFI/Linux/genoa-linux-lts.efi"));
    }

    #[test]
    fn reads_only_known_kernel_policy_values() {
        let env = MockEnv::default();
        env.mock_files
            .borrow_mut()
            .insert(POLICY_PATH.into(), "lts\n".into());
        assert_eq!(read_kernel_flavor(&env), Some(KernelFlavor::Lts));
        env.mock_files
            .borrow_mut()
            .insert(POLICY_PATH.into(), "invalid\n".into());
        assert_eq!(read_kernel_flavor(&env), None);
    }

    #[test]
    fn prepares_lts_uki_at_the_discovered_boot_path() {
        let mut env = MockEnv::default();
        env.available_commands.insert("bootctl".to_string());
        env.mock_files.borrow_mut().insert(
            LTS_PRESET_PATH.to_string(),
            "ALL_kver=\"/boot/vmlinuz-linux-lts\"\ndefault_image=\"/boot/initramfs-linux-lts.img\"\n"
                .to_string(),
        );
        mock_output(&env, "bootctl", &["--print-boot-path"], "/boot\n");

        let uki = prepare_lts_uki(&env).unwrap().expect("BLS UKI expected");

        assert_eq!(
            uki.path,
            PathBuf::from("/boot/EFI/Linux/genoa-linux-lts.efi")
        );
        assert!(
            env.mock_files.borrow().get(LTS_PRESET_PATH).is_some_and(
                |preset| preset.contains("default_uki=\"/boot/EFI/Linux/genoa-linux-lts.efi\"")
            )
        );
        assert!(env.cmd_log.borrow().iter().any(|entry| {
            entry.0 == "sudo"
                && entry.1
                    == [
                        "pacman",
                        "-S",
                        "--needed",
                        "--noconfirm",
                        "linux",
                        "linux-headers",
                        "linux-lts",
                        "linux-lts-headers",
                    ]
                    .iter()
                    .map(|arg| arg.to_string())
                    .collect::<Vec<_>>()
        }));
    }

    #[test]
    fn selects_lts_only_after_bootctl_discovers_it() {
        let mut env = MockEnv::default();
        env.available_commands.insert("bootctl".to_string());
        mock_output(&env, "bootctl", &["status"], "Secure Boot: disabled\n");
        mock_output(
            &env,
            "bootctl",
            &["list"],
            "type: uki\nid: genoa-linux-lts.efi\n",
        );
        let uki = LtsUki {
            path: PathBuf::from("/boot/EFI/Linux/genoa-linux-lts.efi"),
            entry_id: "genoa-linux-lts.efi".to_string(),
        };

        finalize_lts_uki(&env, &uki).unwrap();

        assert!(env.cmd_log.borrow().iter().any(|entry| {
            entry.0 == "sudo"
                && entry.1
                    == ["bootctl", "set-default", "genoa-linux-lts.efi"]
                        .iter()
                        .map(|arg| arg.to_string())
                        .collect::<Vec<_>>()
        }));
    }

    #[test]
    fn secure_boot_saves_the_uki_and_installs_a_resigning_hook() {
        let mut env = MockEnv::default();
        env.available_commands.insert("bootctl".to_string());
        env.available_commands.insert("sbctl".to_string());
        mock_output(&env, "bootctl", &["status"], "Secure Boot: enabled\n");
        mock_output(&env, "bootctl", &["list"], "id: genoa-linux-lts.efi\n");
        let uki = LtsUki {
            path: PathBuf::from("/boot/EFI/Linux/genoa-linux-lts.efi"),
            entry_id: "genoa-linux-lts.efi".to_string(),
        };

        finalize_lts_uki(&env, &uki).unwrap();

        assert!(env.cmd_log.borrow().iter().any(|entry| {
            entry.0 == "sudo"
                && entry.1
                    == [
                        "sbctl",
                        "sign",
                        "--save",
                        "/boot/EFI/Linux/genoa-linux-lts.efi",
                    ]
                    .iter()
                    .map(|arg| arg.to_string())
                    .collect::<Vec<_>>()
        }));
        assert!(
            env.mock_files
                .borrow()
                .get(SBCTL_HOOK_PATH)
                .is_some_and(|hook| hook.contains("Target = nvidia-open-lts"))
        );
    }
}
