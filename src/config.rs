//! Versioned configuration and privacy-preserving path discovery.

use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Current configuration schema.
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

/// Required mode for private application directories on Unix.
pub const PRIVATE_DIRECTORY_MODE: u32 = 0o700;

/// Required mode for private application files on Unix.
pub const PRIVATE_FILE_MODE: u32 = 0o600;

const APP_DIRECTORY: &str = "weftwise";

/// Phase 0 user configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Version of the serialized schema.
    pub schema_version: u32,
    /// Override the desktop animation preference when present.
    pub reduced_motion: Option<bool>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            reduced_motion: None,
        }
    }
}

impl Config {
    /// Parse and validate TOML without exposing values in displayed errors.
    pub fn parse(source: &str) -> Result<Self, ConfigError> {
        let table = source
            .parse::<toml::Table>()
            .map_err(|source| ConfigError::InvalidSyntax { source })?;

        if let Some(key) = table
            .keys()
            .find(|key| !matches!(key.as_str(), "schema_version" | "reduced_motion"))
        {
            return Err(ConfigError::UnknownKey {
                key: diagnostic_key(key),
            });
        }

        let config: Self = table
            .try_into()
            .map_err(|source| ConfigError::InvalidValue { source })?;
        if config.schema_version != CONFIG_SCHEMA_VERSION {
            return Err(ConfigError::UnsupportedSchema {
                found: config.schema_version,
            });
        }

        Ok(config)
    }
}

fn diagnostic_key(key: &str) -> String {
    if key.len() <= 64
        && key
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        key.to_owned()
    } else {
        "<redacted>".to_owned()
    }
}

/// Environment values used to resolve XDG application locations.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct XdgEnvironment {
    /// User home directory used only for XDG fallback rules.
    pub home: Option<PathBuf>,
    /// XDG configuration base directory.
    pub config_home: Option<PathBuf>,
    /// XDG cache base directory.
    pub cache_home: Option<PathBuf>,
    /// XDG state base directory.
    pub state_home: Option<PathBuf>,
    /// Per-login XDG runtime directory.
    pub runtime_dir: Option<PathBuf>,
}

impl std::fmt::Debug for XdgEnvironment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XdgEnvironment")
            .field("home", &self.home.as_ref().map(|_| "<redacted>"))
            .field(
                "config_home",
                &self.config_home.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "cache_home",
                &self.cache_home.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "state_home",
                &self.state_home.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "runtime_dir",
                &self.runtime_dir.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl XdgEnvironment {
    /// Read XDG values from the current process without logging their contents.
    #[must_use]
    pub fn discover() -> Self {
        Self {
            home: environment_path("HOME"),
            config_home: environment_path("XDG_CONFIG_HOME"),
            cache_home: environment_path("XDG_CACHE_HOME"),
            state_home: environment_path("XDG_STATE_HOME"),
            runtime_dir: environment_path("XDG_RUNTIME_DIR"),
        }
    }
}

fn environment_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Resolved application paths. Debug formatting intentionally redacts values.
#[derive(Clone, Eq, PartialEq)]
pub struct ConfigPaths {
    /// Versioned configuration file.
    pub config_file: PathBuf,
    /// Application cache directory.
    pub cache_dir: PathBuf,
    /// Application state directory.
    pub state_dir: PathBuf,
    /// Per-login runtime directory, unavailable when `XDG_RUNTIME_DIR` is absent.
    pub runtime_dir: Option<PathBuf>,
}

impl std::fmt::Debug for ConfigPaths {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfigPaths")
            .field("config_file", &"<redacted>")
            .field("cache_dir", &"<redacted>")
            .field("state_dir", &"<redacted>")
            .field(
                "runtime_dir",
                &self.runtime_dir.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl ConfigPaths {
    /// Resolve paths from the current process environment.
    pub fn discover() -> Result<Self, ConfigPathError> {
        Self::from_environment(&XdgEnvironment::discover())
    }

    /// Resolve paths from explicit values, suitable for deterministic tests.
    pub fn from_environment(environment: &XdgEnvironment) -> Result<Self, ConfigPathError> {
        if environment
            .runtime_dir
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            return Err(ConfigPathError::NotAbsolute {
                variable: "XDG_RUNTIME_DIR",
            });
        }

        let config_base = xdg_base(
            environment.config_home.as_deref(),
            environment.home.as_deref(),
            Path::new(".config"),
            "XDG_CONFIG_HOME",
        )?;
        let cache_base = xdg_base(
            environment.cache_home.as_deref(),
            environment.home.as_deref(),
            Path::new(".cache"),
            "XDG_CACHE_HOME",
        )?;
        let state_base = xdg_base(
            environment.state_home.as_deref(),
            environment.home.as_deref(),
            Path::new(".local/state"),
            "XDG_STATE_HOME",
        )?;

        Ok(Self {
            config_file: config_base.join(APP_DIRECTORY).join("config.toml"),
            cache_dir: cache_base.join(APP_DIRECTORY),
            state_dir: state_base.join(APP_DIRECTORY),
            runtime_dir: environment
                .runtime_dir
                .as_ref()
                .map(|path| path.join(APP_DIRECTORY)),
        })
    }
}

fn xdg_base(
    configured: Option<&Path>,
    home: Option<&Path>,
    fallback: &Path,
    variable: &'static str,
) -> Result<PathBuf, ConfigPathError> {
    if let Some(configured) = configured {
        if configured.is_absolute() {
            return Ok(configured.to_owned());
        }
        return Err(ConfigPathError::NotAbsolute { variable });
    }

    match home {
        Some(path) if path.is_absolute() => Ok(path.join(fallback)),
        Some(_) => Err(ConfigPathError::NotAbsolute { variable: "HOME" }),
        None => Err(ConfigPathError::MissingHome { variable }),
    }
}

/// Safe configuration parsing failures.
#[derive(Error)]
pub enum ConfigError {
    /// TOML could not be parsed.
    #[error("configuration contains invalid TOML")]
    InvalidSyntax {
        /// Original parser failure; do not log with debug formatting.
        #[source]
        source: toml::de::Error,
    },
    /// A key is not part of the versioned schema.
    #[error("unknown configuration key `{key}`")]
    UnknownKey {
        /// Unsupported schema key.
        key: String,
    },
    /// A known key contains a value of the wrong type.
    #[error("configuration contains an invalid value")]
    InvalidValue {
        /// Original deserialization failure; do not log with debug formatting.
        #[source]
        source: toml::de::Error,
    },
    /// A configuration targets an unsupported schema version.
    #[error("unsupported configuration schema version {found}")]
    UnsupportedSchema {
        /// Version found in the configuration.
        found: u32,
    },
}

impl std::fmt::Debug for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ConfigError")
            .field(&self.to_string())
            .finish()
    }
}

