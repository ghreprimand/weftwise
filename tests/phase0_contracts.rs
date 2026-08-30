use std::path::{Path, PathBuf};

use weftwise::config::{
    CONFIG_SCHEMA_VERSION, Config, ConfigError, ConfigPathError, ConfigPaths, DiagnosticPolicy,
    PRIVATE_DIRECTORY_MODE, PRIVATE_FILE_MODE, XdgEnvironment,
};
use weftwise::supervisor::{ASYNC_WORKER_LIMIT, BLOCKING_WORKER_LIMIT};

fn environment() -> XdgEnvironment {
    XdgEnvironment {
        home: Some(PathBuf::from("/synthetic-home")),
        config_home: Some(PathBuf::from("/synthetic-config")),
        cache_home: Some(PathBuf::from("/synthetic-cache")),
        state_home: Some(PathBuf::from("/synthetic-state")),
        runtime_dir: Some(PathBuf::from("/synthetic-runtime")),
    }
}

#[test]
fn xdg_bases_take_precedence_and_keep_application_paths_scoped() {
    let paths = ConfigPaths::from_environment(&environment()).expect("absolute XDG paths");

    assert_eq!(
        paths.config_file,
        Path::new("/synthetic-config/weftwise/config.toml")
    );
    assert_eq!(paths.cache_dir, Path::new("/synthetic-cache/weftwise"));
    assert_eq!(paths.state_dir, Path::new("/synthetic-state/weftwise"));
    assert_eq!(
        paths.runtime_dir.as_deref(),
        Some(Path::new("/synthetic-runtime/weftwise"))
    );
}

#[test]
fn relative_xdg_bases_are_rejected_without_falling_back() {
    for (field, variable) in [
        ("config", "XDG_CONFIG_HOME"),
        ("cache", "XDG_CACHE_HOME"),
        ("state", "XDG_STATE_HOME"),
    ] {
        let mut values = environment();
        match field {
            "config" => values.config_home = Some(PathBuf::from("relative")),
            "cache" => values.cache_home = Some(PathBuf::from("relative")),
            "state" => values.state_home = Some(PathBuf::from("relative")),
            _ => unreachable!(),
        }

        assert_eq!(
            ConfigPaths::from_environment(&values),
            Err(ConfigPathError::NotAbsolute { variable })
        );
    }
}

#[test]
fn missing_xdg_base_and_home_is_an_explicit_error() {
    let values = XdgEnvironment::default();

    assert_eq!(
        ConfigPaths::from_environment(&values),
        Err(ConfigPathError::MissingHome {
            variable: "XDG_CONFIG_HOME"
        })
    );
}

#[test]
fn configuration_contract_rejects_invalid_values_and_unsupported_schemas() {
    assert!(matches!(
        Config::parse("reduced_motion = \"never\"").expect_err("invalid type"),
        ConfigError::InvalidValue { .. }
    ));
    assert!(matches!(
        Config::parse("schema_version = 2").expect_err("unsupported schema"),
        ConfigError::UnsupportedSchema { found: 2 }
    ));
    assert_eq!(Config::default().schema_version, CONFIG_SCHEMA_VERSION);
}

#[test]
fn diagnostics_and_path_debugging_do_not_expose_synthetic_private_values() {
    let paths = ConfigPaths::from_environment(&environment()).expect("valid paths");
    let debug = format!("{paths:?}");

    assert!(!debug.contains("synthetic-home"));
    assert!(!debug.contains("synthetic-config"));
    assert!(!debug.contains("synthetic-cache"));
    assert!(!debug.contains("synthetic-state"));
    assert!(!debug.contains("synthetic-runtime"));

    assert_eq!(
        DiagnosticPolicy::default(),
        DiagnosticPolicy {
            redact_user_paths: true,
            redact_desktop_text: true,
            redact_content: true,
            redact_process_arguments: true,
        }
    );
}

#[test]
fn private_storage_permission_contract_is_restrictive() {
    assert_eq!(PRIVATE_DIRECTORY_MODE, 0o700);
    assert_eq!(PRIVATE_FILE_MODE, 0o600);
}

#[test]
fn phase_zero_module_contract_and_application_id_are_available() {
    assert_eq!(weftwise::APPLICATION_ID, "io.unfinished_works.weftwise");
}

#[test]
fn relm_runtime_limits_match_the_initial_resource_contract() {
    assert_eq!(ASYNC_WORKER_LIMIT, 1);
    assert_eq!(BLOCKING_WORKER_LIMIT, 4);
}
