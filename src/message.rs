//! Typed messages applied by the authoritative root model.

use crate::action::AppAction;
use crate::shell::outputs::ShellEvent;
use crate::state::{InteractionToken, OutputId};

/// Interaction timer category.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TimerKind {
    /// Top-edge dwell timer.
    Dwell,
    /// Ribbon dismissal timer.
    Dismiss,
}

/// Messages accepted by the root component.
#[derive(Clone, Debug)]
pub enum AppMessage {
    /// A typed user-interface action.
    Action(AppAction),
    /// Shell output or geometry lifecycle event.
    Shell(ShellEvent),
    /// A generation-checked interaction timer fired.
    TimerElapsed {
        /// Output that owns the timer.
        output: OutputId,
        /// Timer category.
        kind: TimerKind,
        /// Generation captured when scheduled.
        token: InteractionToken,
    },
    /// GTK's desktop animation preference changed.
    AnimationPreferenceChanged,
    /// Shut down supervised work and terminate the GTK application.
    Shutdown,
}
