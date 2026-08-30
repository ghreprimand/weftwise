use std::ffi::OsString;
use std::path::PathBuf;

use weftwise::services::hyprland::{
    AdapterError, DiscoveryEnvironment, DiscoveryError, EventBuffer, ParseError, SocketPaths,
    WorkspaceCatalog, parse_event_line, parse_snapshot_json,
};
use weftwise::state::{DisplayText, HyprlandEvent, OpenedClient, WorkspaceId};

fn discovery(signature: Option<&str>) -> DiscoveryEnvironment {
    DiscoveryEnvironment {
        runtime_dir: Some(PathBuf::from("/synthetic-runtime")),
        instance_signature: signature.map(OsString::from),
    }
}

#[test]
fn socket_discovery_rejects_unsafe_leaves_and_redacts_paths() {
    let paths = SocketPaths::from_environment(&discovery(Some("synthetic-signature")))
        .expect("synthetic absolute runtime path");
    assert_eq!(
        paths.request(),
        PathBuf::from("/synthetic-runtime/hypr/synthetic-signature/.socket.sock")
    );
    assert_eq!(
        paths.events(),
        PathBuf::from("/synthetic-runtime/hypr/synthetic-signature/.socket2.sock")
    );
    assert!(!format!("{paths:?}").contains("synthetic-runtime"));

    for signature in ["", ".", "..", "contains/slash", "contains space"] {
        assert_eq!(
            SocketPaths::from_environment(&discovery(Some(signature))),
            Err(DiscoveryError::InvalidInstanceSignature)
        );
    }
    assert_eq!(
        SocketPaths::from_environment(&DiscoveryEnvironment {
            runtime_dir: Some(PathBuf::from("relative")),
            instance_signature: Some(OsString::from("synthetic")),
        }),
        Err(DiscoveryError::RelativeRuntimeDirectory)
    );
}

#[test]
fn event_parser_uses_the_first_delimiter_and_tolerates_unknown_events() {
    let parsed = parse_event_line("activewindow>>SyntheticClass,title>>with delimiter\n")
        .expect("recognized legacy event")
        .expect("recognized event");
    assert_eq!(
        parsed,
        HyprlandEvent::ActiveClient {
            address: None,
            class: weftwise::state::DisplayText::new("SyntheticClass", 128),
            title: weftwise::state::DisplayText::new("title>>with delimiter", 256),
        }
    );
    assert_eq!(
        parse_event_line("futureevent>>opaque,payload").unwrap(),
        None
    );
}

#[test]
fn malformed_and_truncated_event_shapes_are_rejected_without_payload_echo() {
    for line in [
        "no-delimiter",
        "workspacev2>>not-an-id,name",
        "focusedmonv2>>",
        "closewindowv2>>not-address",
        "fullscreen>>maybe",
        "openwindow>>abc,one,missing-title",
    ] {
        let error = parse_event_line(line).expect_err("malformed line must fail");
        assert_eq!(error, ParseError::MalformedEvent);
        assert!(!error.to_string().contains(line));
    }
}

#[test]
fn event_buffer_replays_known_events_in_wire_order_and_limits_all_input() {
    let mut buffer = EventBuffer::with_limits(2, 96);
    buffer
        .push_line("future>>ignored")
        .expect("unknown tolerated");
    buffer.push_line("workspacev2>>1,one").expect("first known");
    buffer
        .push_line("workspacev2>>2,two")
        .expect("second known");
    assert_eq!(buffer.len(), 2);
    assert_eq!(
        buffer.drain().collect::<Vec<_>>(),
        vec![
            HyprlandEvent::WorkspaceChanged {
                id: WorkspaceId::new(1),
                name: weftwise::state::DisplayText::new("one", 128),
            },
            HyprlandEvent::WorkspaceChanged {
                id: WorkspaceId::new(2),
                name: weftwise::state::DisplayText::new("two", 128),
            },
        ]
    );
    assert!(buffer.is_empty());

    let mut count_limited = EventBuffer::with_limits(1, 256);
    count_limited.push_line("workspacev2>>1,one").unwrap();
    assert_eq!(
        count_limited.push_line("workspacev2>>2,two"),
        Err(ParseError::BufferLimit)
    );

    let mut byte_limited = EventBuffer::with_limits(8, 8);
    assert_eq!(
        byte_limited.push_line("future>>too-long"),
        Err(ParseError::BufferLimit)
    );
}

