//! Typed actions emitted by presentation components.

use crate::context::arbitration::CandidateAction;
use crate::state::OutputId;

/// Actions understood by the root application dispatcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppAction {
    /// Pointer entered an output's currently active region.
    PointerEntered(OutputId),
    /// Pointer entered a bounded path that cannot reliably sustain edge dwell.
    PointerEnteredImmediate(OutputId),
    /// Pointer left an output's currently active region.
    PointerLeft(OutputId),
    /// The keyboard-pinned Ribbon lost application focus.
    FocusLost(OutputId),
    /// Explicitly open the Panel on an output whose Ribbon is visible.
    OpenPanel(OutputId),
    /// Close the Panel after Escape, outside click, or an explicit request.
    ClosePanel(OutputId),
    /// Adjust the default output volume by conventional display percentage points.
    AdjustOutputVolume(OutputId, i16),
    /// Toggle mute on the default output device.
    ToggleOutputMute(OutputId),
    /// Invoke an action advertised by the selected presentation candidate.
    Candidate(OutputId, CandidateAction),
    /// Request an orderly application shutdown.
    Quit,
}
