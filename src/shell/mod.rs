//! Wayland surface ownership.

use std::env;

use thiserror::Error;

pub mod outputs;
pub mod surface;

/// Layer-shell exclusive-zone policy and retained comparison value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ExclusiveZone {
    /// Do not reserve compositor work area.
    Zero,
    /// Request physical-edge overlay placement without reserving work area.
    #[default]
    NegativeOne,
}

impl ExclusiveZone {
    /// Integer passed to gtk4-layer-shell.
    #[must_use]
    pub const fn value(self) -> i32 {
        match self {
            Self::Zero => 0,
            Self::NegativeOne => -1,
        }
    }
}

/// Manual native-proof controls sourced before GTK startup.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProofOptions {
    /// Exclusive-zone policy to present.
    pub exclusive_zone: ExclusiveZone,
    /// Explicit reduced-motion request, combined with GTK's desktop preference.
    pub reduced_motion: bool,
}

impl ProofOptions {
    /// Parse public-safe proof environment switches.
    pub fn from_environment() -> Result<Self, ProofOptionError> {
        let exclusive_zone = match env::var_os("WEFTWISE_EXCLUSIVE_ZONE") {
            None => ExclusiveZone::NegativeOne,
            Some(value) if value == "0" => ExclusiveZone::Zero,
            Some(value) if value == "-1" => ExclusiveZone::NegativeOne,
            Some(_) => return Err(ProofOptionError::ExclusiveZone),
        };
        let reduced_motion = match env::var_os("WEFTWISE_REDUCED_MOTION") {
            None => false,
            Some(value) if value == "1" || value == "true" => true,
            Some(value) if value == "0" || value == "false" => false,
            Some(_) => return Err(ProofOptionError::ReducedMotion),
        };

        Ok(Self {
            exclusive_zone,
            reduced_motion,
        })
    }
}

/// Invalid manual proof switch. Values are never included in diagnostics.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ProofOptionError {
    /// `WEFTWISE_EXCLUSIVE_ZONE` was not `0` or `-1`.
    #[error("WEFTWISE_EXCLUSIVE_ZONE must be 0 or -1")]
    ExclusiveZone,
    /// `WEFTWISE_REDUCED_MOTION` was not a supported boolean.
    #[error("WEFTWISE_REDUCED_MOTION must be 0, 1, false, or true")]
    ReducedMotion,
}
