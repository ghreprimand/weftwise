use weftwise::shell::{
    ExclusiveZone,
    surface::{ActivationRegion, InputRegionGeometry},
};
use weftwise::state::{
    AppState, DISMISS_DELAY, DWELL_DELAY, GLANCE_DISMISS_DELAY, InteractionEffect,
    InteractionInput, InteractionToken, OutputId, OutputPresentation, PresentationLevel,
};
use weftwise::widgets::SURFACE_HEIGHT;

fn scheduled_dwell(effects: Vec<InteractionEffect>) -> InteractionToken {
    match effects.as_slice() {
        [InteractionEffect::ScheduleDwell(token)] => *token,
        _ => panic!("expected one dwell timer, got {effects:?}"),
    }
}

fn scheduled_dismiss(effects: Vec<InteractionEffect>) -> InteractionToken {
    match effects.as_slice() {
        [InteractionEffect::ScheduleDismiss(token)] => *token,
        _ => panic!("expected one dismissal timer, got {effects:?}"),
    }
}

#[test]
fn dwell_reveal_requires_the_current_timer_and_an_inside_pointer() {
    let mut state = OutputPresentation::new(false);
    let first_dwell = scheduled_dwell(state.update(InteractionInput::PointerEntered));

    assert_eq!(state.update(InteractionInput::PointerLeft), []);
    assert_eq!(
        state.update(InteractionInput::DwellElapsed(first_dwell)),
        []
    );
    assert_eq!(state.level(), PresentationLevel::Selvage);

    let second_dwell = scheduled_dwell(state.update(InteractionInput::PointerEntered));
    assert_eq!(
        state.update(InteractionInput::DwellElapsed(second_dwell)),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Ribbon);
}

#[test]
fn internal_corner_entry_reveals_without_a_dwell_timer() {
    let mut state = OutputPresentation::new(false);

    assert_eq!(
        state.update(InteractionInput::PointerEnteredImmediate),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Ribbon);
    assert!(state.pointer_inside());
}

#[test]
fn keyboard_glance_reveals_without_pointer_ownership_and_has_a_bounded_timeout() {
    let mut state = OutputPresentation::new(false);
    let effects = state.update(InteractionInput::RevealForGlance);
    let [
        InteractionEffect::Render,
        InteractionEffect::ScheduleGlanceDismiss(token),
    ] = effects.as_slice()
    else {
        panic!("expected render plus glance dismissal, got {effects:?}");
    };
    assert_eq!(GLANCE_DISMISS_DELAY.as_millis(), 2_500);
    assert_eq!(state.level(), PresentationLevel::Ribbon);
    assert!(!state.pointer_inside());
    assert_eq!(
        state.update(InteractionInput::DismissElapsed(*token)),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Selvage);
}

#[test]
fn dismissal_is_cancelled_by_reentry_and_stale_timers_cannot_collapse() {
    let mut state = OutputPresentation::new(false);
    let dwell = scheduled_dwell(state.update(InteractionInput::PointerEntered));
    state.update(InteractionInput::DwellElapsed(dwell));

    let dismiss = scheduled_dismiss(state.update(InteractionInput::PointerLeft));
    assert_eq!(state.update(InteractionInput::PointerEntered), []);
    assert_eq!(state.update(InteractionInput::DismissElapsed(dismiss)), []);
    assert_eq!(state.level(), PresentationLevel::Ribbon);

    let second_dismiss = scheduled_dismiss(state.update(InteractionInput::PointerLeft));
    assert_eq!(
        state.update(InteractionInput::DismissElapsed(second_dismiss)),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Selvage);
}

#[test]
fn panel_can_only_open_from_ribbon_and_closes_to_pointer_appropriate_level() {
    let mut state = OutputPresentation::new(false);

    assert_eq!(state.update(InteractionInput::OpenPanel), []);
    let dwell = scheduled_dwell(state.update(InteractionInput::PointerEntered));
    state.update(InteractionInput::DwellElapsed(dwell));
    assert_eq!(
        state.update(InteractionInput::OpenPanel),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Panel);
    assert_eq!(
        state.update(InteractionInput::ClosePanel),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Ribbon);

    assert_eq!(
        state.update(InteractionInput::OpenPanel),
        [InteractionEffect::Render]
    );
    state.update(InteractionInput::PointerLeft);
    assert_eq!(
        state.update(InteractionInput::ClosePanel),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Selvage);
}

