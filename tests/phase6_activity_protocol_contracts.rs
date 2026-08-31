use std::path::PathBuf;
use std::time::Duration;

use weftwise::config::ConfigPaths;
use weftwise::services::activity::{
    ACTIVITY_PROTOCOL_VERSION, ACTIVITY_SOCKET_FILE, ActivityEndpointError, ActivityEvent,
    ActivityKind, ActivityOutcome, ActivityProtocolError, ActivityPublication, ActivityUpdate,
    MAX_ACTIVITY_FRAME_BYTES, MAX_ACTIVITY_LABEL_CHARACTERS, decode_frame, encode_frame,
    endpoint_path,
};

#[test]
fn synthetic_publish_fixture_decodes_to_bounded_typed_state() {
    let event = decode_frame(include_bytes!("fixtures/activity/publish.json"))
        .expect("synthetic fixture must decode");
    let ActivityEvent::Publish(publication) = event else {
        panic!("fixture must publish");
    };

    assert_eq!(publication.id().as_str(), "build.synthetic");
    assert_eq!(publication.kind(), ActivityKind::Build);
    assert_eq!(publication.label(), "Compile synthetic target");
    assert_eq!(publication.accessible_label(), "Build in progress");
    assert_eq!(
        publication
            .progress()
            .map(|progress| progress.basis_points()),
        Some(3_750)
    );
    assert_eq!(publication.expires_after(), Some(Duration::from_secs(60)));
}

#[test]
fn all_operations_round_trip_as_single_bounded_lines() {
    let frames = [
        br#"{"operation":"publish","schema_version":1,"id":"timer.focus","kind":"timer","label":"Focus interval","accessible_label":null,"progress_basis_points":0,"expires_after_millis":1500}"#.as_slice(),
        br#"{"operation":"update","schema_version":1,"id":"timer.focus","label":"Focus interval running","accessible_label":null,"progress_basis_points":5000,"expires_after_millis":1000}"#.as_slice(),
        br#"{"operation":"complete","schema_version":1,"id":"timer.focus","outcome":"succeeded","label":"Focus interval complete"}"#.as_slice(),
        br#"{"operation":"cancel","schema_version":1,"id":"timer.focus"}"#.as_slice(),
    ];

    for source in frames {
        let event = decode_frame(source).expect("source event");
        let encoded = encode_frame(&event).expect("encoded event");
        assert!(encoded.ends_with(b"\n"));
        assert!(encoded.len() <= MAX_ACTIVITY_FRAME_BYTES);
        assert_eq!(decode_frame(&encoded).expect("round trip"), event);
    }
}

#[test]
fn validated_constructors_support_future_endpoint_and_cli_encoding() {
    let publication = ActivityPublication::new(
        "download.synthetic",
        ActivityKind::Download,
        "Download package",
        None,
        Some(2_500),
        Some(Duration::from_secs(30)),
    )
    .expect("publication");
    let publication = ActivityEvent::Publish(publication);
    assert_eq!(
        decode_frame(&encode_frame(&publication).expect("encoded publication"))
            .expect("decoded publication"),
        publication
    );

    let update =
        ActivityUpdate::new("download.synthetic", None, None, Some(7_500), None).expect("update");
    let update = ActivityEvent::Update(update);
    assert_eq!(
        decode_frame(&encode_frame(&update).expect("encoded update")).expect("decoded update"),
        update
    );

    assert_eq!(
        ActivityEvent::cancel("unsafe identity"),
        Err(ActivityProtocolError::InvalidIdentity)
    );
}

#[test]
fn command_strings_and_unknown_fields_are_not_part_of_the_protocol() {
    let with_program = br#"{"operation":"publish","schema_version":1,"id":"build.synthetic","kind":"build","label":"Build","program":"tool","arguments":["--flag"]}"#;
    assert_eq!(
        decode_frame(with_program),
        Err(ActivityProtocolError::Malformed)
    );
}

