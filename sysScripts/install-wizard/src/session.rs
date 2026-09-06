use crate::CmdExecutor;
use std::path::Path;
use toml_edit::{DocumentMut, Item};

/// Configures essential system services and settings, including mkinitcpio sanitation, enabling
/// geoclue/bluetooth/bolt, enabling Pacman cache cleanup, and
/// configuring logind. This function is idempotent and can be safely run multiple times
/// without causing issues.
pub fn configure_system(sys: &impl CmdExecutor, home: &Path) -> Result<(), std::io::Error> {
    sanitize_mkinitcpio(sys)?;
    sys.run_cmd("sudo", &["systemctl", "enable", "geoclue.service"])?;
    sys.run_cmd("sudo", &["systemctl", "enable", "bluetooth.service"])?;
    sys.run_cmd("sudo", &["systemctl", "enable", "bolt.service"])?;
    // Prevent Pacman from eating the entire hard drive over time
    println!("   🧹 Enabling automated Pacman cache cleanup...");
    sys.run_cmd("sudo", &["systemctl", "enable", "--now", "paccache.timer"])?;

    // --- ENVIRONMENT & LOGIND ---
    println!("    🔧 Configuring Session Environment (PATH)...");
    let env_dir = home.join(".config/environment.d");
    let env_name = env_dir.join("99-cargo-path.conf");
    let env_file = env_name
        .to_str()
        .ok_or_else(|| std::io::Error::other("Invalid environment file path"))?;

    sys.create_dir_all(&env_dir)?;
    let content = "PATH=$HOME/.cargo/bin:$PATH\n";
    sys.write_string_to_file(env_file, content)?;

    configure_logind(sys)?;
    configure_shell(sys, home)?;
    Ok(())
}

/// Enables local printing, DNS-SD discovery, and mDNS hostname resolution. This is deliberately
/// separate from the fresh-install-only system setup so updates and config-refresh runs repair
/// these services too.
pub fn configure_printing_services(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    println!("   🖨️  Enabling printing and network printer discovery...");
    sys.run_cmd(
        "sudo",
        &["pacman", "-S", "--needed", "--noconfirm", "nss-mdns"],
    )?;

    let nsswitch_path = Path::new("/etc/nsswitch.conf");
    let nsswitch_content = sys.read_file_to_string(nsswitch_path)?;
    if let Some(updated) = enable_mdns_hostname_resolution(&nsswitch_content) {
        println!("   🌐 Enabling .local hostname resolution for network printers...");
        sys.install_string_to_root_file(nsswitch_path, &updated, "644")?;
    }

    sys.run_cmd("sudo", &["systemctl", "enable", "--now", "cups.service"])?;
    sys.run_cmd(
        "sudo",
        &["systemctl", "enable", "--now", "avahi-daemon.service"],
    )?;
    Ok(())
}

/// Silences routine kernel and systemd status messages so they cannot draw over tuigreet.
///
/// Genoa's normal boot path is managed by kernel-install through `/etc/kernel/cmdline`. GRUB is
/// supported as a fallback for existing installations. In both cases, only missing parameters are
/// added; a user's existing kernel log level is deliberately preserved.
pub fn configure_quiet_boot(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    const KERNEL_CMDLINE_PATH: &str = "/etc/kernel/cmdline";
    const GRUB_DEFAULT_PATH: &str = "/etc/default/grub";

    let kernel_cmdline_path = Path::new(KERNEL_CMDLINE_PATH);
    if sys.path_exists(kernel_cmdline_path) {
        let content = sys.read_file_to_string(kernel_cmdline_path)?;
        if let Some(updated) = add_quiet_boot_parameters(&content) {
            println!("   🔇 Adding quiet boot parameters to /etc/kernel/cmdline...");
            sys.install_string_to_root_file(kernel_cmdline_path, &updated, "644")?;
        } else {
            println!("   ✅ Quiet boot parameters are already configured.");
        }
        return Ok(());
    }

    let grub_default_path = Path::new(GRUB_DEFAULT_PATH);
    if !sys.path_exists(grub_default_path) {
        println!("   ℹ️  No supported bootloader command line found; leaving it unchanged.");
        return Ok(());
    }

    let content = sys.read_file_to_string(grub_default_path)?;
    let Some(updated) = add_quiet_boot_parameters_to_grub(&content) else {
        println!(
            "   ✅ Quiet boot parameters are already configured or GRUB has no default command line."
        );
        return Ok(());
    };

    println!("   🔇 Adding quiet boot parameters to GRUB's default command line...");
    sys.install_string_to_root_file(grub_default_path, &updated, "644")?;
    if sys.path_exists(Path::new("/boot/grub/grub.cfg")) {
        sys.run_cmd("sudo", &["grub-mkconfig", "-o", "/boot/grub/grub.cfg"])?;
    } else {
        eprintln!(
            "   ⚠️  GRUB config was updated, but /boot/grub/grub.cfg was not found to regenerate."
        );
    }
    Ok(())
}

/// Appends only the quieting flags absent from one kernel command line.
///
/// `quiet` suppresses routine kernel and systemd status output. `loglevel=3` further limits
/// kernel-console output, but an existing loglevel is treated as an explicit user preference.
fn add_quiet_boot_parameters(content: &str) -> Option<String> {
    let arguments: Vec<&str> = content.split_whitespace().collect();
    let mut missing = Vec::new();

    if !arguments.contains(&"quiet") {
        missing.push("quiet");
    }
    if !arguments
        .iter()
        .any(|argument| argument.starts_with("loglevel="))
    {
        missing.push("loglevel=3");
    }
    if missing.is_empty() {
        return None;
    }

    let existing = content.trim();
    Some(match existing {
        "" => format!("{}\n", missing.join(" ")),
        _ => format!("{existing} {}\n", missing.join(" ")),
    })
}