#[test]
fn structured_snapshot_normalizes_and_bounds_synthetic_protocol_values() {
    let snapshot = parse_snapshot_json(
        r#"[{"id":1,"name":"SYNTH-1","focused":true,"scale":1.25,"activeWorkspace":{"id":1}}]"#,
        r#"[{"id":1,"name":"one","monitor":"SYNTH-1","windows":1,"hasfullscreen":true}]"#,
        r#"[{"address":"0xAbC123","class":"SyntheticClass","title":"Synthetic title","workspace":{"id":1},"monitor":1,"fullscreen":1}]"#,
        r#"{"id":1}"#,
        r#"{"address":"0xAbC123","class":"SyntheticClass","title":"Synthetic title"}"#,
    )
    .expect("synthetic snapshot");

    assert_eq!(snapshot.outputs.len(), 1);
    let output = snapshot.outputs.values().next().expect("one output");
    assert!(output.focused);
    assert_eq!(output.scale_milli, 1250);
    assert!(output.fullscreen);
    assert_eq!(snapshot.workspaces[&WorkspaceId::new(1)].clients, 1);
    assert_eq!(
        snapshot.active.client.expect("active client").as_str(),
        "abc123"
    );
    assert_eq!(snapshot.clients.len(), 1);
}

#[test]
fn invalid_snapshot_data_is_rejected_without_desktop_payload_in_errors() {
    let payload = "private-synthetic-payload";
    let error = parse_snapshot_json(
        "[]",
        "[]",
        &format!(r#"[{{"address":"{payload}"}}]"#),
        "{}",
        "{}",
    )
    .expect_err("invalid client address");
    assert_eq!(error, ParseError::InvalidSnapshot);
    assert!(!error.to_string().contains(payload));
}

#[test]
fn lifecycle_events_force_resnapshot_instead_of_best_effort_state_guessing() {
    for line in [
        "monitoraddedv2>>SYNTH-3,description",
        "monitorremoved>>SYNTH-1",
        "configreloaded>>",
    ] {
        assert_eq!(
            parse_event_line(line).unwrap(),
            Some(HyprlandEvent::ResnapshotRequired)
        );
    }
}

#[test]
fn real_openwindow_event_retains_address_and_named_workspace() {
    assert_eq!(
        parse_event_line("openwindow>>0xAbC123,synthetic-dev,SyntheticClass,title, with comma")
            .expect("valid event"),
        Some(HyprlandEvent::ClientOpened(OpenedClient {
            address: weftwise::state::ClientAddress::new("abc123").expect("valid address"),
            class: weftwise::state::DisplayText::new("SyntheticClass", 128),
            title: weftwise::state::DisplayText::new("title, with comma", 256),
            workspace_name: weftwise::state::DisplayText::new("synthetic-dev", 128),
        }))
    );
    assert_eq!(
        parse_event_line("openwindow>>0xAbC123,synthetic-dev,,").expect("empty display fields"),
        Some(HyprlandEvent::ClientOpened(OpenedClient {
            address: weftwise::state::ClientAddress::new("abc123").expect("valid address"),
            class: weftwise::state::DisplayText::default(),
            title: weftwise::state::DisplayText::default(),
            workspace_name: weftwise::state::DisplayText::new("synthetic-dev", 128),
        }))
    );
    assert_eq!(parse_event_line("openwindowv2>>opaque").unwrap(), None);
}

#[test]
fn workspace_catalog_forces_a_snapshot_when_open_identity_cannot_be_resolved() {
    let snapshot = parse_snapshot_json(
        r#"[{"id":1,"name":"SYNTH-1","focused":true,"scale":1.0,"activeWorkspace":{"id":-2}}]"#,
        r#"[{"id":-2,"name":"special:synthetic","monitor":"SYNTH-1","windows":0,"hasfullscreen":false}]"#,
        "[]",
        r#"{"id":-2}"#,
        "{}",
    )
    .expect("synthetic special-workspace snapshot");
    let mut catalog = WorkspaceCatalog::from_snapshot(&snapshot);
    let known = parse_event_line("openwindow>>0xabc123,special:synthetic,,")
        .expect("valid event")
        .expect("known event");
    assert!(catalog.observe(&known).is_ok());

    let unknown = parse_event_line("openwindow>>0xdef456,special:missing,,")
        .expect("valid event")
        .expect("known event");
    assert!(matches!(
        catalog.observe(&unknown),
        Err(AdapterError::SnapshotGap)
    ));
}

#[test]
fn paired_legacy_events_do_not_duplicate_address_bearing_updates() {
    for line in [
        "workspace>>one",
        "focusedmon>>SYNTH-1,one",
        "movewindow>>0xabc123,one",
        "windowtitle>>Synthetic title",
    ] {
        assert_eq!(parse_event_line(line).expect("legacy event"), None);
    }

    assert!(parse_event_line("workspacev2>>1,one").unwrap().is_some());
    assert!(
        parse_event_line("focusedmonv2>>SYNTH-1,1")
            .unwrap()
            .is_some()
    );
    assert!(
        parse_event_line("activewindowv2>>0xabc123")
            .unwrap()
            .is_some()
    );
    assert!(
        parse_event_line("activewindow>>SyntheticClass,Synthetic title")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        parse_event_line("activespecialv2>>,,").unwrap(),
        Some(HyprlandEvent::ResnapshotRequired)
    );
    assert_eq!(
        parse_event_line("workspacev2>>-2,special:synthetic").unwrap(),
        Some(HyprlandEvent::WorkspaceChanged {
            id: WorkspaceId::new(-2),
            name: DisplayText::new("special:synthetic", 128),
        })
    );
}