#[test]
fn versions_size_identity_progress_lifetime_and_empty_updates_are_rejected() {
    let unsupported = br#"{"operation":"cancel","schema_version":2,"id":"timer.focus"}"#;
    assert_eq!(
        decode_frame(unsupported),
        Err(ActivityProtocolError::UnsupportedVersion)
    );
    assert_eq!(
        decode_frame(&vec![b' '; MAX_ACTIVITY_FRAME_BYTES + 1]),
        Err(ActivityProtocolError::FrameTooLarge)
    );
    assert_eq!(decode_frame(b" \n"), Err(ActivityProtocolError::EmptyFrame));

    let invalid_id = br#"{"operation":"cancel","schema_version":1,"id":"not allowed"}"#;
    assert_eq!(
        decode_frame(invalid_id),
        Err(ActivityProtocolError::InvalidIdentity)
    );
    let invalid_progress = br#"{"operation":"update","schema_version":1,"id":"build.synthetic","label":null,"accessible_label":null,"progress_basis_points":10001,"expires_after_millis":null}"#;
    assert_eq!(
        decode_frame(invalid_progress),
        Err(ActivityProtocolError::InvalidProgress)
    );
    let invalid_lifetime = br#"{"operation":"update","schema_version":1,"id":"build.synthetic","label":null,"accessible_label":null,"progress_basis_points":null,"expires_after_millis":0}"#;
    assert_eq!(
        decode_frame(invalid_lifetime),
        Err(ActivityProtocolError::InvalidLifetime)
    );
    let empty_update = br#"{"operation":"update","schema_version":1,"id":"build.synthetic","label":null,"accessible_label":null,"progress_basis_points":null,"expires_after_millis":null}"#;
    assert_eq!(
        decode_frame(empty_update),
        Err(ActivityProtocolError::EmptyUpdate)
    );
}

#[test]
fn display_text_is_sanitized_truncated_and_redacted_in_diagnostics() {
    let long = "x".repeat(MAX_ACTIVITY_LABEL_CHARACTERS + 20);
    let frame = format!(
        "{{\"operation\":\"publish\",\"schema_version\":{ACTIVITY_PROTOCOL_VERSION},\"id\":\"render.synthetic\",\"kind\":\"render\",\"label\":\"{long}\\nsecret\",\"accessible_label\":null,\"progress_basis_points\":null,\"expires_after_millis\":null}}"
    );
    let event = decode_frame(frame.as_bytes()).expect("bounded event");
    let ActivityEvent::Publish(publication) = &event else {
        panic!("publish event");
    };
    assert_eq!(
        publication.label().chars().count(),
        MAX_ACTIVITY_LABEL_CHARACTERS
    );
    let debug = format!("{event:?}");
    assert!(!debug.contains(publication.id().as_str()));
    assert!(!debug.contains(publication.label()));
}

#[test]
fn completion_outcome_is_typed() {
    let event = decode_frame(
        br#"{"operation":"complete","schema_version":1,"id":"download.synthetic","outcome":"failed","label":"Download failed"}"#,
    )
    .expect("completion");
    let ActivityEvent::Complete(completion) = event else {
        panic!("complete event");
    };
    assert_eq!(completion.outcome(), ActivityOutcome::Failed);
}

#[test]
fn endpoint_is_reserved_only_beneath_the_application_runtime_directory() {
    let mut paths = ConfigPaths {
        config_file: PathBuf::from("/example-config/config.toml"),
        cache_dir: PathBuf::from("/example-cache"),
        state_dir: PathBuf::from("/example-state"),
        runtime_dir: Some(PathBuf::from("/example-runtime/weftwise")),
    };
    assert_eq!(
        endpoint_path(&paths).expect("endpoint"),
        PathBuf::from("/example-runtime/weftwise").join(ACTIVITY_SOCKET_FILE)
    );
    paths.runtime_dir = None;
    assert_eq!(
        endpoint_path(&paths),
        Err(ActivityEndpointError::Unavailable)
    );
    paths.runtime_dir = Some(PathBuf::from("relative-runtime"));
    assert_eq!(
        endpoint_path(&paths),
        Err(ActivityEndpointError::NotAbsolute)
    );
}
