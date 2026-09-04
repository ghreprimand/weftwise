use weftwise::context::arbitration::CandidateAction;
use weftwise::services::mpris::MediaUpdate;
use weftwise::state::{
    AppState, MediaCapabilities, MediaMetadata, MediaPlaybackStatus, MediaPlayer, MediaPlayerId,
    OutputId,
};

fn player(suffix: &str, owner_generation: u64, capabilities: MediaCapabilities) -> MediaPlayer {
    MediaPlayer::bounded(
        MediaPlayerId::new(&format!("org.mpris.MediaPlayer2.synthetic_{suffix}"))
            .expect("synthetic player identity"),
        owner_generation,
        "Synthetic player",
        MediaPlaybackStatus::Playing,
        MediaMetadata::bounded(
            "Synthetic title",
            &["Synthetic artist".to_owned()],
            None,
            Some(120_000_000),
            10_000_000,
        ),
        capabilities,
        1,
    )
}

fn controls() -> MediaCapabilities {
    MediaCapabilities {
        can_control: true,
        can_play: true,
        can_pause: true,
        can_previous: true,
        can_next: true,
        can_seek: true,
    }
}

#[test]
fn delayed_property_snapshot_cannot_resurrect_a_vanished_owner() {
    let output = OutputId::new(410);
    let mut state = AppState::default();
    state.reconcile_outputs([output], [], false);
    let original = player("race", 7, controls());
    let id = original.id.clone();

    state.apply_media_update(MediaUpdate::Snapshot {
        players: vec![original.clone()],
        observed_millis: 1,
    });
    state.apply_media_update(MediaUpdate::PlayerRemoved {
        id: id.clone(),
        observed_millis: 2,
    });
    // A PropertiesChanged refresh started before the owner vanished must not
    // reintroduce that old owner after its removal was observed.
    state.apply_media_update(MediaUpdate::PlayerChanged {
        player: original,
        observed_millis: 3,
    });

    assert!(!state.media.players.contains_key(&id));
    assert!(state.selected_media_player(output).is_none());
}

#[test]
fn withdrawn_capability_removes_the_previously_advertised_media_action() {
    let output = OutputId::new(411);
    let mut state = AppState::default();
    state.reconcile_outputs([output], [], false);
    let mut current = player("capability", 8, controls());
    state.apply_media_update(MediaUpdate::Snapshot {
        players: vec![current.clone()],
        observed_millis: 1,
    });
    assert!(
        state
            .output_view(output)
            .expect("advertised action")
            .candidate_actions
            .contains(&CandidateAction::MediaNext)
    );

    current.capabilities.can_next = false;
    state.apply_media_update(MediaUpdate::PlayerChanged {
        player: current,
        observed_millis: 2,
    });
    assert!(
        !state
            .output_view(output)
            .expect("withdrawn action")
            .candidate_actions
            .contains(&CandidateAction::MediaNext)
    );
}

#[test]
fn malformed_metadata_values_are_bounded_to_safe_display_data() {
    let metadata = MediaMetadata::bounded(
        "\u{0}\n\u{202e}",
        &vec!["\u{0}".repeat(400); 96],
        Some("data:text/plain,not-artwork"),
        Some(-1),
        -1,
    );

    assert!(metadata.title.as_str().chars().count() <= 256);
    assert!(metadata.artist.as_str().chars().count() <= 256);
    assert!(metadata.art_url.is_none());
    assert_eq!(metadata.duration_micros, 0);
    assert_eq!(metadata.position_micros, 0);
}

#[test]
fn owner_change_path_must_bound_adapter_bookkeeping_before_the_33rd_player() {
    // The root projection is bounded independently. This guards the adapter
    // branch too, so owner changes cannot retain unbounded owner/destination
    // entries while the root silently drops their player updates.
    let source = include_str!("../src/services/mpris.rs");
    let owner_change = source
        .split("signal = owner_changes.next() => {")
        .nth(1)
        .and_then(|tail| tail.split("message = property_changes.next() => {").next())
        .expect("owner-change branch");
    assert!(
        owner_change.contains("players.len() < MAX_PLAYERS || players.contains_key(&id)"),
        "owner-change bookkeeping must reject a new 33rd MPRIS player"
    );
}
