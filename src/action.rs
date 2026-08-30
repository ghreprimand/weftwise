//! Typed actions emitted by presentation components.

use crate::context::arbitration::CandidateAction;
use crate::state::OutputId;

/// Actions understood by the root application dispatcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppAction {
    /// Pointer entered an output's currently active region.
    PointerEntered(OutputId),
    /// Pointer left an output's currently active region.
    PointerLeft(OutputId),
    /// Explicitly open the Panel on an output whose Ribbon is visible.
    OpenPanel(OutputId),
    /// Close the Panel after Escape, outside click, or an explicit request.
    ClosePanel(OutputId),
    /// Invoke an action advertised by the selected presentation candidate.
    Candidate(OutputId, CandidateAction),
    /// Request an orderly application shutdown.
    Quit,
}