/// Safe failures from XDG location resolution.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ConfigPathError {
    /// A configured XDG base directory is not absolute.
    #[error("{variable} must contain an absolute path")]
    NotAbsolute {
        /// Name of the invalid environment variable.
        variable: &'static str,
    },
    /// An XDG base and the home fallback are both unavailable.
    #[error("cannot resolve {variable} because HOME is unavailable")]
    MissingHome {
        /// XDG base that could not be resolved.
        variable: &'static str,
    },
}

/// Default logging boundary. These categories are always redacted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticPolicy {
    /// Never log user-specific filesystem locations.
    pub redact_user_paths: bool,
    /// Never log compositor window or workspace text.
    pub redact_desktop_text: bool,
    /// Never log media, notification, or clipboard contents.
    pub redact_content: bool,
    /// Never log external process arguments.
    pub redact_process_arguments: bool,
}

impl Default for DiagnosticPolicy {
    fn default() -> Self {
        Self {
            redact_user_paths: true,
            redact_desktop_text: true,
            redact_content: true,
            redact_process_arguments: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_environment() -> XdgEnvironment {
        XdgEnvironment {
            home: Some(PathBuf::from("/example-home")),
            config_home: None,
            cache_home: None,
            state_home: None,
            runtime_dir: Some(PathBuf::from("/example-runtime")),
        }
    }

    #[test]
    fn parses_default_configuration() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn rejects_unknown_keys() {
        let error = Config::parse("unsupported = true").unwrap_err();
        assert!(matches!(error, ConfigError::UnknownKey { .. }));
        assert_eq!(error.to_string(), "unknown configuration key `unsupported`");
    }

    #[test]
    fn redacts_untrusted_unknown_key_text() {
        let error = Config::parse("\"private key text\" = true").unwrap_err();
        assert_eq!(error.to_string(), "unknown configuration key `<redacted>`");
        assert!(!format!("{error:?}").contains("private key text"));
    }

    #[test]
    fn rejects_unknown_schema_versions() {
        let error = Config::parse("schema_version = 7").unwrap_err();
        assert!(matches!(error, ConfigError::UnsupportedSchema { found: 7 }));
    }

    #[test]
    fn applies_xdg_fallbacks() {
        let paths = ConfigPaths::from_environment(&synthetic_environment()).unwrap();
        assert_eq!(
            paths.config_file,
            Path::new("/example-home/.config/weftwise/config.toml")
        );
        assert_eq!(paths.cache_dir, Path::new("/example-home/.cache/weftwise"));
        assert_eq!(
            paths.state_dir,
            Path::new("/example-home/.local/state/weftwise")
        );
        assert_eq!(
            paths.runtime_dir.as_deref(),
            Some(Path::new("/example-runtime/weftwise"))
        );
    }

    #[test]
    fn redacts_paths_from_debug_output() {
        let paths = ConfigPaths::from_environment(&synthetic_environment()).unwrap();
        let output = format!("{paths:?}");
        assert!(!output.contains("example-home"));
        assert!(!output.contains("example-runtime"));
    }

    #[test]
    fn diagnostics_are_redacted_by_default() {
        let policy = DiagnosticPolicy::default();
        assert!(policy.redact_user_paths);
        assert!(policy.redact_desktop_text);
        assert!(policy.redact_content);
        assert!(policy.redact_process_arguments);
    }
}
