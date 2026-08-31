//! Versioned configuration and privacy-preserving path discovery.

use std::env;
use std::fs::File;
use std::io::{self, Read};
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
const MAX_CONFIG_BYTES: u64 = 64 * 1024;

/// Phase 0 user configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Version of the serialized schema.
    pub schema_version: u32,
    /// Override the desktop animation preference when present.
    pub reduced_motion: Option<bool>,
    /// Pointer activation geometry for the collapsed surface.
    pub activation: ActivationConfig,
    /// Persistent Ribbon regions sourced from root state.
    pub ribbon: RibbonConfig,
    /// Validated semantic theme tokens.
    pub theme: ThemeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            reduced_motion: None,
            activation: ActivationConfig::default(),
            ribbon: RibbonConfig::default(),
            theme: ThemeConfig::default(),
        }
    }
}

/// Placement policy for the collapsed pointer activation region.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivationMode {
    /// Use a bounded island within the widest top-edge segment not covered by another output.
    #[default]
    ExposedEdge,
    /// Retain the original full-width top-edge trigger for comparison or rollback.
    FullWidth,
}

/// Alignment of an activation island inside its selected exposed segment.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivationAnchor {
    /// Place the island near the segment's left/start edge.
    Start,
    /// Center the island within the exposed segment.
    Center,
    /// Place the island near the segment's right/end edge.
    #[default]
    End,
}

/// Bounded collapsed pointer activation settings, in GDK logical pixels.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ActivationConfig {
    /// Activation placement policy.
    pub mode: ActivationMode,
    /// Maximum activation island width.
    pub width: u16,
    /// Top-island depth and corner-leg width; the visual Selvage remains thinner.
    pub height: u8,
    /// Inset from the selected exposed segment boundary.
    pub margin: u16,
    /// Island alignment within the selected exposed segment.
    pub anchor: ActivationAnchor,
    /// Reveal immediately on pointer entry instead of waiting for dwell.
    pub reveal_on_entry: bool,
}

impl Default for ActivationConfig {
    fn default() -> Self {
        Self {
            mode: ActivationMode::ExposedEdge,
            width: 96,
            height: 12,
            margin: 12,
            anchor: ActivationAnchor::End,
            reveal_on_entry: false,
        }
    }
}

/// Persistent Ribbon region visibility.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct RibbonConfig {
    /// Show the output-local active workspace at the start.
    pub show_workspace: bool,
    /// Show the selected candidate or active client context in the center.
    pub show_context: bool,
    /// Show the in-process clock at the end.
    pub show_clock: bool,
}

impl Default for RibbonConfig {
    fn default() -> Self {
        Self {
            show_workspace: true,
            show_context: true,
            show_clock: true,
        }
    }
}

/// Semantic GTK theme tokens accepted from versioned configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// Main translucent Ribbon background.
    pub background: String,
    /// Panel and raised-control background.
    pub surface: String,
    /// Primary foreground text.
    pub text: String,
    /// Secondary foreground and inactive marks.
    pub muted: String,
    /// Selected-state accent.
    pub accent: String,
    /// Subtle outline color.
    pub border: String,
    /// Warning-state color.
    pub warning: String,
    /// Critical-state color.
    pub critical: String,
    /// Font family restricted to a safe CSS identifier subset.
    pub font_family: String,
    /// Ribbon font size in points.
    pub font_size: u8,
    /// Raised-surface corner radius in logical pixels.
    pub radius: u8,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            background: "#111012F2".to_owned(),
            surface: "#191719FA".to_owned(),
            text: "#DDD6D0".to_owned(),
            muted: "#928985".to_owned(),
            accent: "#E06B5F".to_owned(),
            border: "#443735".to_owned(),
            warning: "#D6A34A".to_owned(),
            critical: "#EF5F5A".to_owned(),
            font_family: "monospace".to_owned(),
            font_size: 10,
            radius: 7,
        }
    }
}

