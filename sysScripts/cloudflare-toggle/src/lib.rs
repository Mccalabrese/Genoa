//! Shared DNS state management for `cf-toggle` and `cf-status`.
//!
//! The toggle deliberately does not edit `/etc/resolv.conf`. That file belongs
//! to NetworkManager on this system and may be regenerated whenever a network
//! connection changes. Instead, this module adds a small NetworkManager
//! `global-dns` drop-in while dnscrypt-proxy is enabled and removes it when the
//! proxy is disabled.

use anyhow::{Context, Result, bail};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DNSCRYPT_SERVICE: &str = "dnscrypt-proxy";
const NETWORKMANAGER_CONFIG: &str = "90-dnscrypt-proxy.conf";
// Written by the first NetworkManager-based release. Keep this exact value so
// existing installations can migrate without treating an unrelated drop-in as
// ours.
const LEGACY_MANAGED_DOH_CONFIG: &str = "# Managed by cf-toggle. Do not edit; remove this file to disable the override.\n[global-dns]\nresolve-mode=exclusive\n\n[global-dns-domain-*]\nservers=127.0.0.1,::1\n";
const MANAGED_DOH_CONFIG: &str = "# Managed by cf-toggle. Do not edit.\n[global-dns]\nresolve-mode=exclusive\n\n[global-dns-domain-*]\nservers=127.0.0.1,::1\n";
const MANAGED_DIRECT_CONFIG: &str = "# Managed by cf-toggle. Do not edit.\n[global-dns]\nresolve-mode=exclusive\n\n[global-dns-domain-*]\nservers=1.1.1.1,1.0.0.1\n";

/// Whether the encrypted DNS proxy is currently running.
///
/// This is intentionally service-based because the UI indicator represents
/// dnscrypt-proxy itself. The toggle uses the richer managed/legacy state when
/// deciding which action to take.
pub fn dnscrypt_proxy_is_active() -> bool {
    SystemCommandExecutor.service_is_active(DNSCRYPT_SERVICE)
}

/// The NetworkManager-owned state for the DNS proxy toggle.
#[derive(Debug, Clone)]
pub struct DnsManager {
    config_path: PathBuf,
    resolver_path: PathBuf,
}

impl Default for DnsManager {
    fn default() -> Self {
        Self::new(PathBuf::from("/etc/NetworkManager/conf.d").join(NETWORKMANAGER_CONFIG))
    }
}

impl DnsManager {
    pub fn new(config_path: PathBuf) -> Self {
        Self::with_resolver_path(config_path, PathBuf::from("/etc/resolv.conf"))
    }

    fn with_resolver_path(config_path: PathBuf, resolver_path: PathBuf) -> Self {
        Self {
            config_path,
            resolver_path,
        }
    }

    /// Returns the active toggle state.
    ///
    /// The fallback recognizes the previous hand-written resolver state only
    /// long enough to migrate it: the first disable replaces it with the
    /// managed direct-Cloudflare DNS configuration.
    pub fn is_enabled(&self) -> bool {
        if self.has_doh_config() {
            dnscrypt_proxy_is_active()
        } else {
            legacy_loopback_resolver_present()
        }
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        let mut executor = SystemCommandExecutor;
        self.set_enabled_with(&mut executor, enabled)
    }

    fn set_enabled_with(&self, executor: &mut impl CommandExecutor, enabled: bool) -> Result<()> {
        ensure_networkmanager_running(executor)?;
        if enabled {
            self.enable(executor)
        } else {
            self.disable(executor)
        }
    }

    fn enable(&self, executor: &mut impl CommandExecutor) -> Result<()> {
        let previous = read_optional(&self.config_path)?;
        if let Some(content) = previous.as_deref()
            && !is_managed_config(content)
        {
            bail!(
                "Refusing to replace {} because it is not managed by cf-toggle",
                self.config_path.display()
            );
        }

        // Start the listener before NetworkManager begins routing DNS to it.
        executor.run("systemctl", &["enable", "--now", DNSCRYPT_SERVICE])?;

        write_managed_config(&self.config_path, MANAGED_DOH_CONFIG)?;
        if let Err(error) = reload_networkmanager_dns(executor, &self.resolver_path) {
            restore_config(&self.config_path, previous.as_deref())
                .context("Failed to restore the previous NetworkManager DNS configuration")?;
            let _ = reload_networkmanager_dns(executor, &self.resolver_path);
            return Err(error).context("NetworkManager did not accept the DNS proxy configuration");
        }
        Ok(())
    }

