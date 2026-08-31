//! Bounded transport-independent protocol for local tracked activity.
//!
//! The future Unix-socket adapter and CLI share this schema. This module does
//! not create the endpoint or authenticate peers; it only defines and validates
//! one newline-delimited JSON frame at a time.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::ConfigPaths;
use crate::context::arbitration::{CandidateId, Progress};
use crate::state::DisplayText;

/// Supported local activity protocol version.
pub const ACTIVITY_PROTOCOL_VERSION: u16 = 1;
/// Maximum encoded frame size, including an optional line delimiter.
pub const MAX_ACTIVITY_FRAME_BYTES: usize = 16 * 1024;
/// Maximum visible characters retained from one activity label.
pub const MAX_ACTIVITY_LABEL_CHARACTERS: usize = 160;
/// Maximum accessible characters retained from one activity label.
pub const MAX_ACTIVITY_ACCESSIBLE_CHARACTERS: usize = 320;
/// Maximum producer-requested activity lifetime.
pub const MAX_ACTIVITY_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);
/// Versioned socket leaf reserved beneath Weftwise's XDG runtime directory.
pub const ACTIVITY_SOCKET_FILE: &str = "activity-v1.sock";

/// Stable protocol-safe identity for one producer-owned activity.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ActivityId(CandidateId);

impl ActivityId {
    /// Validate an activity identity without retaining invalid input.
    pub fn new(value: &str) -> Result<Self, ActivityProtocolError> {
        CandidateId::new(value)
            .map(Self)
            .ok_or(ActivityProtocolError::InvalidIdentity)
    }

    /// Borrow the validated identity for correlation or encoding.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Convert to the arbitration identity used by root state.
    #[must_use]
    pub fn candidate_id(&self) -> CandidateId {
        self.0.clone()
    }
}

impl fmt::Debug for ActivityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted-activity>")
    }
}

/// Semantic category of tracked local work.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// Countdown or elapsed timer.
    Timer,
    /// Software build or test run.
    Build,
    /// Bounded download progress.
    Download,
    /// Media, graphics, or document render.
    Render,
    /// Completed typed command result without command text.
    CommandResult,
}

/// Terminal result supplied by a producer.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityOutcome {
    /// Work completed successfully.
    Succeeded,
    /// Work completed unsuccessfully.
    Failed,
}

/// Initial state for one tracked activity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityPublication {
    id: ActivityId,
    kind: ActivityKind,
    label: DisplayText,
    accessible_label: DisplayText,
    progress: Option<Progress>,
    expires_after: Option<Duration>,
}

impl ActivityPublication {
    /// Construct a validated publication for the future endpoint or CLI.
    pub fn new(
        id: &str,
        kind: ActivityKind,
        label: &str,
        accessible_label: Option<&str>,
        progress_basis_points: Option<u16>,
        expires_after: Option<Duration>,
    ) -> Result<Self, ActivityProtocolError> {
        let label = required_text(label, MAX_ACTIVITY_LABEL_CHARACTERS)?;
        let accessible_label = optional_text(accessible_label, MAX_ACTIVITY_ACCESSIBLE_CHARACTERS)?
            .unwrap_or_else(|| label.clone());
        Ok(Self {
            id: ActivityId::new(id)?,
            kind,
            label,
            accessible_label,
            progress: validate_progress(progress_basis_points)?,
            expires_after: validate_duration(expires_after)?,
        })
    }

    /// Validated stable identity.
    #[must_use]
    pub fn id(&self) -> &ActivityId {
        &self.id
    }

    /// Semantic activity kind.
    #[must_use]
    pub const fn kind(&self) -> ActivityKind {
        self.kind
    }

    /// Sanitized visible label.
    #[must_use]
    pub fn label(&self) -> &str {
        self.label.as_str()
    }

    /// Sanitized accessible label.
    #[must_use]
    pub fn accessible_label(&self) -> &str {
        self.accessible_label.as_str()
    }

    /// Optional bounded progress.
    #[must_use]
    pub const fn progress(&self) -> Option<Progress> {
        self.progress
    }

    /// Optional bounded lifetime requested by the producer.
    #[must_use]
    pub const fn expires_after(&self) -> Option<Duration> {
        self.expires_after
    }
}

/// Partial replacement values for an existing activity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityUpdate {
    id: ActivityId,
    label: Option<DisplayText>,
    accessible_label: Option<DisplayText>,
    progress: Option<Progress>,
    expires_after: Option<Duration>,
}

