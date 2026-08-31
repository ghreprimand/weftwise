use weftwise::context::arbitration::Severity;
use weftwise::context::feedback::{FeedbackEvent, FeedbackKind};
use weftwise::context::privacy::{PrivacyEvidence, PrivacyState, PrivacyUpdate};
use weftwise::services::audio::{
    AudioCapabilities, AudioCommandKind, AudioDirection, AudioNode, AudioNodeId, AudioUpdate,
    AudioVolume, Generation, MAX_AUDIO_NAME_CHARACTERS, MAX_AUDIO_NODES, MovableStreamState,
};
use weftwise::services::capture::{CaptureGraph, CaptureKind, DeviceApi, NodeRole, PortDirection};
use weftwise::state::{
    AdapterAvailability, AppState, HyprlandEvent, HyprlandSnapshot, HyprlandUpdate, MarkPattern,
    MarkShape, OutputId,
};

fn state_with_output(id: u64) -> (AppState, OutputId) {
    let output = OutputId::new(id);
    let mut state = AppState::default();
    state.reconcile_outputs([output], [], false);
    (state, output)
}

#[test]
fn privacy_states_remain_distinct_and_only_confirmed_or_uncertain_evidence_is_rendered() {
    let (mut state, output) = state_with_output(61);

    state.apply_privacy_update(
        PrivacyUpdate::Supported {
            evidence: PrivacyEvidence::Microphone,
            supported: true,
        },
        1,
    );
    assert_eq!(
        state.privacy.state(PrivacyEvidence::Microphone),
        PrivacyState::Unknown
    );
    assert!(
        state
            .output_view(output)
            .expect("unknown view")
            .attention
            .is_empty()
    );

    state.apply_privacy_update(
        PrivacyUpdate::Observed {
            evidence: PrivacyEvidence::Microphone,
            state: PrivacyState::Active,
        },
        2,
    );
    let active = state.output_view(output).expect("active view");
    assert_eq!(active.attention.len(), 1);
    assert_eq!(active.attention[0].shape, MarkShape::Triangle);
    assert_eq!(active.attention[0].pattern, MarkPattern::Striped);
    assert_eq!(active.attention[0].accessible_label, "Microphone active");

    state.apply_privacy_update(
        PrivacyUpdate::Observed {
            evidence: PrivacyEvidence::Microphone,
            state: PrivacyState::Inactive,
        },
        3,
    );
    assert_eq!(
        state.privacy.state(PrivacyEvidence::Microphone),
        PrivacyState::Inactive
    );
    assert!(
        state
            .output_view(output)
            .expect("inactive view")
            .attention
            .is_empty()
    );
}

#[test]
fn unavailable_and_stale_privacy_evidence_stays_visible_until_a_fresh_observation() {
    let (mut state, output) = state_with_output(62);
    state.apply_privacy_update(
        PrivacyUpdate::Supported {
            evidence: PrivacyEvidence::ScreenShare,
            supported: true,
        },
        1,
    );
    state.apply_privacy_update(PrivacyUpdate::Unavailable(PrivacyEvidence::ScreenShare), 2);
    let unavailable = state.output_view(output).expect("unavailable view");
    assert_eq!(
        state.privacy.state(PrivacyEvidence::ScreenShare),
        PrivacyState::Unavailable
    );
    assert_eq!(unavailable.attention[0].shape, MarkShape::Triangle);
    assert_eq!(
        unavailable.attention[0].accessible_label,
        "Screen sharing unavailable"
    );

    state.apply_privacy_update(
        PrivacyUpdate::Supported {
            evidence: PrivacyEvidence::Camera,
            supported: true,
        },
        3,
    );
    state.apply_privacy_update(
        PrivacyUpdate::Observed {
            evidence: PrivacyEvidence::Camera,
            state: PrivacyState::Active,
        },
        4,
    );
    state.apply_privacy_update(PrivacyUpdate::Degraded, 5);
    assert_eq!(
        state.privacy.state(PrivacyEvidence::Camera),
        PrivacyState::Stale
    );
    let stale = state.output_view(output).expect("stale view");
    assert!(
        stale
            .attention
            .iter()
            .any(|mark| mark.accessible_label == "Camera state stale")
    );
    assert_eq!(
        state.privacy.state(PrivacyEvidence::ScreenShare),
        PrivacyState::Unavailable
    );

    state.apply_privacy_update(
        PrivacyUpdate::Observed {
            evidence: PrivacyEvidence::Camera,
            state: PrivacyState::Inactive,
        },
        6,
    );
    assert_eq!(
        state.privacy.state(PrivacyEvidence::Camera),
        PrivacyState::Inactive
    );
    let recovered = state.output_view(output).expect("recovered view");
    assert_eq!(recovered.attention.len(), 1);
    assert_eq!(
        recovered.attention[0].accessible_label,
        "Screen sharing unavailable"
    );
}

