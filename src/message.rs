//! Typed messages applied by the authoritative root model.

use crate::action::AppAction;
use crate::context::privacy::PrivacyUpdate;
use crate::services::activity::ActivityObservation;
use crate::services::audio::AudioUpdate;
use crate::services::clock::ClockTick;
use crate::services::mpris::MediaUpdate;
use crate::shell::outputs::ShellEvent;
use crate::state::{HyprlandUpdate, InteractionToken, OutputId};

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
    /// Ordered state from the direct Hyprland adapter.
    Hyprland(HyprlandUpdate),
    /// Boundary-aligned in-process clock update.
    Clock(ClockTick),
    /// Ordered state from the MPRIS session-bus adapter.
    Media(MediaUpdate),
    /// Ordered state from the direct PipeWire audio adapter.
    Audio(AudioUpdate),
    /// Validated local activity received from an authenticated peer.
    Activity(ActivityObservation),
    /// Privacy evidence observed through a selected native adapter.
    Privacy {
        /// Typed evidence update without source metadata.
        update: PrivacyUpdate,
        /// Adapter-relative observation time.
        observed_millis: u64,
    },
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
