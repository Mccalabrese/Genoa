//! Signed Genoa release staging.
//!
//! User checkouts are intentionally never updated here. They are a place for
//! personal dotfiles and experimentation. Release-owned code lives under the
//! XDG data directory and is activated only after its signed tag verifies.

use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct ReleaseConfig {
    pub enabled: bool,
    pub tag_prefix: String,
}

pub const RELEASE_SIGNER_FINGERPRINT: &str = "744CD469098D940E32A519F44E957FACF379B7A8";
// This copy intentionally lives beneath `sysScripts`: the legacy updater only
// refreshes that subtree, so it must be available while it rebuilds the first
// signed-release-aware updater on existing machines.
const RELEASE_PUBLIC_KEY: &str = include_str!("../assets/genoa-pubkey.asc");

#[derive(Debug, Clone)]
pub struct StagedRelease {
    pub tag: String,
    pub path: PathBuf,
    data_root: PathBuf,
}

/// Returns whether this user has already initialized the updater's isolated
/// release-verification keyring. No user-controlled GPG configuration is used.
pub fn release_trust_is_initialized() -> Result<bool> {
    let keyring = release_data_root()?.join("keyring");
    if !keyring.is_dir() {
        return Ok(false);
    }
    keyring_contains_pinned_key(&keyring)
}

/// Initializes the updater's isolated verification keyring from its embedded
/// public key. This never imports into the user's personal GPG keyring.
pub fn initialize_pinned_release_trust() -> Result<()> {
    let data_root = release_data_root()?;
    initialize_release_trust(&data_root).map(|_| ())
}

impl StagedRelease {
    /// Returns whether this verified checkout is already the active runtime.
    pub fn is_active(&self) -> Result<bool> {
        let current = self.data_root.join("current");
        if !current.exists() {
            return Ok(false);
        }
        let active = fs::canonicalize(&current)
            .with_context(|| format!("Failed to resolve {}", current.display()))?;
        let expected = fs::canonicalize(&self.path)
            .with_context(|| format!("Failed to resolve {}", self.path.display()))?;
        Ok(active == expected)
    }

    /// Makes this already-verified release the active runtime release.
    pub fn activate(&self) -> Result<()> {
        let current = self.data_root.join("current");
        let pending = self
            .data_root
            .join(format!(".current-{}", std::process::id()));
        if fs::symlink_metadata(&pending).is_ok() {
            fs::remove_file(&pending)
                .with_context(|| format!("Failed to clear {}", pending.display()))?;
        }
        symlink(&self.path, &pending).with_context(|| {
            format!(
                "Failed to create pending active-release link {}",
                pending.display()
            )
        })?;
        fs::rename(&pending, &current)
            .with_context(|| format!("Failed to activate release {}", self.tag))?;

        let state = self.data_root.join("active-release");
        let state_pending = self
            .data_root
            .join(format!(".active-release-{}", std::process::id()));
        if fs::symlink_metadata(&state_pending).is_ok() {
            fs::remove_file(&state_pending)
                .with_context(|| format!("Failed to clear {}", state_pending.display()))?;
        }
        fs::write(&state_pending, format!("{}\n", self.tag))
            .with_context(|| format!("Failed to write {}", state_pending.display()))?;
        fs::rename(&state_pending, &state)
            .with_context(|| format!("Failed to write {}", state.display()))?;
        Ok(())
    }
}

