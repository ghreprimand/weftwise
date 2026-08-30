//! Typed actions emitted by presentation components.

/// Actions understood by the root application dispatcher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppAction {
    /// Request an orderly application shutdown.
    Quit,
}
