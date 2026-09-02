use weftwise::state::{
    AppState, InteractionEffect, InteractionInput, InteractionToken, OutputId, OutputPresentation,
    PresentationLevel,
};

fn scheduled_dwell(effects: Vec<InteractionEffect>) -> InteractionToken {
    match effects.as_slice() {
        [InteractionEffect::ScheduleDwell(token)] => *token,
        _ => panic!("expected one dwell timer, got {effects:?}"),
    }
}

fn scheduled_glance_dismiss(effects: Vec<InteractionEffect>) -> InteractionToken {
    match effects.as_slice() {
        [
            InteractionEffect::Render,
            InteractionEffect::ScheduleGlanceDismiss(token),
        ] => *token,
        _ => panic!("expected glance render and dismissal, got {effects:?}"),
    }
}

#[test]
fn unpin_rearms_only_normal_dwell_reveal() {
    let mut state = OutputPresentation::new(false);
    state.update(InteractionInput::PinRibbon);
    assert!(state.is_pinned());
    assert_eq!(
        state.update(InteractionInput::UnpinRibbon),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Selvage);
    assert!(!state.is_pinned());

    let stale_dwell = scheduled_dwell(state.update(InteractionInput::PointerEntered));
    assert_eq!(state.update(InteractionInput::PointerLeft), []);
    let current_dwell = scheduled_dwell(state.update(InteractionInput::PointerEntered));
    assert_eq!(
        state.update(InteractionInput::DwellElapsed(stale_dwell)),
        []
    );
    assert_eq!(state.level(), PresentationLevel::Selvage);
    assert_eq!(
        state.update(InteractionInput::DwellElapsed(current_dwell)),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Ribbon);
}

#[test]
fn unpin_while_panel_is_open_closes_panel_and_collapses() {
    let mut state = OutputPresentation::new(false);
    state.update(InteractionInput::PinRibbon);
    state.update(InteractionInput::OpenPanel);
    assert_eq!(state.level(), PresentationLevel::Panel);

    assert_eq!(
        state.update(InteractionInput::UnpinRibbon),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Selvage);
    assert!(!state.is_pinned());
}

#[test]
fn pin_survives_panel_round_trip_and_invalidates_prior_glance_timer() {
    let mut state = OutputPresentation::new(false);
    let glance_timer = scheduled_glance_dismiss(state.update(InteractionInput::RevealForGlance));
    state.update(InteractionInput::PinRibbon);
    assert!(state.is_pinned());

    assert_eq!(
        state.update(InteractionInput::OpenPanel),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Panel);
    assert!(state.is_pinned());
    assert_eq!(
        state.update(InteractionInput::ClosePanel),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Ribbon);
    assert!(state.is_pinned());
    assert_eq!(
        state.update(InteractionInput::DismissElapsed(glance_timer)),
        []
    );
    assert_eq!(state.level(), PresentationLevel::Ribbon);
}

#[test]
fn unpin_is_a_noop_when_the_ribbon_is_not_held_open() {
    let mut state = OutputPresentation::new(false);
    let before = state.clone();
    assert_eq!(state.update(InteractionInput::UnpinRibbon), []);
    assert_eq!(state, before);
}

#[test]
fn unpin_and_close_panel_invalidate_earlier_dismissal_tokens() {
    let mut state = OutputPresentation::new(false);
    let glance_timer = scheduled_glance_dismiss(state.update(InteractionInput::RevealForGlance));
    state.update(InteractionInput::PinRibbon);
    state.update(InteractionInput::UnpinRibbon);
    assert_eq!(
        state.update(InteractionInput::DismissElapsed(glance_timer)),
        []
    );
    assert_eq!(state.level(), PresentationLevel::Selvage);

    let dwell = scheduled_dwell(state.update(InteractionInput::PointerEntered));
    state.update(InteractionInput::DwellElapsed(dwell));
    let dismiss = match state.update(InteractionInput::PointerLeft).as_slice() {
        [InteractionEffect::ScheduleDismiss(token)] => *token,
        effects => panic!("expected one dismissal timer, got {effects:?}"),
    };
    state.update(InteractionInput::OpenPanel);
    state.update(InteractionInput::ClosePanel);
    assert_eq!(state.update(InteractionInput::DismissElapsed(dismiss)), []);
}

#[test]
fn output_removal_is_safe_for_pinned_and_panel_presentations() {
    let pinned = OutputId::new(81);
    let panel = OutputId::new(82);
    let retained = OutputId::new(83);
    let mut state = AppState::default();
    state.reconcile_outputs([pinned, panel, retained], [], false);

    state
        .output_mut(pinned)
        .unwrap()
        .update(InteractionInput::PinRibbon);
    let panel_presentation = state.output_mut(panel).unwrap();
    panel_presentation.update(InteractionInput::PointerEnteredImmediate);
    panel_presentation.update(InteractionInput::OpenPanel);
    assert!(state.output(pinned).unwrap().is_pinned());
    assert_eq!(
        state.output(panel).unwrap().level(),
        PresentationLevel::Panel
    );

    state.reconcile_outputs([], [pinned, panel], false);
    assert!(state.output(pinned).is_none());
    assert!(state.output(panel).is_none());
    assert_eq!(
        state.output(retained).map(OutputPresentation::level),
        Some(PresentationLevel::Selvage)
    );
}

#[test]
fn glance_after_unpin_is_a_fresh_reducer_glance() {
    let mut state = OutputPresentation::new(false);
    state.update(InteractionInput::PinRibbon);
    state.update(InteractionInput::UnpinRibbon);

    let _timer = scheduled_glance_dismiss(state.update(InteractionInput::RevealForGlance));
    assert_eq!(state.level(), PresentationLevel::Ribbon);
    assert!(!state.is_pinned());
}
