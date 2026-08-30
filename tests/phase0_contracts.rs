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

#[test]
fn ci_workflow_keeps_immutable_and_non_persistent_inputs() {
    let workflow = include_str!("../.github/workflows/ci.yml");

    assert!(workflow.contains("image: archlinux:base-devel@sha256:"));
    assert!(workflow.contains("persist-credentials: false"));

    for line in workflow.lines() {
        let trimmed = line.trim_start();
        if let Some(action) = trimmed.strip_prefix("uses: ") {
            let (_, revision) = action
                .split_once('@')
                .expect("external actions must include a revision");
            let revision = revision.split_whitespace().next().expect("action revision");
            assert_eq!(revision.len(), 40, "actions must use full commit SHAs");
            assert!(
                revision.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "action revision must be hexadecimal"
            );
        }
    }

    let lines: Vec<_> = workflow.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let indentation = line.len() - line.trim_start().len();
        if !line.trim_start().starts_with("run:") {
            continue;
        }

        assert!(
            !line.contains("${{"),
            "expression opener in run declaration"
        );
        for nested in lines.iter().skip(index + 1) {
            if nested.trim().is_empty() {
                continue;
            }
            let nested_indentation = nested.len() - nested.trim_start().len();
            if nested_indentation <= indentation {
                break;
            }
            assert!(!nested.contains("${{"), "expression opener in run block");
        }
    }
}

#[test]
fn public_license_terms_remain_consistent() {
    let manifest = include_str!("../Cargo.toml");
    let readme = include_str!("../README.md");
    let contributing = include_str!("../CONTRIBUTING.md");
    let license = include_str!("../LICENSE");

    assert!(manifest.contains("license = \"GPL-3.0-only\""));
    assert!(readme.contains("licensed under **GPL-3.0-only**"));
    assert!(contributing.contains("under **GPL-3.0-only**"));
    assert!(contributing.contains("Developer Certificate of Origin 1.1"));
    assert!(license.contains("GNU GENERAL PUBLIC LICENSE"));
    assert!(license.contains("Version 3, 29 June 2007"));
}
