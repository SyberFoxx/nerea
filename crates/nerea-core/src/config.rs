use std::{
    fs,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// API keys never live here: they belong to the OS keyring.
///
/// A typo'd key fails loudly instead of being silently ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<Fallback>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fallback {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: "opencode-go".to_owned(),
            model: None,
            fallback: Some(Fallback { provider: "deepseek".to_owned(), model: None }),
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not determine the user config directory")]
    NoConfigDir,

    #[error("failed to read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("invalid TOML in {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },

    #[error("failed to encode config: {0}")]
    Encode(#[from] toml::ser::Error),
}

impl Config {
    /// `$XDG_CONFIG_HOME/nerea/config.toml`, typically `~/.config/nerea/config.toml`.
    pub fn path() -> Result<PathBuf, ConfigError> {
        let dirs = ProjectDirs::from("", "", "nerea").ok_or(ConfigError::NoConfigDir)?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// A missing file yields the defaults; a broken file is an error.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&Self::path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|source| ConfigError::Parse {
                path: path.display().to_string(),
                source,
            }),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.display().to_string(),
                source,
            }),
        }
    }

    pub fn save(&self) -> Result<PathBuf, ConfigError> {
        let path = Self::path()?;
        self.save_to(&path)?;
        Ok(path)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| ConfigError::Write {
                path: parent.display().to_string(),
                source,
            })?;
        }
        // Write beside the target and rename: a crash mid-write cannot truncate
        // a config the user already had.
        let mut staging = path.as_os_str().to_owned();
        staging.push(".tmp");
        let staging = PathBuf::from(staging);

        fs::write(&staging, self.to_toml()?).map_err(|source| ConfigError::Write {
            path: staging.display().to_string(),
            source,
        })?;

        fs::rename(&staging, path).map_err(|source| ConfigError::Write {
            path: path.display().to_string(),
            source,
        })
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        Ok(toml::to_string_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_point_at_opencode_go_with_deepseek_fallback() {
        let config = Config::default();
        assert_eq!(config.provider, "opencode-go");
        let fallback = config.fallback.as_ref().expect("fallback");
        assert_eq!(fallback.provider, "deepseek");
    }

    #[test]
    fn round_trips_through_toml() {
        let config = Config {
            provider: "deepseek".to_owned(),
            model: Some("deepseek-v4-pro".to_owned()),
            fallback: None,
        };
        let text = config.to_toml().expect("encode");
        let parsed: Config = toml::from_str(&text).expect("decode");
        assert_eq!(parsed, config);
    }

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("nerea-config-test-{}-{name}.toml", std::process::id()))
    }

    #[test]
    fn save_round_trips_without_secrets_and_leaves_no_staging_file() {
        let path = temp_path("roundtrip");
        let _ = fs::remove_file(&path);

        Config::default().save_to(&path).expect("save");
        let text = fs::read_to_string(&path).expect("read");
        assert!(!text.to_lowercase().contains("api_key"), "keys never live in config");
        assert_eq!(Config::load_from(&path).expect("reload"), Config::default());

        let staged = PathBuf::from(format!("{}.tmp", path.display()));
        assert!(!staged.exists(), "atomic write must not leave a staging file");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn malformed_file_is_an_error_not_a_default() {
        let path = temp_path("malformed");
        fs::write(&path, "provider =").expect("write fixture");
        assert!(Config::load_from(&path).is_err(), "a broken config must not be ignored");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let result: Result<Config, _> = toml::from_str("provider = \"deepseek\"\nmodle = \"typo\"\n");
        assert!(result.is_err(), "a typo'd key must not be silently ignored");
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let config = Config::load_from(Path::new("/nonexistent/nerea/config.toml")).expect("load");
        assert_eq!(config, Config::default());
    }
}
