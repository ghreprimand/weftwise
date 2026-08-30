//! Authoritative product and presentation state.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::config::Config;

/// Delay before a pointer at the top edge reveals the Ribbon.
pub const DWELL_DELAY: Duration = Duration::from_millis(240);

/// Delay before a pointer departure collapses the Ribbon.
pub const DISMISS_DELAY: Duration = Duration::from_millis(360);

/// Process-local identity assigned to a GDK output surface.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OutputId(u64);

impl OutputId {
    /// Construct an output identity from the surface manager's monotonic counter.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

/// Visible top-edge presentation level.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PresentationLevel {
    /// Only the pointer-active 2-3 pixel Selvage is visible.
    #[default]
    Selvage,
    /// The labeled Ribbon is visible and pointer-active.
    Ribbon,
    /// The interactive Panel is explicitly open.
    Panel,
}

/// Generation token that invalidates superseded dwell and dismissal timers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InteractionToken(u64);

/// Deterministic inputs accepted by an output interaction state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionInput {
    /// Pointer entered the active top-edge region.
    PointerEntered,
    /// Pointer left the active visible region.
    PointerLeft,
    /// A previously scheduled dwell timer completed.
    DwellElapsed(InteractionToken),
    /// A previously scheduled dismissal timer completed.
    DismissElapsed(InteractionToken),
    /// The user explicitly requested the Panel from the Ribbon.
    OpenPanel,
    /// The Panel closed through Escape, outside click, or an explicit action.
    ClosePanel,
    /// The effective GTK and application motion preference changed.
    SetReducedMotion(bool),
}

/// Side effects emitted by the pure interaction reducer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionEffect {
    /// Schedule a dwell timer with the supplied generation.
    ScheduleDwell(InteractionToken),
    /// Schedule a dismissal timer with the supplied generation.
    ScheduleDismiss(InteractionToken),
    /// Re-render the output surface from authoritative state.
    Render,
}

/// Root-owned interaction state for one output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputPresentation {
    level: PresentationLevel,
    pointer_inside: bool,
    reduced_motion: bool,
    generation: u64,
}

impl OutputPresentation {
    /// Create a collapsed output presentation.
    #[must_use]
    pub const fn new(reduced_motion: bool) -> Self {
        Self {
            level: PresentationLevel::Selvage,
            pointer_inside: false,
            reduced_motion,
            generation: 0,
        }
    }

    /// Current presentation level.
    #[must_use]
    pub const fn level(&self) -> PresentationLevel {
        self.level
    }

    /// Whether reveal animation must be disabled.
    #[must_use]
    pub const fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    /// Whether the pointer is in the current input region.
    #[must_use]
    pub const fn pointer_inside(&self) -> bool {
        self.pointer_inside
    }

    /// Apply one deterministic input and return required side effects.
    pub fn update(&mut self, input: InteractionInput) -> Vec<InteractionEffect> {
        match input {
            InteractionInput::PointerEntered => self.pointer_entered(),
            InteractionInput::PointerLeft => self.pointer_left(),
            InteractionInput::DwellElapsed(token) => self.dwell_elapsed(token),
            InteractionInput::DismissElapsed(token) => self.dismiss_elapsed(token),
            InteractionInput::OpenPanel => self.open_panel(),
            InteractionInput::ClosePanel => self.close_panel(),
            InteractionInput::SetReducedMotion(reduced) => self.set_reduced_motion(reduced),
        }
    }

    fn pointer_entered(&mut self) -> Vec<InteractionEffect> {
        self.pointer_inside = true;
        let token = self.next_token();
        if self.level == PresentationLevel::Selvage {
            vec![InteractionEffect::ScheduleDwell(token)]
        } else {
            Vec::new()
        }
    }

    fn pointer_left(&mut self) -> Vec<InteractionEffect> {
        self.pointer_inside = false;
        let token = self.next_token();
        if self.level == PresentationLevel::Ribbon {
            vec![InteractionEffect::ScheduleDismiss(token)]
        } else {
            Vec::new()
        }
    }

    fn dwell_elapsed(&mut self, token: InteractionToken) -> Vec<InteractionEffect> {
        if token == self.token() && self.pointer_inside && self.level == PresentationLevel::Selvage
        {
            self.level = PresentationLevel::Ribbon;
            vec![InteractionEffect::Render]
        } else {
            Vec::new()
        }
    }

    fn dismiss_elapsed(&mut self, token: InteractionToken) -> Vec<InteractionEffect> {
        if token == self.token() && !self.pointer_inside && self.level == PresentationLevel::Ribbon
        {
            self.level = PresentationLevel::Selvage;
            vec![InteractionEffect::Render]
        } else {
            Vec::new()
        }
    }

    fn open_panel(&mut self) -> Vec<InteractionEffect> {
        if self.level == PresentationLevel::Ribbon {
            self.next_token();
            self.level = PresentationLevel::Panel;
            vec![InteractionEffect::Render]
        } else {
            Vec::new()
        }
    }

