//! Pure deterministic presentation candidate arbitration.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use crate::state::{DisplayText, OutputName};

/// Maximum typed actions retained on one untrusted producer candidate.
pub const MAX_CANDIDATE_ACTIONS: usize = 8;
/// Maximum live candidate identities retained across all producers.
pub const MAX_CANDIDATES: usize = 256;
/// Maximum visible characters retained from one producer label.
pub const MAX_CANDIDATE_LABEL_CHARACTERS: usize = 256;
/// Maximum accessible characters retained from one producer label.
pub const MAX_CANDIDATE_ACCESSIBLE_CHARACTERS: usize = 512;
/// Maximum characters retained for a future producer identity.
pub const MAX_CANDIDATE_SOURCE_CHARACTERS: usize = 64;
/// Maximum absolute seek delta accepted from a media candidate.
pub const MAXIMUM_MEDIA_SEEK_MILLIS: i64 = 86_400_000;
/// Maximum minimum-display interval accepted from one producer.
pub const MAXIMUM_MINIMUM_DISPLAY: Duration = Duration::from_secs(30);

/// Normalized monotonic time used by the pure arbitration reducer.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Timestamp(u64);

impl Timestamp {
    /// Construct a normalized millisecond timestamp.
    #[must_use]
    pub const fn from_millis(value: u64) -> Self {
        Self(value)
    }

    /// Return the normalized millisecond value.
    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    fn saturating_add(self, duration: Duration) -> Self {
        let milliseconds = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        Self(self.0.saturating_add(milliseconds))
    }
}

/// Stable producer-defined identity used for deduplication and update-in-place.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CandidateId(String);

impl CandidateId {
    /// Validate a bounded protocol-safe identity.
    #[must_use]
    pub fn new(value: &str) -> Option<Self> {
        if value.is_empty()
            || value.len() > 128
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-')
            })
        {
            return None;
        }
        Some(Self(value.to_owned()))
    }

    /// Borrow the stable identity for in-process correlation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CandidateId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted-candidate>")
    }
}

/// Adapter or root-state producer responsible for a candidate.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CandidateSource {
    /// Root-owned clock fallback.
    Clock,
    /// Root-owned compositor context.
    Compositor,
    /// MPRIS media adapter.
    Media,
    /// Audio adapter or temporary audio feedback.
    Audio,
    /// Privacy evidence aggregation.
    Privacy,
    /// Process or local activity protocol.
    Activity,
    /// System-health adapter.
    System,
    /// Notification summary adapter.
    Notification,
    /// A future bounded producer identity.
    Other(DisplayText),
}

/// Semantic role of candidate content.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum PresentationKind {
    /// Lowest-priority clock or other always-available content.
    Fallback,
    /// Ordinary workspace or active-client context.
    Context,
    /// Long-lived progress or media activity.
    Activity,
    /// Short-lived command, volume, or brightness feedback.
    Feedback,
    /// Exceptional system state requiring attention.
    Warning,
    /// Capture or other privacy evidence.
    Privacy,
}

impl PresentationKind {
    const fn rank(self) -> u8 {
        match self {
            Self::Fallback => 0,
            Self::Context => 1,
            Self::Activity => 2,
            Self::Feedback => 3,
            Self::Warning => 4,
            Self::Privacy => 5,
        }
    }

    /// Stable Selvage region used by this semantic kind.
    #[must_use]
    pub const fn region(self) -> CandidateRegion {
        match self {
            Self::Fallback | Self::Context => CandidateRegion::None,
            Self::Activity | Self::Feedback => CandidateRegion::Activity,
            Self::Warning | Self::Privacy => CandidateRegion::Attention,
        }
    }
}

/// Color-independent Selvage placement for a selected candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateRegion {
    /// The candidate does not add a compact mark.
    None,
    /// Stable center activity region.
    Activity,
    /// Stable end attention region.
    Attention,
}

/// Semantic severity independent of visual color.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    /// Ordinary non-exceptional information.
    #[default]
    Normal,
    /// Notable but non-warning information.
    Notice,
    /// A degraded or unsafe state requiring attention.
    Warning,
    /// A critical state that must remain unmistakable.
    Critical,
}

/// Whether and how a candidate may interrupt a sticky selection.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum PreemptionClass {
    /// Never interrupts an active minimum display interval.
    #[default]
    Passive,
    /// May replace passive content during its minimum interval.
    Interruptible,
    /// Immediate user feedback may interrupt lower classes.
    Immediate,
    /// Privacy-critical evidence interrupts every lower class.
    PrivacyCritical,
}

