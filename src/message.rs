//! Typed messages applied by the authoritative root model.

/// Messages accepted by the Phase 0 root component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppMessage {
    /// Shut down supervised work and terminate the GTK application.
    Shutdown,
}
