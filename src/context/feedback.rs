//! Transport-independent temporary feedback emitter.
//!
//! Temporary feedback is short-lived on-screen confirmation of a discrete
//! change: a volume or brightness step, a microphone toggle, a screenshot, or
//! the result of a launched command. This module owns the pure policy that
//! turns those events into bounded, self-expiring presentation candidates. It
//! performs no I/O; a future adapter feeds events in and arbitration consumes
//! the emitted inputs.
//!
//! Each feedback kind maps to one stable candidate identity, so repeated
//! events update a single candidate in place (deduplication) rather than
//! stacking. Every candidate carries an explicit expiration (TTL) and its
//! untrusted text and progress are bounded before emission. A per-kind minimum
//! interval rate-limits rapid streams such as a dragged volume slider: events
//! inside the interval are coalesced to the latest value and flushed once the
//! interval elapses, so the final value is never lost.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::context::arbitration::{
    ArbitrationInput, CandidateAction, CandidateId, CandidateSource, PreemptionClass,
    PresentationCandidate, PresentationKind, Progress, Severity, Timestamp,
};
use crate::state::DisplayText;

/// Maximum visible characters retained from a feedback label.
const FEEDBACK_LABEL_CHARACTERS: usize = 128;
/// Maximum accessible characters retained from a feedback label.
const FEEDBACK_ACCESSIBLE_CHARACTERS: usize = 256;

/// A category of temporary feedback.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FeedbackKind {
    /// Output volume changed.
    Volume,
    /// Microphone mute state changed.
    Microphone,
    /// Display brightness changed.
    Brightness,
    /// A screenshot was captured.
    Screenshot,
    /// A launched command produced a result.
    CommandResult,
}

impl FeedbackKind {
    /// Stable candidate identity for this feedback kind.
    const fn stable_id(self) -> &'static str {
        match self {
            Self::Volume => "feedback.volume",
            Self::Microphone => "feedback.microphone",
            Self::Brightness => "feedback.brightness",
            Self::Screenshot => "feedback.screenshot",
            Self::CommandResult => "feedback.command_result",
        }
    }

    /// Producer source used for stale-source invalidation.
    ///
    /// Volume and microphone feedback share the audio adapter's source so an
    /// audio-adapter loss clears them alongside persistent audio state.
    /// Brightness is system feedback; screenshots and command results are
    /// local activity.
    const fn source(self) -> CandidateSource {
        match self {
            Self::Volume | Self::Microphone => CandidateSource::Audio,
            Self::Brightness => CandidateSource::System,
            Self::Screenshot | Self::CommandResult => CandidateSource::Activity,
        }
    }

    /// Whether this feedback offers an explicit dismiss action.
    const fn is_dismissible(self) -> bool {
        matches!(self, Self::Screenshot | Self::CommandResult)
    }

    /// Bounds governing rate limiting and lifetime for this kind.
    const fn limits(self) -> FeedbackLimits {
        match self {
            // Continuous streams: throttle hard, expire quickly.
            Self::Volume | Self::Brightness => FeedbackLimits {
                min_interval: Duration::from_millis(40),
                ttl: Duration::from_millis(1_500),
                minimum_display: Duration::from_millis(400),
            },
            // Discrete toggles.
            Self::Microphone => FeedbackLimits {
                min_interval: Duration::from_millis(100),
                ttl: Duration::from_millis(2_000),
                minimum_display: Duration::from_millis(600),
            },
            // Results the user may want to read or dismiss.
            Self::Screenshot | Self::CommandResult => FeedbackLimits {
                min_interval: Duration::from_millis(250),
                ttl: Duration::from_secs(6),
                minimum_display: Duration::from_secs(1),
            },
        }
    }
}

/// Bounds governing one feedback kind's rate limit and lifetime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FeedbackLimits {
    min_interval: Duration,
    ttl: Duration,
    minimum_display: Duration,
}

/// One temporary feedback observation from an adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeedbackEvent {
    /// Feedback category.
    pub kind: FeedbackKind,
    /// Bounded visible label.
    pub label: String,
    /// Bounded accessible label; falls back to the visible label when empty.
    pub accessible_label: String,
    /// Semantic severity independent of color.
    pub severity: Severity,
    /// Optional bounded progress in basis points from zero through 10,000.
    pub progress: Option<u16>,
}