#[test]
fn hyprland_screencast_clients_are_counted_and_gaps_remain_uncertain() {
    let (mut state, output) = state_with_output(65);
    state.apply_hyprland_update(HyprlandUpdate::Snapshot(HyprlandSnapshot::default()));
    assert!(state.privacy.is_supported(PrivacyEvidence::ScreenShare));
    assert_eq!(
        state.privacy.state(PrivacyEvidence::ScreenShare),
        PrivacyState::Unknown
    );

    state.apply_hyprland_update(HyprlandUpdate::Event(HyprlandEvent::ScreencastChanged(
        true,
    )));
    state.apply_hyprland_update(HyprlandUpdate::Event(HyprlandEvent::ScreencastChanged(
        true,
    )));
    state.apply_hyprland_update(HyprlandUpdate::Event(HyprlandEvent::ScreencastChanged(
        false,
    )));
    assert_eq!(
        state.privacy.state(PrivacyEvidence::ScreenShare),
        PrivacyState::Active
    );
    assert_eq!(state.output_view(output).unwrap().attention.len(), 1);

    state.apply_hyprland_update(HyprlandUpdate::Event(HyprlandEvent::ScreencastChanged(
        false,
    )));
    assert_eq!(
        state.privacy.state(PrivacyEvidence::ScreenShare),
        PrivacyState::Unknown
    );
    state.apply_hyprland_update(HyprlandUpdate::Gap);
    assert_eq!(
        state.privacy.state(PrivacyEvidence::ScreenShare),
        PrivacyState::Stale
    );
    assert_eq!(state.output_view(output).unwrap().attention.len(), 1);

    state.apply_hyprland_update(HyprlandUpdate::Snapshot(HyprlandSnapshot::default()));
    assert_eq!(
        state.privacy.state(PrivacyEvidence::ScreenShare),
        PrivacyState::Unknown
    );
}

#[test]
fn rapid_volume_feedback_is_coalesced_to_the_latest_bounded_value() {
    let (mut state, output) = state_with_output(63);
    let first = FeedbackEvent::new(FeedbackKind::Volume, "Volume 40%")
        .with_accessible_label("Synthetic output volume 40%")
        .with_progress(4_000);
    assert_eq!(state.apply_feedback(first, 1), vec![output]);

    let long_value = format!("Synthetic volume {}", "x".repeat(300));
    let rapid = FeedbackEvent::new(FeedbackKind::Volume, long_value)
        .with_accessible_label("Synthetic output volume 60%")
        .with_progress(50_000);
    assert!(state.apply_feedback(rapid, 20).is_empty());
    assert!(state.flush_feedback(39).is_empty());
    assert_eq!(state.flush_feedback(41), vec![output]);

    let view = state.output_view(output).expect("feedback view");
    assert_eq!(view.activity.len(), 1);
    assert_eq!(
        view.activity[0].accessible_label,
        "Synthetic output volume 60%"
    );
    assert_eq!(view.activity[0].progress_basis_points, Some(10_000));
    assert_eq!(view.activity[0].shape, MarkShape::Bar);
    assert_eq!(view.activity[0].pattern, MarkPattern::Solid);
}

