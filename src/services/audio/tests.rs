//! Unit tests for the transport-independent PipeWire audio model.
//!
//! Kept in a sibling file so `mod.rs` stays focused on the model itself; as a
//! child module of `audio` these tests retain access to its private items.

use super::*;

fn sink(id: u32, name: &str) -> AudioNode {
    AudioNode::bounded(
        AudioNodeId::new(id),
        AudioDirection::Sink,
        name,
        "Synthetic sink",
        AudioVolume::from_linear_millis(500),
        false,
        true,
    )
}

fn source(id: u32, name: &str) -> AudioNode {
    AudioNode::bounded(
        AudioNodeId::new(id),
        AudioDirection::Source,
        name,
        "Synthetic source",
        AudioVolume::from_linear_millis(800),
        false,
        true,
    )
}

#[test]
fn volume_is_clamped_and_finite() {
    assert_eq!(
        AudioVolume::from_linear_millis(9_999).linear_millis(),
        MAX_VOLUME_LINEAR_MILLIS
    );
    assert_eq!(AudioVolume::from_linear(f32::NAN).linear_millis(), 0);
    assert_eq!(AudioVolume::from_linear(-1.0).linear_millis(), 0);
    assert_eq!(AudioVolume::from_linear(1.0).linear_millis(), 1_000);
    assert_eq!(
        AudioVolume::from_linear(100.0).linear_millis(),
        MAX_VOLUME_LINEAR_MILLIS
    );
}

#[test]
fn cubic_percent_is_bounded_display_only() {
    // Unity linear maps to 100 percent cubic.
    assert_eq!(AudioVolume::from_linear(1.0).cubic_percent(), 100);
    assert_eq!(AudioVolume::from_linear(0.0).cubic_percent(), 0);
}

#[test]
fn cubic_display_percent_round_trips_for_ui_commands() {
    for percent in [0, 10, 50, 100, 150] {
        assert_eq!(
            AudioVolume::from_cubic_percent(percent).cubic_percent(),
            percent
        );
    }
    assert_eq!(
        AudioVolume::from_cubic_percent(u16::MAX).linear_millis(),
        MAX_VOLUME_LINEAR_MILLIS
    );
}

#[test]
fn snapshot_bounds_nodes_and_filters_unknown_defaults() {
    let mut state = AudioState::default();
    let nodes = (0..(MAX_AUDIO_NODES as u32 + 5))
        .map(|id| sink(id, &format!("sink-{id}")))
        .collect::<Vec<_>>();
    state.apply_snapshot(
        nodes,
        Some(AudioNodeId::new(0)),
        Some(AudioNodeId::new(9_999)),
        Generation::new(1),
    );
    assert_eq!(state.len(), MAX_AUDIO_NODES);
    assert!(state.default_sink().is_some());
    // The unknown default source identity is dropped.
    assert!(state.default_source().is_none());
    assert_eq!(state.availability, AdapterAvailability::Ready);
}

#[test]
fn removing_a_node_clears_defaults_referencing_it() {
    let mut state = AudioState::default();
    state.apply_snapshot(
        vec![sink(1, "a"), source(2, "b")],
        Some(AudioNodeId::new(1)),
        Some(AudioNodeId::new(2)),
        Generation::new(1),
    );
    state.remove_node(AudioNodeId::new(1));
    assert!(state.default_sink().is_none());
    assert!(state.default_source().is_some());
}

#[test]
fn set_default_rejects_wrong_direction_and_unknown() {
    let mut state = AudioState::default();
    state.apply_snapshot(
        vec![sink(1, "a"), source(2, "b")],
        None,
        None,
        Generation::new(1),
    );
    state.set_default(AudioDirection::Sink, Some(AudioNodeId::new(2)));
    assert!(state.default_sink().is_none());
    state.set_default(AudioDirection::Sink, Some(AudioNodeId::new(1)));
    assert_eq!(
        state.default_sink().map(|node| node.id),
        Some(AudioNodeId::new(1))
    );
}

