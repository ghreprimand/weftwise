use weftwise::context::arbitration::CandidateAction;
use weftwise::services::mpris::MediaUpdate;
use weftwise::state::{
    AdapterAvailability, AppState, MediaCapabilities, MediaMetadata, MediaPlaybackStatus,
    MediaPlayer, MediaPlayerId, OutputId,
};

fn player(
    suffix: &str,
    status: MediaPlaybackStatus,
    sequence: u64,
    capabilities: MediaCapabilities,
) -> MediaPlayer {
    MediaPlayer::bounded(
        MediaPlayerId::new(&format!("org.mpris.MediaPlayer2.synthetic_{suffix}"))
            .expect("synthetic player identity"),
        1,
        "Synthetic player",
        status,
        MediaMetadata::bounded(
            &format!("Synthetic title {suffix}"),
            &["Synthetic artist".to_owned()],
            Some("https://example.invalid/art.png"),
            Some(200_000_000),
            50_000_000,
        ),
        capabilities,
        sequence,
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
fn active_player_order_is_playing_then_paused_then_recent_and_stable_identity() {
    let output = OutputId::new(50);
    let mut state = AppState::default();
    state.reconcile_outputs([output], [], false);
    state.apply_media_update(MediaUpdate::Snapshot {
        players: vec![
            player("paused", MediaPlaybackStatus::Paused, 100, controls()),
            player("playing_z", MediaPlaybackStatus::Playing, 1, controls()),
            player("playing_a", MediaPlaybackStatus::Playing, 1, controls()),
        ],
        observed_millis: 1,
    });
    assert_eq!(
        state.media.active.as_ref().map(MediaPlayerId::as_str),
        Some("org.mpris.MediaPlayer2.synthetic_playing_a")
    );

    state.apply_media_update(MediaUpdate::PlayerChanged {
        player: player("playing_z", MediaPlaybackStatus::Playing, 200, controls()),
        observed_millis: 2,
    });
    assert_eq!(
        state.media.active.as_ref().map(MediaPlayerId::as_str),
        Some("org.mpris.MediaPlayer2.synthetic_playing_z")
    );
}

#[test]
fn metadata_duration_position_and_urls_are_bounded_before_rendering() {
    let metadata = MediaMetadata::bounded(
        &"x".repeat(400),
        &vec!["artist".repeat(80); 40],
        Some("javascript:unsafe"),
        Some(i64::MAX),
        i64::MAX,
    );
    assert_eq!(metadata.title.as_str().chars().count(), 256);
    assert!(metadata.artist.as_str().chars().count() <= 256);
    assert!(metadata.art_url.is_none());
    assert_eq!(metadata.duration_micros, 604_800_000_000);
    assert_eq!(metadata.position_micros, metadata.duration_micros);
    let missing = MediaMetadata::bounded("", &[], None, Some(-1), -1);
    assert_eq!(missing, MediaMetadata::default());
    assert_eq!(
        MediaPlaybackStatus::parse("Unknown"),
        MediaPlaybackStatus::Unknown
    );
    let formatted = MediaMetadata::bounded("safe\u{202e}name\nnext", &[], None, None, 0);
    assert_eq!(formatted.title.as_str(), "safename next");

    let mut misleading_capabilities = controls();
    misleading_capabilities.can_control = false;
    assert_eq!(
        player(
            "misleading",
            MediaPlaybackStatus::Playing,
            1,
            misleading_capabilities,
        )
        .capabilities,
        MediaCapabilities::default()
    );
}

#[test]
fn ribbon_progress_and_controls_are_projected_only_when_advertised() {
    let output = OutputId::new(51);
    let mut state = AppState::default();
    state.reconcile_outputs([output], [], false);
    state.apply_media_update(MediaUpdate::Snapshot {
        players: vec![player(
            "controlled",
            MediaPlaybackStatus::Playing,
            1,
            controls(),
        )],
        observed_millis: 1,
    });
    let view = state.output_view(output).expect("media projection");
    assert_eq!(
        view.ribbon_label,
        "Synthetic title controlled · Synthetic artist"
    );
    assert_eq!(view.activity.len(), 1);
    assert_eq!(view.activity[0].progress_basis_points, Some(2_500));
    assert_eq!(
        view.candidate_actions,
        vec![
            CandidateAction::MediaPlayPause,
            CandidateAction::MediaPrevious,
            CandidateAction::MediaNext,
            CandidateAction::MediaSeek(-10_000),
            CandidateAction::MediaSeek(10_000),
        ]
    );

    state.apply_media_update(MediaUpdate::PlayerChanged {
        player: player(
            "controlled",
            MediaPlaybackStatus::Playing,
            2,
            MediaCapabilities::default(),
        ),
        observed_millis: 2,
    });
    assert!(
        state
            .output_view(output)
            .expect("unsupported projection")
            .candidate_actions
            .is_empty()
    );
}

#[test]
fn disappearance_bus_loss_and_restart_never_leave_stale_media_selected() {
    let output = OutputId::new(52);
    let mut state = AppState::default();
    state.reconcile_outputs([output], [], false);
    let first = player("restart", MediaPlaybackStatus::Playing, 1, controls());
    let id = first.id.clone();
    state.apply_media_update(MediaUpdate::Snapshot {
        players: vec![first],
        observed_millis: 1,
    });
    assert!(state.selected_media_player(output).is_some());

    state.apply_media_update(MediaUpdate::PlayerRemoved {
        id: id.clone(),
        observed_millis: 2,
    });
    assert!(state.selected_media_player(output).is_none());

    let mut restarted = player("restart", MediaPlaybackStatus::Paused, 3, controls());
    restarted.owner_generation = 2;
    state.apply_media_update(MediaUpdate::PlayerChanged {
        player: restarted,
        observed_millis: 3,
    });
    assert!(state.selected_media_player(output).is_some());
    state.apply_media_update(MediaUpdate::Unavailable);
    assert_eq!(state.media.availability, AdapterAvailability::Stale);
    assert!(state.selected_media_player(output).is_none());

    state.apply_media_update(MediaUpdate::Snapshot {
        players: vec![player(
            "restart",
            MediaPlaybackStatus::Playing,
            4,
            controls(),
        )],
        observed_millis: 4,
    });
    assert_eq!(state.media.availability, AdapterAvailability::Ready);
    assert_eq!(
        state.media.active.as_ref().map(MediaPlayerId::as_str),
        Some(id.as_str())
    );
}

#[test]
fn stopped_media_expires_and_unknown_status_never_replaces_fallback() {
    let output = OutputId::new(53);
    let mut state = AppState::default();
    state.reconcile_outputs([output], [], false);
    state.apply_media_update(MediaUpdate::Snapshot {
        players: vec![player(
            "initially_stopped",
            MediaPlaybackStatus::Stopped,
            0,
            controls(),
        )],
        observed_millis: 1,
    });
    assert!(state.selected_media_player(output).is_none());

    state.apply_media_update(MediaUpdate::Snapshot {
        players: vec![player(
            "recent",
            MediaPlaybackStatus::Stopped,
            1,
            controls(),
        )],
        observed_millis: 1,
    });
    assert!(state.selected_media_player(output).is_some());

    state.apply_media_update(MediaUpdate::Tick {
        observed_millis: 30_000,
    });
    assert!(state.selected_media_player(output).is_some());
    state.apply_media_update(MediaUpdate::Tick {
        observed_millis: 30_001,
    });
    assert!(state.selected_media_player(output).is_none());

    state.apply_media_update(MediaUpdate::Snapshot {
        players: vec![player(
            "unknown",
            MediaPlaybackStatus::Unknown,
            40_000,
            controls(),
        )],
        observed_millis: 40_000,
    });
    assert!(state.selected_media_player(output).is_none());
}