impl Config {
    /// Parse and validate TOML without exposing values in displayed errors.
    pub fn parse(source: &str) -> Result<Self, ConfigError> {
        let table = source
            .parse::<toml::Table>()
            .map_err(|source| ConfigError::InvalidSyntax { source })?;

        if let Some(key) = table.keys().find(|key| {
            !matches!(
                key.as_str(),
                "schema_version" | "reduced_motion" | "activation" | "ribbon" | "theme"
            )
        }) {
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

        config.validate()?;
        Ok(config)
    }

    /// Load the bounded XDG configuration file, or defaults when it is absent.
    pub fn load(path: &Path) -> Result<Self, ConfigLoadError> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(source) => return Err(ConfigLoadError::Read { source }),
        };
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|source| ConfigLoadError::Read { source })?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(ConfigLoadError::TooLarge);
        }
        let source = std::str::from_utf8(&bytes).map_err(|_| ConfigLoadError::Encoding)?;
        Self::parse(source).map_err(ConfigLoadError::Parse)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if !(32..=512).contains(&self.activation.width) {
            return Err(ConfigError::InvalidSetting {
                setting: "activation.width",
            });
        }
        if !(3..=20).contains(&self.activation.height) {
            return Err(ConfigError::InvalidSetting {
                setting: "activation.height",
            });
        }
        if self.activation.margin > 128 {
            return Err(ConfigError::InvalidSetting {
                setting: "activation.margin",
            });
        }
        for (setting, color) in [
            ("theme.background", self.theme.background.as_str()),
            ("theme.surface", self.theme.surface.as_str()),
            ("theme.text", self.theme.text.as_str()),
            ("theme.muted", self.theme.muted.as_str()),
            ("theme.accent", self.theme.accent.as_str()),
            ("theme.border", self.theme.border.as_str()),
            ("theme.warning", self.theme.warning.as_str()),
            ("theme.critical", self.theme.critical.as_str()),
        ] {
            if !valid_hex_color(color) {
                return Err(ConfigError::InvalidSetting { setting });
            }
        }
        if self.theme.font_family.is_empty()
            || self.theme.font_family.len() > 64
            || !self.theme.font_family.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, ' ' | '-')
            })
        {
            return Err(ConfigError::InvalidSetting {
                setting: "theme.font_family",
            });
        }
        if !(8..=24).contains(&self.theme.font_size) {
            return Err(ConfigError::InvalidSetting {
                setting: "theme.font_size",
            });
        }
        if self.theme.radius > 24 {
            return Err(ConfigError::InvalidSetting {
                setting: "theme.radius",
            });
        }
        Ok(())
    }
}

fn valid_hex_color(value: &str) -> bool {
    matches!(value.len(), 7 | 9)
        && value.starts_with('#')
        && value[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
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
    /// A supported setting is outside its bounded safe range.
    #[error("configuration setting `{setting}` is invalid")]
    InvalidSetting {
        /// Stable public setting name; never sourced from user input.
        setting: &'static str,
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

/// Safe configuration loading failures that never include the resolved path.
#[derive(Error)]
pub enum ConfigLoadError {
    /// The file exceeded the configured input bound.
    #[error("configuration file exceeds 64 KiB")]
    TooLarge,
    /// The file could not be read.
    #[error("configuration file could not be read")]
    Read {
        /// Underlying operating-system failure without the requested path.
        #[source]
        source: io::Error,
    },
    /// The file was not valid UTF-8.
    #[error("configuration file is not valid UTF-8")]
    Encoding,
    /// The bounded TOML configuration was invalid.
    #[error(transparent)]
    Parse(#[from] ConfigError),
}

impl std::fmt::Debug for ConfigLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("ConfigLoadError")
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
    fn parses_bounded_activation_ribbon_and_theme_settings() {
        let config = Config::parse(
            r##"
                [activation]
                mode = "exposed-edge"
                width = 128
                height = 7
                margin = 16
                anchor = "end"
                reveal_on_entry = true

                [ribbon]
                show_workspace = true
                show_context = false
                show_clock = true

                [theme]
                accent = "#AABBCC"
                font_family = "Synthetic Mono"
                radius = 9
            "##,
        )
        .expect("valid nested configuration");
        assert_eq!(config.activation.width, 128);
        assert_eq!(config.activation.height, 7);
        assert!(config.activation.reveal_on_entry);
        assert!(!config.ribbon.show_context);
        assert_eq!(config.theme.accent, "#AABBCC");
        assert_eq!(config.theme.font_family, "Synthetic Mono");
        assert_eq!(config.theme.radius, 9);
    }

    #[test]
    fn rejects_css_injection_and_unbounded_activation_geometry() {
        for source in [
            "[theme]\naccent = \"red; } button { color: red\"",
            "[theme]\nfont_family = \"mono\\\"; color: red\"",
            "[activation]\nwidth = 4096",
            "[activation]\nheight = 24",
        ] {
            assert!(matches!(
                Config::parse(source).expect_err("unsafe setting"),
                ConfigError::InvalidSetting { .. }
            ));
        }
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