    fn disable(&self, executor: &mut impl CommandExecutor) -> Result<()> {
        let previous = read_optional(&self.config_path)?;
        if let Some(content) = previous.as_deref()
            && !is_managed_config(content)
        {
            bail!(
                "Refusing to replace {} because it is not managed by cf-toggle",
                self.config_path.display()
            );
        }

        // Preserve the previous off-mode behavior: use direct Cloudflare DNS
        // rather than an unreliable router-provided resolver. Apply it before
        // stopping dnscrypt so no query is left pointing at 127.0.0.1.
        write_managed_config(&self.config_path, MANAGED_DIRECT_CONFIG)?;
        if let Err(error) = reload_networkmanager_dns(executor, &self.resolver_path) {
            restore_config(&self.config_path, previous.as_deref())
                .context("Failed to restore the DNS proxy configuration after reload failure")?;
            let _ = reload_networkmanager_dns(executor, &self.resolver_path);
            return Err(error)
                .context("NetworkManager did not restore its connection DNS settings");
        }

        executor.run("systemctl", &["disable", "--now", DNSCRYPT_SERVICE])
    }

    fn has_doh_config(&self) -> bool {
        fs::read_to_string(&self.config_path).is_ok_and(|content| {
            matches!(
                content.as_str(),
                LEGACY_MANAGED_DOH_CONFIG | MANAGED_DOH_CONFIG
            )
        })
    }
}

trait CommandExecutor {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<()>;
    fn service_is_active(&self, service_name: &str) -> bool;
}

struct SystemCommandExecutor;

impl CommandExecutor for SystemCommandExecutor {
    fn run(&mut self, program: &str, args: &[&str]) -> Result<()> {
        run_checked(program, args)
    }

    fn service_is_active(&self, service_name: &str) -> bool {
        Command::new("systemctl")
            .args(["is-active", "--quiet", service_name])
            .status()
            .is_ok_and(|status| status.success())
    }
}

fn ensure_networkmanager_running(executor: &mut impl CommandExecutor) -> Result<()> {
    executor
        .run("nmcli", &["general", "status"])
        .context("NetworkManager must be running to manage the DNS proxy")
}

fn reload_networkmanager_dns(
    executor: &mut impl CommandExecutor,
    resolver_path: &Path,
) -> Result<()> {
    // Reload the drop-in, then explicitly regenerate resolver state. The
    // latter is important when `/etc/resolv.conf` is a regular file rather
    // than a systemd-resolved symlink.
    executor.run("nmcli", &["general", "reload", "conf"])?;
    executor.run("nmcli", &["general", "reload", "dns-rc"])?;
    ensure_regular_file_mode(resolver_path, 0o644)
}

/// NetworkManager retains the mode of a pre-existing regular resolver file.
/// Resolver libraries run by desktop applications need it to be world-readable.
fn ensure_regular_file_mode(path: &Path, expected_mode: u32) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if !metadata.file_type().is_file() {
        bail!("{} is not a regular file or symlink", path.display());
    }

    let actual_mode = metadata.permissions().mode() & 0o777;
    if actual_mode != expected_mode {
        let mut permissions = metadata.permissions();
        permissions.set_mode(expected_mode);
        fs::set_permissions(path, permissions)
            .with_context(|| format!("Failed to set {} to {expected_mode:o}", path.display()))?;
    }
    Ok(())
}

fn run_checked(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("Failed to execute {program}"))?;
    if !status.success() {
        bail!("{program} {} exited with {status}", args.join(" "));
    }
    Ok(())
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("Failed to read {}", path.display())),
    }
}

fn write_managed_config(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("NetworkManager configuration path has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("Failed to create {}", parent.display()))?;
    atomic_write(path, content)
}

fn restore_config(path: &Path, previous: Option<&str>) -> Result<()> {
    match previous {
        Some(content) => atomic_write(path, content),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("Failed to remove {}", path.display()))
            }
        },
    }
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    let temporary = path.with_extension("conf.tmp");
    fs::write(&temporary, content)
        .with_context(|| format!("Failed to write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| {
        format!(
            "Failed to replace {} with {}",
            path.display(),
            temporary.display()
        )
    })
}

fn legacy_loopback_resolver_present() -> bool {
    fs::read_to_string("/etc/resolv.conf")
        .is_ok_and(|content| resolver_contains_dnscrypt_loopback(&content))
}