/// Bounded progress represented in basis points from zero through 10,000.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Progress(u16);

impl Progress {
    /// Construct clamped progress in basis points.
    #[must_use]
    pub const fn from_basis_points(value: u16) -> Self {
        Self(if value > 10_000 { 10_000 } else { value })
    }

    /// Return bounded progress in basis points.
    #[must_use]
    pub const fn basis_points(self) -> u16 {
        self.0
    }
}

/// Typed action advertised by selected content.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CandidateAction {
    /// Reveal the candidate's details in the Panel.
    RevealDetails,
    /// Dismiss dismissible temporary content.
    Dismiss,
    /// Toggle the selected media player's playback state.
    MediaPlayPause,
    /// Request the selected media player's previous item.
    MediaPrevious,
    /// Request the selected media player's next item.
    MediaNext,
    /// Seek the selected media player by a signed millisecond delta.
    MediaSeek(i64),
}

/// Complete bounded candidate published into arbitration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationCandidate {
    /// Stable identity used for deduplication.
    pub id: CandidateId,
    /// Producer identity used for stale-source invalidation.
    pub source: CandidateSource,
    /// Semantic content kind.
    pub kind: PresentationKind,
    /// Semantic severity.
    pub severity: Severity,
    /// Bounded visible label.
    pub label: DisplayText,
    /// Bounded accessible label that does not depend on color.
    pub accessible_label: DisplayText,
    /// Normalized creation time.
    pub created_at: Timestamp,
    /// Normalized most recent producer update time.
    pub updated_at: Timestamp,
    /// Optional exclusive expiration boundary.
    pub expires_at: Option<Timestamp>,
    /// Minimum duration retained after selection unless explicitly preempted.
    pub minimum_display: Duration,
    /// Interruption policy.
    pub preemption: PreemptionClass,
    /// Optional bounded progress.
    pub progress: Option<Progress>,
    /// Bounded typed actions.
    pub actions: Vec<CandidateAction>,
    /// Optional compositor output affinity.
    pub output_affinity: Option<OutputName>,
}

impl PresentationCandidate {
    fn normalize(&mut self) {
        if self.updated_at < self.created_at {
            self.updated_at = self.created_at;
        }
        self.label = DisplayText::new(self.label.as_str(), MAX_CANDIDATE_LABEL_CHARACTERS);
        self.accessible_label = DisplayText::new(
            self.accessible_label.as_str(),
            MAX_CANDIDATE_ACCESSIBLE_CHARACTERS,
        );
        if let CandidateSource::Other(source) = &mut self.source {
            *source = DisplayText::new(source.as_str(), MAX_CANDIDATE_SOURCE_CHARACTERS);
        }
        for action in &mut self.actions {
            if let CandidateAction::MediaSeek(delta) = action {
                *delta = (*delta).clamp(-MAXIMUM_MEDIA_SEEK_MILLIS, MAXIMUM_MEDIA_SEEK_MILLIS);
            }
        }
        self.actions.sort_unstable();
        self.actions.dedup();
        self.actions.truncate(MAX_CANDIDATE_ACTIONS);
        self.minimum_display = self.minimum_display.min(MAXIMUM_MINIMUM_DISPLAY);
        if self.accessible_label.is_empty() {
            self.accessible_label = self.label.clone();
        }
    }

    fn expired_at(&self, now: Timestamp) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    fn applies_to(&self, output: Option<&OutputName>) -> bool {
        self.output_affinity
            .as_ref()
            .is_none_or(|affinity| output == Some(affinity))
    }
}

/// One pure reducer input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArbitrationInput {
    /// Insert a new identity or replace its data without resetting stickiness.
    Upsert(PresentationCandidate),
    /// Remove one source-scoped candidate explicitly.
    Remove {
        /// Producer that owns the identity.
        source: CandidateSource,
        /// Stable identity within that producer.
        id: CandidateId,
    },
    /// Remove every candidate from a producer whose state is stale.
    SourceStale(CandidateSource),
    /// Remove selection memory for a compositor output that disappeared.
    OutputRemoved(OutputName),
    /// Re-evaluate expiration and minimum-duration boundaries.
    Tick,
}

