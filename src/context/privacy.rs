//! Transport-independent privacy evidence domain.
//!
//! This module owns the pure privacy state model and produces privacy
//! presentation candidates. It performs no D-Bus, PipeWire, or process work: a
//! future evidence adapter feeds observations in, and arbitration consumes the
//! candidates emitted here. Every source remains unsupported until its selected
//! adapter explicitly declares it observable. Unsupported detections are not
//! reported as inactive.
//!
//! The five evidence states remain distinct at all times. A missing or failed
//! source is never collapsed into an inactive indicator: a confirmed-off source
//! is `Inactive`, a source that has simply not reported yet is `Unknown`, a
//! source whose observation failed is `Unavailable`, and a previously observed
//! source that can no longer be trusted is `Stale`. Failure states stay visible
//! as uncertainty rather than silently implying that nothing is happening.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::context::arbitration::{
    CandidateId, CandidateSource, PreemptionClass, PresentationCandidate, PresentationKind,
    Severity, Timestamp,
};
use crate::state::DisplayText;

/// Minimum interval a privacy candidate stays selected once shown.
const PRIVACY_MINIMUM_DISPLAY: Duration = Duration::from_secs(2);
/// Maximum characters retained for a privacy accessible label.
const PRIVACY_ACCESSIBLE_CHARACTERS: usize = 128;

/// A privacy-relevant activity Weftwise can surface.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PrivacyEvidence {
    /// Microphone capture is in progress.
    Microphone,
    /// Camera capture is in progress.
    Camera,
    /// A screen-sharing session is active.
    ScreenShare,
    /// A recording session is active.
    Recording,
    /// An idle inhibitor is holding the session awake.
    IdleInhibitor,
}

impl PrivacyEvidence {
    /// Every evidence source in deterministic order.
    pub const ALL: [Self; 5] = [
        Self::Microphone,
        Self::Camera,
        Self::ScreenShare,
        Self::Recording,
        Self::IdleInhibitor,
    ];

    /// Stable candidate identity for this evidence source.
    const fn stable_id(self) -> &'static str {
        match self {
            Self::Microphone => "privacy.microphone",
            Self::Camera => "privacy.camera",
            Self::ScreenShare => "privacy.screenshare",
            Self::Recording => "privacy.recording",
            Self::IdleInhibitor => "privacy.idle_inhibitor",
        }
    }

    /// Human-facing noun used in accessible labels.
    const fn noun(self) -> &'static str {
        match self {
            Self::Microphone => "Microphone",
            Self::Camera => "Camera",
            Self::ScreenShare => "Screen sharing",
            Self::Recording => "Recording",
            Self::IdleInhibitor => "Idle inhibitor",
        }
    }

    /// Whether an active state for this source is privacy-critical capture.
    ///
    /// Capture of the user (microphone, camera, screen, recording) is
    /// privacy-critical and preempts every lower class. An idle inhibitor is a
    /// notable session state but is not user capture, so it is treated as an
    /// interruptible warning rather than critical.
    const fn is_critical_capture(self) -> bool {
        !matches!(self, Self::IdleInhibitor)
    }
}

/// Distinct privacy evidence states.
///
/// These never collapse into one another. In particular an adapter failure
/// produces `Unavailable` or `Stale`, never `Inactive`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PrivacyState {
    /// The activity is confirmed in progress.
    Active,
    /// The activity is confirmed not in progress.
    Inactive,
    /// The source is supported but has not reported a state yet.
    #[default]
    Unknown,
    /// The source cannot currently be observed.
    Unavailable,
    /// A previously observed state is now too old to trust.
    Stale,
}

/// One reducer input describing a privacy observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivacyUpdate {
    /// An adapter declares whether it can observe a source at all.
    Supported {
        /// Evidence source.
        evidence: PrivacyEvidence,
        /// Whether the adapter can observe this source.
        supported: bool,
    },
    /// An adapter reports a concrete observed state.
    Observed {
        /// Evidence source.
        evidence: PrivacyEvidence,
        /// Observed state.
        state: PrivacyState,
    },
    /// A single source became unobservable.
    Unavailable(PrivacyEvidence),
    /// The whole adapter degraded; supported observations become stale.
    Degraded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SourceRecord {
    supported: bool,
    state: PrivacyState,
    updated_at: Timestamp,
}

impl Default for SourceRecord {
    fn default() -> Self {
        Self {
            supported: false,
            state: PrivacyState::Unknown,
            updated_at: Timestamp::from_millis(0),
        }
    }
}

/// Root-owned pure privacy evidence domain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivacyDomain {
    sources: BTreeMap<PrivacyEvidence, SourceRecord>,
}

impl Default for PrivacyDomain {
    fn default() -> Self {
        let mut sources = BTreeMap::new();
        for evidence in PrivacyEvidence::ALL {
            sources.insert(evidence, SourceRecord::default());
        }
        Self { sources }
    }
}