/// Updates the normal GRUB boot command line while retaining comments and every existing option.
fn add_quiet_boot_parameters_to_grub(content: &str) -> Option<String> {
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let had_trailing_newline = content.ends_with('\n');

    for line in &mut lines {
        let replacement = {
            let trimmed = line.trim_start();
            let Some(value) = trimmed.strip_prefix("GRUB_CMDLINE_LINUX_DEFAULT=") else {
                continue;
            };
            let Some(value) = value.strip_prefix('"') else {
                continue;
            };
            let Some((command_line, suffix)) = value.split_once('"') else {
                continue;
            };
            let Some(updated_command_line) = add_quiet_boot_parameters(command_line) else {
                return None;
            };

            let indentation = &line[..line.len() - trimmed.len()];
            format!(
                "{indentation}GRUB_CMDLINE_LINUX_DEFAULT=\"{}\"{suffix}",
                updated_command_line.trim()
            )
        };
        *line = replacement;

        let mut updated = lines.join("\n");
        if had_trailing_newline {
            updated.push('\n');
        }
        return Some(updated);
    }

    None
}

/// Adds the Avahi NSS resolver to the active `hosts:` lookup chain without disturbing the
/// system's other name-service settings. `nss-mdns` must run before `resolve`/`dns`, otherwise
/// CUPS can discover a DNS-SD printer but cannot resolve its `printer.local` hostname.
fn enable_mdns_hostname_resolution(content: &str) -> Option<String> {
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let had_trailing_newline = content.ends_with('\n');

    for line in &mut lines {
        let replacement = {
            let trimmed = line.trim_start();
            let Some(services) = trimmed.strip_prefix("hosts:") else {
                continue;
            };

            let (services, comment) = match services.split_once('#') {
                Some((services, comment)) => (services, Some(comment)),
                None => (services, None),
            };
            let mut services: Vec<&str> = services.split_whitespace().collect();
            if services.iter().any(|service| service.starts_with("mdns")) {
                return None;
            }

            let insert_at = services
                .iter()
                .position(|service| *service == "resolve" || *service == "dns")
                .unwrap_or(services.len());
            services.splice(insert_at..insert_at, ["mdns_minimal", "[NOTFOUND=return]"]);

            let indentation = &line[..line.len() - trimmed.len()];
            let mut replacement = format!("{indentation}hosts: {}", services.join(" "));
            if let Some(comment) = comment {
                replacement.push_str(" #");
                replacement.push_str(comment);
            }
            replacement
        };
        *line = replacement;

        let mut updated = lines.join("\n");
        if had_trailing_newline {
            updated.push('\n');
        }
        return Some(updated);
    }

    None
}

/// Cleans up the `mkinitcpio.conf` file to fix the known Archinstall 2025 bug that appends 'o"' to
/// the end of the file,
fn sanitize_mkinitcpio(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    // --- SANITIZE MKINITCPIO (Fix Archinstall 2025 Bug) ---
    // This protects NVIDIA users from the 'o"' corruption crash.
    println!("   🧹 Checking mkinitcpio.conf for corruption...");
    let mkinit_path = Path::new("/etc/mkinitcpio.conf");

    // Check if the file specifically ends with the garbage (ignoring whitespace)
    // We read it first to be safe, rather than firing sed blindly.
    if let Ok(content) = sys.read_file_to_string(mkinit_path) {
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
            let new_content = lines.join("\n") + "\n";
            sys.install_string_to_root_file(mkinit_path, new_content.as_str(), "644")?;
        }
    }
    Ok(())
}

/// Configures dnscrypt-proxy to use Cloudflare's DNS servers for enhanced privacy and security.
///
/// This is intentionally callable outside fresh-install setup so existing systems receive
/// configuration repairs during both ordinary updates and config refreshes.
pub fn configure_dns(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    // --- DNS Crypt Proxy CONFIGURATION ---
    println!("   🔧 Configuring dnscrypt-proxy (DNS Proxy)...");

    // 1. Ensure package is installed (failsafe)
    sys.run_cmd(
        "sudo",
        &["pacman", "-S", "--needed", "--noconfirm", "dnscrypt-proxy"],
    )?;
    // 2. Configure TOML to use Cloudflare.
    //
    // `server_names` and `listen_addresses` are top-level settings. Editing lines
    // by prefix can accidentally place either setting in `[static]`, making the
    // dnscrypt configuration invalid. toml_edit preserves the rest of Arch's
    // configuration while addressing these keys in their proper scopes.
    let dns_conf = Path::new("/etc/dnscrypt-proxy/dnscrypt-proxy.toml");
    let content = sys.read_file_to_string(dns_conf)?;
    let new_content = update_dnscrypt_config(&content)?;
    sys.install_string_to_root_file(dns_conf, &new_content, "644")?;
    // 3. Enable the service
    sys.run_cmd("sudo", &["systemctl", "enable", "--now", "dnscrypt-proxy"])?;

    // 4. Clean up old Cloudflared artifacts if they exist
    let _ = sys.run_cmd_ignore_err(
        "sudo",
        &["systemctl", "disable", "--now", "cloudflared-dns"],
    );
    let _ = sys.run_cmd_ignore_err(
        "sudo",
        &["rm", "-f", "/etc/systemd/system/cloudflared-dns.service"],
    );
    sys.run_cmd("sudo", &["systemctl", "daemon-reload"])?;
    Ok(())
}

fn update_dnscrypt_config(content: &str) -> Result<String, std::io::Error> {
    let mut document = content
        .parse::<DocumentMut>()
        .map_err(|error| std::io::Error::other(format!("Invalid dnscrypt-proxy TOML: {error}")))?;

    // Repair configurations produced by the previous installer implementation.
    // These settings are not valid inside the `[static]` table.
    if let Some(static_table) = document.get_mut("static").and_then(Item::as_table_mut) {
        static_table.remove("server_names");
        static_table.remove("listen_addresses");
    }

    let server_names = "['cloudflare']"
        .parse::<Item>()
        .map_err(|error| std::io::Error::other(format!("Invalid server_names value: {error}")))?;
    let listen_addresses = "['127.0.0.1:53', '[::1]:53']"
        .parse::<Item>()
        .map_err(|error| {
            std::io::Error::other(format!("Invalid listen_addresses value: {error}"))
        })?;

    let root = document.as_table_mut();
    root.insert("server_names", server_names);
    root.insert("listen_addresses", listen_addresses);
    let updated = document.to_string();

    // toml_edit normalizes insignificant leading whitespace when parsing. Avoid
    // rewriting an already-correct system configuration just for that.
    if updated.trim() == content.trim() {
        Ok(content.to_string())
    } else {
        Ok(updated)
    }
}