#[test]
fn stale_and_unavailable_preserve_state_semantics() {
    let mut state = AudioState::default();
    state.mark_unavailable();
    assert_eq!(state.availability, AdapterAvailability::Unavailable);
    state.apply_snapshot(
        vec![sink(1, "a")],
        Some(AudioNodeId::new(1)),
        None,
        Generation::new(1),
    );
    state.mark_stale();
    assert_eq!(state.availability, AdapterAvailability::Stale);
    state.mark_unavailable();
    // Retained nodes downgrade to Stale, never a false empty Unavailable.
    assert_eq!(state.availability, AdapterAvailability::Stale);
}

#[test]
fn command_validation_is_capability_gated() {
    let mut state = AudioState::default();
    state.apply_snapshot(
        vec![sink(1, "a"), source(2, "b")],
        None,
        None,
        Generation::new(1),
    );

    assert!(
        state
            .validate(AudioCommandKind::SetVolume {
                id: AudioNodeId::new(1),
                volume: AudioVolume::from_linear_millis(500),
            })
            .is_ok()
    );
    // Setting volume on a source is the wrong direction.
    assert_eq!(
        state.validate(AudioCommandKind::SetVolume {
            id: AudioNodeId::new(2),
            volume: AudioVolume::from_linear_millis(500),
        }),
        Err(AudioCommandError::WrongDirection)
    );
    // Muting the microphone source is allowed.
    assert!(
        state
            .validate(AudioCommandKind::SetMute {
                id: AudioNodeId::new(2),
                muted: true,
            })
            .is_ok()
    );
    // Unknown node.
    assert_eq!(
        state.validate(AudioCommandKind::ToggleMute {
            id: AudioNodeId::new(99),
        }),
        Err(AudioCommandError::UnknownNode)
    );
    // Move stream requires a sink target.
    assert_eq!(
        state.validate(AudioCommandKind::MoveStream {
            stream: AudioNodeId::new(1),
            target: AudioNodeId::new(2),
        }),
        Err(AudioCommandError::WrongDirection)
    );
    assert_eq!(
        state.validate(AudioCommandKind::MoveStream {
            stream: AudioNodeId::new(2),
            target: AudioNodeId::new(1),
        }),
        Err(AudioCommandError::Unsupported)
    );
}

#[test]
fn unavailable_capability_blocks_control() {
    let mut state = AudioState::default();
    let mut node = sink(1, "a");
    node.available = false;
    node.capabilities = AudioCapabilities::default();
    // A complete snapshot establishes a live generation, so the capability
    // gate (not the readiness gate) is what rejects the request.
    state.apply_snapshot(vec![node], None, None, Generation::new(1));
    assert_eq!(
        state.validate(AudioCommandKind::SetVolume {
            id: AudioNodeId::new(1),
            volume: AudioVolume::from_linear_millis(100),
        }),
        Err(AudioCommandError::Unsupported)
    );
}

#[test]
fn scalar_control_requires_a_live_ready_generation() {
    let mut state = AudioState::default();
    // Without a snapshot the adapter is not Ready: every scalar control is
    // refused with NotReady before any capability check.
    state.upsert_node(sink(1, "a"));
    assert_eq!(
        state.validate(AudioCommandKind::SetVolume {
            id: AudioNodeId::new(1),
            volume: AudioVolume::from_linear_millis(100),
        }),
        Err(AudioCommandError::NotReady)
    );

    // A complete snapshot adopts the transport generation and stamps the
    // command with it.
    state.apply_snapshot(
        vec![sink(1, "a")],
        Some(AudioNodeId::new(1)),
        None,
        Generation::new(1),
    );
    let stamped = state
        .validate(AudioCommandKind::SetVolume {
            id: AudioNodeId::new(1),
            volume: AudioVolume::from_linear_millis(100),
        })
        .expect("ready connection stamps the command");
    assert!(matches!(stamped.kind, AudioCommandKind::SetVolumeOn { .. }));

    // A reconnect adopts a newer generation, so the queued stamp is rejected
    // instead of retargeting a coincidentally reused id.
    state.mark_stale();
    state.apply_snapshot(
        vec![sink(1, "a")],
        Some(AudioNodeId::new(1)),
        None,
        Generation::new(2),
    );
    assert_eq!(
        state.validate(stamped.kind),
        Err(AudioCommandError::NotReady)
    );
}