impl FeedbackEvent {
    /// Construct a minimal normal-severity feedback event.
    #[must_use]
    pub fn new(kind: FeedbackKind, label: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            accessible_label: String::new(),
            severity: Severity::Normal,
            progress: None,
        }
    }

    /// Attach bounded progress in basis points.
    #[must_use]
    pub fn with_progress(mut self, basis_points: u16) -> Self {
        self.progress = Some(basis_points);
        self
    }

    /// Attach an explicit severity.
    #[must_use]
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Attach a bounded accessible label.
    #[must_use]
    pub fn with_accessible_label(mut self, accessible_label: impl Into<String>) -> Self {
        self.accessible_label = accessible_label.into();
        self
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct Channel {
    last_emit: Option<Timestamp>,
    pending: Option<FeedbackEvent>,
}

/// Root-owned pure temporary feedback emitter.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FeedbackEmitter {
    channels: BTreeMap<FeedbackKind, Channel>,
}

impl FeedbackEmitter {
    /// Offer one feedback event at a normalized time.
    ///
    /// Returns an upsert when the event is emitted immediately. When the
    /// per-kind minimum interval has not elapsed, the event is retained as the
    /// latest pending value and `None` is returned; a later [`Self::flush`]
    /// past the interval emits it.
    pub fn offer(&mut self, event: FeedbackEvent, now: Timestamp) -> Option<ArbitrationInput> {
        let kind = event.kind;
        let limits = kind.limits();
        let channel = self.channels.entry(kind).or_default();
        let ready = channel
            .last_emit
            .is_none_or(|last| elapsed(last, now) >= limits.min_interval);
        if ready {
            channel.last_emit = Some(now);
            channel.pending = None;
            Some(ArbitrationInput::Upsert(build_candidate(
                &event, now, limits,
            )))
        } else {
            channel.pending = Some(event);
            None
        }
    }

    /// Emit any pending coalesced events whose interval has now elapsed.
    pub fn flush(&mut self, now: Timestamp) -> Vec<ArbitrationInput> {
        let mut inputs = Vec::new();
        for (kind, channel) in &mut self.channels {
            let limits = kind.limits();
            let ready = channel
                .last_emit
                .is_none_or(|last| elapsed(last, now) >= limits.min_interval);
            if ready && let Some(event) = channel.pending.take() {
                channel.last_emit = Some(now);
                inputs.push(ArbitrationInput::Upsert(build_candidate(
                    &event, now, limits,
                )));
            }
        }
        inputs
    }

    /// Explicitly remove one feedback kind's candidate.
    #[must_use]
    pub fn dismiss(&mut self, kind: FeedbackKind) -> ArbitrationInput {
        if let Some(channel) = self.channels.get_mut(&kind) {
            channel.pending = None;
        }
        ArbitrationInput::Remove {
            source: kind.source(),
            id: CandidateId::new(kind.stable_id()).expect("static feedback identity"),
        }
    }
}

fn elapsed(from: Timestamp, to: Timestamp) -> Duration {
    Duration::from_millis(to.as_millis().saturating_sub(from.as_millis()))
}