#[test]
fn reduced_motion_preserves_interaction_semantics_and_only_renders_on_change() {
    let mut state = OutputPresentation::new(false);

    assert_eq!(
        state.update(InteractionInput::SetReducedMotion(true)),
        [InteractionEffect::Render]
    );
    assert!(state.reduced_motion());
    assert_eq!(state.update(InteractionInput::SetReducedMotion(true)), []);

    let dwell = scheduled_dwell(state.update(InteractionInput::PointerEntered));
    assert_eq!(
        state.update(InteractionInput::DwellElapsed(dwell)),
        [InteractionEffect::Render]
    );
    assert_eq!(state.level(), PresentationLevel::Ribbon);
}

#[test]
fn output_reconciliation_preserves_existing_presentations_and_removes_only_requested_outputs() {
    let first = OutputId::new(1);
    let second = OutputId::new(2);
    let third = OutputId::new(3);
    let mut state = AppState::default();

    state.reconcile_outputs([first, second], [], false);
    let dwell = scheduled_dwell(
        state
            .output_mut(first)
            .expect("first output")
            .update(InteractionInput::PointerEntered),
    );
    state
        .output_mut(first)
        .expect("first output")
        .update(InteractionInput::DwellElapsed(dwell));

    state.reconcile_outputs([third], [second], true);

    assert_eq!(
        state.output(first).map(OutputPresentation::level),
        Some(PresentationLevel::Ribbon)
    );
    assert_eq!(state.output(second), None);
    assert_eq!(
        state.output(third).map(OutputPresentation::reduced_motion),
        Some(true)
    );
    assert_eq!(state.output_ids().collect::<Vec<_>>(), vec![first, third]);
}

#[test]
fn input_regions_are_fixed_to_visible_height_and_start_empty_before_layout() {
    let activation = ActivationRegion {
        x: 400,
        width: 96,
        height: 8,
        immediate: false,
    };
    assert_eq!(
        InputRegionGeometry::for_level(0, PresentationLevel::Selvage, activation),
        InputRegionGeometry {
            x: 0,
            y: 0,
            width: 0,
            height: 8,
        }
    );
    assert_eq!(
        InputRegionGeometry::for_level(512, PresentationLevel::Ribbon, activation),
        InputRegionGeometry {
            x: 0,
            y: 0,
            width: 512,
            height: SURFACE_HEIGHT,
        }
    );
    assert_eq!(
        InputRegionGeometry::for_level(512, PresentationLevel::Panel, activation),
        InputRegionGeometry {
            x: 0,
            y: 0,
            width: 512,
            height: SURFACE_HEIGHT,
        }
    );
    assert_eq!(
        InputRegionGeometry::left_edge_leg(512, PresentationLevel::Selvage, activation),
        InputRegionGeometry {
            x: 0,
            y: 0,
            width: 8,
            height: SURFACE_HEIGHT,
        }
    );
    assert_eq!(
        InputRegionGeometry::right_edge_leg(512, PresentationLevel::Selvage, activation),
        InputRegionGeometry {
            x: 504,
            y: 0,
            width: 8,
            height: SURFACE_HEIGHT,
        }
    );
    assert_eq!(
        InputRegionGeometry::mirrored_top_island(512, PresentationLevel::Selvage, activation,),
        InputRegionGeometry {
            x: 16,
            y: 0,
            width: 96,
            height: 8,
        }
    );
}

#[test]
fn phase_one_timing_and_exclusive_zone_contracts_are_explicit() {
    assert_eq!(DWELL_DELAY.as_millis(), 240);
    assert_eq!(DISMISS_DELAY.as_millis(), 360);
    assert_eq!(ExclusiveZone::Zero.value(), 0);
    assert_eq!(ExclusiveZone::NegativeOne.value(), -1);
    assert_eq!(ExclusiveZone::default(), ExclusiveZone::NegativeOne);
}
