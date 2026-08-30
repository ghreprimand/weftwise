//! Authoritative product state.

use crate::config::Config;

/// Root-owned application state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppState {
    /// Validated user configuration.
    pub config: Config,
}
