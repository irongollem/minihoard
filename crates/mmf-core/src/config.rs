use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const QUALIFIER: &str = "nl";
const ORGANIZATION: &str = "crocode";
const APPLICATION: &str = "minihoard";

/// Persistent, non-secret configuration. The OAuth refresh token is **not**
/// stored here — it lives in the OS keychain (see [`crate::auth`]). This file
/// only references how to obtain/refresh it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// MyMiniFactory API client id (from your account's API settings).
    pub client_id: String,

    /// Loopback port used to catch the OAuth redirect during `login`.
    #[serde(default = "default_redirect_port")]
    pub redirect_port: u16,

    /// Where downloaded archives are written.
    pub download_dir: PathBuf,

    /// Where unpacked release folders are written.
    pub unpack_dir: PathBuf,

    /// Default behavior toggles for the `sync` command.
    #[serde(default)]
    pub defaults: Defaults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    /// Unpack archives automatically after download.
    #[serde(default = "default_true")]
    pub unpack: bool,

    /// Verify SHA-256 checksums when the API provides them.
    #[serde(default = "default_true")]
    pub verify_checksums: bool,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            unpack: true,
            verify_checksums: true,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_redirect_port() -> u16 {
    8723
}

impl Config {
    /// Canonical config file path (`~/Library/Application Support/minihoard/config.toml`
    /// on macOS, XDG equivalents elsewhere).
    pub fn default_path() -> Result<PathBuf> {
        let dirs = project_dirs()?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Path to the secrets file (`<config_dir>/credentials.json`, mode 0600).
    pub fn credentials_path() -> Result<PathBuf> {
        let dirs = project_dirs()?;
        Ok(dirs.config_dir().join("credentials.json"))
    }

    /// Default base directory for downloads/unpacks (`~/.../minihoard/data`).
    pub fn default_data_dir() -> Result<PathBuf> {
        let dirs = project_dirs()?;
        Ok(dirs.data_dir().to_path_buf())
    }

    /// Load config from the default path.
    pub fn load() -> Result<Self> {
        Self::load_from(&Self::default_path()?)
    }

    /// Load config from an explicit path.
    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::ConfigMissing(path.to_path_buf()));
        }
        let text = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&text)?;
        Ok(config)
    }

    /// Persist config to the default path, creating parent dirs as needed.
    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::default_path()?)
    }

    /// Persist config to an explicit path.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .ok_or_else(|| Error::Config("could not resolve OS config directory".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let cfg = Config {
            client_id: "abc123".into(),
            redirect_port: 8723,
            download_dir: PathBuf::from("/tmp/dl"),
            unpack_dir: PathBuf::from("/tmp/unpack"),
            defaults: Defaults::default(),
        };
        let dir = std::env::temp_dir().join("minihoard-test-config");
        let path = dir.join("config.toml");
        cfg.save_to(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.client_id, "abc123");
        assert!(loaded.defaults.unpack);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_config_is_typed_error() {
        let path = std::env::temp_dir().join("minihoard-does-not-exist/config.toml");
        match Config::load_from(&path) {
            Err(Error::ConfigMissing(_)) => {}
            other => panic!("expected ConfigMissing, got {other:?}"),
        }
    }
}