#[test]
fn feedback_ttl_expires_without_requiring_an_unrelated_state_change() {
    let (mut state, output) = state_with_output(64);
    assert_eq!(
        state.apply_feedback(FeedbackEvent::new(FeedbackKind::Volume, "Volume 50%"), 1),
        vec![output]
    );
    assert_eq!(
        state
            .output_view(output)
            .expect("initial view")
            .activity
            .len(),
        1
    );

    // Volume feedback has a 1,500ms TTL. Advancing the feedback clock beyond
    // the boundary must remove it even when no newer event is pending.
    assert_eq!(state.flush_feedback(1_501), vec![output]);
    assert!(
        state
            .output_view(output)
            .expect("expired view")
            .activity
            .is_empty()
    );
}

#[test]
fn dismissible_feedback_has_actions_and_explicit_dismissal_clears_it() {
    let (mut state, output) = state_with_output(65);
    let event = FeedbackEvent::new(FeedbackKind::CommandResult, "Synthetic operation complete")
        .with_severity(Severity::Notice);
    assert_eq!(state.apply_feedback(event, 1), vec![output]);
    assert_eq!(
        state
            .output_view(output)
            .expect("command result")
            .candidate_actions
            .len(),
        1
    );

    assert_eq!(
        state.dismiss_feedback(FeedbackKind::CommandResult, 2),
        vec![output]
    );
    let dismissed = state.output_view(output).expect("dismissed result");
    assert!(dismissed.activity.is_empty());
    assert!(dismissed.candidate_actions.is_empty());
}

fn synthetic_audio_node(id: u32, direction: AudioDirection, volume: u32, muted: bool) -> AudioNode {
    AudioNode::bounded(
        AudioNodeId::new(id),
        direction,
        &format!("synthetic-{id}"),
        "Synthetic audio device",
        AudioVolume::from_linear_millis(volume),
        muted,
        true,
    )
}

#[test]
fn audio_snapshot_is_bounded_and_establishes_state_before_following_deltas() {
    let (mut state, output) = state_with_output(66);
    let long_name = format!("synthetic\u{200b}-{}", "x".repeat(300));
    let mut first = AudioNode::bounded(
        AudioNodeId::new(0),
        AudioDirection::Sink,
        &long_name,
        "Synthetic audio device",
        AudioVolume::from_linear_millis(9_999),
        false,
        true,
    );
    first.capabilities = AudioCapabilities {
        can_set_volume: true,
        can_set_mute: true,
    };
    let mut nodes = vec![first];
    nodes.extend(
        (1..=(MAX_AUDIO_NODES as u32 + 6))
            .map(|id| synthetic_audio_node(id, AudioDirection::Sink, 500, false)),
    );

    assert_eq!(state.audio.availability, AdapterAvailability::Starting);
    assert_eq!(
        state.apply_audio_update(AudioUpdate::Snapshot {
            nodes,
            default_sink: Some(AudioNodeId::new(0)),
            default_source: Some(AudioNodeId::new(u32::MAX)),
            observed_millis: 1,
        }),
        vec![output]
    );
    assert_eq!(state.audio.availability, AdapterAvailability::Ready);
    assert_eq!(state.audio.len(), MAX_AUDIO_NODES);
    let sink = state.audio.default_sink().expect("resolved default sink");
    assert_eq!(sink.volume.linear_millis(), 4_000);
    assert!(sink.name.as_str().chars().count() <= MAX_AUDIO_NAME_CHARACTERS);
    assert!(!sink.name.as_str().contains('\u{200b}'));
    assert!(state.audio.default_source().is_none());

    let replacement = synthetic_audio_node(0, AudioDirection::Sink, 750, true);
    assert_eq!(
        state.apply_audio_update(AudioUpdate::NodeChanged {
            node: replacement,
            observed_millis: 2,
        }),
        vec![output]
    );
    let updated = state.audio.default_sink().expect("delta after snapshot");
    assert_eq!(updated.volume.linear_millis(), 750);
    assert!(updated.muted);
}