/// Fetches and verifies the newest compatible tag, then checks it out into a
/// release-owned location. This never changes the caller's repository.
pub fn stage_latest_release(
    source_repo: &Path,
    config: &ReleaseConfig,
) -> Result<Option<StagedRelease>> {
    if !config.enabled {
        return Ok(None);
    }
    validate_tag_prefix(&config.tag_prefix)?;

    if !source_repo.join(".git").exists() {
        bail!(
            "{} is not a Git checkout; cannot discover the Genoa release remote",
            source_repo.display()
        );
    }

    let origin = git_output(source_repo, ["remote", "get-url", "origin"])?;
    if origin.trim().is_empty() {
        bail!("The Genoa checkout has no origin URL");
    }

    let data_root = release_data_root()?;
    let keyring = initialize_release_trust(&data_root)?;
    let cache_repo = data_root.join("release-cache");
    let releases_dir = data_root.join("releases");
    fs::create_dir_all(&releases_dir)
        .with_context(|| format!("Failed to create {}", releases_dir.display()))?;

    if cache_repo.exists() {
        if !cache_repo.join(".git").exists() {
            bail!(
                "{} exists but is not a managed Genoa release cache",
                cache_repo.display()
            );
        }
        git_status(&cache_repo, ["remote", "set-url", "origin", origin.trim()])?;
    } else {
        let cache_path = path_arg(&cache_repo)?;
        git_status(
            source_repo,
            [
                "clone",
                "--no-checkout",
                "--filter=blob:none",
                origin.trim(),
                cache_path,
            ],
        )?;
    }

    git_status(&cache_repo, ["fetch", "--force", "--tags", "origin"])?;
    let tags = git_output(
        &cache_repo,
        [
            "tag",
            "--list",
            "--sort=-version:refname",
            &format!("{}*", config.tag_prefix),
        ],
    )?;
    let tag = tags
        .lines()
        .map(str::trim)
        .find(|candidate| is_valid_release_tag(candidate, &config.tag_prefix))
        .ok_or_else(|| {
            anyhow!(
                "No release tags matching '{}' were found",
                config.tag_prefix
            )
        })?
        .to_string();

    verify_tag(&cache_repo, &tag, &keyring)?;

    let release_path = releases_dir.join(&tag);
    if release_path.exists() {
        verify_existing_checkout(&cache_repo, &release_path, &tag)?;
    } else {
        let release_path_arg = path_arg(&release_path)?;
        git_status(
            &cache_repo,
            ["worktree", "add", "--detach", release_path_arg, &tag],
        )?;
    }

    Ok(Some(StagedRelease {
        tag,
        path: release_path,
        data_root,
    }))
}

fn verify_existing_checkout(cache_repo: &Path, release_path: &Path, tag: &str) -> Result<()> {
    if !release_path.join(".git").exists() {
        bail!(
            "{} exists but is not the managed checkout for {tag}",
            release_path.display()
        );
    }
    let expected = git_output(cache_repo, ["rev-parse", &format!("{}^{{commit}}", tag)])?;
    let actual = git_output(release_path, ["rev-parse", "HEAD"])?;
    if expected.trim() != actual.trim() {
        bail!(
            "Existing release checkout {} does not match verified tag {tag}",
            release_path.display()
        );
    }
    let status = git_output(release_path, ["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        bail!(
            "Existing release checkout {} has local changes; refusing to use it",
            release_path.display()
        );
    }
    Ok(())
}

fn verify_tag(cache_repo: &Path, tag: &str, keyring: &Path) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cache_repo)
        .args(["verify-tag", "--raw", tag])
        .env("GNUPGHOME", keyring)
        .output()
        .context("Failed to execute git verify-tag")?;
    let verification = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        bail!("Release tag {tag} did not verify: {}", verification.trim());
    }
    let signer_found = verification.lines().any(|line| {
        line.contains("VALIDSIG")
            && line.split_whitespace().any(|field| {
                normalize_field_fingerprint(field)
                    .is_some_and(|value| value == RELEASE_SIGNER_FINGERPRINT)
            })
    });
    if !signer_found {
        bail!(
            "Release tag {tag} verified, but not with pinned Genoa signing fingerprint {RELEASE_SIGNER_FINGERPRINT}"
        );
    }
    Ok(())
}

fn git_status<const N: usize>(repo: &Path, args: [&str; N]) -> Result<()> {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .context("Failed to execute git")?;
    if !status.success() {
        bail!("Git command failed in {}", repo.display());
    }
    Ok(())
}