    fn close_panel(&mut self) -> Vec<InteractionEffect> {
        if self.level == PresentationLevel::Panel {
            self.next_token();
            self.level = if self.pointer_inside {
                PresentationLevel::Ribbon
            } else {
                PresentationLevel::Selvage
            };
            vec![InteractionEffect::Render]
        } else {
            Vec::new()
        }
    }

    fn set_reduced_motion(&mut self, reduced: bool) -> Vec<InteractionEffect> {
        if self.reduced_motion == reduced {
            Vec::new()
        } else {
            self.reduced_motion = reduced;
            vec![InteractionEffect::Render]
        }
    }

    fn next_token(&mut self) -> InteractionToken {
        self.generation = self.generation.wrapping_add(1);
        self.token()
    }

    const fn token(&self) -> InteractionToken {
        InteractionToken(self.generation)
    }
}

/// Root-owned application state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppState {
    /// Validated user configuration.
    pub config: Config,
    outputs: BTreeMap<OutputId, OutputPresentation>,
}

impl AppState {
    /// Add and remove output presentation state after shell reconciliation.
    pub fn reconcile_outputs(
        &mut self,
        added: impl IntoIterator<Item = OutputId>,
        removed: impl IntoIterator<Item = OutputId>,
        reduced_motion: bool,
    ) {
        for id in removed {
            self.outputs.remove(&id);
        }
        for id in added {
            self.outputs
                .entry(id)
                .or_insert_with(|| OutputPresentation::new(reduced_motion));
        }
    }

    /// Look up an output presentation.
    #[must_use]
    pub fn output(&self, id: OutputId) -> Option<&OutputPresentation> {
        self.outputs.get(&id)
    }

    /// Mutably look up an output presentation.
    pub fn output_mut(&mut self, id: OutputId) -> Option<&mut OutputPresentation> {
        self.outputs.get_mut(&id)
    }

    /// Iterate over output identities without exposing shell objects.
    pub fn output_ids(&self) -> impl Iterator<Item = OutputId> + '_ {
        self.outputs.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduled_dwell(state: &mut OutputPresentation) -> InteractionToken {
        let effects = state.update(InteractionInput::PointerEntered);
        let [InteractionEffect::ScheduleDwell(token)] = effects.as_slice() else {
            panic!("pointer entry must schedule dwell");
        };
        *token
    }

    #[test]
    fn dwell_reveals_only_while_pointer_remains_inside() {
        let mut state = OutputPresentation::new(false);
        let token = scheduled_dwell(&mut state);
        state.update(InteractionInput::PointerLeft);
        assert!(
            state
                .update(InteractionInput::DwellElapsed(token))
                .is_empty()
        );
        assert_eq!(state.level(), PresentationLevel::Selvage);
    }

    #[test]
    fn completed_dwell_reveals_the_ribbon() {
        let mut state = OutputPresentation::new(false);
        let token = scheduled_dwell(&mut state);
        assert_eq!(
            state.update(InteractionInput::DwellElapsed(token)),
            vec![InteractionEffect::Render]
        );
        assert_eq!(state.level(), PresentationLevel::Ribbon);
    }

    #[test]
    fn interrupted_dismissal_cannot_collapse_the_ribbon() {
        let mut state = OutputPresentation::new(false);
        let dwell = scheduled_dwell(&mut state);
        state.update(InteractionInput::DwellElapsed(dwell));
        let effects = state.update(InteractionInput::PointerLeft);
        let [InteractionEffect::ScheduleDismiss(dismiss)] = effects.as_slice() else {
            panic!("pointer leave must schedule dismissal");
        };
        state.update(InteractionInput::PointerEntered);
        assert!(
            state
                .update(InteractionInput::DismissElapsed(*dismiss))
                .is_empty()
        );
        assert_eq!(state.level(), PresentationLevel::Ribbon);
    }

    #[test]
    fn panel_is_pinned_until_explicitly_closed() {
        let mut state = OutputPresentation::new(false);
        let dwell = scheduled_dwell(&mut state);
        state.update(InteractionInput::DwellElapsed(dwell));
        state.update(InteractionInput::OpenPanel);
        assert_eq!(state.level(), PresentationLevel::Panel);

        state.update(InteractionInput::PointerLeft);
        assert_eq!(state.level(), PresentationLevel::Panel);
        state.update(InteractionInput::ClosePanel);
        assert_eq!(state.level(), PresentationLevel::Selvage);
    }

    #[test]
    fn reduced_motion_changes_render_without_changing_level() {
        let mut state = OutputPresentation::new(false);
        assert_eq!(
            state.update(InteractionInput::SetReducedMotion(true)),
            vec![InteractionEffect::Render]
        );
        assert!(state.reduced_motion());
        assert_eq!(state.level(), PresentationLevel::Selvage);
    }
}