#[test]
fn endpoint_class_matches_exactly() {
    assert_eq!(
        is_audio_endpoint_class("Audio/Sink"),
        Some(AudioDirection::Sink)
    );
    assert_eq!(
        is_audio_endpoint_class(" Audio/Source \n"),
        Some(AudioDirection::Source)
    );
    for near in [
        "Audio/Sink/Virtual",
        "Audio/Source/Internal",
        "XAudio/Sink",
        "Audio/SinkX",
        "Stream/Output/Audio",
        "Audio/Duplex",
        "",
    ] {
        assert_eq!(is_audio_endpoint_class(near), None, "{near} must not match");
    }
}

#[test]
fn degraded_retains_partial_state_as_stale_and_blocks_control() {
    let mut state = AudioState::default();
    state.apply_snapshot(
        vec![sink(1, "a")],
        Some(AudioNodeId::new(1)),
        None,
        Generation::new(1),
    );
    assert_eq!(state.availability, AdapterAvailability::Ready);

    state.mark_degraded();
    // Partial data is retained but as Stale uncertainty, and no scalar
    // control can be issued because the generation is cleared.
    assert_eq!(state.availability, AdapterAvailability::Stale);
    assert!(state.node(AudioNodeId::new(1)).is_some());
    assert_eq!(
        state.validate(AudioCommandKind::ToggleMute {
            id: AudioNodeId::new(1)
        }),
        Err(AudioCommandError::NotReady)
    );

    // An incremental upsert must not restore Ready over a degraded state.
    state.upsert_node(sink(1, "a"));
    assert_eq!(state.availability, AdapterAvailability::Stale);
    // Only a clean resnapshot re-establishes Ready and a fresh generation.
    state.apply_snapshot(
        vec![sink(1, "a")],
        Some(AudioNodeId::new(1)),
        None,
        Generation::new(1),
    );
    assert_eq!(state.availability, AdapterAvailability::Ready);
}

#[test]
fn command_error_reasons_are_content_free() {
    assert_eq!(
        AudioCommandError::UnknownNode.reason(),
        "device unavailable"
    );
    assert_eq!(AudioCommandError::Transport.reason(), "audio service error");
}

#[test]
fn set_default_validation_is_capability_and_availability_gated() {
    let mut state = AudioState::default();
    let mut unavailable = sink(3, "offline");
    unavailable.available = false;
    state.apply_snapshot(
        vec![sink(1, "a"), source(2, "b"), unavailable],
        None,
        None,
        Generation::new(1),
    );

    // An available sink can become the default sink.
    assert!(
        state
            .validate(AudioCommandKind::SetDefault {
                direction: AudioDirection::Sink,
                id: AudioNodeId::new(1),
            })
            .is_ok()
    );
    // A source cannot become the default sink.
    assert_eq!(
        state.validate(AudioCommandKind::SetDefault {
            direction: AudioDirection::Sink,
            id: AudioNodeId::new(2),
        }),
        Err(AudioCommandError::WrongDirection)
    );
    // An unavailable node cannot be selected as a default.
    assert_eq!(
        state.validate(AudioCommandKind::SetDefault {
            direction: AudioDirection::Sink,
            id: AudioNodeId::new(3),
        }),
        Err(AudioCommandError::Unsupported)
    );
    // An unknown node is rejected.
    assert_eq!(
        state.validate(AudioCommandKind::SetDefault {
            direction: AudioDirection::Source,
            id: AudioNodeId::new(99),
        }),
        Err(AudioCommandError::UnknownNode)
    );
}

#[test]
fn default_metadata_key_targets_the_configured_preference() {
    // Weftwise writes the persistent configured key, never the runtime
    // output key WirePlumber owns.
    assert_eq!(
        default_metadata_key(AudioDirection::Sink),
        "default.configured.audio.sink"
    );
    assert_eq!(
        default_metadata_key(AudioDirection::Source),
        "default.configured.audio.source"
    );
}