///Configures the user's shell to Zsh and sets up Tmux Plugin Manager for enhanced terminal
///experience.
fn configure_shell(sys: &impl CmdExecutor, home: &Path) -> Result<(), std::io::Error> {
    println!("    🔧 Setting Shell to Zsh...");
    let user = sys
        .get_env_var("USER")
        .unwrap_or_else(|| "root".to_string());
    if let Err(e) = sys.run_cmd("sudo", &["chsh", "-s", "/usr/bin/zsh", &user]) {
        eprintln!("   ⚠️  Failed to change shell: {}", e)
    };

    println!("    ✨ Setting up Tmux Plugin Manager...");
    let tpm_dir = home.join(".tmux/plugins/tpm");
    if !sys.path_exists(&tpm_dir) {
        if let Some(tpm_str) = tpm_dir.to_str() {
            if let Err(e) = sys.run_cmd(
                "git",
                &["clone", "https://github.com/tmux-plugins/tpm", tpm_str],
            ) {
                eprintln!("   ⚠️  Failed to clone TPM: {}", e)
            }
        } else {
            eprintln!("   ⚠️  Invalid path for TPM directory.");
        };
    }
    Ok(())
}

///Configures systemd-logind to ensure that user processes are killed on logout, preventing
///lingering sessions and resource leaks.
fn configure_logind(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    println!("    🔧 Configuring Logind...");
    let logind_conf = Path::new("/etc/systemd/logind.conf");
    let content = match sys.read_file_to_string(logind_conf) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("   ⚠️  Failed to read logind.conf: {}", e);
            return Err(e);
        }
    };
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    let mut found = false;
    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("KillUserProcesses=") || trimmed.starts_with("#KillUserProcesses=") {
            if trimmed == "KillUserProcesses=yes" {
                println!("   ✅ KillUserProcesses is already set to yes.");
                found = true;
                break;
            }
            found = true;
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
    }
    let new_content = lines.join("\n") + "\n";
    sys.install_string_to_root_file(logind_conf, new_content.as_str(), "644")?;
    Ok(())
}

/// First, we check if TLP is currently a depreciated symlink on the users system. If so, we delete the symlink.
/// Next, on all computers we copy tlp.conf from ~/genoa/tlp.conf to /etc/tlp.conf and start/restart the tlp service.
/// This is needed to enable battery optimizations on laptops.
pub fn configure_tlp(sys: &impl CmdExecutor, repo_root: &Path) -> Result<(), std::io::Error> {
    println!("    🔧 Configuring TLP (Power Management)...");
    let tlp_conf_src = repo_root.join("tlp.conf");
    let tlp_conf_dest = Path::new("/etc/tlp.conf");
    if sys.path_exists(Path::new(tlp_conf_dest)) && sys.is_symlink(Path::new(tlp_conf_dest)) {
        println!("   ⚠️  Detected deprecated TLP symlink. Removing...");
        sys.run_cmd("sudo", &["rm", "-f", "/etc/tlp.conf"])?;
    }
    let modified = sys.install_string_to_root_file(
        tlp_conf_dest,
        &sys.read_file_to_string(&tlp_conf_src)?,
        "644",
    )?;
    if modified {
        let _ = sys.run_cmd_ignore_err("sudo", &["systemctl", "enable", "tlp.service"]);
        let _ = sys.run_cmd_ignore_err("systemctl", &["is-active", "--quiet", "tlp.service"]);
        sys.run_cmd("sudo", &["systemctl", "restart", "tlp.service"])?;
    } else {
        eprintln!("   ✅ TLP is already correctly configured. No changes needed.");
    }
    Ok(())
}

