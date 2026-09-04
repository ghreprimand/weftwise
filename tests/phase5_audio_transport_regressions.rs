use weftwise::services::audio::{
    AudioCapabilities, AudioCommandError, AudioCommandKind, AudioCommandOutcome, AudioDirection,
    AudioNode, AudioNodeId, AudioUpdate, AudioVolume, Generation,
};
use weftwise::state::{AdapterAvailability, AppState, OutputId};

fn state_with_output(id: u64) -> (AppState, OutputId) {
    let output = OutputId::new(id);
    let mut state = AppState::default();
    state.reconcile_outputs([output], [], false);
    (state, output)
}

fn node(id: u32, direction: AudioDirection, muted: bool) -> AudioNode {
    AudioNode::bounded(
        AudioNodeId::new(id),
        direction,
        &format!("synthetic-{id}"),
        "Synthetic audio device",
        AudioVolume::from_linear_millis(500),
        muted,
        true,
    )
}

#[test]
fn transport_restart_retains_stale_state_then_accepts_only_the_fresh_snapshot() {
    let (mut state, output) = state_with_output(510);
    let initial = node(1, AudioDirection::Sink, false);
    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![initial],
        default_sink: Some(AudioNodeId::new(1)),
        default_source: None,
        generation: Generation::new(1),
        observed_millis: 1,
    });

    assert!(state.apply_audio_update(AudioUpdate::Connecting).is_empty());
    assert_eq!(state.audio.availability, AdapterAvailability::Stale);
    assert_eq!(
        state.apply_audio_update(AudioUpdate::Unavailable),
        vec![output]
    );

    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![node(2, AudioDirection::Sink, false)],
        default_sink: Some(AudioNodeId::new(2)),
        default_source: None,
        generation: Generation::new(2),
        observed_millis: 2,
    });
    assert_eq!(state.audio.availability, AdapterAvailability::Ready);
    assert!(state.audio.node(AudioNodeId::new(1)).is_none());
    assert_eq!(
        state.audio.default_sink().map(|sink| sink.id),
        Some(AudioNodeId::new(2))
    );
}

#[test]
fn command_outcomes_report_success_and_transport_errors_without_device_content() {
    let (mut state, output) = state_with_output(511);
    state.apply_audio_update(AudioUpdate::CommandOutcome {
        outcome: AudioCommandOutcome {
            label: "Set output volume".to_owned(),
            error: None,
        },
        observed_millis: 1,
    });
    assert_eq!(
        state
            .output_view(output)
            .expect("success feedback")
            .activity[0]
            .accessible_label,
        "Set output volume request sent"
    );

    state.apply_audio_update(AudioUpdate::CommandOutcome {
        outcome: AudioCommandOutcome {
            label: "Set output volume".to_owned(),
            error: Some(AudioCommandError::Transport),
        },
        observed_millis: 2,
    });
    state.flush_feedback(251);
    assert_eq!(
        state
            .output_view(output)
            .expect("failure feedback")
            .activity[0]
            .accessible_label,
        "Set output volume: audio service error"
    );
}

#[test]
fn default_source_needs_a_known_baseline_before_microphone_feedback() {
    let (mut state, output) = state_with_output(512);
    let mut source = node(3, AudioDirection::Source, false);
    source.capabilities = AudioCapabilities::default();
    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![source.clone()],
        default_sink: None,
        default_source: Some(source.id),
        generation: Generation::new(1),
        observed_millis: 1,
    });

    source.capabilities.can_set_mute = true;
    source.muted = true;
    state.apply_audio_update(AudioUpdate::NodeChanged {
        node: source.clone(),
        observed_millis: 2,
    });
    assert!(
        state
            .output_view(output)
            .expect("first source observation")
            .activity
            .is_empty()
    );

    source.muted = false;
    state.apply_audio_update(AudioUpdate::NodeChanged {
        node: source,
        observed_millis: 3,
    });
    assert_eq!(
        state
            .output_view(output)
            .expect("known source change")
            .activity[0]
            .accessible_label,
        "Microphone on"
    );
}

#[test]
fn rapid_microphone_mute_deltas_coalesce_to_the_last_known_state() {
    let (mut state, output) = state_with_output(513);
    let source = node(4, AudioDirection::Source, false);
    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![source],
        default_sink: None,
        default_source: Some(AudioNodeId::new(4)),
        generation: Generation::new(1),
        observed_millis: 1,
    });

    for (at, muted) in [(2, true), (3, false), (4, true)] {
        state.apply_audio_update(AudioUpdate::NodeChanged {
            node: node(4, AudioDirection::Source, muted),
            observed_millis: at,
        });
    }
    assert_eq!(state.flush_feedback(105), vec![output]);
    let view = state
        .output_view(output)
        .expect("coalesced microphone feedback");
    assert_eq!(view.activity.len(), 1);
    assert_eq!(view.activity[0].accessible_label, "Microphone muted");
}

#[test]
fn queued_non_move_audio_command_is_rejected_after_connection_restart() {
    let (mut state, _) = state_with_output(514);
    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![node(5, AudioDirection::Sink, false)],
        default_sink: Some(AudioNodeId::new(5)),
        default_source: None,
        generation: Generation::new(1),
        observed_millis: 1,
    });
    let (queued, _) = state.audio_command(
        AudioCommandKind::SetVolume {
            id: AudioNodeId::new(5),
            volume: AudioVolume::from_linear_millis(700),
        },
        2,
    );
    let queued = queued.expect("initial connection accepts command");

    state.apply_audio_update(AudioUpdate::Connecting);
    state.apply_audio_update(AudioUpdate::Snapshot {
        nodes: vec![node(5, AudioDirection::Sink, false)],
        default_sink: Some(AudioNodeId::new(5)),
        default_source: None,
        generation: Generation::new(2),
        observed_millis: 3,
    });

    assert!(
        state.audio.validate(queued.kind).is_err(),
        "a command without a connection generation must not retarget a reused node id"
    );
}