#[test]
fn initial_audio_snapshot_does_not_present_an_unknown_volume_as_zero() {
    let (mut state, output) = state_with_output(69);
    let mut placeholder = synthetic_audio_node(12, AudioDirection::Sink, 0, false);
    placeholder.capabilities = AudioCapabilities::default();

    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![placeholder.clone()],
        default_sink: Some(placeholder.id),
        default_source: None,
        observed_millis: 1,
    });
    assert!(
        state
            .output_view(output)
            .expect("placeholder view")
            .activity
            .is_empty()
    );

    let mut observed = placeholder;
    observed.volume = AudioVolume::from_linear_millis(800);
    observed.capabilities.can_set_volume = true;
    state.apply_audio_update(AudioUpdate::NodeChanged {
        node: observed.clone(),
        observed_millis: 2,
    });
    assert!(
        state
            .output_view(output)
            .expect("first observed view")
            .activity
            .is_empty()
    );

    observed.volume = AudioVolume::from_linear_millis(700);
    state.apply_audio_update(AudioUpdate::NodeChanged {
        node: observed,
        observed_millis: 50,
    });
    let changed = state.output_view(output).expect("changed volume view");
    assert_eq!(changed.activity.len(), 1);
    assert_ne!(
        changed.activity[0].accessible_label,
        "Output volume 0 percent"
    );
}

#[test]
fn audio_retains_stale_state_across_loss_and_recovers_on_a_fresh_snapshot() {
    let (mut state, output) = state_with_output(67);
    let initial = synthetic_audio_node(8, AudioDirection::Sink, 500, false);
    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![initial],
        default_sink: Some(AudioNodeId::new(8)),
        default_source: None,
        observed_millis: 1,
    });
    assert_eq!(state.audio.availability, AdapterAvailability::Ready);

    assert!(state.apply_audio_update(AudioUpdate::Connecting).is_empty());
    assert_eq!(state.audio.availability, AdapterAvailability::Stale);
    assert_eq!(
        state.apply_audio_update(AudioUpdate::Unavailable),
        vec![output]
    );
    assert_eq!(state.audio.availability, AdapterAvailability::Stale);
    assert_eq!(
        state.audio.default_sink().map(|node| node.id),
        Some(AudioNodeId::new(8))
    );

    let recovered = synthetic_audio_node(9, AudioDirection::Sink, 900, false);
    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![recovered],
        default_sink: Some(AudioNodeId::new(9)),
        default_source: None,
        observed_millis: 2,
    });
    assert_eq!(state.audio.availability, AdapterAvailability::Ready);
    assert_eq!(
        state.audio.default_sink().map(|node| node.id),
        Some(AudioNodeId::new(9))
    );
}

#[test]
fn audio_commands_are_capability_gated_and_unsupported_route_actions_are_visible() {
    let (mut state, output) = state_with_output(68);
    let mut sink = synthetic_audio_node(10, AudioDirection::Sink, 500, false);
    sink.capabilities = AudioCapabilities::default();
    let source = synthetic_audio_node(11, AudioDirection::Source, 500, false);
    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![sink, source],
        default_sink: None,
        default_source: None,
        observed_millis: 1,
    });

    let (command, changed) = state.audio_command(
        AudioCommandKind::SetVolume {
            id: AudioNodeId::new(10),
            volume: AudioVolume::from_linear_millis(650),
        },
        2,
    );
    assert!(command.is_none());
    assert_eq!(changed, vec![output]);

    let (route, changed) = state.audio_command(
        AudioCommandKind::MoveStream {
            stream: AudioNodeId::new(11),
            target: AudioNodeId::new(10),
        },
        300,
    );
    assert!(route.is_none());
    assert_eq!(changed, vec![output]);
    let view = state
        .output_view(output)
        .expect("unsupported route feedback");
    assert_eq!(view.activity.len(), 1);
    assert_eq!(
        view.activity[0].accessible_label,
        "Audio: control unsupported"
    );
}