impl PrivacyDomain {
    /// Apply one privacy reducer input at a normalized time.
    pub fn apply(&mut self, update: PrivacyUpdate, now: Timestamp) {
        match update {
            PrivacyUpdate::Supported {
                evidence,
                supported,
            } => {
                let record = self.sources.entry(evidence).or_default();
                if record.supported != supported {
                    record.state = PrivacyState::Unknown;
                }
                record.supported = supported;
                record.updated_at = now;
            }
            PrivacyUpdate::Observed { evidence, state } => {
                let record = self.sources.entry(evidence).or_default();
                record.state = state;
                record.updated_at = now;
            }
            PrivacyUpdate::Unavailable(evidence) => {
                let record = self.sources.entry(evidence).or_default();
                record.state = PrivacyState::Unavailable;
                record.updated_at = now;
            }
            PrivacyUpdate::Degraded => {
                for record in self.sources.values_mut() {
                    if record.supported
                        && matches!(
                            record.state,
                            PrivacyState::Active | PrivacyState::Inactive | PrivacyState::Unknown
                        )
                    {
                        record.state = PrivacyState::Stale;
                        record.updated_at = now;
                    }
                }
            }
        }
    }

    /// Current state of one evidence source.
    #[must_use]
    pub fn state(&self, evidence: PrivacyEvidence) -> PrivacyState {
        self.sources
            .get(&evidence)
            .map_or(PrivacyState::Unknown, |record| record.state)
    }

    /// Whether an evidence source is currently observable.
    #[must_use]
    pub fn is_supported(&self, evidence: PrivacyEvidence) -> bool {
        self.sources
            .get(&evidence)
            .is_some_and(|record| record.supported)
    }

    /// Build the bounded privacy candidates for the current state.
    ///
    /// Only supported sources produce candidates. A supported `Active` source
    /// produces a critical or warning candidate; a supported source in a
    /// failure state (`Unavailable` or `Stale`) produces an uncertainty
    /// candidate so the failure stays visible. `Inactive` and not-yet-reported
    /// `Unknown` supported sources produce no candidate, and unsupported sources never
    /// produce candidates.
    #[must_use]
    pub fn candidates(&self, now: Timestamp) -> Vec<PresentationCandidate> {
        let mut candidates = Vec::new();
        for evidence in PrivacyEvidence::ALL {
            let Some(record) = self.sources.get(&evidence) else {
                continue;
            };
            if !record.supported {
                continue;
            }
            let candidate = match record.state {
                PrivacyState::Active => Some(active_candidate(evidence, now)),
                PrivacyState::Unavailable | PrivacyState::Stale => {
                    Some(uncertain_candidate(evidence, record.state, now))
                }
                PrivacyState::Inactive | PrivacyState::Unknown => None,
            };
            if let Some(candidate) = candidate {
                candidates.push(candidate);
            }
        }
        candidates
    }
}

fn base_candidate(
    evidence: PrivacyEvidence,
    severity: Severity,
    preemption: PreemptionClass,
    accessible: &str,
    now: Timestamp,
) -> PresentationCandidate {
    PresentationCandidate {
        id: CandidateId::new(evidence.stable_id()).expect("static privacy identity"),
        source: CandidateSource::Privacy,
        kind: PresentationKind::Privacy,
        severity,
        label: DisplayText::new(evidence.noun(), 64),
        accessible_label: DisplayText::new(accessible, PRIVACY_ACCESSIBLE_CHARACTERS),
        created_at: now,
        updated_at: now,
        expires_at: None,
        minimum_display: PRIVACY_MINIMUM_DISPLAY,
        preemption,
        progress: None,
        actions: Vec::new(),
        output_affinity: None,
    }
}

fn active_candidate(evidence: PrivacyEvidence, now: Timestamp) -> PresentationCandidate {
    let (severity, preemption) = if evidence.is_critical_capture() {
        (Severity::Critical, PreemptionClass::PrivacyCritical)
    } else {
        (Severity::Warning, PreemptionClass::Interruptible)
    };
    let accessible = format!("{} active", evidence.noun());
    base_candidate(evidence, severity, preemption, &accessible, now)
}