fn build_candidate(
    event: &FeedbackEvent,
    now: Timestamp,
    limits: FeedbackLimits,
) -> PresentationCandidate {
    let kind = event.kind;
    let accessible_label = if event.accessible_label.trim().is_empty() {
        &event.label
    } else {
        &event.accessible_label
    };
    let actions = if kind.is_dismissible() {
        vec![CandidateAction::Dismiss]
    } else {
        Vec::new()
    };
    let ttl_millis = u64::try_from(limits.ttl.as_millis()).unwrap_or(u64::MAX);
    let expires_at = Some(Timestamp::from_millis(
        now.as_millis().saturating_add(ttl_millis),
    ));
    PresentationCandidate {
        id: CandidateId::new(kind.stable_id()).expect("static feedback identity"),
        source: kind.source(),
        kind: PresentationKind::Feedback,
        severity: event.severity,
        label: DisplayText::new(&event.label, FEEDBACK_LABEL_CHARACTERS),
        accessible_label: DisplayText::new(accessible_label, FEEDBACK_ACCESSIBLE_CHARACTERS),
        created_at: now,
        updated_at: now,
        expires_at,
        minimum_display: limits.minimum_display,
        preemption: PreemptionClass::Immediate,
        progress: event.progress.map(Progress::from_basis_points),
        actions,
        output_affinity: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: u64) -> Timestamp {
        Timestamp::from_millis(millis)
    }

    fn upsert_id(input: &ArbitrationInput) -> String {
        match input {
            ArbitrationInput::Upsert(candidate) => candidate.id.as_str().to_owned(),
            other => panic!("expected upsert, got {other:?}"),
        }
    }

    #[test]
    fn first_event_emits_a_bounded_expiring_candidate() {
        let mut emitter = FeedbackEmitter::default();
        let input = emitter
            .offer(FeedbackEvent::new(FeedbackKind::Volume, "50%"), at(0))
            .expect("first event emits");
        match input {
            ArbitrationInput::Upsert(candidate) => {
                assert_eq!(candidate.kind, PresentationKind::Feedback);
                assert_eq!(candidate.preemption, PreemptionClass::Immediate);
                assert!(candidate.expires_at.is_some());
                assert!(candidate.actions.is_empty());
                assert_eq!(candidate.accessible_label.as_str(), "50%");
            }
            other => panic!("expected upsert, got {other:?}"),
        }
    }

    #[test]
    fn stable_identity_deduplicates_across_events() {
        let mut emitter = FeedbackEmitter::default();
        let first = emitter
            .offer(FeedbackEvent::new(FeedbackKind::Volume, "50%"), at(0))
            .expect("first");
        let second = emitter
            .offer(FeedbackEvent::new(FeedbackKind::Volume, "70%"), at(1_000))
            .expect("second past interval");
        assert_eq!(upsert_id(&first), upsert_id(&second));
    }

    #[test]
    fn rapid_events_are_rate_limited_and_flushed_to_the_latest_value() {
        let mut emitter = FeedbackEmitter::default();
        assert!(
            emitter
                .offer(FeedbackEvent::new(FeedbackKind::Volume, "50%"), at(0))
                .is_some()
        );
        // Within the 40ms interval: coalesced, not emitted.
        assert!(
            emitter
                .offer(FeedbackEvent::new(FeedbackKind::Volume, "55%"), at(10))
                .is_none()
        );
        assert!(
            emitter
                .offer(FeedbackEvent::new(FeedbackKind::Volume, "60%"), at(20))
                .is_none()
        );
        // A flush before the interval elapses emits nothing.
        assert!(emitter.flush(at(30)).is_empty());
        // Past the interval, the latest value flushes exactly once.
        let flushed = emitter.flush(at(50));
        assert_eq!(flushed.len(), 1);
        match &flushed[0] {
            ArbitrationInput::Upsert(candidate) => assert_eq!(candidate.label.as_str(), "60%"),
            other => panic!("expected upsert, got {other:?}"),
        }
        // No further pending work remains.
        assert!(emitter.flush(at(200)).is_empty());
    }

    #[test]
    fn distinct_kinds_do_not_share_a_channel() {
        let mut emitter = FeedbackEmitter::default();
        let volume = emitter
            .offer(FeedbackEvent::new(FeedbackKind::Volume, "50%"), at(0))
            .expect("volume");
        // A brightness event at the same instant is independent.
        let brightness = emitter
            .offer(FeedbackEvent::new(FeedbackKind::Brightness, "80%"), at(0))
            .expect("brightness");
        assert_ne!(upsert_id(&volume), upsert_id(&brightness));
    }

    #[test]
    fn dismissible_kinds_carry_a_dismiss_action() {
        let mut emitter = FeedbackEmitter::default();
        let input = emitter
            .offer(FeedbackEvent::new(FeedbackKind::Screenshot, "Saved"), at(0))
            .expect("screenshot");
        match input {
            ArbitrationInput::Upsert(candidate) => {
                assert_eq!(candidate.actions, vec![CandidateAction::Dismiss]);
            }
            other => panic!("expected upsert, got {other:?}"),
        }
    }

    #[test]
    fn untrusted_text_and_progress_are_bounded() {
        let mut emitter = FeedbackEmitter::default();
        let long = "x".repeat(FEEDBACK_LABEL_CHARACTERS + 50);
        let input = emitter
            .offer(
                FeedbackEvent::new(FeedbackKind::CommandResult, long).with_progress(50_000),
                at(0),
            )
            .expect("command result");
        match input {
            ArbitrationInput::Upsert(candidate) => {
                assert_eq!(
                    candidate.label.as_str().chars().count(),
                    FEEDBACK_LABEL_CHARACTERS
                );
                assert_eq!(candidate.progress.map(Progress::basis_points), Some(10_000));
            }
            other => panic!("expected upsert, got {other:?}"),
        }
    }

    #[test]
    fn dismiss_targets_the_kind_specific_identity_and_source() {
        let mut emitter = FeedbackEmitter::default();
        let _ = emitter.offer(FeedbackEvent::new(FeedbackKind::Volume, "50%"), at(0));
        match emitter.dismiss(FeedbackKind::Volume) {
            ArbitrationInput::Remove { source, id } => {
                assert_eq!(source, CandidateSource::Audio);
                assert_eq!(id.as_str(), "feedback.volume");
            }
            other => panic!("expected remove, got {other:?}"),
        }
    }
}
