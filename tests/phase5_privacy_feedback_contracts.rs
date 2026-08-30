use weftwise::context::arbitration::Severity;
use weftwise::context::feedback::{FeedbackEvent, FeedbackKind};
use weftwise::context::privacy::{PrivacyEvidence, PrivacyState, PrivacyUpdate};
use weftwise::state::{AppState, MarkPattern, MarkShape, OutputId};

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