fn uncertain_candidate(
    evidence: PrivacyEvidence,
    state: PrivacyState,
    now: Timestamp,
) -> PresentationCandidate {
    let qualifier = match state {
        PrivacyState::Unavailable => "unavailable",
        PrivacyState::Stale => "state stale",
        _ => "status unknown",
    };
    let accessible = format!("{} {qualifier}", evidence.noun());
    base_candidate(
        evidence,
        Severity::Warning,
        PreemptionClass::Interruptible,
        &accessible,
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(millis: u64) -> Timestamp {
        Timestamp::from_millis(millis)
    }

    #[test]
    fn unsupported_sources_never_produce_candidates() {
        let mut domain = PrivacyDomain::default();
        assert!(domain.candidates(at(0)).is_empty());
        // Even an active observation stays silent while unsupported.
        domain.apply(
            PrivacyUpdate::Observed {
                evidence: PrivacyEvidence::Microphone,
                state: PrivacyState::Active,
            },
            at(1),
        );
        assert!(domain.candidates(at(1)).is_empty());

        // Enabling support requires a fresh observation; it cannot expose a
        // value received before the adapter declared its coverage.
        domain.apply(
            PrivacyUpdate::Supported {
                evidence: PrivacyEvidence::Microphone,
                supported: true,
            },
            at(2),
        );
        assert_eq!(
            domain.state(PrivacyEvidence::Microphone),
            PrivacyState::Unknown
        );
        assert!(domain.candidates(at(2)).is_empty());
    }

    #[test]
    fn supported_active_capture_is_privacy_critical() {
        let mut domain = PrivacyDomain::default();
        domain.apply(
            PrivacyUpdate::Supported {
                evidence: PrivacyEvidence::Microphone,
                supported: true,
            },
            at(0),
        );
        domain.apply(
            PrivacyUpdate::Observed {
                evidence: PrivacyEvidence::Microphone,
                state: PrivacyState::Active,
            },
            at(1),
        );
        let candidates = domain.candidates(at(1));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].severity, Severity::Critical);
        assert_eq!(candidates[0].preemption, PreemptionClass::PrivacyCritical);
    }

    #[test]
    fn active_idle_inhibitor_is_interruptible_warning_not_critical() {
        let mut domain = PrivacyDomain::default();
        domain.apply(
            PrivacyUpdate::Supported {
                evidence: PrivacyEvidence::IdleInhibitor,
                supported: true,
            },
            at(0),
        );
        domain.apply(
            PrivacyUpdate::Observed {
                evidence: PrivacyEvidence::IdleInhibitor,
                state: PrivacyState::Active,
            },
            at(1),
        );
        let candidates = domain.candidates(at(1));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].severity, Severity::Warning);
        assert_eq!(candidates[0].preemption, PreemptionClass::Interruptible);
    }

    #[test]
    fn inactive_and_unknown_supported_sources_produce_no_candidate() {
        let mut domain = PrivacyDomain::default();
        domain.apply(
            PrivacyUpdate::Supported {
                evidence: PrivacyEvidence::Camera,
                supported: true,
            },
            at(0),
        );
        // Not-yet-reported Unknown produces no candidate.
        assert!(domain.candidates(at(0)).is_empty());
        domain.apply(
            PrivacyUpdate::Observed {
                evidence: PrivacyEvidence::Camera,
                state: PrivacyState::Inactive,
            },
            at(1),
        );
        assert!(domain.candidates(at(1)).is_empty());
    }

    #[test]
    fn failed_source_stays_visible_as_uncertainty_not_inactive() {
        let mut domain = PrivacyDomain::default();
        domain.apply(
            PrivacyUpdate::Supported {
                evidence: PrivacyEvidence::ScreenShare,
                supported: true,
            },
            at(0),
        );
        domain.apply(
            PrivacyUpdate::Unavailable(PrivacyEvidence::ScreenShare),
            at(1),
        );
        let candidates = domain.candidates(at(1));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].severity, Severity::Warning);
        assert_eq!(
            domain.state(PrivacyEvidence::ScreenShare),
            PrivacyState::Unavailable
        );
    }

    #[test]
    fn degraded_marks_supported_observations_stale_but_preserves_failures() {
        let mut domain = PrivacyDomain::default();
        domain.apply(
            PrivacyUpdate::Supported {
                evidence: PrivacyEvidence::Recording,
                supported: true,
            },
            at(0),
        );
        domain.apply(
            PrivacyUpdate::Observed {
                evidence: PrivacyEvidence::Recording,
                state: PrivacyState::Active,
            },
            at(1),
        );
        domain.apply(PrivacyUpdate::Degraded, at(2));
        // An active capture that can no longer be trusted becomes stale, not
        // inactive, and stays visible.
        assert_eq!(
            domain.state(PrivacyEvidence::Recording),
            PrivacyState::Stale
        );
        let candidates = domain.candidates(at(2));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].severity, Severity::Warning);

        // A source already Unavailable is not overwritten by Degraded.
        domain.apply(
            PrivacyUpdate::Unavailable(PrivacyEvidence::Recording),
            at(3),
        );
        domain.apply(PrivacyUpdate::Degraded, at(4));
        assert_eq!(
            domain.state(PrivacyEvidence::Recording),
            PrivacyState::Unavailable
        );
    }
}
