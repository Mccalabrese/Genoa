//! System update and signed Genoa release updater.
//!
//! The user's `~/Genoa` checkout is a customization workspace. This binary
//! never mutates it. Managed code is staged under XDG data storage from a
//! signed release tag, while user dotfiles and local package choices remain
//! outside the release checkout.

mod release;

use anyhow::{Context, Result, bail};
use notify_rust::{Notification, Urgency};
use release::{
    RELEASE_SIGNER_FINGERPRINT, ReleaseConfig, initialize_pinned_release_trust,
    release_trust_is_initialized, stage_latest_release,
};
use serde::Deserialize;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const LOGO: &str = concat!(
    "\n",
    "\"++++++++++\n",
    "     ++++++++++++++\n",
    "    ++++++++++++++++\n",
    "   ++++++++++++++++++\n",
    "  ++++++++++++++++++++\n",
    " +++++++++====+++++++++\n",
    " ++++++=:......:=++++++\n",
    " +++++=:..........:=+++++\n",
    " ++++=..............=++++\n",
    " +++=.=##=......=##-.=+++\n",
    "++++:-%%-.-....-%%:.-:++++\n",
    "+++=.*%%. *....#%%..*.=+++\n",
    "+++-.#%%#*%....%%%###.-+++\n",
    "+++-.#%%%%#....#%%%%#.-+++\n",
    "+++-.+%%%%*....*%%%%+.-+++\n",
    " ++=.:#%%#:....:#%%#:.=++\n",
    " +++..:=+:......:+=:..+++\n",
    "++++-................-++++\n",
    "+++++:..............:+++++\n",
);
// Keep every command launched by the updater on Arch's system path. This is
// set before any child process (including a later sudo prompt) is started, so
// a user-writable directory such as ~/.local/bin cannot shadow an executable.
const TRUSTED_SYSTEM_PATH: &str = "/usr/bin:/bin";
const SYSTEM_RUSTUP: &str = "/usr/bin/rustup";

fn restrict_command_path() {
    // SAFETY: this runs at process entry, before this program creates threads
    // or launches a child process. Rust 2024 marks environment mutation unsafe
    // because concurrent mutation is unsound.
    unsafe { std::env::set_var("PATH", TRUSTED_SYSTEM_PATH) };
}

fn expand_path(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    PathBuf::from(path)
}

#[derive(Deserialize, Debug)]
struct Global {
    terminal: String,
}

#[derive(Deserialize, Debug)]
struct UpdaterConfig {
    update_command: Vec<String>,
    icon_success: String,
    icon_error: String,
    window_title: String,
}

#[derive(Deserialize, Debug)]
struct RepoConfig {
    root: String,
}

#[derive(Deserialize, Debug)]
struct ReleaseUpdatesConfig {
    #[serde(default = "default_release_updates_enabled")]
    enabled: bool,
    #[serde(default = "default_tag_prefix")]
    tag_prefix: String,
}

impl Default for ReleaseUpdatesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tag_prefix: default_tag_prefix(),
        }
    }
}

fn default_release_updates_enabled() -> bool {
    true
}

fn default_tag_prefix() -> String {
    "genoa-v".to_string()
}

#[derive(Deserialize, Debug)]
struct GlobalConfig {
    global: Global,
    updater: UpdaterConfig,
    repo: Option<RepoConfig>,
    #[serde(default)]
    release_updates: ReleaseUpdatesConfig,
}

fn load_config() -> Result<GlobalConfig> {
    let config_path = dirs::home_dir()
        .context("Cannot find home dir")?
        .join(".config/rust-dotfiles/config.toml");
    let config_str = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config: {}", config_path.display()))?;
    toml::from_str(&config_str).context("Failed to parse config.toml")
}

fn resolve_workspace_path(repo_cfg: Option<&RepoConfig>) -> Option<PathBuf> {
    if let Some(repo_cfg) = repo_cfg {
        let configured_path = expand_path(&repo_cfg.root);
        if configured_path.exists() {
            return Some(configured_path);
        }
    }

    let home = dirs::home_dir()?;
    [home.join("Genoa"), home.join("rust-wayland-power")]
        .into_iter()
        .find(|path| path.exists())
}

