use std::{
    env,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

pub const MIN_TIMEOUT_SECONDS: u32 = 30;
pub const MAX_TIMEOUT_SECONDS: u32 = 86_400;

/// The compositor whose first-run idle behavior should be preserved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdleProfile {
    /// Niri's managed idle-service defaults.
    Niri,
    /// Sway's managed idle-service defaults.
    Sway,
}

impl IdleProfile {
    /// Returns the former managed-service policy for this compositor.
    #[must_use]
    pub const fn defaults(self) -> IdleSettings {
        match self {
            Self::Niri => IdleSettings {
                dim_after_seconds: 180,
                lock_after_seconds: 600,
                display_off_after_seconds: 660,
                suspend_after_seconds: 1_200,
            },
            Self::Sway => IdleSettings {
                dim_after_seconds: 60,
                lock_after_seconds: 240,
                display_off_after_seconds: 300,
                suspend_after_seconds: 600,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct IdleSettings {
    pub dim_after_seconds: u32,
    pub lock_after_seconds: u32,
    pub display_off_after_seconds: u32,
    pub suspend_after_seconds: u32,
}

impl Default for IdleSettings {
    fn default() -> Self {
        IdleProfile::Niri.defaults()
    }
}

impl IdleSettings {
    fn load_saved() -> Result<Option<Self>, String> {
        let path = config_path()?;
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("could not read {}: {error}", path.display())),
        };
        let settings = toml::from_str::<Self>(&contents)
            .map_err(|error| format!("could not parse {}: {error}", path.display()))?;
        settings.validate()?;
        Ok(Some(settings))
    }

    /// Loads the saved policy, preserving `defaults` until its first save.
    ///
    /// An unreadable, malformed, or unsafe saved policy is replaced in memory
    /// with `defaults`; the accompanying warning is safe to show to the user.
    #[must_use]
    pub fn load_or_default(defaults: Self) -> (Self, Option<String>) {
        match Self::load_saved() {
            Ok(Some(settings)) => (settings, None),
            Ok(None) => (defaults, None),
            Err(error) => (defaults, Some(error)),
        }
    }

    /// Atomically persists a validated idle policy for the active user.
    ///
    /// # Errors
    ///
    /// Returns an error when validation, directory preparation, or atomic file
    /// replacement fails.
    pub fn save(&self) -> Result<(), String> {
        self.validate()?;
        let path = config_path()?;
        let directory = path
            .parent()
            .ok_or_else(|| "the idle settings path has no parent directory".to_string())?;
        fs::create_dir_all(directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("could not secure {}: {error}", directory.display()))?;

        let contents = toml::to_string_pretty(self)
            .map_err(|error| format!("could not encode idle settings: {error}"))?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)
            .map_err(|error| format!("could not create a temporary settings file: {error}"))?;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("could not secure the temporary settings file: {error}"))?;
        temporary
            .write_all(contents.as_bytes())
            .map_err(|error| format!("could not write idle settings: {error}"))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| format!("could not save idle settings: {error}"))?;
        temporary.persist(&path).map_err(|error| {
            format!(
                "could not replace {} with the new idle settings: {}",
                path.display(),
                error.error
            )
        })?;
        Ok(())
    }

    /// Verifies that every timeout is bounded and the lock sequence is ordered.
    ///
    /// # Errors
    ///
    /// Returns an error when a timeout is out of range or a later action could
    /// occur before its required predecessor.
    pub fn validate(&self) -> Result<(), String> {
        for (name, seconds) in [
            ("Dim display", self.dim_after_seconds),
            ("Lock session", self.lock_after_seconds),
            ("Turn displays off", self.display_off_after_seconds),
            ("Suspend", self.suspend_after_seconds),
        ] {
            if !(MIN_TIMEOUT_SECONDS..=MAX_TIMEOUT_SECONDS).contains(&seconds) {
                return Err(format!(
                    "{name} must be between {MIN_TIMEOUT_SECONDS} seconds and {MAX_TIMEOUT_SECONDS} seconds"
                ));
            }
        }
        if self.dim_after_seconds >= self.lock_after_seconds {
            return Err("Display dimming must happen before locking".to_string());
        }
        if self.lock_after_seconds >= self.display_off_after_seconds {
            return Err("The session must lock before displays turn off".to_string());
        }
        if self.display_off_after_seconds >= self.suspend_after_seconds {
            return Err("Displays must turn off before the system suspends".to_string());
        }
        Ok(())
    }
}

/// Returns the active user's Genoa idle-policy path.
///
/// # Errors
///
/// Returns an error when neither `XDG_CONFIG_HOME` nor `HOME` is available.
pub fn config_path() -> Result<PathBuf, String> {
    let configuration_home = match env::var_os("XDG_CONFIG_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join(".config"))
            .ok_or_else(|| "HOME and XDG_CONFIG_HOME are unavailable".to_string())?,
    };
    Ok(configuration_home.join("genoa/idle.toml"))
}

/// Returns a private per-session path inside the user's runtime directory.
///
/// # Errors
///
/// Returns an error when the runtime directory cannot be created or secured.
pub fn runtime_path(name: &str) -> Result<PathBuf, String> {
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "XDG_RUNTIME_DIR is unavailable".to_string())?;
    let directory = runtime.join("genoa");
    fs::create_dir_all(&directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("could not secure {}: {error}", directory.display()))?;
    Ok(directory.join(name))
}

/// Stores a small helper value in the private session runtime directory.
///
/// # Errors
///
/// Returns an error when the runtime file cannot be opened, written, or synced.
pub fn write_runtime_value(name: &str, value: &str) -> Result<(), String> {
    let path = runtime_path(name)?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    file.write_all(value.as_bytes())
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    file.sync_data()
        .map_err(|error| format!("could not save {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{IdleProfile, IdleSettings};

    #[test]
    fn defaults_are_safe_and_ordered() {
        assert!(IdleSettings::default().validate().is_ok());
        assert!(IdleProfile::Sway.defaults().validate().is_ok());
    }

    #[test]
    fn rejects_out_of_order_timeouts() {
        let settings = IdleSettings {
            lock_after_seconds: 200,
            dim_after_seconds: 600,
            ..IdleSettings::default()
        };
        assert!(settings.validate().is_err());
    }
}