fn resolver_contains_dnscrypt_loopback(content: &str) -> bool {
    content.lines().any(|line| {
        let mut fields = line.split_whitespace();
        matches!(
            (fields.next(), fields.next()),
            (Some("nameserver"), Some("127.0.0.1" | "::1"))
        )
    })
}

fn is_managed_config(content: &str) -> bool {
    matches!(
        content,
        LEGACY_MANAGED_DOH_CONFIG | MANAGED_DOH_CONFIG | MANAGED_DIRECT_CONFIG
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_configs_preserve_both_dns_modes() {
        assert_eq!(
            MANAGED_DOH_CONFIG,
            "# Managed by cf-toggle. Do not edit.\n[global-dns]\nresolve-mode=exclusive\n\n[global-dns-domain-*]\nservers=127.0.0.1,::1\n"
        );
        assert_eq!(
            MANAGED_DIRECT_CONFIG,
            "# Managed by cf-toggle. Do not edit.\n[global-dns]\nresolve-mode=exclusive\n\n[global-dns-domain-*]\nservers=1.1.1.1,1.0.0.1\n"
        );
    }

    #[test]
    fn legacy_detection_only_matches_nameserver_loopback_entries() {
        assert!(resolver_contains_dnscrypt_loopback(
            "nameserver 127.0.0.1\n"
        ));
        assert!(resolver_contains_dnscrypt_loopback("nameserver ::1\n"));
        assert!(!resolver_contains_dnscrypt_loopback(
            "# nameserver 127.0.0.1\n"
        ));
        assert!(!resolver_contains_dnscrypt_loopback("search 127.0.0.1\n"));
        assert!(!resolver_contains_dnscrypt_loopback("nameserver 1.1.1.1\n"));
    }

    #[test]
    fn manager_state_requires_the_exact_owned_drop_in() {
        let directory = test_directory("state");
        let path = directory.join(NETWORKMANAGER_CONFIG);
        let manager = test_manager(path.clone());

        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, "[global-dns-domain-*]\nservers=127.0.0.1\n").unwrap();
        assert!(!manager.has_doh_config());

        fs::write(&path, LEGACY_MANAGED_DOH_CONFIG).unwrap();
        assert!(manager.has_doh_config());

        fs::write(&path, MANAGED_DOH_CONFIG).unwrap();
        assert!(manager.has_doh_config());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn direct_cloudflare_config_is_owned_and_can_be_reenabled() {
        let directory = test_directory("direct-to-doh");
        let path = directory.join(NETWORKMANAGER_CONFIG);
        let manager = test_manager(path.clone());
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, MANAGED_DIRECT_CONFIG).unwrap();
        let mut executor = FakeExecutor::default();

        manager.set_enabled_with(&mut executor, true).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), MANAGED_DOH_CONFIG);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn atomic_write_replaces_the_complete_file() {
        let directory = test_directory("atomic-write");
        let path = directory.join(NETWORKMANAGER_CONFIG);
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, "old").unwrap();

        atomic_write(&path, MANAGED_DOH_CONFIG).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), MANAGED_DOH_CONFIG);
        assert!(!path.with_extension("conf.tmp").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn restore_config_removes_a_new_drop_in() {
        let directory = test_directory("restore-none");
        let path = directory.join(NETWORKMANAGER_CONFIG);
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, MANAGED_DOH_CONFIG).unwrap();

        restore_config(&path, None).unwrap();

        assert!(!path.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn restore_config_preserves_preexisting_content() {
        let directory = test_directory("restore-existing");
        let path = directory.join(NETWORKMANAGER_CONFIG);
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, MANAGED_DOH_CONFIG).unwrap();

        restore_config(&path, Some("custom configuration\n")).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "custom configuration\n");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn regular_resolver_file_is_repaired_to_world_readable() {
        let directory = test_directory("resolv-permissions");
        let path = directory.join("resolv.conf");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, "nameserver 1.1.1.1\n").unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(&path, permissions).unwrap();

        ensure_regular_file_mode(&path, 0o644).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn enable_starts_the_proxy_before_networkmanager_uses_loopback() {
        let directory = test_directory("enable-order");
        let path = directory.join(NETWORKMANAGER_CONFIG);
        let manager = test_manager(path.clone());
        let mut executor = FakeExecutor::default();

        manager.set_enabled_with(&mut executor, true).unwrap();

        assert_eq!(
            executor.calls,
            [
                "nmcli general status",
                "systemctl enable --now dnscrypt-proxy",
                "nmcli general reload conf",
                "nmcli general reload dns-rc",
            ]
        );
        assert_eq!(fs::read_to_string(path).unwrap(), MANAGED_DOH_CONFIG);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn disable_restores_direct_cloudflare_dns_before_stopping_the_proxy() {
        let directory = test_directory("disable-order");
        let path = directory.join(NETWORKMANAGER_CONFIG);
        let manager = test_manager(path.clone());
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, MANAGED_DOH_CONFIG).unwrap();
        let mut executor = FakeExecutor::default();

        manager.set_enabled_with(&mut executor, false).unwrap();

        assert_eq!(
            executor.calls,
            [
                "nmcli general status",
                "nmcli general reload conf",
                "nmcli general reload dns-rc",
                "systemctl disable --now dnscrypt-proxy",
            ]
        );
        assert_eq!(fs::read_to_string(path).unwrap(), MANAGED_DIRECT_CONFIG);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn disable_migrates_the_previous_managed_drop_in() {
        let directory = test_directory("legacy-disable");
        let path = directory.join(NETWORKMANAGER_CONFIG);
        let manager = test_manager(path.clone());
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, LEGACY_MANAGED_DOH_CONFIG).unwrap();
        let mut executor = FakeExecutor::default();

        manager.set_enabled_with(&mut executor, false).unwrap();

        assert_eq!(fs::read_to_string(path).unwrap(), MANAGED_DIRECT_CONFIG);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_disable_reload_restores_the_loopback_override_and_keeps_dnscrypt_running() {
        let directory = test_directory("disable-rollback");
        let path = directory.join(NETWORKMANAGER_CONFIG);
        let manager = test_manager(path.clone());
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, MANAGED_DOH_CONFIG).unwrap();
        let mut executor = FakeExecutor::failing_once("nmcli general reload dns-rc");

        assert!(manager.set_enabled_with(&mut executor, false).is_err());

        assert_eq!(fs::read_to_string(&path).unwrap(), MANAGED_DOH_CONFIG);
        assert!(
            !executor
                .calls
                .iter()
                .any(|call| call == "systemctl disable --now dnscrypt-proxy")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn refuses_to_overwrite_or_remove_a_non_owned_drop_in() {
        let directory = test_directory("foreign-config");
        let path = directory.join(NETWORKMANAGER_CONFIG);
        let manager = test_manager(path.clone());
        fs::create_dir_all(&directory).unwrap();
        fs::write(&path, "[global-dns-domain-*]\nservers=9.9.9.9\n").unwrap();

        let mut enable_executor = FakeExecutor::default();
        assert!(
            manager
                .set_enabled_with(&mut enable_executor, true)
                .is_err()
        );
        assert_eq!(enable_executor.calls, ["nmcli general status"]);

        let mut disable_executor = FakeExecutor::default();
        assert!(
            manager
                .set_enabled_with(&mut disable_executor, false)
                .is_err()
        );
        assert_eq!(disable_executor.calls, ["nmcli general status"]);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "[global-dns-domain-*]\nservers=9.9.9.9\n"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[derive(Default)]
    struct FakeExecutor {
        calls: Vec<String>,
        fail_once: Option<String>,
    }

    impl FakeExecutor {
        fn failing_once(command: &str) -> Self {
            Self {
                calls: Vec::new(),
                fail_once: Some(command.to_string()),
            }
        }
    }

    impl CommandExecutor for FakeExecutor {
        fn run(&mut self, program: &str, args: &[&str]) -> Result<()> {
            let command = format!("{program} {}", args.join(" "));
            self.calls.push(command.clone());
            if self.fail_once.as_deref() == Some(command.as_str()) {
                self.fail_once = None;
                bail!("simulated command failure");
            }
            Ok(())
        }

        fn service_is_active(&self, _service_name: &str) -> bool {
            true
        }
    }

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "cf-toggle-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn test_manager(config_path: PathBuf) -> DnsManager {
        let resolver_path = config_path.parent().unwrap().join("resolv.conf");
        fs::create_dir_all(resolver_path.parent().unwrap()).unwrap();
        fs::write(&resolver_path, "nameserver 1.1.1.1\n").unwrap();
        DnsManager::with_resolver_path(config_path, resolver_path)
    }
}