#[test]
fn audio_service_loss_hotplug_and_recovery_replace_stale_defaults() {
    let (mut state, output) = state_with_output(70);
    let first = synthetic_audio_node(20, AudioDirection::Sink, 500, false);
    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![first],
        default_sink: Some(AudioNodeId::new(20)),
        default_source: None,
        observed_millis: 1,
    });

    // Transport loss retains an explicitly stale projection rather than
    // treating the old endpoint as live. A fresh graph can then replace it.
    assert_eq!(
        state.apply_audio_update(AudioUpdate::Unavailable),
        vec![output]
    );
    assert_eq!(state.audio.availability, AdapterAvailability::Stale);

    let hotplugged = synthetic_audio_node(21, AudioDirection::Sink, 650, false);
    assert_eq!(
        state.apply_audio_update(AudioUpdate::Snapshot {
            nodes: vec![hotplugged],
            default_sink: Some(AudioNodeId::new(21)),
            default_source: None,
            observed_millis: 2,
        }),
        vec![output]
    );
    assert_eq!(state.audio.availability, AdapterAvailability::Ready);
    assert!(state.audio.node(AudioNodeId::new(20)).is_none());
    assert_eq!(
        state.audio.default_sink().map(|node| node.id),
        Some(AudioNodeId::new(21))
    );
}

#[test]
fn route_removal_revokes_move_capability_before_a_new_route_is_offered() {
    let (mut state, output) = state_with_output(71);
    let sink = synthetic_audio_node(30, AudioDirection::Sink, 500, false);
    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![sink],
        default_sink: Some(AudioNodeId::new(30)),
        default_source: None,
        observed_millis: 1,
    });
    state.apply_audio_update(AudioUpdate::MovableStreamChanged {
        state: MovableStreamState::Active {
            stream: AudioNodeId::new(31),
            generation: Generation::new(4),
        },
        observed_millis: 2,
    });
    assert!(
        state
            .audio_command(
                AudioCommandKind::MoveStream {
                    stream: AudioNodeId::new(31),
                    target: AudioNodeId::new(30),
                },
                3,
            )
            .0
            .is_some()
    );

    // The adapter removes the active stream selection before another request
    // can be dispatched. The former route must not remain actionable.
    state.apply_audio_update(AudioUpdate::MovableStreamChanged {
        state: MovableStreamState::Unavailable,
        observed_millis: 4,
    });
    let (command, changed) = state.audio_command(
        AudioCommandKind::MoveStream {
            stream: AudioNodeId::new(31),
            target: AudioNodeId::new(30),
        },
        5,
    );
    assert!(command.is_none());
    assert_eq!(changed, vec![output]);
    assert_eq!(
        state.output_view(output).unwrap().activity[0].accessible_label,
        "Audio: control unsupported"
    );
}

#[test]
fn rapid_audio_volume_deltas_coalesce_to_the_latest_default_sink_state() {
    let (mut state, output) = state_with_output(72);
    let initial = synthetic_audio_node(40, AudioDirection::Sink, 400, false);
    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![initial],
        default_sink: Some(AudioNodeId::new(40)),
        default_source: None,
        observed_millis: 1,
    });

    for (at, volume) in [(2, 500), (3, 600), (4, 700)] {
        state.apply_audio_update(AudioUpdate::NodeChanged {
            node: synthetic_audio_node(40, AudioDirection::Sink, volume, false),
            observed_millis: at,
        });
    }
    assert_eq!(state.flush_feedback(45), vec![output]);
    let view = state.output_view(output).expect("coalesced audio feedback");
    assert_eq!(view.activity.len(), 1);
    // PipeWire's cubic UI scale makes linear 700 map to 89 percent.
    assert_eq!(
        view.activity[0].accessible_label,
        "Output volume 89 percent"
    );
    assert_eq!(view.activity[0].progress_basis_points, Some(8_900));
}

