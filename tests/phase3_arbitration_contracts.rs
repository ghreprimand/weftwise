use std::time::Duration;

use weftwise::context::arbitration::{
    ArbitrationInput, Arbitrator, CandidateAction, CandidateId, CandidateSource, PreemptionClass,
    PresentationCandidate, PresentationKind, Progress, Severity, Timestamp,
};
use weftwise::state::{
    AppState, DisplayText, InteractionEffect, InteractionInput, MarkPattern, MarkShape, OutputId,
    OutputName, OutputPresentation, PresentationLevel,
};

fn timestamp(milliseconds: u64) -> Timestamp {
    Timestamp::from_millis(milliseconds)
}

fn candidate(id: &str, kind: PresentationKind, updated_at: u64) -> PresentationCandidate {
    PresentationCandidate {
        id: CandidateId::new(id).expect("synthetic candidate identity"),
        source: CandidateSource::Activity,
        kind,
        severity: Severity::Normal,
        label: DisplayText::new(id, 128),
        accessible_label: DisplayText::new(&format!("Synthetic {id} state"), 256),
        created_at: timestamp(updated_at),
        updated_at: timestamp(updated_at),
        expires_at: None,
        minimum_display: Duration::ZERO,
        preemption: PreemptionClass::Passive,
        progress: None,
        actions: Vec::new(),
        output_affinity: None,
    }
}

fn output_name(value: &str) -> OutputName {
    OutputName::new(value).expect("synthetic output")
}

#[test]
fn priority_inversion_uses_semantic_order_before_fallback() {
    let mut arbitrator = Arbitrator::default();
    let mut clock = candidate("clock", PresentationKind::Fallback, 0);
    clock.source = CandidateSource::Clock;
    let mut warning = candidate("warning", PresentationKind::Warning, 0);
    warning.severity = Severity::Warning;

    arbitrator.apply(ArbitrationInput::Upsert(clock), timestamp(0));
    arbitrator.apply(ArbitrationInput::Upsert(warning), timestamp(0));

    let selected = arbitrator
        .select_for(None, timestamp(0))
        .expect("warning must outrank fallback");
    assert_eq!(selected.id.as_str(), "warning");
    assert_eq!(selected.kind, PresentationKind::Warning);
}

#[test]
fn equal_priority_ties_are_independent_of_arrival_order() {
    let alpha = candidate("alpha", PresentationKind::Activity, 10);
    let beta = candidate("beta", PresentationKind::Activity, 10);
    let mut first = Arbitrator::default();
    let mut second = Arbitrator::default();

    first.apply(ArbitrationInput::Upsert(beta.clone()), timestamp(10));
    first.apply(ArbitrationInput::Upsert(alpha.clone()), timestamp(10));
    second.apply(ArbitrationInput::Upsert(alpha), timestamp(10));
    second.apply(ArbitrationInput::Upsert(beta), timestamp(10));

    let first_selection = first.select_for(None, timestamp(10)).expect("selection");
    let second_selection = second.select_for(None, timestamp(10)).expect("selection");
    assert_eq!(first_selection.id.as_str(), "alpha");
    assert_eq!(first_selection, second_selection);
}

#[test]
fn rapid_updates_deduplicate_and_reject_stale_producer_revisions() {
    let mut arbitrator = Arbitrator::default();
    let original = candidate("build", PresentationKind::Activity, 1);
    assert!(arbitrator.apply(ArbitrationInput::Upsert(original), timestamp(1)));

    let mut update = candidate("build", PresentationKind::Activity, 2);
    update.label = DisplayText::new("Synthetic build: 50%", 128);
    update.progress = Some(Progress::from_basis_points(5_000));
    update.actions = vec![CandidateAction::Dismiss, CandidateAction::Dismiss];
    assert!(arbitrator.apply(ArbitrationInput::Upsert(update), timestamp(2)));
    assert_eq!(arbitrator.len(), 1);

    let mut stale = candidate("build", PresentationKind::Activity, 1);
    stale.label = DisplayText::new("Synthetic stale update", 128);
    assert!(!arbitrator.apply(ArbitrationInput::Upsert(stale), timestamp(2)));

    let selected = arbitrator
        .select_for(None, timestamp(2))
        .expect("selection");
    assert_eq!(selected.label, "Synthetic build: 50%");
    assert_eq!(selected.progress.map(Progress::basis_points), Some(5_000));
    assert_eq!(selected.actions, vec![CandidateAction::Dismiss]);
}

#[test]
fn expiry_stale_sources_and_clock_changes_never_leave_stale_content_selected() {
    let mut arbitrator = Arbitrator::default();
    let mut clock = candidate("clock", PresentationKind::Fallback, 0);
    clock.source = CandidateSource::Clock;
    let mut temporary = candidate("temporary", PresentationKind::Feedback, 1);
    temporary.expires_at = Some(timestamp(50));

    arbitrator.apply(ArbitrationInput::Upsert(clock), timestamp(0));
    arbitrator.apply(ArbitrationInput::Upsert(temporary), timestamp(1));
    assert_eq!(
        arbitrator
            .select_for(None, timestamp(1))
            .expect("temporary selection")
            .id
            .as_str(),
        "temporary"
    );
    assert_eq!(
        arbitrator
            .select_for(None, timestamp(50))
            .expect("clock fallback")
            .id
            .as_str(),
        "clock"
    );

    let mut state = AppState::default();
    let output = OutputId::new(31);
    state.reconcile_outputs([output], [], false);
    assert_eq!(state.set_clock_label("09:30".to_owned()), vec![output]);
    assert_eq!(
        state.output_view(output).expect("view").ribbon_label,
        "09:30"
    );
    assert_eq!(state.set_clock_label("09:31".to_owned()), vec![output]);
    assert_eq!(
        state.output_view(output).expect("view").ribbon_label,
        "09:31"
    );

    let mut stale_activity = candidate("stale-activity", PresentationKind::Activity, 60);
    stale_activity.source = CandidateSource::Activity;
    arbitrator.apply(ArbitrationInput::Upsert(stale_activity), timestamp(60));
    arbitrator.apply(
        ArbitrationInput::SourceStale(CandidateSource::Activity),
        timestamp(61),
    );
    assert_eq!(
        arbitrator
            .select_for(None, timestamp(61))
            .expect("only the clock remains")
            .id
            .as_str(),
        "clock"
    );
}

