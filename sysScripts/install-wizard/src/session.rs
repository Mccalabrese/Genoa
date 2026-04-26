use crate::CmdExecutor;
use std::path::Path;

/// Configures essential system services and settings, including mkinitcpio sanitation, enabling
/// geoclue/bluetooth/bolt, enabling Pacman cache cleanup, and
/// configuring logind. This function is idempotent and can be safely run multiple times
/// without causing issues.
pub fn configure_system(sys: &impl CmdExecutor, home: &Path) -> Result<(), std::io::Error> {
    sanitize_mkinitcpio(sys)?;
    sys.run_cmd("sudo", &["systemctl", "enable", "geoclue.service"])?;
    sys.run_cmd("sudo", &["systemctl", "enable", "bluetooth.service"])?;
    sys.run_cmd("sudo", &["systemctl", "enable", "bolt.service"])?;
    configure_dns(sys)?;
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

/// Cleans up the `mkinitcpio.conf` file to fix the known Archinstall 2025 bug that appends 'o"' to
/// the end of the file,
fn sanitize_mkinitcpio(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    // --- SANITIZE MKINITCPIO (Fix Archinstall 2025 Bug) ---
    // This protects NVIDIA users from the 'o"' corruption crash.
    println!("   🧹 Checking mkinitcpio.conf for corruption...");
    let mkinit_path = "/etc/mkinitcpio.conf";

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

///Configures dnscrypt-proxy to use Cloudflare's DNS servers for enhanced privacy and security.
fn configure_dns(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    // --- DNS Crypt Proxy CONFIGURATION ---
    println!("   🔧 Configuring dnscrypt-proxy (DNS Proxy)...");

    // 1. Ensure package is installed (failsafe)
    sys.run_cmd(
        "sudo",
        &["pacman", "-S", "--needed", "--noconfirm", "dnscrypt-proxy"],
    )?;
    // 2. Configure TOML to use Cloudflare
    let dns_conf = "/etc/dnscrypt-proxy/dnscrypt-proxy.toml";
    let content = sys.read_file_to_string(dns_conf)?;
    let mut modified = false;
    let mut found_names = Vec::new();
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
    for line in &mut lines {
        let normalized = line.trim_start().trim_start_matches('#').trim_start();
        if normalized.starts_with("server_names =") {
            found_names.push("server_names".to_string());
            if line == "server_names = ['cloudflare']" {
                continue; // Already correct
            }
            *line = "server_names = ['cloudflare']".to_string();
            modified = true;
        } else if normalized.starts_with("listen_addresses =") {
            found_names.push("listen_addresses".to_string());
            if line == "listen_addresses = ['127.0.0.1:53', '[::1]:53']" {
                continue; // Already correct
            }
            *line = "listen_addresses = ['127.0.0.1:53', '[::1]:53']".to_string();
            modified = true;
        }
    }
    if !found_names.contains(&"server_names".to_string()) {
        lines.push("server_names = ['cloudflare']".to_string());
        modified = true;
    }
    if !found_names.contains(&"listen_addresses".to_string()) {
        lines.push("listen_addresses = ['127.0.0.1:53', '[::1]:53']".to_string());
        modified = true;
    }
    if modified {
        let new_content = lines.join("\n") + "\n";
        sys.install_string_to_root_file(dns_conf, new_content.as_str(), "644")?;
    }
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
    let logind_conf = "/etc/systemd/logind.conf";
    let content = match sys.read_file_to_string(logind_conf) {
        Ok(content) => content,
        Err(e) => {
            eprintln!("   ⚠️  Failed to read logind.conf: {}", e);
            return Err(e);
        }
    };
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
        let new_content = lines.join("\n") + "\n";
        sys.install_string_to_root_file(logind_conf, new_content.as_str(), "644")?;
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
    let mut session_flag = false;

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
        let source_name = match session_files.iter().find(|name| name.contains(expected_name)) {
            Some(name) => name,
            None => {
                println!(
                    "   ⚠️  Warning: Expected session containing '{}' not found. Skipping.",
                    expected_name
                );
                continue;
            }
        };

        let std_path = format!("{}/{}", sessions_dir, source_name);
        let custom_path = format!("{}/{}", proxy_dir, custom_name);
        let content = match sys.read_file_to_string(&std_path) {
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
        sys.install_string_to_root_file(&custom_path, &new_content, "644")?;
        session_flag = true;
    }
    if session_flag {
        configure_greetd(sys)?;
    } else {
        println!("   ⚠️  No session files were updated. Skipping Greetd configuration.");
    }
    Ok(())
}

/// Configures Greetd with a custom tuigreet session and disables other DMs.
fn configure_greetd(sys: &impl CmdExecutor) -> Result<(), std::io::Error> {
    println!("    🔧 Configuring Greetd...");
    let greetd_path = "/etc/greetd/config.toml";
    let existing_content = sys.read_file_to_string(greetd_path).unwrap_or_default();
    let greetd_config = r#"
[terminal]
vt = 1
[default_session]
command = "tuigreet --time --remember --sessions /etc/greetd/genoa-sessions"
user = "greeter"
"#;
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
            "\nserver_names = cloudflare\nlisten_addresses = [127.0.0.1:53]\n".to_string(),
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
                .to_string(),
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
}