/// Renames session files to enforce a specific order in Greetd/Tuigreet.
/// Strategy: Make a proxy directory in /etc/greetd/genoa-sessions and copy/patch the .desktop files
/// there with new Exec lines pointing to /usr/local/bin/genoa-proxy (or sway-hybrid for the sway
/// session if NVIDIA is detected). This way we don't mess with the system files directly and can
/// maintain order and custom display names without risking package manager conflicts.
pub fn enforce_session_order(
    sys: &impl CmdExecutor,
    is_nvidia: bool,
    repo_root: &Path,
) -> Result<(), std::io::Error> {
    println!("   🔧 Enforcing Session Order (Renaming .desktop files)...");

    let sessions_dir = "/usr/share/wayland-sessions";
    let proxy_dir = "/etc/greetd/genoa-sessions";
    let script_path = repo_root.join("scripts/session-launch.sh");
    let script_src = match script_path.to_str() {
        Some(s) => s,
        None => {
            eprintln!("   ⚠️  Invalid path for session launch script.");
            return Err(std::io::Error::other("Invalid script path"));
        }
    };
    let script_dest = "/usr/local/bin/genoa-proxy";
    let mut found_session = false;

    //install /Genoa/scripts/session-launch.sh to /usr/local/bin/genoa-proxy in a single atomic step
    sys.run_cmd(
        "sudo",
        &[
            "install",
            "-m",
            "755",
            "-o",
            "root",
            "-g",
            "root",
            script_src,
            script_dest,
        ],
    )?;

    sys.create_root_dir_all(Path::new(&proxy_dir))?;
    let session_files = sys.list_dir_file_names(Path::new(sessions_dir))?;

    // Tuple: (Expected Name Fragment, Safe Custom Name, Display Name)
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

    for (expected_name, custom_name, display_name) in updates {
        let source_name = match session_files
            .iter()
            .find(|name| name.contains(expected_name))
        {
            Some(name) => name,
            None => {
                println!(
                    "   ⚠️  Warning: Expected session containing '{}' not found. Skipping.",
                    expected_name
                );
                continue;
            }
        };
        found_session = true;
        let std_path_string = format!("{}/{}", sessions_dir, source_name);
        let custom_path_string = format!("{}/{}", proxy_dir, custom_name);
        let std_path = Path::new(&std_path_string);
        let custom_path = Path::new(&custom_path_string);
        let content = match sys.read_file_to_string(std_path) {
            Err(e) => {
                println!(
                    "   ⚠️  Warning: Failed to read {}: {}. Skipping.",
                    source_name, e
                );
                continue;
            }
            Ok(content) => content,
        };
        let exec_line = if expected_name.contains("sway") && is_nvidia {
            "Exec=/usr/local/bin/sway-hybrid".to_string()
        } else {
            format!(
                "Exec=/usr/local/bin/genoa-proxy /usr/share/wayland-sessions/{}",
                source_name
            )
        };
        let new_content = content
            .lines()
            .map(|line| {
                if line.starts_with("Exec=") {
                    exec_line.to_string()
                } else if line.starts_with("Name=") {
                    format!("Name={}", display_name)
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<String>>()
            .join("\n");
        let _ = sys.install_string_to_root_file(custom_path, &new_content, "644")?;
    }
    if found_session {
        configure_greetd(sys)?;
    } else {
        println!("   ⚠️  No supported session files were found. Skipping Greetd configuration.");
    }
    Ok(())
}

/// Configures Greetd with a custom tuigreet session and disables other DMs.
fn configure_greetd(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    println!("    🔧 Configuring Greetd...");
    let greetd_path = Path::new("/etc/greetd/config.toml");
    let greetd_config = r#"
[terminal]
vt = 1
[default_session]
command = "tuigreet --time --remember --sessions /etc/greetd/genoa-sessions"
user = "greeter"
"#;
    let existing_content = sys.read_file_to_string(greetd_path).unwrap_or_default();
    if existing_content.trim() != greetd_config.trim() {
        sys.install_string_to_root_file(greetd_path, greetd_config, "644")?;
    }
    let _ = sys.run_cmd_ignore_err("sudo", &["systemctl", "disable", "gdm", "sddm", "lightdm"]);
    sys.run_cmd(
        "sudo",
        &["systemctl", "enable", "--force", "greetd.service"],
    )?;
    Ok(())
}

//----------- Unit Tests ---------------------
//--------------------------------------------
//

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_env::MockEnv;

    #[test]
    fn test_configure_dns_execution_order() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/dnscrypt-proxy/dnscrypt-proxy.toml".to_string(),
            "\nserver_names = ['quad9-dnscrypt-ip4-filter-pri']\nlisten_addresses = ['127.0.0.1:5353']\n".to_string(),
        );
        let result = configure_dns(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            6,
            "Expected exactly 6 commands to be run for DNS configuration"
        );
        assert_eq!(
            log[0],
            (
                "sudo".to_string(),
                vec![
                    "pacman".to_string(),
                    "-S".to_string(),
                    "--needed".to_string(),
                    "--noconfirm".to_string(),
                    "dnscrypt-proxy".to_string()
                ]
            )
        );
        assert!(
            log[1].0 == "sudo"
                && log[1].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string()
                ])
        );
        assert_eq!(
            log[2],
            (
                "sudo".to_string(),
                vec![
                    "systemctl".to_string(),
                    "enable".to_string(),
                    "--now".to_string(),
                    "dnscrypt-proxy".to_string()
                ]
            )
        );
        assert_eq!(
            log[3],
            (
                "sudo".to_string(),
                vec![
                    "systemctl".to_string(),
                    "disable".to_string(),
                    "--now".to_string(),
                    "cloudflared-dns".to_string()
                ]
            )
        );
        assert_eq!(
            log[4],
            (
                "sudo".to_string(),
                vec![
                    "rm".to_string(),
                    "-f".to_string(),
                    "/etc/systemd/system/cloudflared-dns.service".to_string()
                ]
            )
        );
        assert_eq!(
            log[5],
            (
                "sudo".to_string(),
                vec!["systemctl".to_string(), "daemon-reload".to_string()]
            )
        );
    }
    #[test]
    fn test_configure_dns_no_update_needed() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/dnscrypt-proxy/dnscrypt-proxy.toml".to_string(),
            "\nserver_names = ['cloudflare']\nlisten_addresses = ['127.0.0.1:53', '[::1]:53']"
                .to_string()
                + "\n",
        );
        let result = configure_dns(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            5,
            "Expected exactly 5 commands to be run for DNS configuration"
        );
        assert_eq!(
            log[0],
            (
                "sudo".to_string(),
                vec![
                    "pacman".to_string(),
                    "-S".to_string(),
                    "--needed".to_string(),
                    "--noconfirm".to_string(),
                    "dnscrypt-proxy".to_string()
                ]
            )
        );
        assert_eq!(
            log[1],
            (
                "sudo".to_string(),
                vec![
                    "systemctl".to_string(),
                    "enable".to_string(),
                    "--now".to_string(),
                    "dnscrypt-proxy".to_string()
                ]
            )
        );
        assert_eq!(
            log[2],
            (
                "sudo".to_string(),
                vec![
                    "systemctl".to_string(),
                    "disable".to_string(),
                    "--now".to_string(),
                    "cloudflared-dns".to_string()
                ]
            )
        );
        assert_eq!(
            log[3],
            (
                "sudo".to_string(),
                vec![
                    "rm".to_string(),
                    "-f".to_string(),
                    "/etc/systemd/system/cloudflared-dns.service".to_string()
                ]
            )
        );
        assert_eq!(
            log[4],
            (
                "sudo".to_string(),
                vec!["systemctl".to_string(), "daemon-reload".to_string()]
            )
        );
        let binding = env.mock_files.borrow();
        let updated_file = binding
            .get("/etc/dnscrypt-proxy/dnscrypt-proxy.toml")
            .unwrap();
        assert_eq!(
            updated_file,
            "\nserver_names = ['cloudflare']\nlisten_addresses = ['127.0.0.1:53', '[::1]:53']\n"
        );
    }

    #[test]
    fn test_dns_config_repairs_invalid_static_entries() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/dnscrypt-proxy/dnscrypt-proxy.toml".to_string(),
            r#"# server_names = ['scaleway-fr', 'google']