#[test]
fn default_metadata_value_is_bounded_json_or_none() {
    assert_eq!(
        default_metadata_value("synthetic-sink"),
        Some(r#"{"name":"synthetic-sink"}"#.to_owned())
    );
    // Empty and NUL-bearing names produce no value.
    assert_eq!(default_metadata_value(""), None);
    assert_eq!(default_metadata_value("bad\0name"), None);
    // Quotes and backslashes are JSON-escaped, never injected raw.
    assert_eq!(
        default_metadata_value(r#"a"b\c"#),
        Some(r#"{"name":"a\"b\\c"}"#.to_owned())
    );
}

#[test]
fn default_metadata_binds_one_object_until_it_is_removed() {
    // No handle retained yet: the first `default` metadata object binds.
    assert!(should_bind_default_metadata(None));
    // A handle is already retained: a second concurrent object is ignored
    // so the route target cannot be silently swapped.
    assert!(!should_bind_default_metadata(Some(41)));
}

#[test]
fn default_metadata_clears_only_on_its_own_global_removal() {
    // A `global_remove` for the retained object's ID releases the handle.
    assert!(should_clear_default_metadata(Some(41), 41));
    // A removal for any other global leaves the handle intact.
    assert!(!should_clear_default_metadata(Some(41), 42));
    // With nothing retained there is nothing to clear.
    assert!(!should_clear_default_metadata(None, 41));
    // After clearing, the slot is empty and the next object may rebind.
    assert!(should_bind_default_metadata(None));
}

fn candidate(id: u32, running: bool, movable: bool) -> MovableStreamCandidate {
    MovableStreamCandidate {
        id: AudioNodeId::new(id),
        running,
        movable,
        has_metadata_permission: true,
    }
}

const GEN: Generation = Generation::new(1);

fn ready_enablement() -> MoveEnablement {
    MoveEnablement {
        policy_allows: true,
        metadata_writable: true,
        overflowed: false,
    }
}

#[test]
fn allow_moving_streams_is_tri_state() {
    assert_eq!(
        parse_allow_moving_streams(Some("true")),
        MoveMovingPolicy::Allowed
    );
    assert_eq!(
        parse_allow_moving_streams(Some(" 1 ")),
        MoveMovingPolicy::Allowed
    );
    assert_eq!(
        parse_allow_moving_streams(Some("false")),
        MoveMovingPolicy::Denied
    );
    assert_eq!(
        parse_allow_moving_streams(Some("0")),
        MoveMovingPolicy::Denied
    );
    // Absent or unrecognized values are Unknown, which permits an attempt
    // but never claims capability.
    assert_eq!(parse_allow_moving_streams(None), MoveMovingPolicy::Unknown);
    assert_eq!(
        parse_allow_moving_streams(Some("maybe")),
        MoveMovingPolicy::Unknown
    );
    assert!(MoveMovingPolicy::Unknown.permits_attempt());
    assert!(MoveMovingPolicy::Allowed.permits_attempt());
    assert!(!MoveMovingPolicy::Denied.permits_attempt());
}

#[test]
fn metadata_write_requires_write_and_execute() {
    // Both W (0o200) and X (0o100) are required.
    assert!(metadata_permits_target_write(PW_PERM_W | PW_PERM_X));
    assert!(metadata_permits_target_write(0o700));
    assert!(!metadata_permits_target_write(PW_PERM_W));
    assert!(!metadata_permits_target_write(PW_PERM_X));
    assert!(!metadata_permits_target_write(0o400));
    assert!(!metadata_permits_target_write(0));
}

#[test]
fn selection_requires_exactly_one_running_movable_stream() {
    // Zero running movable streams is unavailable.
    assert_eq!(
        select_movable_stream(&[], ready_enablement(), GEN),
        MovableStreamState::Unavailable
    );
    // A single running movable stream is the unique active subject and
    // carries the connection generation it was selected on.
    assert_eq!(
        select_movable_stream(&[candidate(7, true, true)], ready_enablement(), GEN),
        MovableStreamState::Active {
            stream: AudioNodeId::new(7),
            generation: GEN,
        }
    );
    // A non-running or non-movable stream does not count.
    assert_eq!(
        select_movable_stream(
            &[candidate(7, false, true), candidate(8, true, false)],
            ready_enablement(),
            GEN,
        ),
        MovableStreamState::Unavailable
    );
    // Two running movable streams are ambiguous.
    assert_eq!(
        select_movable_stream(
            &[candidate(7, true, true), candidate(8, true, true)],
            ready_enablement(),
            GEN,
        ),
        MovableStreamState::Ambiguous
    );
}

#[test]
fn selection_ignores_a_subject_without_metadata_permission() {
    // The only running movable stream lacks PW_PERM_M on its subject, so it
    // cannot be moved and the action is unavailable rather than offered.
    let no_perm = MovableStreamCandidate {
        has_metadata_permission: false,
        ..candidate(7, true, true)
    };
    assert_eq!(
        select_movable_stream(&[no_perm], ready_enablement(), GEN),
        MovableStreamState::Unavailable
    );
    // When one of two running movable streams lacks the permission, the
    // other is the unique movable subject rather than an ambiguous pair.
    assert_eq!(
        select_movable_stream(
            &[no_perm, candidate(8, true, true)],
            ready_enablement(),
            GEN
        ),
        MovableStreamState::Active {
            stream: AudioNodeId::new(8),
            generation: GEN,
        }
    );
    assert!(subject_permits_metadata(PW_PERM_M));
    assert!(subject_permits_metadata(0o710));
    assert!(!subject_permits_metadata(PW_PERM_W | PW_PERM_X));
    assert!(!subject_permits_metadata(0));
}

#[test]
fn movable_stream_class_requires_exact_trimmed_equality() {
    assert!(is_movable_stream_class("Stream/Output/Audio"));
    // Surrounding whitespace is trimmed before the exact comparison.
    assert!(is_movable_stream_class("  Stream/Output/Audio\n"));
    // Decorated, prefixed, suffixed, or unrelated classes are rejected.
    assert!(!is_movable_stream_class("Stream/Output/Audio/Virtual"));
    assert!(!is_movable_stream_class("Stream/Output/Audiofoo"));
    assert!(!is_movable_stream_class("xStream/Output/Audio"));
    assert!(!is_movable_stream_class("Stream/Output/Video"));
    assert!(!is_movable_stream_class("Stream/Input/Audio"));
    assert!(!is_movable_stream_class(""));
    assert_eq!(MOVABLE_STREAM_MEDIA_CLASS, "Stream/Output/Audio");
}

#[test]
fn stale_generation_move_is_rejected() {
    // A move built against the live generation is fresh; the same command
    // after a reconnect to a newer generation is stale.
    assert!(move_is_fresh(Generation::new(3), Generation::new(3)));
    assert!(!move_is_fresh(Generation::new(3), Generation::new(4)));
    assert!(!move_is_fresh(Generation::new(4), Generation::new(3)));
}

#[test]
fn selection_disables_on_policy_permission_or_overflow() {
    let one = [candidate(7, true, true)];
    // Explicit policy denial disables even a unique candidate.
    assert_eq!(
        select_movable_stream(
            &one,
            MoveEnablement {
                policy_allows: false,
                ..ready_enablement()
            },
            GEN,
        ),
        MovableStreamState::Disabled
    );
    // A metadata object without write permission disables the action.
    assert_eq!(
        select_movable_stream(
            &one,
            MoveEnablement {
                metadata_writable: false,
                ..ready_enablement()
            },
            GEN,
        ),
        MovableStreamState::Disabled
    );
    // Inventory overflow disables rather than guessing over a partial set.
    assert_eq!(
        select_movable_stream(
            &one,
            MoveEnablement {
                overflowed: true,
                ..ready_enablement()
            },
            GEN,
        ),
        MovableStreamState::Disabled
    );
}

#[test]
fn target_object_value_is_the_decimal_serial() {
    assert_eq!(target_object_metadata_value(0), "0");
    assert_eq!(target_object_metadata_value(4_294_967_297), "4294967297");
    assert_eq!(TARGET_OBJECT_METADATA_KEY, "target.object");
    assert_eq!(TARGET_OBJECT_METADATA_TYPE, "Spa:Id");
}

#[test]
fn move_stream_validates_against_active_selection() {
    let mut state = AudioState::default();
    state.apply_snapshot(
        vec![sink(1, "a"), source(2, "b")],
        None,
        None,
        Generation::new(1),
    );

    // With no movable stream, the move is unsupported regardless of subject.
    assert_eq!(
        state.validate(AudioCommandKind::MoveStream {
            stream: AudioNodeId::new(5),
            target: AudioNodeId::new(1),
        }),
        Err(AudioCommandError::Unsupported)
    );

    // With an active movable stream, moving it to an available sink is ok,
    // and validation binds the request to the selection's generation.
    state.set_movable_stream(MovableStreamState::Active {
        stream: AudioNodeId::new(5),
        generation: Generation::new(7),
    });
    assert_eq!(
        state.validate(AudioCommandKind::MoveStream {
            stream: AudioNodeId::new(5),
            target: AudioNodeId::new(1),
        }),
        Ok(AudioCommand {
            kind: AudioCommandKind::MoveStreamTo {
                stream: AudioNodeId::new(5),
                target: AudioNodeId::new(1),
                generation: Generation::new(7),
            },
        })
    );
    // A stamped adapter command whose generation still matches re-validates,
    // but one built against a stale generation is rejected.
    assert!(
        state
            .validate(AudioCommandKind::MoveStreamTo {
                stream: AudioNodeId::new(5),
                target: AudioNodeId::new(1),
                generation: Generation::new(7),
            })
            .is_ok()
    );
    assert_eq!(
        state.validate(AudioCommandKind::MoveStreamTo {
            stream: AudioNodeId::new(5),
            target: AudioNodeId::new(1),
            generation: Generation::new(6),
        }),
        Err(AudioCommandError::Unsupported)
    );
    // A subject that is not the active stream is unsupported.
    assert_eq!(
        state.validate(AudioCommandKind::MoveStream {
            stream: AudioNodeId::new(6),
            target: AudioNodeId::new(1),
        }),
        Err(AudioCommandError::Unsupported)
    );
    // A source target is the wrong direction even with an active stream.
    assert_eq!(
        state.validate(AudioCommandKind::MoveStream {
            stream: AudioNodeId::new(5),
            target: AudioNodeId::new(2),
        }),
        Err(AudioCommandError::WrongDirection)
    );
    // An unknown target node is rejected.
    assert_eq!(
        state.validate(AudioCommandKind::MoveStream {
            stream: AudioNodeId::new(5),
            target: AudioNodeId::new(99),
        }),
        Err(AudioCommandError::UnknownNode)
    );
    // Ambiguous or disabled selection offers no move.
    state.set_movable_stream(MovableStreamState::Ambiguous);
    assert_eq!(
        state.validate(AudioCommandKind::MoveStream {
            stream: AudioNodeId::new(5),
            target: AudioNodeId::new(1),
        }),
        Err(AudioCommandError::Unsupported)
    );
}

#[test]
fn movable_stream_state_accessors() {
    let active = MovableStreamState::Active {
        stream: AudioNodeId::new(3),
        generation: Generation::new(9),
    };
    assert_eq!(active.active(), Some(AudioNodeId::new(3)));
    assert_eq!(
        active.active_generation(),
        Some((AudioNodeId::new(3), Generation::new(9)))
    );
    assert!(active.can_move());
    assert_eq!(MovableStreamState::Unavailable.active(), None);
    assert_eq!(MovableStreamState::Unavailable.active_generation(), None);
    assert!(!MovableStreamState::Ambiguous.can_move());
    assert!(!MovableStreamState::Disabled.can_move());
    // Default is the safe Unavailable.
    assert_eq!(
        MovableStreamState::default(),
        MovableStreamState::Unavailable
    );
    // The generation newtype round-trips its counter value.
    assert_eq!(Generation::new(42).get(), 42);
}