#[test]
fn capture_graph_handles_out_of_order_events_and_removal_without_false_activity() {
    let mut graph = CaptureGraph::new();
    // State arriving before its object announcement is ignored, so an event
    // race cannot surface a capture indicator before a complete path exists.
    assert!(!graph.set_node_running(50, true));
    assert!(!graph.set_link_active(53, true));
    assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);

    assert!(graph.upsert_port(51, 50, PortDirection::Output));
    assert!(graph.upsert_port(54, 55, PortDirection::Input));
    assert!(graph.upsert_link(53, 51, 54));
    assert!(graph.upsert_device(49, DeviceApi::Alsa));
    assert!(graph.upsert_node(50, NodeRole::AudioSource, Some(49)));
    assert!(graph.upsert_node(55, NodeRole::AudioInputStream, None));
    assert!(graph.set_node_running(50, true));
    assert!(graph.set_node_running(55, true));
    assert!(graph.set_link_active(53, true));
    assert!(graph.path_active(CaptureKind::Microphone));

    assert!(graph.remove(53));
    assert!(!graph.path_active(CaptureKind::Microphone));
    assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
}

#[test]
fn stale_privacy_evidence_remains_visible_across_service_loss_until_recovered() {
    let (mut state, output) = state_with_output(73);
    for evidence in [PrivacyEvidence::Microphone, PrivacyEvidence::Camera] {
        state.apply_privacy_update(
            PrivacyUpdate::Supported {
                evidence,
                supported: true,
            },
            1,
        );
        state.apply_privacy_update(
            PrivacyUpdate::Observed {
                evidence,
                state: PrivacyState::Active,
            },
            2,
        );
    }
    state.apply_privacy_update(PrivacyUpdate::Degraded, 3);
    let stale = state.output_view(output).expect("stale privacy view");
    assert_eq!(
        state.privacy.state(PrivacyEvidence::Microphone),
        PrivacyState::Stale
    );
    assert_eq!(
        state.privacy.state(PrivacyEvidence::Camera),
        PrivacyState::Stale
    );
    assert!(
        stale
            .attention
            .iter()
            .any(|mark| mark.accessible_label.ends_with("state stale"))
    );

    for evidence in [PrivacyEvidence::Microphone, PrivacyEvidence::Camera] {
        state.apply_privacy_update(
            PrivacyUpdate::Observed {
                evidence,
                state: PrivacyState::Inactive,
            },
            4,
        );
    }
    let recovered = state.output_view(output).expect("recovered privacy view");
    assert!(recovered.attention.is_empty());
    assert_eq!(
        state.privacy.state(PrivacyEvidence::Microphone),
        PrivacyState::Inactive
    );
    assert_eq!(
        state.privacy.state(PrivacyEvidence::Camera),
        PrivacyState::Inactive
    );
}

#[cfg(feature = "audio-transport")]
#[tokio::test]
async fn audio_command_sender_stays_open_while_a_supervisor_retains_it() {
    use tokio::time::{Duration, timeout};
    use weftwise::services::audio::{COMMAND_CAPACITY, command_channel};

    let (sender, mut receiver) = command_channel();
    let retained = sender.clone();
    drop(sender);
    retained
        .send(weftwise::services::audio::AudioCommand {
            kind: AudioCommandKind::SetMute {
                id: AudioNodeId::new(12),
                muted: true,
            },
        })
        .await
        .expect("retained sender accepts command");
    assert_eq!(COMMAND_CAPACITY, 16);
    assert!(receiver.recv().await.is_some());
    assert!(
        timeout(Duration::from_millis(1), receiver.recv())
            .await
            .is_err()
    );
    drop(retained);
    assert!(receiver.recv().await.is_none());
}