impl ActivityUpdate {
    /// Construct a validated non-empty update for the future endpoint or CLI.
    pub fn new(
        id: &str,
        label: Option<&str>,
        accessible_label: Option<&str>,
        progress_basis_points: Option<u16>,
        expires_after: Option<Duration>,
    ) -> Result<Self, ActivityProtocolError> {
        if label.is_none()
            && accessible_label.is_none()
            && progress_basis_points.is_none()
            && expires_after.is_none()
        {
            return Err(ActivityProtocolError::EmptyUpdate);
        }
        Ok(Self {
            id: ActivityId::new(id)?,
            label: optional_text(label, MAX_ACTIVITY_LABEL_CHARACTERS)?,
            accessible_label: optional_text(accessible_label, MAX_ACTIVITY_ACCESSIBLE_CHARACTERS)?,
            progress: validate_progress(progress_basis_points)?,
            expires_after: validate_duration(expires_after)?,
        })
    }

    /// Validated stable identity.
    #[must_use]
    pub fn id(&self) -> &ActivityId {
        &self.id
    }

    /// Optional sanitized visible label replacement.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_ref().map(DisplayText::as_str)
    }

    /// Optional sanitized accessible label replacement.
    #[must_use]
    pub fn accessible_label(&self) -> Option<&str> {
        self.accessible_label.as_ref().map(DisplayText::as_str)
    }

    /// Optional bounded progress replacement.
    #[must_use]
    pub const fn progress(&self) -> Option<Progress> {
        self.progress
    }

    /// Optional bounded lifetime replacement.
    #[must_use]
    pub const fn expires_after(&self) -> Option<Duration> {
        self.expires_after
    }
}

/// Terminal state for an existing activity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivityCompletion {
    id: ActivityId,
    outcome: ActivityOutcome,
    label: Option<DisplayText>,
}

impl ActivityCompletion {
    /// Construct a validated completion for the future endpoint or CLI.
    pub fn new(
        id: &str,
        outcome: ActivityOutcome,
        label: Option<&str>,
    ) -> Result<Self, ActivityProtocolError> {
        Ok(Self {
            id: ActivityId::new(id)?,
            outcome,
            label: optional_text(label, MAX_ACTIVITY_LABEL_CHARACTERS)?,
        })
    }

    /// Validated stable identity.
    #[must_use]
    pub fn id(&self) -> &ActivityId {
        &self.id
    }

    /// Typed terminal result.
    #[must_use]
    pub const fn outcome(&self) -> ActivityOutcome {
        self.outcome
    }

    /// Optional sanitized terminal label.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_ref().map(DisplayText::as_str)
    }
}

/// One validated protocol operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivityEvent {
    /// Create or replace producer-owned activity state.
    Publish(ActivityPublication),
    /// Update selected fields on existing activity state.
    Update(ActivityUpdate),
    /// Complete existing activity state with a typed result.
    Complete(ActivityCompletion),
    /// Remove existing activity state without publishing a result.
    Cancel(ActivityId),
}

impl ActivityEvent {
    /// Construct a validated cancellation for the future endpoint or CLI.
    pub fn cancel(id: &str) -> Result<Self, ActivityProtocolError> {
        ActivityId::new(id).map(Self::Cancel)
    }

    /// Stable identity targeted by this operation.
    #[must_use]
    pub fn id(&self) -> &ActivityId {
        match self {
            Self::Publish(event) => event.id(),
            Self::Update(event) => event.id(),
            Self::Complete(event) => event.id(),
            Self::Cancel(id) => id,
        }
    }
}

/// Protocol parsing and validation failure without payload-bearing diagnostics.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ActivityProtocolError {
    /// Frame exceeded the fixed byte limit before JSON parsing.
    #[error("the activity frame exceeds the size limit")]
    FrameTooLarge,
    /// Frame contained no JSON value.
    #[error("the activity frame is empty")]
    EmptyFrame,
    /// JSON shape, field type, or enum value was invalid.
    #[error("the activity frame is malformed")]
    Malformed,
    /// Schema version is not supported by this process.
    #[error("the activity protocol version is unsupported")]
    UnsupportedVersion,
    /// Identity is empty, overlong, or contains protocol-unsafe bytes.
    #[error("the activity identity is invalid")]
    InvalidIdentity,
    /// A supplied label becomes empty after sanitation.
    #[error("the activity label is invalid")]
    InvalidLabel,
    /// Progress was outside zero through 10,000 basis points.
    #[error("the activity progress is invalid")]
    InvalidProgress,
    /// Requested lifetime was zero or exceeded the fixed maximum.
    #[error("the activity lifetime is invalid")]
    InvalidLifetime,
    /// An update did not contain a replacement value.
    #[error("the activity update contains no changes")]
    EmptyUpdate,
    /// A validated event could not be encoded.
    #[error("the activity frame could not be encoded")]
    Encoding,
}