listen_addresses = ['127.0.0.1:53', '[::1]:53']

[static]
server_names = ['cloudflare']
"#
            .to_string(),
        );

        configure_dns(&env).expect("DNS configuration should repair old installer output");
        let updated = env
            .mock_files
            .borrow()
            .get("/etc/dnscrypt-proxy/dnscrypt-proxy.toml")
            .unwrap()
            .clone();
        let document = updated
            .parse::<DocumentMut>()
            .expect("repaired configuration must be valid TOML");

        assert_eq!(
            document["server_names"]
                .as_value()
                .unwrap()
                .to_string()
                .trim(),
            "['cloudflare']"
        );
        assert!(!updated.contains("[static]\nserver_names"));
        assert!(
            updated.find("\nserver_names = ['cloudflare']").unwrap()
                < updated.find("[static]").unwrap()
        );
    }

    #[test]
    fn test_mkinit() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/mkinitcpio.conf".to_string(),
            "\ntest config content\no\"".to_string(),
        );
        let result = sanitize_mkinitcpio(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            1,
            "Expected exactly one command to be run for mkinitcpio sanitization"
        );
        assert!(
            log[0].0 == "sudo"
                && log[0].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string()
                ])
        );
        let binding = env.mock_files.borrow();
        let updated = binding.get("/etc/mkinitcpio.conf").unwrap();
        assert_eq!(updated, "\ntest config content\n");
    }
    #[test]
    fn test_mkinit_clean_config() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/mkinitcpio.conf".to_string(),
            "\ntest config content\n".to_string(),
        );
        let result = sanitize_mkinitcpio(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert!(
            log.is_empty(),
            "Expected no commands to be run for clean config"
        );
    }
    #[test]
    fn test_config_shell() {
        let mut env = MockEnv::default();
        env.env_vars
            .insert("USER".to_string(), "testuser".to_string());
        let result = configure_shell(&env, std::path::Path::new("/home/testuser"));
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            2,
            "Expected two commands to be run when TPM does not exist"
        );
        assert!(
            log[0].0 == "sudo"
                && log[0].1.starts_with(&[
                    "chsh".to_string(),
                    "-s".to_string(),
                    "/usr/bin/zsh".to_string(),
                ])
        );
        assert_eq!(
            log[1],
            (
                "git".to_string(),
                vec![
                    "clone".to_string(),
                    "https://github.com/tmux-plugins/tpm".to_string(),
                    "/home/testuser/.tmux/plugins/tpm".to_string()
                ]
            )
        );
    }
    #[test]
    fn test_config_shell_tpm_exists() {
        let mut env = MockEnv::default();
        env.env_vars
            .insert("USER".to_string(), "testuser".to_string());
        env.mock_files.borrow_mut().insert(
            "/home/testuser/.tmux/plugins/tpm".to_string(),
            "".to_string(),
        );
        let result = configure_shell(&env, std::path::Path::new("/home/testuser"));
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            1,
            "Expected one commands to be run when TPM already exists"
        );
        assert!(
            log[0].0 == "sudo"
                && log[0].1.starts_with(&[
                    "chsh".to_string(),
                    "-s".to_string(),
                    "/usr/bin/zsh".to_string(),
                ])
        );
    }
    #[test]
    fn test_config_logind_happy_path() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/systemd/logind.conf".to_string(),
            "\n[Login]\nKillUserProcesses=yes\n".to_string(),
        );
        let result = configure_logind(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            0,
            "Expected No commands to execute and no modifications to be performed"
        );
    }
    #[test]
    fn test_config_logind_replacement_path() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/systemd/logind.conf".to_string(),
            "\n[Login]\n#KillUserProcesses=no\n".to_string(),
        );
        let result = configure_logind(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            1,
            "Expected exactly one command to be run for logind configuration"
        );
        assert!(
            log[0].0 == "sudo"
                && log[0].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string(),
                    "-o".to_string(),
                    "root".to_string(),
                    "-g".to_string(),
                    "root".to_string(),
                ])
        );
        let binding = env.mock_files.borrow();
        let updated = binding.get("/etc/systemd/logind.conf").unwrap();
        assert!(updated.contains("KillUserProcesses=yes"));
    }
    #[test]
    fn test_config_logind_insertion_path() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/systemd/logind.conf".to_string(),
            "\n[Login]\n# Some other config\n".to_string(),
        );
        let result = configure_logind(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            1,
            "Expected exactly one command to be run for logind configuration"
        );
        assert!(
            log[0].0 == "sudo"
                && log[0].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string(),
                    "-o".to_string(),
                    "root".to_string(),
                    "-g".to_string(),
                    "root".to_string(),
                ])
        );
    }
    #[test]
    fn test_config_logind_no_login_section() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/systemd/logind.conf".to_string(),
            "\n[SomeOtherSection]\nConfig=Value\n".to_string(),
        );
        let result = configure_logind(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            1,
            "Expected exactly one command to be run for logind configuration"
        );
        assert!(
            log[0].0 == "sudo"
                && log[0].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string(),
                    "-o".to_string(),
                    "root".to_string(),
                    "-g".to_string(),
                    "root".to_string(),
                ])
        );
    }
    #[test]
    fn test_config_greetd_happy_path() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/greetd/config.toml".to_string(),
            "[terminal]\nvt = 1\n[default_session]\ncommand = \"tuigreet --time --remember --sessions /etc/greetd/genoa-sessions\"\nuser = \"greeter\"".to_string());
        let result = configure_greetd(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            2,
            "Expected 2 commands to execute and no modifications to be performed"
        );
        assert!(
            log[0].0 == "sudo"
                && log[0].1.starts_with(&[
                    "systemctl".to_string(),
                    "disable".to_string(),
                    "gdm".to_string(),
                    "sddm".to_string(),
                    "lightdm".to_string()
                ])
        );
        assert!(
            log[1].0 == "sudo"
                && log[1].1.starts_with(&[
                    "systemctl".to_string(),
                    "enable".to_string(),
                    "--force".to_string(),
                    "greetd.service".to_string()
                ])
        );
    }
    #[test]
    fn test_config_greetd_update_path() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/greetd/config.toml".to_string(),
            "other data".to_string(),
        );
        let result = configure_greetd(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            3,
            "Expected 3 commands to execute and modifications to be performed"
        );
        assert!(
            log[0].0 == "sudo"
                && log[0].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string()
                ])
        );
        assert!(
            log[1].0 == "sudo"
                && log[1].1.starts_with(&[
                    "systemctl".to_string(),
                    "disable".to_string(),
                    "gdm".to_string(),
                    "sddm".to_string(),
                    "lightdm".to_string()
                ])
        );
        assert!(
            log[2].0 == "sudo"
                && log[2].1.starts_with(&[
                    "systemctl".to_string(),
                    "enable".to_string(),
                    "--force".to_string(),
                    "greetd.service".to_string()
                ])
        );
        let binding = env.mock_files.borrow();
        let updated = binding.get("/etc/greetd/config.toml").unwrap();
        assert!(updated.contains("/etc/greetd/genoa-sessions"));
    }
    #[test]
    fn test_config_system_env_setup() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/dnscrypt-proxy/dnscrypt-proxy.toml".to_string(),
            "\nserver_names = ['cloudflare']\nlisten_addresses = ['127.0.0.1:53', '[::1]:53']"
                .to_string(),
        );
        env.mock_files.borrow_mut().insert(
            "/etc/systemd/logind.conf".to_string(),
            "\n[Login]\n# Some other config\n".to_string(),
        );
        let result = configure_system(&env, std::path::Path::new("/home/testuser"));
        let path = "/home/testuser/.config/environment.d/99-cargo-path.conf".to_string();
        assert!(result.is_ok());
        assert_eq!(
            env.mock_files.borrow().get(&path).unwrap(),
            "PATH=$HOME/.cargo/bin:$PATH\n"
        );
    }

    #[test]
    fn test_enable_mdns_hostname_resolution_inserts_before_system_resolvers() {
        let original = "hosts: mymachines resolve [!UNAVAIL=return] files myhostname dns\n";

        assert_eq!(
            enable_mdns_hostname_resolution(original),
            Some(
                "hosts: mymachines mdns_minimal [NOTFOUND=return] resolve [!UNAVAIL=return] files myhostname dns\n"
                    .to_string()
            )
        );
    }

    #[test]
    fn test_enable_mdns_hostname_resolution_leaves_existing_mdns_config_unchanged() {
        let original = "hosts: mymachines mdns4_minimal [NOTFOUND=return] resolve dns\n";

        assert_eq!(enable_mdns_hostname_resolution(original), None);
    }

    #[test]
    fn test_add_quiet_boot_parameters_is_idempotent_and_preserves_existing_loglevel() {
        assert_eq!(
            add_quiet_boot_parameters("root=UUID=example rw\n"),
            Some("root=UUID=example rw quiet loglevel=3\n".to_string())
        );
        assert_eq!(
            add_quiet_boot_parameters("root=UUID=example quiet loglevel=7\n"),
            None
        );
    }

    #[test]
    fn test_configure_quiet_boot_updates_kernel_cmdline_once() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/kernel/cmdline".to_string(),
            "root=UUID=example rw\n".to_string(),
        );

        configure_quiet_boot(&env).expect("quiet boot configuration should succeed");

        assert_eq!(
            env.mock_files.borrow().get("/etc/kernel/cmdline"),
            Some(&"root=UUID=example rw quiet loglevel=3\n".to_string())
        );
        assert_eq!(env.cmd_log.borrow().len(), 1);

        configure_quiet_boot(&env).expect("quiet boot configuration should be idempotent");
        assert_eq!(env.cmd_log.borrow().len(), 1);
    }

    #[test]
    fn test_configure_quiet_boot_updates_and_regenerates_grub() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/default/grub".to_string(),
            "GRUB_CMDLINE_LINUX_DEFAULT=\"root=UUID=example rw\"\n".to_string(),
        );
        env.mock_files
            .borrow_mut()
            .insert("/boot/grub/grub.cfg".to_string(), String::new());

        configure_quiet_boot(&env).expect("GRUB quiet boot configuration should succeed");

        assert_eq!(
            env.mock_files.borrow().get("/etc/default/grub"),
            Some(
                &"GRUB_CMDLINE_LINUX_DEFAULT=\"root=UUID=example rw quiet loglevel=3\"\n"
                    .to_string()
            )
        );
        assert_eq!(env.cmd_log.borrow().len(), 2);
        assert_eq!(
            env.cmd_log.borrow()[1],
            (
                "sudo".to_string(),
                vec![
                    "grub-mkconfig".to_string(),
                    "-o".to_string(),
                    "/boot/grub/grub.cfg".to_string(),
                ],
            )
        );
    }

    #[test]
    fn test_configure_printing_services_enables_resolver_cups_and_avahi() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/nsswitch.conf".to_string(),
            "hosts: mymachines resolve [!UNAVAIL=return] files myhostname dns\n".to_string(),
        );

        configure_printing_services(&env).expect("printing services should be enabled");

        assert_eq!(
            env.mock_files.borrow().get("/etc/nsswitch.conf"),
            Some(
                &"hosts: mymachines mdns_minimal [NOTFOUND=return] resolve [!UNAVAIL=return] files myhostname dns\n"
                    .to_string()
            )
        );

        assert_eq!(
            *env.cmd_log.borrow(),
            vec![
                (
                    "sudo".to_string(),
                    vec![
                        "pacman".to_string(),
                        "-S".to_string(),
                        "--needed".to_string(),
                        "--noconfirm".to_string(),
                        "nss-mdns".to_string(),
                    ],
                ),
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
                        "/etc/nsswitch.conf".to_string(),
                    ],
                ),
                (
                    "sudo".to_string(),
                    vec![
                        "systemctl".to_string(),
                        "enable".to_string(),
                        "--now".to_string(),
                        "cups.service".to_string(),
                    ],
                ),
                (
                    "sudo".to_string(),
                    vec![
                        "systemctl".to_string(),
                        "enable".to_string(),
                        "--now".to_string(),
                        "avahi-daemon.service".to_string(),
                    ],
                ),
            ]
        );
    }

    #[test]
    fn test_dns_config_partial_update() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/dnscrypt-proxy/dnscrypt-proxy.toml".to_string(),
            "\nserver_names = ['cloudflare']\nlisten_addresses = ['8.8.8.8:53']\n".to_string(),
        );
        let result = configure_dns(&env);
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            6,
            "Expected exactly 6 commands to be run for DNS configuration"
        );
        assert_eq!(
            log[0],
            (
                "sudo".to_string(),
                vec![
                    "pacman".to_string(),
                    "-S".to_string(),
                    "--needed".to_string(),
                    "--noconfirm".to_string(),
                    "dnscrypt-proxy".to_string()
                ]
            )
        );
        assert!(
            log[1].0 == "sudo"
                && log[1].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string()
                ])
        );
        assert_eq!(
            log[2],
            (
                "sudo".to_string(),
                vec![
                    "systemctl".to_string(),
                    "enable".to_string(),
                    "--now".to_string(),
                    "dnscrypt-proxy".to_string()
                ]
            )
        );
        assert_eq!(
            log[3],
            (
                "sudo".to_string(),
                vec![
                    "systemctl".to_string(),
                    "disable".to_string(),
                    "--now".to_string(),
                    "cloudflared-dns".to_string()
                ]
            )
        );
        assert_eq!(
            log[4],
            (
                "sudo".to_string(),
                vec![
                    "rm".to_string(),
                    "-f".to_string(),
                    "/etc/systemd/system/cloudflared-dns.service".to_string()
                ]
            )
        );
        assert_eq!(
            log[5],
            (
                "sudo".to_string(),
                vec!["systemctl".to_string(), "daemon-reload".to_string()]
            )
        );
        let binding = env.mock_files.borrow();
        let updated_file = binding
            .get("/etc/dnscrypt-proxy/dnscrypt-proxy.toml")
            .unwrap();
        assert_eq!(
            updated_file,
            "server_names = ['cloudflare']\nlisten_addresses = ['127.0.0.1:53', '[::1]:53']\n"
        );
    }

    #[test]
    fn test_dns_config_missing_lines_appended() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/etc/dnscrypt-proxy/dnscrypt-proxy.toml".to_string(),
            "\n# dnscrypt-proxy config\n".to_string(),
        );
        let result = configure_dns(&env);
        assert!(result.is_ok());

        let binding = env.mock_files.borrow();
        let updated_file = binding
            .get("/etc/dnscrypt-proxy/dnscrypt-proxy.toml")
            .unwrap();
        assert!(updated_file.contains("server_names = ['cloudflare']"));
        assert!(updated_file.contains("listen_addresses = ['127.0.0.1:53', '[::1]:53']"));
    }
    #[test]
    fn test_enforce_session_order() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/usr/share/wayland-sessions/10-niri.desktop".to_string(),
            "Name=Niri\nExec=/usr/bin/niri\n".to_string(),
        );
        env.mock_files.borrow_mut().insert(
            "/usr/share/wayland-sessions/50-sway.desktop".to_string(),
            "Name=Sway\nExec=/usr/bin/sway\n".to_string(),
        );
        let result = enforce_session_order(&env, true, std::path::Path::new("/repo-root"));
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            6,
            "Expected exactly 6 commands to be run for session order enforcement"
        );
        assert!(
            log[0].0 == "sudo"
                && log[0].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "755".to_string(),
                    "-o".to_string(),
                    "root".to_string(),
                    "-g".to_string(),
                    "root".to_string(),
                    "/repo-root/scripts/session-launch.sh".to_string(),
                    "/usr/local/bin/genoa-proxy".to_string()
                ])
        );
        assert!(
            log[1].0 == "sudo"
                && log[1].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string(),
                    "-o".to_string(),
                    "root".to_string(),
                    "-g".to_string(),
                    "root".to_string()
                ])
                && log[1].1.last()
                    == Some(&"/etc/greetd/genoa-sessions/10-niri.desktop".to_string())
        );
        assert!(
            log[2].0 == "sudo"
                && log[2].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string(),
                    "-o".to_string(),
                    "root".to_string(),
                    "-g".to_string(),
                    "root".to_string()
                ])
                && log[2].1.last()
                    == Some(&"/etc/greetd/genoa-sessions/20-sway.desktop".to_string())
        );
    }

    #[test]
    fn test_enforce_session_order_no_changes_still_configures_greetd() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/usr/share/wayland-sessions/10-niri.desktop".to_string(),
            "Name=Niri\nExec=/usr/bin/niri\n".to_string(),
        );
        env.mock_files.borrow_mut().insert(
            "/usr/share/wayland-sessions/50-sway.desktop".to_string(),
            "Name=Sway\nExec=/usr/bin/sway\n".to_string(),
        );
        env.mock_files.borrow_mut().insert(
            "/etc/greetd/genoa-sessions/10-niri.desktop".to_string(),
            "Name=1. Niri\nExec=/usr/local/bin/genoa-proxy /usr/share/wayland-sessions/10-niri.desktop"
                .to_string(),
        );
        env.mock_files.borrow_mut().insert(
            "/etc/greetd/genoa-sessions/20-sway.desktop".to_string(),
            "Name=2. Sway (Battery)\nExec=/usr/local/bin/sway-hybrid".to_string(),
        );
        env.mock_files.borrow_mut().insert(
            "/etc/greetd/config.toml".to_string(),
            "[terminal]\nvt = 1\n[default_session]\ncommand = \"tuigreet --time --remember --sessions /etc/greetd/genoa-sessions\"\nuser = \"greeter\""
                .to_string(),
        );

        let result = enforce_session_order(&env, true, std::path::Path::new("/repo-root"));
        let log = env.cmd_log.borrow();

        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            3,
            "Expected script install plus greetd service commands when sessions are already current"
        );
        assert_eq!(
            log[1],
            (
                "sudo".to_string(),
                vec![
                    "systemctl".to_string(),
                    "disable".to_string(),
                    "gdm".to_string(),
                    "sddm".to_string(),
                    "lightdm".to_string()
                ]
            )
        );
        assert_eq!(
            log[2],
            (
                "sudo".to_string(),
                vec![
                    "systemctl".to_string(),
                    "enable".to_string(),
                    "--force".to_string(),
                    "greetd.service".to_string()
                ]
            )
        );
    }
    #[test]
    fn test_configure_tlp() {
        let env = MockEnv::default();
        env.mock_files
            .borrow_mut()
            .insert("/repo-root/tlp.conf".to_string(), "new config".to_string());
        env.mock_files
            .borrow_mut()
            .insert("/etc/tlp.conf".to_string(), "old config".to_string());
        let result = configure_tlp(&env, std::path::Path::new("/repo-root"));
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            4,
            "Expected exactly 4 commands to be run for TLP configuration"
        );
        assert!(
            log[0].0 == "sudo"
                && log[0].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string(),
                    "-o".to_string(),
                    "root".to_string(),
                    "-g".to_string(),
                    "root".to_string(),
                    "/tmp/mock_file".to_string(),
                    "/etc/tlp.conf".to_string()
                ])
        );
        assert!(
            log[1].0 == "sudo"
                && log[1].1.starts_with(&[
                    "systemctl".to_string(),
                    "enable".to_string(),
                    "tlp.service".to_string()
                ])
        );
        assert!(
            log[2].0 == "systemctl"
                && log[2].1.starts_with(&[
                    "is-active".to_string(),
                    "--quiet".to_string(),
                    "tlp.service".to_string()
                ])
        );
        assert!(
            log[3].0 == "sudo"
                && log[3].1.starts_with(&[
                    "systemctl".to_string(),
                    "restart".to_string(),
                    "tlp.service".to_string()
                ])
        );
        let binding = env.mock_files.borrow();
        let updated = binding.get("/etc/tlp.conf").unwrap();
        assert_eq!(updated, "new config");
    }

    #[test]
    fn test_configure_tlp_symlink_cleanup() {
        let env = MockEnv::default();
        env.mock_files
            .borrow_mut()
            .insert("/repo-root/tlp.conf".to_string(), "new config".to_string());
        env.mock_files.borrow_mut().insert(
            "/etc/tlp.conf".to_string(),
            "legacy link target contents".to_string(),
        );
        env.symlink_paths.borrow_mut().insert(
            "/etc/tlp.conf".to_string(),
            "mock_symlink_target".to_string(),
        );

        let result = configure_tlp(&env, std::path::Path::new("/repo-root"));
        let log = env.cmd_log.borrow();
        assert!(result.is_ok());
        assert_eq!(
            log.len(),
            5,
            "Expected symlink cleanup plus install and service commands"
        );
        assert_eq!(
            log[0],
            (
                "sudo".to_string(),
                vec![
                    "rm".to_string(),
                    "-f".to_string(),
                    "/etc/tlp.conf".to_string()
                ]
            )
        );
        assert!(
            log[1].0 == "sudo"
                && log[1].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "644".to_string(),
                    "-o".to_string(),
                    "root".to_string(),
                    "-g".to_string(),
                    "root".to_string(),
                    "/tmp/mock_file".to_string(),
                    "/etc/tlp.conf".to_string()
                ])
        );
        let binding = env.mock_files.borrow();
        let updated = binding.get("/etc/tlp.conf").unwrap();
        assert_eq!(updated, "new config");
    }

    #[test]
    fn test_enforce_session_order_missing_sessions_skips_greetd() {
        let env = MockEnv::default();
        let result = enforce_session_order(&env, false, std::path::Path::new("/repo-root"));
        assert!(result.is_ok());

        let log = env.cmd_log.borrow();
        assert_eq!(log.len(), 1, "Expected only proxy script install");
        assert!(
            log[0].0 == "sudo"
                && log[0].1.starts_with(&[
                    "install".to_string(),
                    "-m".to_string(),
                    "755".to_string(),
                    "-o".to_string(),
                    "root".to_string(),
                    "-g".to_string(),
                    "root".to_string(),
                    "/repo-root/scripts/session-launch.sh".to_string(),
                    "/usr/local/bin/genoa-proxy".to_string()
                ])
        );
    }

    #[test]
    fn test_enforce_session_order_non_nvidia_exec_lines() {
        let env = MockEnv::default();
        env.mock_files.borrow_mut().insert(
            "/usr/share/wayland-sessions/10-niri.desktop".to_string(),
            "Name=Niri\nExec=/usr/bin/niri\n".to_string(),
        );
        env.mock_files.borrow_mut().insert(
            "/usr/share/wayland-sessions/50-sway.desktop".to_string(),
            "Name=Sway\nExec=/usr/bin/sway\n".to_string(),
        );

        let result = enforce_session_order(&env, false, std::path::Path::new("/repo-root"));
        assert!(result.is_ok());

        let binding = env.mock_files.borrow();
        let niri_session = binding
            .get("/etc/greetd/genoa-sessions/10-niri.desktop")
            .unwrap();
        let sway_session = binding
            .get("/etc/greetd/genoa-sessions/20-sway.desktop")
            .unwrap();

        assert!(niri_session.contains("Name=1. Niri"));
        assert!(niri_session.contains(
            "Exec=/usr/local/bin/genoa-proxy /usr/share/wayland-sessions/10-niri.desktop"
        ));
        assert!(sway_session.contains("Name=2. Sway (Battery)"));
        assert!(sway_session.contains(
            "Exec=/usr/local/bin/genoa-proxy /usr/share/wayland-sessions/50-sway.desktop"
        ));
    }
}
