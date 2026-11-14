//! Launcher configuration.
//!
//! The Microsoft application id is not a secret for a public client, but it
//! is still stored next to the account file with user-only permissions.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoResultExt, Result};

/// Settings stored in the user config directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    /// Azure application (client) id used for Microsoft device-code login.
    #[serde(default)]
    pub ms_client_id: String,
    /// Random id sent to the game as `${clientid}`.
    #[serde(default)]
    pub client_id: String,
    /// Game directory chosen in the terminal UI. Empty uses the default.
    #[serde(default)]
    pub game_dir: String,
    /// Heap size passed to the game, such as `2G`. Empty uses `2G`.
    #[serde(default)]
    pub memory: String,
    /// Java binary chosen in the terminal UI. Empty lets Weathered pick one.
    #[serde(default)]
    pub java_path: String,
    /// Offline player name used by the terminal UI.
    #[serde(default)]
    pub offline_username: String,
    /// Concurrent downloads. Zero uses 16.
    #[serde(default)]
    pub download_jobs: u32,
    /// Minimum heap, such as `512M`. Empty omits `-Xms`.
    #[serde(default)]
    pub memory_min: String,
    /// Extra JVM arguments used when an instance does not set its own.
    #[serde(default)]
    pub jvm_args: String,
    /// Instance id pinned to the top of the list and selected at startup.
    #[serde(default)]
    pub default_instance: String,
    /// `name`, `created`, or `played`.
    #[serde(default)]
    pub instance_sort: String,
}

impl Config {
    /// Directory that holds `config.toml` and `accounts.json`.
    pub fn dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("weathered")
    }

    /// Path of `config.toml`.
    pub fn path() -> PathBuf {
        Self::dir().join("config.toml")
    }

    /// Path of the Microsoft account store.
    pub fn accounts_path() -> PathBuf {
        Self::dir().join("accounts.json")
    }

    /// Load the config, or the defaults when the file does not exist yet.
    pub fn load() -> Result<Self> {
        let path = Self::path();
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).io_at(&path)?;
        toml::from_str(&text)
            .map_err(|err| Error::message(format!("could not parse {}: {err}", path.display())))
    }

    /// Write the config, replacing any previous file.
    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).io_at(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|err| Error::message(format!("could not encode config: {err}")))?;
        let tmp = path.with_extension("toml.part");
        std::fs::write(&tmp, text).io_at(&tmp)?;
        restrict_private(&tmp);
        #[cfg(windows)]
        if path.exists() {
            std::fs::remove_file(&path).io_at(&path)?;
        }
        std::fs::rename(&tmp, &path).io_at(&path)?;
        Ok(())
    }

    /// Download concurrency. Zero or missing means 16.
    pub fn jobs(&self) -> usize {
        if self.download_jobs == 0 {
            16
        } else {
            self.download_jobs as usize
        }
    }

    /// Return the game client id, creating and saving one on first use.
    pub fn ensure_client_id(&mut self) -> String {
        if self.client_id.trim().is_empty() {
            self.client_id = uuid::Uuid::new_v4().to_string();
            let _ = self.save();
        }
        self.client_id.clone()
    }
}

/// Pick a Microsoft client id.
///
/// The command-line value wins, then the environment variable, then the
/// config file. Empty strings are ignored. `env` is passed in so tests do not
/// have to mutate the process environment.
pub fn resolve_ms_client_id(
    cli: Option<&str>,
    env: Option<&str>,
    config: &Config,
) -> Option<String> {
    for candidate in [cli, env, Some(config.ms_client_id.as_str())] {
        if let Some(value) = candidate.map(str::trim).filter(|value| !value.is_empty()) {
            return Some(value.to_string());
        }
    }
    None
}

pub(crate) fn restrict_private(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_id_precedence_is_cli_then_env_then_config() {
        let config = Config {
            ms_client_id: "from-config".into(),
            ..Config::default()
        };
        assert_eq!(
            resolve_ms_client_id(Some(" from-cli "), Some("from-env"), &config).as_deref(),
            Some("from-cli")
        );
        assert_eq!(
            resolve_ms_client_id(None, Some("from-env"), &config).as_deref(),
            Some("from-env")
        );
        assert_eq!(
            resolve_ms_client_id(Some("  "), None, &config).as_deref(),
            Some("from-config")
        );
        assert_eq!(resolve_ms_client_id(None, None, &Config::default()), None);
    }
}