/// Runtime endpoint path resolution failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ActivityEndpointError {
    /// The login environment does not expose an XDG runtime directory.
    #[error("the activity runtime directory is unavailable")]
    Unavailable,
    /// The configured application runtime directory is not absolute.
    #[error("the activity runtime directory is invalid")]
    NotAbsolute,
}

/// Resolve the reserved protocol socket path without creating it.
pub fn endpoint_path(paths: &ConfigPaths) -> Result<PathBuf, ActivityEndpointError> {
    let runtime = paths
        .runtime_dir
        .as_ref()
        .ok_or(ActivityEndpointError::Unavailable)?;
    if !runtime.is_absolute() {
        return Err(ActivityEndpointError::NotAbsolute);
    }
    Ok(runtime.join(ACTIVITY_SOCKET_FILE))
}

/// Decode and validate one bounded JSON-lines protocol frame.
pub fn decode_frame(frame: &[u8]) -> Result<ActivityEvent, ActivityProtocolError> {
    if frame.len() > MAX_ACTIVITY_FRAME_BYTES {
        return Err(ActivityProtocolError::FrameTooLarge);
    }
    let frame = frame.strip_suffix(b"\n").unwrap_or(frame);
    let frame = frame.strip_suffix(b"\r").unwrap_or(frame);
    if frame.iter().all(u8::is_ascii_whitespace) {
        return Err(ActivityProtocolError::EmptyFrame);
    }
    let wire: WireEvent =
        serde_json::from_slice(frame).map_err(|_| ActivityProtocolError::Malformed)?;
    wire.validate()
}

