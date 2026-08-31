//! Root-state integration for authenticated local activity events.

use std::time::Duration;

use crate::context::arbitration::{
    ArbitrationInput, CandidateSource, PreemptionClass, PresentationCandidate, PresentationKind,
    Severity, Timestamp,
};
use crate::services::activity::{
    ActivityCompletion, ActivityEvent, ActivityId, ActivityKind, ActivityObservation,
    ActivityOutcome, ActivityPublication, ActivityUpdate,
};

use super::{AppState, DisplayText, OutputId};

/// Maximum concurrently retained producer-owned activity identities.
pub const MAX_TRACKED_ACTIVITIES: usize = 128;
const TERMINAL_FEEDBACK_LIFETIME: Duration = Duration::from_secs(4);
const MINIMUM_ACTIVITY_DISPLAY: Duration = Duration::from_millis(750);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ActivityRecord {
    id: ActivityId,
    kind: ActivityKind,
    label: DisplayText,
    accessible_label: DisplayText,
    progress: Option<crate::context::arbitration::Progress>,
    expires_after: Option<Duration>,
    created_at: Timestamp,
    updated_at: Timestamp,
}

impl AppState {
    /// Apply one authenticated local activity operation to root-owned state.
    pub fn apply_activity_observation(
        &mut self,
        observation: ActivityObservation,
    ) -> Vec<OutputId> {
        let now = self.advance_now(observation.observed_millis);
        match observation.event {
            ActivityEvent::Publish(publication) => self.publish_activity(publication, now),
            ActivityEvent::Update(update) => self.update_activity(update, now),
            ActivityEvent::Complete(completion) => self.complete_activity(completion, now),
            ActivityEvent::Cancel(id) => self.remove_activity(id, now),
        }
    }

    fn publish_activity(
        &mut self,
        publication: ActivityPublication,
        now: Timestamp,
    ) -> Vec<OutputId> {
        if self.activities.len() >= MAX_TRACKED_ACTIVITIES
            && !self.activities.contains_key(publication.id())
        {
            return Vec::new();
        }
        let record = ActivityRecord {
            id: publication.id().clone(),
            kind: publication.kind(),
            label: DisplayText::new(publication.label(), 256),
            accessible_label: DisplayText::new(publication.accessible_label(), 512),
            progress: publication.progress(),
            expires_after: publication.expires_after(),
            created_at: now,
            updated_at: now,
        };
        let candidate = record.candidate();
        self.activities.insert(record.id.clone(), record);
        self.apply_arbitration(ArbitrationInput::Upsert(candidate), now)
    }

    fn update_activity(&mut self, update: ActivityUpdate, now: Timestamp) -> Vec<OutputId> {
        let Some(record) = self.activities.get_mut(update.id()) else {
            return Vec::new();
        };
        if let Some(label) = update.label() {
            record.label = DisplayText::new(label, 256);
        }
        if let Some(accessible_label) = update.accessible_label() {
            record.accessible_label = DisplayText::new(accessible_label, 512);
        }
        if let Some(progress) = update.progress() {
            record.progress = Some(progress);
        }
        if let Some(expires_after) = update.expires_after() {
            record.expires_after = Some(expires_after);
        }
        record.updated_at = now;
        let candidate = record.candidate();
        self.apply_arbitration(ArbitrationInput::Upsert(candidate), now)
    }

    fn complete_activity(
        &mut self,
        completion: ActivityCompletion,
        now: Timestamp,
    ) -> Vec<OutputId> {
        let former = self.activities.remove(completion.id());
        let label = completion.label().map_or_else(
            || {
                completion_fallback(
                    former.as_ref().map(|record| record.kind),
                    completion.outcome(),
                )
            },
            |label| DisplayText::new(label, 256),
        );
        let candidate = PresentationCandidate {
            id: completion.id().candidate_id(),
            source: CandidateSource::Activity,
            kind: PresentationKind::Feedback,
            severity: match completion.outcome() {
                ActivityOutcome::Succeeded => Severity::Notice,
                ActivityOutcome::Failed => Severity::Warning,
            },
            label: label.clone(),
            accessible_label: label,
            created_at: now,
            updated_at: now,
            expires_at: Some(after(now, TERMINAL_FEEDBACK_LIFETIME)),
            minimum_display: MINIMUM_ACTIVITY_DISPLAY,
            preemption: PreemptionClass::Immediate,
            progress: None,
            actions: Vec::new(),
            output_affinity: None,
        };
        self.apply_arbitration(ArbitrationInput::Upsert(candidate), now)
    }

    fn remove_activity(&mut self, id: ActivityId, now: Timestamp) -> Vec<OutputId> {
        self.activities.remove(&id);
        self.apply_arbitration(
            ArbitrationInput::Remove {
                source: CandidateSource::Activity,
                id: id.candidate_id(),
            },
            now,
        )
    }
}

impl ActivityRecord {
    fn candidate(&self) -> PresentationCandidate {
        PresentationCandidate {
            id: self.id.candidate_id(),
            source: CandidateSource::Activity,
            kind: PresentationKind::Activity,
            severity: Severity::Normal,
            label: self.label.clone(),
            accessible_label: self.accessible_label.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            expires_at: self
                .expires_after
                .map(|duration| after(self.updated_at, duration)),
            minimum_display: MINIMUM_ACTIVITY_DISPLAY,
            preemption: PreemptionClass::Interruptible,
            progress: self.progress,
            actions: Vec::new(),
            output_affinity: None,
        }
    }
}

fn after(now: Timestamp, duration: Duration) -> Timestamp {
    let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    Timestamp::from_millis(now.as_millis().saturating_add(millis))
}

fn completion_fallback(kind: Option<ActivityKind>, outcome: ActivityOutcome) -> DisplayText {
    let kind = match kind {
        Some(ActivityKind::Timer) => "Timer",
        Some(ActivityKind::Build) => "Build",
        Some(ActivityKind::Download) => "Download",
        Some(ActivityKind::Render) => "Render",
        Some(ActivityKind::CommandResult) | None => "Activity",
    };
    let outcome = match outcome {
        ActivityOutcome::Succeeded => "complete",
        ActivityOutcome::Failed => "failed",
    };
    DisplayText::new(&format!("{kind} {outcome}"), 256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_activity_identity_count_is_bounded() {
        let mut state = AppState::default();
        for index in 0..=MAX_TRACKED_ACTIVITIES {
            let publication = ActivityPublication::new(
                &format!("build.synthetic.{index}"),
                ActivityKind::Build,
                "Synthetic build",
                None,
                None,
                None,
            )
            .expect("bounded synthetic publication");
            state.apply_activity_observation(ActivityObservation {
                event: ActivityEvent::Publish(publication),
                observed_millis: index as u64,
            });
        }
        assert_eq!(state.activities.len(), MAX_TRACKED_ACTIVITIES);
    }
}
