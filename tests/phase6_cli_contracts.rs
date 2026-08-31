use std::process::Command;

use weftwise::cli::{CliCommand, parse_arguments};
use weftwise::services::activity::{ActivityEvent, ActivityKind, ActivityOutcome};

#[test]
fn cli_parses_every_activity_operation_into_typed_protocol_state() {
    let publish = parse_arguments([
        "activity",
        "publish",
        "timer.synthetic",
        "timer",
        "Synthetic timer",
        "--progress-bp",
        "2500",
    ])
    .expect("publish command");
    let CliCommand::Activity(ActivityEvent::Publish(publication)) = publish else {
        panic!("publish must remain typed");
    };
    assert_eq!(publication.kind(), ActivityKind::Timer);
    assert_eq!(
        publication.progress().map(|value| value.basis_points()),
        Some(2500)
    );

    let update = parse_arguments([
        "activity",
        "update",
        "timer.synthetic",
        "--progress-bp",
        "5000",
    ])
    .expect("update command");
    assert!(matches!(
        update,
        CliCommand::Activity(ActivityEvent::Update(_))
    ));

    let complete = parse_arguments(["activity", "complete", "timer.synthetic", "failed"])
        .expect("complete command");
    let CliCommand::Activity(ActivityEvent::Complete(completion)) = complete else {
        panic!("completion must remain typed");
    };
    assert_eq!(completion.outcome(), ActivityOutcome::Failed);

    let cancel =
        parse_arguments(["activity", "cancel", "timer.synthetic"]).expect("cancel command");
    assert!(matches!(
        cancel,
        CliCommand::Activity(ActivityEvent::Cancel(_))
    ));
}

#[test]
fn cli_rejects_shell_shaped_fields_without_echoing_values() {
    let output = Command::new(env!("CARGO_BIN_EXE_weftwise"))
        .args([
            "activity",
            "publish",
            "build.synthetic",
            "build",
            "Synthetic build",
            "--command",
            "synthetic-tool --synthetic-flag",
        ])
        .output()
        .expect("run CLI");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 diagnostics");
    assert!(stderr.contains("invalid command-line invocation"));
    assert!(!stderr.contains("synthetic-tool"));
    assert!(output.stdout.is_empty());
}

#[test]
fn help_documents_display_data_and_no_command_string_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_weftwise"))
        .arg("--help")
        .output()
        .expect("run help");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("UTF-8 help");
    assert!(stdout.contains("weftwise reveal"));
    assert!(stdout.contains("twice within 500 ms"));
    assert!(stdout.contains("activity publish"));
    assert!(stdout.contains("shell strings are not accepted"));
    assert!(output.stderr.is_empty());
}