/// Encode one validated event as a bounded newline-delimited JSON frame.
pub fn encode_frame(event: &ActivityEvent) -> Result<Vec<u8>, ActivityProtocolError> {
    let wire = WireEvent::from(event);
    let mut frame = serde_json::to_vec(&wire).map_err(|_| ActivityProtocolError::Encoding)?;
    frame.push(b'\n');
    if frame.len() > MAX_ACTIVITY_FRAME_BYTES {
        return Err(ActivityProtocolError::FrameTooLarge);
    }
    Ok(frame)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum WireEvent {
    Publish {
        schema_version: u16,
        id: String,
        kind: ActivityKind,
        label: String,
        accessible_label: Option<String>,
        progress_basis_points: Option<u16>,
        expires_after_millis: Option<u64>,
    },
    Update {
        schema_version: u16,
        id: String,
        label: Option<String>,
        accessible_label: Option<String>,
        progress_basis_points: Option<u16>,
        expires_after_millis: Option<u64>,
    },
    Complete {
        schema_version: u16,
        id: String,
        outcome: ActivityOutcome,
        label: Option<String>,
    },
    Cancel {
        schema_version: u16,
        id: String,
    },
}

impl WireEvent {
    fn validate(self) -> Result<ActivityEvent, ActivityProtocolError> {
        match self {
            Self::Publish {
                schema_version,
                id,
                kind,
                label,
                accessible_label,
                progress_basis_points,
                expires_after_millis,
            } => {
                validate_version(schema_version)?;
                let id = ActivityId::new(&id)?;
                let label = required_text(&label, MAX_ACTIVITY_LABEL_CHARACTERS)?;
                let accessible_label = optional_text(
                    accessible_label.as_deref(),
                    MAX_ACTIVITY_ACCESSIBLE_CHARACTERS,
                )?
                .unwrap_or_else(|| label.clone());
                Ok(ActivityEvent::Publish(ActivityPublication {
                    id,
                    kind,
                    label,
                    accessible_label,
                    progress: validate_progress(progress_basis_points)?,
                    expires_after: validate_lifetime(expires_after_millis)?,
                }))
            }
            Self::Update {
                schema_version,
                id,
                label,
                accessible_label,
                progress_basis_points,
                expires_after_millis,
            } => {
                validate_version(schema_version)?;
                if label.is_none()
                    && accessible_label.is_none()
                    && progress_basis_points.is_none()
                    && expires_after_millis.is_none()
                {
                    return Err(ActivityProtocolError::EmptyUpdate);
                }
                Ok(ActivityEvent::Update(ActivityUpdate {
                    id: ActivityId::new(&id)?,
                    label: optional_text(label.as_deref(), MAX_ACTIVITY_LABEL_CHARACTERS)?,
                    accessible_label: optional_text(
                        accessible_label.as_deref(),
                        MAX_ACTIVITY_ACCESSIBLE_CHARACTERS,
                    )?,
                    progress: validate_progress(progress_basis_points)?,
                    expires_after: validate_lifetime(expires_after_millis)?,
                }))
            }
            Self::Complete {
                schema_version,
                id,
                outcome,
                label,
            } => {
                validate_version(schema_version)?;
                Ok(ActivityEvent::Complete(ActivityCompletion {
                    id: ActivityId::new(&id)?,
                    outcome,
                    label: optional_text(label.as_deref(), MAX_ACTIVITY_LABEL_CHARACTERS)?,
                }))
            }
            Self::Cancel { schema_version, id } => {
                validate_version(schema_version)?;
                Ok(ActivityEvent::Cancel(ActivityId::new(&id)?))
            }
        }
    }
}

impl From<&ActivityEvent> for WireEvent {
    fn from(event: &ActivityEvent) -> Self {
        match event {
            ActivityEvent::Publish(event) => Self::Publish {
                schema_version: ACTIVITY_PROTOCOL_VERSION,
                id: event.id.as_str().to_owned(),
                kind: event.kind,
                label: event.label.as_str().to_owned(),
                accessible_label: Some(event.accessible_label.as_str().to_owned()),
                progress_basis_points: event.progress.map(Progress::basis_points),
                expires_after_millis: event.expires_after.map(duration_millis),
            },
            ActivityEvent::Update(event) => Self::Update {
                schema_version: ACTIVITY_PROTOCOL_VERSION,
                id: event.id.as_str().to_owned(),
                label: event.label.as_ref().map(|label| label.as_str().to_owned()),
                accessible_label: event
                    .accessible_label
                    .as_ref()
                    .map(|label| label.as_str().to_owned()),
                progress_basis_points: event.progress.map(Progress::basis_points),
                expires_after_millis: event.expires_after.map(duration_millis),
            },
            ActivityEvent::Complete(event) => Self::Complete {
                schema_version: ACTIVITY_PROTOCOL_VERSION,
                id: event.id.as_str().to_owned(),
                outcome: event.outcome,
                label: event.label.as_ref().map(|label| label.as_str().to_owned()),
            },
            ActivityEvent::Cancel(id) => Self::Cancel {
                schema_version: ACTIVITY_PROTOCOL_VERSION,
                id: id.as_str().to_owned(),
            },
        }
    }
}

fn validate_version(version: u16) -> Result<(), ActivityProtocolError> {
    (version == ACTIVITY_PROTOCOL_VERSION)
        .then_some(())
        .ok_or(ActivityProtocolError::UnsupportedVersion)
}

fn required_text(value: &str, maximum: usize) -> Result<DisplayText, ActivityProtocolError> {
    let value = DisplayText::new(value, maximum);
    (!value.is_empty())
        .then_some(value)
        .ok_or(ActivityProtocolError::InvalidLabel)
}

fn optional_text(
    value: Option<&str>,
    maximum: usize,
) -> Result<Option<DisplayText>, ActivityProtocolError> {
    value.map(|value| required_text(value, maximum)).transpose()
}

fn validate_progress(value: Option<u16>) -> Result<Option<Progress>, ActivityProtocolError> {
    value
        .map(|value| {
            (value <= 10_000)
                .then_some(Progress::from_basis_points(value))
                .ok_or(ActivityProtocolError::InvalidProgress)
        })
        .transpose()
}

fn validate_lifetime(value: Option<u64>) -> Result<Option<Duration>, ActivityProtocolError> {
    value
        .map(|value| {
            let duration = Duration::from_millis(value);
            (value > 0 && duration <= MAX_ACTIVITY_LIFETIME)
                .then_some(duration)
                .ok_or(ActivityProtocolError::InvalidLifetime)
        })
        .transpose()
}

fn validate_duration(value: Option<Duration>) -> Result<Option<Duration>, ActivityProtocolError> {
    validate_lifetime(value.map(duration_millis))
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
