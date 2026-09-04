//! Root-state integration for privacy evidence and temporary feedback.

use crate::context::arbitration::{ArbitrationInput, CandidateSource};
use crate::context::feedback::{FeedbackEvent, FeedbackKind};
use crate::context::privacy::{PrivacyEvidence, PrivacyState, PrivacyUpdate};

use super::{AppState, OutputId};

impl AppState {
    pub(super) fn support_hyprland_screencast_evidence(&mut self) {
        self.apply_privacy_update(
            PrivacyUpdate::Supported {
                evidence: PrivacyEvidence::ScreenShare,
                supported: true,
            },
            0,
        );
    }

    pub(super) fn reset_hyprland_screencast_evidence(&mut self) {
        self.hyprland_screencasts = 0;
        self.support_hyprland_screencast_evidence();
        self.publish_hyprland_screencast_state(PrivacyState::Unknown);
    }

    pub(super) fn apply_hyprland_screencast_event(&mut self, active: bool) {
        self.hyprland_screencasts = if active {
            self.hyprland_screencasts.saturating_add(1)
        } else {
            self.hyprland_screencasts.saturating_sub(1)
        };
        let state = if self.hyprland_screencasts > 0 {
            PrivacyState::Active
        } else {
            PrivacyState::Unknown
        };
        self.publish_hyprland_screencast_state(state);
    }

    pub(super) fn stale_hyprland_screencast_evidence(&mut self) {
        self.hyprland_screencasts = 0;
        self.publish_hyprland_screencast_state(PrivacyState::Stale);
    }

    pub(super) fn unavailable_hyprland_screencast_evidence(&mut self) {
        self.hyprland_screencasts = 0;
        self.apply_privacy_update(PrivacyUpdate::Unavailable(PrivacyEvidence::ScreenShare), 0);
    }

    fn publish_hyprland_screencast_state(&mut self, state: PrivacyState) {
        self.apply_privacy_update(
            PrivacyUpdate::Observed {
                evidence: PrivacyEvidence::ScreenShare,
                state,
            },
            0,
        );
    }

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
        // A re-affirm carries the current state to refresh a source's age; when
        // it does not change the presentation, `apply` still refreshes
        // `updated_at` (so aging is deferred) but the candidates are not
        // republished, avoiding a needless rerender.
        let before = self.privacy.presentation_fingerprint();
        self.privacy.apply(update, now);
        if self.privacy.presentation_fingerprint() == before {
            return Vec::new();
        }
        self.arbitration
            .apply(ArbitrationInput::SourceStale(CandidateSource::Privacy), now);
        for candidate in self.privacy.candidates(now) {
            self.arbitration
                .apply(ArbitrationInput::Upsert(candidate), now);
        }
        self.refresh_arbitration_selections();
        self.output_ids().collect()
    }

    /// Advance monotonic time and age stale privacy evidence.
    ///
    /// Invoked from the clock tick. Supported evidence older than the age
    /// threshold becomes conservatively `Stale`; the privacy candidates are
    /// republished only when a source actually changed, so an ordinary tick
    /// with nothing to age produces no output.
    pub fn age_privacy(&mut self, observed_millis: u64) -> Vec<OutputId> {
        let now = self.advance_now(observed_millis);
        if !self.privacy.expire_stale(now) {
            return Vec::new();
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::privacy::PRIVACY_EVIDENCE_MAX_AGE_MILLIS;

    fn support_active(state: &mut AppState, evidence: PrivacyEvidence, at: u64) {
        state.apply_privacy_update(
            PrivacyUpdate::Supported {
                evidence,
                supported: true,
            },
            at,
        );
        state.apply_privacy_update(
            PrivacyUpdate::Observed {
                evidence,
                state: PrivacyState::Active,
            },
            at,
        );
    }

    #[test]
    fn reaffirmed_evidence_does_not_age_while_a_silent_one_does() {
        let mut state = AppState::default();
        support_active(&mut state, PrivacyEvidence::Microphone, 1);
        support_active(&mut state, PrivacyEvidence::Camera, 1);
        assert_eq!(
            state.privacy.state(PrivacyEvidence::Microphone),
            PrivacyState::Active
        );

        // The microphone re-affirms its unchanged Active state near the age
        // threshold; the camera stays silent. An unchanged re-affirm refreshes
        // the age without rerendering any output.
        let reaffirm_at = 1 + PRIVACY_EVIDENCE_MAX_AGE_MILLIS - 1;
        let outputs = state.apply_privacy_update(
            PrivacyUpdate::Observed {
                evidence: PrivacyEvidence::Microphone,
                state: PrivacyState::Active,
            },
            reaffirm_at,
        );
        assert!(
            outputs.is_empty(),
            "an unchanged re-affirm must not republish"
        );

        // Age well past the threshold. The silent camera ages to stale; the
        // re-affirmed microphone, refreshed at reaffirm_at, does not.
        state.age_privacy(reaffirm_at + PRIVACY_EVIDENCE_MAX_AGE_MILLIS - 10);
        assert_eq!(
            state.privacy.state(PrivacyEvidence::Microphone),
            PrivacyState::Active,
            "re-affirmed evidence must not age to false uncertainty"
        );
        assert_eq!(
            state.privacy.state(PrivacyEvidence::Camera),
            PrivacyState::Stale,
            "a genuinely silent source must age to stale"
        );
    }
}