/// Immutable selected content projection consumed by root rendering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationProjection {
    /// Stable selected identity.
    pub id: CandidateId,
    /// Producer that owns the selected identity.
    pub source: CandidateSource,
    /// Semantic content kind.
    pub kind: PresentationKind,
    /// Semantic severity.
    pub severity: Severity,
    /// Bounded visible label.
    pub label: String,
    /// Bounded accessible label.
    pub accessible_label: String,
    /// Optional bounded progress.
    pub progress: Option<Progress>,
    /// Bounded typed actions.
    pub actions: Vec<CandidateAction>,
    /// Affinity retained so the root can place global detail on one output.
    pub output_affinity: Option<OutputName>,
}

impl From<&PresentationCandidate> for PresentationProjection {
    fn from(candidate: &PresentationCandidate) -> Self {
        Self {
            id: candidate.id.clone(),
            source: candidate.source.clone(),
            kind: candidate.kind,
            severity: candidate.severity,
            label: candidate.label.as_str().to_owned(),
            accessible_label: candidate.accessible_label.as_str().to_owned(),
            progress: candidate.progress,
            actions: candidate.actions.clone(),
            output_affinity: candidate.output_affinity.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Selection {
    key: CandidateKey,
    selected_at: Timestamp,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CandidateKey {
    source: CandidateSource,
    id: CandidateId,
}

impl From<&PresentationCandidate> for CandidateKey {
    fn from(candidate: &PresentationCandidate) -> Self {
        Self {
            source: candidate.source.clone(),
            id: candidate.id.clone(),
        }
    }
}

/// Root-owned deterministic candidate set and per-output sticky selections.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Arbitrator {
    candidates: BTreeMap<CandidateKey, PresentationCandidate>,
    selections: BTreeMap<Option<OutputName>, Selection>,
    last_now: Timestamp,
}

impl Arbitrator {
    /// Apply one reducer input at normalized time and report whether state changed.
    pub fn apply(&mut self, input: ArbitrationInput, now: Timestamp) -> bool {
        let before_candidates = self.candidates.clone();
        let before_selections = self.selections.clone();
        let now = self.normalize_now(now);
        self.remove_expired(now);
        match input {
            ArbitrationInput::Upsert(mut candidate) => {
                candidate.normalize();
                let key = CandidateKey::from(&candidate);
                let existing = self.candidates.get(&key);
                let stale_or_conflicting_revision = existing.is_some_and(|current| {
                    candidate.updated_at < current.updated_at
                        || (candidate.updated_at == current.updated_at && candidate != *current)
                });
                if !stale_or_conflicting_revision {
                    if candidate.expired_at(now) {
                        self.candidates.remove(&key);
                    } else if existing.is_some() || self.candidates.len() < MAX_CANDIDATES {
                        self.candidates.insert(key, candidate);
                    }
                }
            }
            ArbitrationInput::Remove { source, id } => {
                self.candidates.remove(&CandidateKey { source, id });
            }
            ArbitrationInput::SourceStale(source) => {
                self.candidates
                    .retain(|_, candidate| candidate.source != source);
            }
            ArbitrationInput::OutputRemoved(output) => {
                self.selections.remove(&Some(output));
            }
            ArbitrationInput::Tick => {}
        }
        self.remove_orphaned_selections();
        self.candidates != before_candidates || self.selections != before_selections
    }

    /// Select a deterministic projection for an output at normalized time.
    ///
    /// Passing `None` selects only global candidates for a non-output consumer.
    pub fn select_for(
        &mut self,
        output: Option<&OutputName>,
        now: Timestamp,
    ) -> Option<PresentationProjection> {
        let now = self.normalize_now(now);
        self.remove_expired(now);
        self.remove_orphaned_selections();
        let key = output.cloned();
        let best = self.best_key(output).cloned();
        let current = self.selections.get(&key).cloned();
        let selected = match (current, best) {
            (None, None) => None,
            (None, Some(best)) => Some(Selection {
                key: best,
                selected_at: now,
            }),
            (Some(_), None) => None,
            (Some(current), Some(best)) if current.key == best => Some(current),
            (Some(current), Some(best)) => {
                let current_candidate = &self.candidates[&current.key];
                let challenger = &self.candidates[&best];
                let minimum_elapsed = now
                    >= current
                        .selected_at
                        .saturating_add(current_candidate.minimum_display);
                let explicitly_preempts = challenger.preemption > current_candidate.preemption;
                let normalized_late_arrival = challenger.created_at <= current.selected_at
                    && compare_candidates(challenger, current_candidate).is_gt();
                if minimum_elapsed || explicitly_preempts || normalized_late_arrival {
                    Some(Selection {
                        key: best,
                        selected_at: now,
                    })
                } else {
                    Some(current)
                }
            }
        };

        match selected {
            Some(selection) => {
                let projection = self
                    .candidates
                    .get(&selection.key)
                    .map(PresentationProjection::from);
                self.selections.insert(key, selection);
                projection
            }
            None => {
                self.selections.remove(&key);
                None
            }
        }
    }

    /// Number of live deduplicated candidates.
    #[must_use]
    pub fn len(&self) -> usize {
        self.candidates.len()
    }

    /// Whether no live candidates remain.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    fn remove_expired(&mut self, now: Timestamp) {
        self.candidates
            .retain(|_, candidate| !candidate.expired_at(now));
    }

    fn normalize_now(&mut self, now: Timestamp) -> Timestamp {
        self.last_now = self.last_now.max(now);
        self.last_now
    }

    fn remove_orphaned_selections(&mut self) {
        self.selections
            .retain(|_, selection| self.candidates.contains_key(&selection.key));
    }

    fn best_key(&self, output: Option<&OutputName>) -> Option<&CandidateKey> {
        self.candidates
            .iter()
            .filter(|(_, candidate)| candidate.applies_to(output))
            .max_by(|(_, left), (_, right)| compare_candidates(left, right))
            .map(|(key, _)| key)
    }
}

fn compare_candidates(left: &PresentationCandidate, right: &PresentationCandidate) -> Ordering {
    left.preemption
        .cmp(&right.preemption)
        .then_with(|| left.severity.cmp(&right.severity))
        .then_with(|| left.kind.rank().cmp(&right.kind.rank()))
        .then_with(|| left.updated_at.cmp(&right.updated_at))
        // Earlier creation and lexically smaller identity win final ties.
        .then_with(|| right.created_at.cmp(&left.created_at))
        .then_with(|| right.id.cmp(&left.id))
        .then_with(|| right.source.cmp(&left.source))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, kind: PresentationKind, created: u64) -> PresentationCandidate {
        PresentationCandidate {
            id: CandidateId::new(id).expect("synthetic identity"),
            source: CandidateSource::Activity,
            kind,
            severity: Severity::Normal,
            label: DisplayText::new(id, 64),
            accessible_label: DisplayText::new(&format!("{id} status"), 128),
            created_at: Timestamp::from_millis(created),
            updated_at: Timestamp::from_millis(created),
            expires_at: None,
            minimum_display: Duration::from_secs(2),
            preemption: PreemptionClass::Passive,
            progress: None,
            actions: Vec::new(),
            output_affinity: None,
        }
    }

    #[test]
    fn stable_tie_breaking_is_independent_of_insertion_order() {
        let mut first = Arbitrator::default();
        let mut second = Arbitrator::default();
        let alpha = candidate("alpha", PresentationKind::Activity, 10);
        let beta = candidate("beta", PresentationKind::Activity, 10);
        first.apply(
            ArbitrationInput::Upsert(beta.clone()),
            Timestamp::from_millis(10),
        );
        first.select_for(None, Timestamp::from_millis(10));
        first.apply(
            ArbitrationInput::Upsert(alpha.clone()),
            Timestamp::from_millis(10),
        );
        second.apply(ArbitrationInput::Upsert(alpha), Timestamp::from_millis(10));
        second.apply(ArbitrationInput::Upsert(beta), Timestamp::from_millis(10));

        assert_eq!(
            first
                .select_for(None, Timestamp::from_millis(10))
                .expect("selection")
                .id
                .as_str(),
            "alpha"
        );
        assert_eq!(
            first.select_for(None, Timestamp::from_millis(10)),
            second.select_for(None, Timestamp::from_millis(10))
        );
    }

    #[test]
    fn passive_challenger_waits_for_the_minimum_display_interval() {
        let mut arbitrator = Arbitrator::default();
        let current = candidate("current", PresentationKind::Context, 0);
        arbitrator.apply(ArbitrationInput::Upsert(current), Timestamp::from_millis(0));
        assert_eq!(
            arbitrator
                .select_for(None, Timestamp::from_millis(0))
                .expect("initial")
                .id
                .as_str(),
            "current"
        );
        let challenger = candidate("challenger", PresentationKind::Activity, 100);
        arbitrator.apply(
            ArbitrationInput::Upsert(challenger),
            Timestamp::from_millis(100),
        );
        assert_eq!(
            arbitrator
                .select_for(None, Timestamp::from_millis(100))
                .expect("sticky")
                .id
                .as_str(),
            "current"
        );
        assert_eq!(
            arbitrator
                .select_for(None, Timestamp::from_millis(2_000))
                .expect("released")
                .id
                .as_str(),
            "challenger"
        );
    }

    #[test]
    fn privacy_critical_content_preempts_sticky_activity() {
        let mut arbitrator = Arbitrator::default();
        arbitrator.apply(
            ArbitrationInput::Upsert(candidate("activity", PresentationKind::Activity, 0)),
            Timestamp::from_millis(0),
        );
        arbitrator.select_for(None, Timestamp::from_millis(0));
        let mut privacy = candidate("privacy", PresentationKind::Privacy, 1);
        privacy.severity = Severity::Critical;
        privacy.preemption = PreemptionClass::PrivacyCritical;
        arbitrator.apply(ArbitrationInput::Upsert(privacy), Timestamp::from_millis(1));
        assert_eq!(
            arbitrator
                .select_for(None, Timestamp::from_millis(1))
                .expect("privacy")
                .id
                .as_str(),
            "privacy"
        );
    }

    #[test]
    fn expiration_reveals_the_fallback_even_during_stickiness() {
        let mut arbitrator = Arbitrator::default();
        let fallback = candidate("clock", PresentationKind::Fallback, 0);
        let mut feedback = candidate("feedback", PresentationKind::Feedback, 1);
        feedback.expires_at = Some(Timestamp::from_millis(50));
        arbitrator.apply(
            ArbitrationInput::Upsert(fallback),
            Timestamp::from_millis(0),
        );
        arbitrator.apply(
            ArbitrationInput::Upsert(feedback),
            Timestamp::from_millis(1),
        );
        assert_eq!(
            arbitrator
                .select_for(None, Timestamp::from_millis(1))
                .expect("feedback")
                .id
                .as_str(),
            "feedback"
        );
        assert_eq!(
            arbitrator
                .select_for(None, Timestamp::from_millis(50))
                .expect("fallback")
                .id
                .as_str(),
            "clock"
        );
    }

    #[test]
    fn update_in_place_deduplicates_and_bounds_producer_fields() {
        let mut arbitrator = Arbitrator::default();
        let mut original = candidate("build", PresentationKind::Activity, 0);
        original.actions = vec![CandidateAction::Dismiss; MAX_CANDIDATE_ACTIONS + 4];
        original.actions.push(CandidateAction::MediaSeek(i64::MAX));
        original.minimum_display = Duration::from_secs(90);
        original.accessible_label = DisplayText::default();
        original.label = DisplayText::new(
            &"x".repeat(MAX_CANDIDATE_LABEL_CHARACTERS + 20),
            MAX_CANDIDATE_LABEL_CHARACTERS + 20,
        );
        arbitrator.apply(
            ArbitrationInput::Upsert(original),
            Timestamp::from_millis(0),
        );
        let projection = arbitrator
            .select_for(None, Timestamp::from_millis(0))
            .expect("selection");
        assert_eq!(arbitrator.len(), 1);
        assert_eq!(
            projection.actions,
            vec![
                CandidateAction::Dismiss,
                CandidateAction::MediaSeek(MAXIMUM_MEDIA_SEEK_MILLIS),
            ]
        );
        assert_eq!(
            projection.label.chars().count(),
            MAX_CANDIDATE_LABEL_CHARACTERS
        );
        assert_eq!(projection.accessible_label, projection.label);

        let mut update = candidate("build", PresentationKind::Activity, 0);
        update.updated_at = Timestamp::from_millis(20);
        update.label = DisplayText::new("updated", 64);
        arbitrator.apply(ArbitrationInput::Upsert(update), Timestamp::from_millis(20));
        assert_eq!(arbitrator.len(), 1);
        assert_eq!(
            arbitrator
                .select_for(None, Timestamp::from_millis(20))
                .expect("updated")
                .label,
            "updated"
        );
    }

    #[test]
    fn stale_and_conflicting_updates_do_not_replace_current_state() {
        let mut arbitrator = Arbitrator::default();
        let mut current = candidate("build", PresentationKind::Activity, 10);
        current.updated_at = Timestamp::from_millis(20);
        current.label = DisplayText::new("current", 64);
        arbitrator.apply(
            ArbitrationInput::Upsert(current),
            Timestamp::from_millis(20),
        );

        let mut stale = candidate("build", PresentationKind::Activity, 10);
        stale.updated_at = Timestamp::from_millis(15);
        stale.label = DisplayText::new("stale", 64);
        arbitrator.apply(ArbitrationInput::Upsert(stale), Timestamp::from_millis(20));
        assert_eq!(
            arbitrator
                .select_for(None, Timestamp::from_millis(20))
                .expect("current selection")
                .label,
            "current"
        );

        let mut conflict = candidate("build", PresentationKind::Activity, 10);
        conflict.updated_at = Timestamp::from_millis(20);
        conflict.label = DisplayText::new("conflict", 64);
        arbitrator.apply(
            ArbitrationInput::Upsert(conflict),
            Timestamp::from_millis(20),
        );
        assert_eq!(
            arbitrator
                .select_for(None, Timestamp::from_millis(20))
                .expect("unchanged selection")
                .label,
            "current"
        );
    }

    #[test]
    fn candidate_set_is_bounded_without_evicting_live_state() {
        let mut arbitrator = Arbitrator::default();
        for index in 0..=MAX_CANDIDATES {
            let item = candidate(
                &format!("item-{index}"),
                PresentationKind::Activity,
                u64::try_from(index).expect("bounded index"),
            );
            arbitrator.apply(
                ArbitrationInput::Upsert(item),
                Timestamp::from_millis(u64::try_from(index).expect("bounded index")),
            );
        }
        assert_eq!(arbitrator.len(), MAX_CANDIDATES);
    }

    #[test]
    fn output_affinity_and_stale_source_are_enforced() {
        let first = OutputName::new("SYNTH-1").expect("output");
        let second = OutputName::new("SYNTH-2").expect("output");
        let mut local = candidate("local", PresentationKind::Warning, 0);
        local.output_affinity = Some(first.clone());
        let mut arbitrator = Arbitrator::default();
        arbitrator.apply(ArbitrationInput::Upsert(local), Timestamp::from_millis(0));
        assert!(
            arbitrator
                .select_for(Some(&first), Timestamp::from_millis(0))
                .is_some()
        );
        assert!(
            arbitrator
                .select_for(Some(&second), Timestamp::from_millis(0))
                .is_none()
        );
        arbitrator.apply(
            ArbitrationInput::SourceStale(CandidateSource::Activity),
            Timestamp::from_millis(1),
        );
        assert!(arbitrator.is_empty());
    }

    #[test]
    fn identical_ids_from_different_sources_do_not_collide() {
        let mut activity = candidate("shared", PresentationKind::Activity, 0);
        activity.source = CandidateSource::Activity;
        let mut privacy = candidate("shared", PresentationKind::Privacy, 1);
        privacy.source = CandidateSource::Privacy;
        privacy.severity = Severity::Critical;
        privacy.preemption = PreemptionClass::PrivacyCritical;
        let mut arbitrator = Arbitrator::default();
        arbitrator.apply(
            ArbitrationInput::Upsert(activity),
            Timestamp::from_millis(0),
        );
        arbitrator.apply(ArbitrationInput::Upsert(privacy), Timestamp::from_millis(1));

        assert_eq!(arbitrator.len(), 2);
        assert_eq!(
            arbitrator
                .select_for(None, Timestamp::from_millis(1))
                .expect("privacy selection")
                .source,
            CandidateSource::Privacy
        );
        arbitrator.apply(
            ArbitrationInput::SourceStale(CandidateSource::Privacy),
            Timestamp::from_millis(2),
        );
        assert_eq!(arbitrator.len(), 1);
        assert_eq!(
            arbitrator
                .select_for(None, Timestamp::from_millis(2))
                .expect("activity remains")
                .source,
            CandidateSource::Activity
        );
    }

    #[test]
    fn backwards_clock_input_cannot_resurrect_expired_content() {
        let mut expiring = candidate("temporary", PresentationKind::Feedback, 0);
        expiring.expires_at = Some(Timestamp::from_millis(50));
        let mut arbitrator = Arbitrator::default();
        arbitrator.apply(
            ArbitrationInput::Upsert(expiring),
            Timestamp::from_millis(0),
        );
        assert!(
            arbitrator
                .select_for(None, Timestamp::from_millis(40))
                .is_some()
        );
        assert!(
            arbitrator
                .select_for(None, Timestamp::from_millis(50))
                .is_none()
        );
        assert!(
            arbitrator
                .select_for(None, Timestamp::from_millis(10))
                .is_none()
        );
    }
}