fn git_output<const N: usize>(repo: &Path, args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .context("Failed to execute git")?;
    if !output.status.success() {
        bail!(
            "Git command failed in {}: {}",
            repo.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn path_arg(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("Path is not valid UTF-8: {}", path.display()))
}

fn release_data_root() -> Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .ok_or_else(|| anyhow!("Cannot determine XDG data directory"))?
        .join("genoa"))
}

fn initialize_release_trust(data_root: &Path) -> Result<PathBuf> {
    let keyring = data_root.join("keyring");
    fs::create_dir_all(&keyring)
        .with_context(|| format!("Failed to create {}", keyring.display()))?;
    fs::set_permissions(&keyring, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("Failed to secure {}", keyring.display()))?;

    if keyring_contains_pinned_key(&keyring)? {
        return Ok(keyring);
    }

    let mut child = Command::new("gpg")
        .args(["--homedir"])
        .arg(&keyring)
        .args(["--batch", "--import"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("GnuPG is required to initialize Genoa release verification")?;
    child
        .stdin
        .take()
        .context("Failed to open GnuPG input")?
        .write_all(RELEASE_PUBLIC_KEY.as_bytes())
        .context("Failed to import embedded Genoa release key")?;
    let status = child
        .wait()
        .context("Failed to wait for GnuPG key import")?;
    if !status.success() {
        bail!("GnuPG could not import the embedded Genoa release key: {status}");
    }
    if !keyring_contains_pinned_key(&keyring)? {
        bail!("Embedded Genoa release key did not contain the pinned fingerprint");
    }
    Ok(keyring)
}

fn keyring_contains_pinned_key(keyring: &Path) -> Result<bool> {
    let output = Command::new("gpg")
        .args(["--homedir"])
        .arg(keyring)
        .args([
            "--batch",
            "--with-colons",
            "--fingerprint",
            RELEASE_SIGNER_FINGERPRINT,
        ])
        .output()
        .context("GnuPG is required to inspect Genoa release verification")?;
    if !output.status.success() {
        return Ok(false);
    }
    Ok(String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        line.strip_prefix("fpr:::::::::")
            .and_then(|rest| rest.split(':').next())
            .is_some_and(|fingerprint| fingerprint == RELEASE_SIGNER_FINGERPRINT)
    }))
}

fn normalize_field_fingerprint(value: &str) -> Option<String> {
    let normalized = value.replace([' ', ':'], "").to_ascii_uppercase();
    (!normalized.is_empty() && normalized.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then_some(normalized)
}

fn validate_tag_prefix(prefix: &str) -> Result<()> {
    if prefix.is_empty()
        || !prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("tag_prefix may contain only letters, numbers, '.', '_' and '-'");
    }
    Ok(())
}

fn is_valid_release_tag(tag: &str, prefix: &str) -> bool {
    tag.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_tags_must_use_the_configured_safe_prefix() {
        assert!(is_valid_release_tag("genoa-v2.1.1", "genoa-v"));
        assert!(!is_valid_release_tag("v2.1.1", "genoa-v"));
        assert!(!is_valid_release_tag("genoa-v../oops", "genoa-v"));
    }

    #[test]
    fn embedded_key_contains_the_pinned_primary_fingerprint() {
        assert_eq!(RELEASE_SIGNER_FINGERPRINT.len(), 40);
        assert!(
            RELEASE_SIGNER_FINGERPRINT
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert!(RELEASE_PUBLIC_KEY.starts_with("-----BEGIN PGP PUBLIC KEY BLOCK-----"));
    }

    #[test]
    fn active_release_matches_its_verified_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("data");
        let release_path = data_root.join("releases/genoa-v0.3.0");
        fs::create_dir_all(&release_path).unwrap();
        symlink(&release_path, data_root.join("current")).unwrap();

        let release = StagedRelease {
            tag: "genoa-v0.3.0".to_string(),
            path: release_path,
            data_root,
        };
        assert!(release.is_active().unwrap());
    }
}