#[test]
fn privacy_critical_content_preempts_sticky_activity() {
    let mut arbitrator = Arbitrator::default();
    let mut activity = candidate("activity", PresentationKind::Activity, 0);
    activity.minimum_display = Duration::from_secs(30);
    arbitrator.apply(ArbitrationInput::Upsert(activity), timestamp(0));
    assert_eq!(
        arbitrator
            .select_for(None, timestamp(0))
            .expect("activity selection")
            .id
            .as_str(),
        "activity"
    );

    let mut privacy = candidate("privacy", PresentationKind::Privacy, 1);
    privacy.severity = Severity::Critical;
    privacy.preemption = PreemptionClass::PrivacyCritical;
    arbitrator.apply(ArbitrationInput::Upsert(privacy), timestamp(1));
    let selected = arbitrator
        .select_for(None, timestamp(1))
        .expect("privacy must preempt sticky activity");
    assert_eq!(selected.id.as_str(), "privacy");
    assert_eq!(selected.severity, Severity::Critical);
}

#[test]
fn output_affinity_changes_selection_without_leaking_to_other_outputs() {
    let first_name = output_name("SYNTH-OUTPUT-1");
    let second_name = output_name("SYNTH-OUTPUT-2");
    let mut arbitrator = Arbitrator::default();
    let clock = candidate("clock", PresentationKind::Fallback, 0);
    let mut local = candidate("local", PresentationKind::Activity, 1);
    local.output_affinity = Some(first_name.clone());

    arbitrator.apply(ArbitrationInput::Upsert(clock), timestamp(0));
    arbitrator.apply(ArbitrationInput::Upsert(local), timestamp(1));
    assert_eq!(
        arbitrator
            .select_for(Some(&first_name), timestamp(1))
            .expect("local selection")
            .id
            .as_str(),
        "local"
    );
    assert_eq!(
        arbitrator
            .select_for(Some(&second_name), timestamp(1))
            .expect("global clock")
            .id
            .as_str(),
        "clock"
    );

    arbitrator.apply(ArbitrationInput::OutputRemoved(first_name), timestamp(2));
    assert_eq!(
        arbitrator
            .select_for(Some(&second_name), timestamp(2))
            .expect("unaffected output")
            .id
            .as_str(),
        "clock"
    );
}

#[test]
fn rendered_attention_marks_have_non_color_semantics_and_bounded_typed_actions() {
    let output = OutputId::new(41);
    let mut state = AppState::default();
    state.reconcile_outputs([output], [], false);
    let mut privacy = candidate("capture", PresentationKind::Privacy, 1);
    privacy.severity = Severity::Critical;
    privacy.preemption = PreemptionClass::PrivacyCritical;
    privacy.accessible_label = DisplayText::new("Synthetic capture active, critical", 256);
    privacy.actions = vec![CandidateAction::RevealDetails, CandidateAction::Dismiss];

    assert_eq!(
        state.apply_arbitration(ArbitrationInput::Upsert(privacy), timestamp(1)),
        vec![output]
    );
    let view = state.output_view(output).expect("render projection");
    assert!(view.activity.is_empty());
    assert_eq!(view.attention.len(), 1);
    assert_eq!(view.attention[0].shape, MarkShape::Triangle);
    assert_eq!(view.attention[0].pattern, MarkPattern::Striped);
    assert_eq!(
        view.attention[0].accessible_label,
        "Synthetic capture active, critical"
    );
    assert_eq!(
        view.candidate_actions,
        vec![CandidateAction::RevealDetails, CandidateAction::Dismiss]
    );
}

#[test]
fn keyboard_panel_inputs_and_reduced_motion_preserve_authoritative_state() {
    let mut presentation = OutputPresentation::new(false);
    let dwell = match presentation
        .update(InteractionInput::PointerEntered)
        .as_slice()
    {
        [InteractionEffect::ScheduleDwell(token)] => *token,
        effects => panic!("expected dwell schedule, got {effects:?}"),
    };
    assert_eq!(
        presentation.update(InteractionInput::DwellElapsed(dwell)),
        [InteractionEffect::Render]
    );
    assert_eq!(presentation.level(), PresentationLevel::Ribbon);
    assert_eq!(
        presentation.update(InteractionInput::OpenPanel),
        [InteractionEffect::Render]
    );
    assert_eq!(presentation.level(), PresentationLevel::Panel);
    assert_eq!(
        presentation.update(InteractionInput::ClosePanel),
        [InteractionEffect::Render]
    );
    assert_eq!(presentation.level(), PresentationLevel::Ribbon);

    assert_eq!(
        presentation.update(InteractionInput::SetReducedMotion(true)),
        [InteractionEffect::Render]
    );
    assert!(presentation.reduced_motion());
    assert_eq!(presentation.level(), PresentationLevel::Ribbon);
}
