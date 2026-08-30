//! Root-state integration for privacy evidence and temporary feedback.

use crate::context::arbitration::{ArbitrationInput, CandidateSource};
use crate::context::feedback::{FeedbackEvent, FeedbackKind};
use crate::context::privacy::PrivacyUpdate;

use super::{AppState, OutputId};

impl AppState {
    /// Apply one privacy observation and republish privacy candidates.
    ///
    /// The privacy domain is republished as a whole: stale `Privacy`-source
    /// candidates are cleared and the current supported evidence is upserted,
    /// so a source that stops being active no longer lingers in arbitration.
    pub fn apply_privacy_update(
        &mut self,
        update: PrivacyUpdate,
        observed_millis: u64,
    ) -> Vec<OutputId> {
        let now = self.advance_now(observed_millis);
        self.privacy.apply(update, now);
        self.arbitration
            .apply(ArbitrationInput::SourceStale(CandidateSource::Privacy), now);
        for candidate in self.privacy.candidates(now) {
            self.arbitration
                .apply(ArbitrationInput::Upsert(candidate), now);
        }
        self.refresh_arbitration_selections();
        self.output_ids().collect()
    }

    /// Offer one temporary feedback event to the rate-limited emitter.
    pub fn apply_feedback(&mut self, event: FeedbackEvent, observed_millis: u64) -> Vec<OutputId> {
        let now = self.advance_now(observed_millis);
        if let Some(input) = self.feedback.offer(event, now) {
            self.arbitration.apply(input, now);
            self.refresh_arbitration_selections();
            self.output_ids().collect()
        } else {
            Vec::new()
        }
    }

    /// Flush any coalesced feedback whose rate-limit interval has elapsed.
    ///
    /// This also advances the arbitration clock so temporary candidates whose
    /// TTL has elapsed are removed, even when no newer event is pending.
    pub fn flush_feedback(&mut self, observed_millis: u64) -> Vec<OutputId> {
        let now = self.advance_now(observed_millis);
        let before = self.selected_candidates.clone();
        let mut changed = false;
        for input in self.feedback.flush(now) {
            if self.arbitration.apply(input, now) {
                changed = true;
            }
        }
        if self.arbitration.apply(ArbitrationInput::Tick, now) {
            changed = true;
        }
        self.refresh_arbitration_selections();
        if changed || before != self.selected_candidates {
            self.output_ids().collect()
        } else {
            Vec::new()
        }
    }

    /// Explicitly dismiss one temporary feedback candidate.
    pub fn dismiss_feedback(&mut self, kind: FeedbackKind, observed_millis: u64) -> Vec<OutputId> {
        let now = self.advance_now(observed_millis);
        let input = self.feedback.dismiss(kind);
        self.arbitration.apply(input, now);
        self.refresh_arbitration_selections();
        self.output_ids().collect()
    }
}