fn check_dependency(cmd: &str) -> bool {
    Command::new(cmd)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn send_notification(summary: &str, body: &str, icon: &Path, urgency: Urgency) -> Result<()> {
    Notification::new()
        .summary(summary)
        .body(body)
        .icon(icon.to_str().unwrap_or_default())
        .urgency(urgency)
        .show()
        .context("Failed to send desktop notification")?;
    Ok(())
}

fn run_update_command(command: &[String]) -> Result<()> {
    let (program, args) = command
        .split_first()
        .context("updater.update_command may not be empty")?;
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("Failed to run update command {program}"))?;
    if !status.success() {
        bail!("System package update failed with {status}");
    }
    Ok(())
}

fn prompt_yes_no(question: &str) -> bool {
    print!("{question} [y/N] ");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .is_ok_and(|_| matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

fn initialize_release_trust_interactively() -> Result<()> {
    if release_trust_is_initialized()? {
        println!("✅ Signed-release verification is already initialized.");
        return Ok(());
    }

    println!("\n🔐 Genoa is moving to signed runtime releases.");
    println!("   This updater contains the pinned release signing fingerprint:");
    println!("   {RELEASE_SIGNER_FINGERPRINT}");
    println!(
        "   Verify that fingerprint through an independent Genoa announcement before continuing."
    );
    println!(
        "   It will initialize a private Genoa verification keyring under ~/.local/share/genoa."
    );
    println!("   No key will be downloaded, and your personal GPG keyring will not be changed.");
    if !prompt_yes_no("Initialize signed-release verification now?") {
        println!("   ℹ️  Signed-release verification was not initialized.");
        return Ok(());
    }
    initialize_pinned_release_trust()?;
    println!(
        "   ✅ Signed-release verification initialized. Future runtime updates require a verified tag."
    );
    Ok(())
}

fn offer_firmware_update() -> Result<()> {
    if !check_dependency("/usr/bin/fwupdmgr") {
        println!("   ℹ️  fwupdmgr is not installed; skipping firmware check.");
        return Ok(());
    }

    println!("\n🔌 Checking firmware updates...");
    let refresh = Command::new("/usr/bin/sudo")
        .args(["/usr/bin/fwupdmgr", "refresh"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !firmware_refresh_succeeded(&refresh) {
        bail!("Firmware metadata refresh failed with {refresh}");
    }
    let updates = Command::new("/usr/bin/fwupdmgr")
        .arg("get-updates")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !firmware_updates_available(&updates) {
        return Ok(());
    }
    if prompt_yes_no("Firmware updates are available. Apply them now?") {
        let status = Command::new("/usr/bin/sudo")
            .args(["/usr/bin/fwupdmgr", "update"])
            .status()?;
        if !status.success() {
            bail!("Firmware update failed with {status}");
        }
    }
    Ok(())
}

fn firmware_refresh_succeeded(status: &std::process::ExitStatus) -> bool {
    // fwupdmgr reserves exit status 2 for a command which had no action to
    // take but still completed successfully, e.g. already-current metadata.
    status.success() || status.code() == Some(2)
}

fn firmware_updates_available(status: &std::process::ExitStatus) -> bool {
    // fwupdmgr uses status 2 for an otherwise successful check with no
    // available updates. Only its normal success status means there is an
    // update worth asking the user to apply.
    status.success()
}

fn refresh_from_release(release_root: &Path) -> Result<()> {
    let installer_dir = release_root.join("sysScripts/install-wizard");
    let manifest = installer_dir.join("Cargo.toml");
    if !manifest.exists() {
        bail!(
            "Verified release does not contain install-wizard at {}",
            installer_dir.display()
        );
    }

    println!("\n🦀 Building verified Genoa release...");
    let build = Command::new(SYSTEM_RUSTUP)
        .args([
            "run",
            "stable",
            "cargo",
            "build",
            "--locked",
            "--release",
            "-q",
        ])
        .current_dir(&installer_dir)
        .status()
        .context("Failed to build release installer")?;
    if !build.success() {
        bail!("Building the verified release installer failed with {build}");
    }

    let installer = installer_dir.join("target/release/install-wizard");
    println!("🔄 Refreshing managed runtime components...");
    let status = Command::new(&installer)
        .args(["--refresh-configs", "--release-root"])
        .env("REPO_ROOT", release_root)
        .status()
        .with_context(|| format!("Failed to run {}", installer.display()))?;
    if !status.success() {
        bail!("Verified release refresh failed with {status}");
    }
    Ok(())
}

fn update_release(workspace: Option<&Path>, config: &ReleaseUpdatesConfig) -> Result<()> {
    if !config.enabled {
        println!("\nℹ️  Genoa release updates are disabled; package update completed.");
        return Ok(());
    }
    let Some(workspace) = workspace else {
        bail!("Cannot stage a Genoa release because no existing Genoa workspace was found");
    };
    if !release_trust_is_initialized()? {
        initialize_release_trust_interactively()?;
        if !release_trust_is_initialized()? {
            return Ok(());
        }
    }
    let release_config = ReleaseConfig {
        enabled: config.enabled,
        tag_prefix: config.tag_prefix.clone(),
    };
    let Some(release) = stage_latest_release(workspace, &release_config)? else {
        println!("\nℹ️  Genoa release updates are disabled; package update completed.");
        return Ok(());
    };
    if release.is_active()? {
        return Ok(());
    }

    println!("\n✨ Verified Genoa release {}", release.tag);
    if !prompt_yes_no("Install this verified release without touching your Genoa workspace?") {
        println!("   ℹ️  Release installation skipped by user.");
        return Ok(());
    }
    refresh_from_release(&release.path)?;
    release.activate()?;
    println!("   ✅ Active Genoa runtime is now {}.", release.tag);
    Ok(())
}

fn run_worker(config: &GlobalConfig) -> Result<()> {
    println!("{LOGO}");
    println!("🚀 Starting system package update...");
    run_update_command(&config.updater.update_command)?;
    offer_firmware_update()?;
    let workspace = resolve_workspace_path(config.repo.as_ref());
    update_release(workspace.as_deref(), &config.release_updates)
}

fn run_launcher(config: &GlobalConfig) -> Result<()> {
    let update_bin = config
        .updater
        .update_command
        .first()
        .context("updater.update_command may not be empty")?;
    if !check_dependency(&config.global.terminal) {
        bail!("Terminal not found: {}", config.global.terminal);
    }
    if !check_dependency(update_bin) {
        bail!("Update helper not found: {update_bin}");
    }

    let current_exe = std::env::current_exe().context("Failed to locate sys-update")?;
    let status = Command::new(&config.global.terminal)
        .arg(format!("--title={}", config.updater.window_title))
        .arg("-e")
        .arg(current_exe)
        .arg("--worker")
        .arg("--pause-on-exit")
        .status()
        .with_context(|| format!("Failed to launch {}", config.global.terminal))?;

    let success_icon = expand_path(&config.updater.icon_success);
    let error_icon = expand_path(&config.updater.icon_error);
    if status.success() {
        send_notification(
            "System Update Complete",
            "Packages and selected Genoa release updates completed.",
            &success_icon,
            Urgency::Low,
        )
    } else {
        send_notification(
            "System Update Failed",
            "The update process encountered an error. Your Genoa workspace was not modified.",
            &error_icon,
            Urgency::Critical,
        )
    }
}

fn main() -> Result<()> {
    restrict_command_path();
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--initialize-release-trust")
    {
        return initialize_release_trust_interactively();
    }
    let config = load_config()?;
    let args: Vec<String> = std::env::args().collect();
    if args.iter().skip(1).any(|arg| arg == "--worker") {
        let result = run_worker(&config);
        if args.iter().any(|arg| arg == "--pause-on-exit") {
            match &result {
                Ok(()) => println!("\n🏁 Process finished successfully. Closing in 5s..."),
                Err(error) => eprintln!("\n❌ Process failed:\n{error:#}\n\nClosing in 5s..."),
            }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
        result
    } else {
        run_launcher(&config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn workspace_resolution_prefers_the_configured_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let configured = temp.path().join("workspace");
        fs::create_dir(&configured).unwrap();
        let resolved = resolve_workspace_path(Some(&RepoConfig {
            root: configured.to_string_lossy().to_string(),
        }));
        assert_eq!(resolved.as_deref(), Some(configured.as_path()));
    }

    #[test]
    fn firmware_refresh_accepts_the_documented_no_action_status() {
        assert!(firmware_refresh_succeeded(
            &std::process::ExitStatus::from_raw(0)
        ));
        assert!(firmware_refresh_succeeded(
            &std::process::ExitStatus::from_raw(2 << 8)
        ));
        assert!(!firmware_refresh_succeeded(
            &std::process::ExitStatus::from_raw(1 << 8)
        ));
    }

    #[test]
    fn firmware_prompt_is_reserved_for_real_updates() {
        assert!(firmware_updates_available(
            &std::process::ExitStatus::from_raw(0)
        ));
        assert!(!firmware_updates_available(
            &std::process::ExitStatus::from_raw(2 << 8)
        ));
        assert!(!firmware_updates_available(
            &std::process::ExitStatus::from_raw(1 << 8)
        ));
    }
}
