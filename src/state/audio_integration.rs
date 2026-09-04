//! Root-state integration for typed audio updates and commands.

use crate::context::arbitration::Severity;
use crate::context::feedback::{FeedbackEvent, FeedbackKind};
use crate::services::audio::{
    AudioCommand, AudioCommandKind, AudioCommandOutcome, AudioNode, AudioUpdate, MovableStreamState,
};

use super::{AppState, OutputId};

impl AppState {
    /// Apply one ordered audio adapter update and emit any user-facing feedback.
    ///
    /// Volume and mute changes on the default sink or source produce temporary
    /// feedback candidates; local command outcomes produce request feedback.
    pub fn apply_audio_update(&mut self, update: AudioUpdate) -> Vec<OutputId> {
        let observed_millis = audio_update_observed_millis(&update);
        let now = self.advance_now(observed_millis);
        let previous_sink = self.audio.default_sink().cloned();
        let previous_source = self.audio.default_source().cloned();
        let is_node_observation = matches!(update, AudioUpdate::NodeChanged { .. });
        let mut feedback_events: Vec<FeedbackEvent> = Vec::new();

        match update {
            AudioUpdate::Connecting => {
                self.audio.mark_stale();
                // A reconnecting transport cannot vouch for the previous
                // selection; clear it so a stale active stream cannot validate a
                // move to a vanished subject until the adapter re-publishes.
                self.audio
                    .set_movable_stream(MovableStreamState::Unavailable);
                return Vec::new();
            }
            AudioUpdate::Snapshot {
                nodes,
                default_sink,
                default_source,
                generation,
                ..
            } => self
                .audio
                .apply_snapshot(nodes, default_sink, default_source, generation),
            AudioUpdate::NodeChanged { node, .. } => self.audio.upsert_node(node),
            AudioUpdate::NodeRemoved { id, .. } => self.audio.remove_node(id),
            AudioUpdate::DefaultChanged {
                direction, node, ..
            } => self.audio.set_default(direction, node),
            AudioUpdate::CommandOutcome { outcome, .. } => {
                feedback_events.push(audio_outcome_feedback(&outcome));
            }
            AudioUpdate::MovableStreamChanged { state, .. } => {
                // A capability projection only; it gates the move action and
                // produces no user-facing feedback candidate.
                self.audio.set_movable_stream(state);
                return Vec::new();
            }
            AudioUpdate::Degraded => {
                // An endpoint inventory overflow leaves the node set partial;
                // retain it as Stale uncertainty and clear control/move state.
                self.audio.mark_degraded();
                return self.output_ids().collect();
            }
            AudioUpdate::Unavailable => {
                self.audio.mark_unavailable();
                self.audio
                    .set_movable_stream(MovableStreamState::Unavailable);
                return self.output_ids().collect();
            }
        }

        if is_node_observation
            && let (Some(old), Some(sink)) = (previous_sink.as_ref(), self.audio.default_sink())
        {
            let changed = old.id == sink.id
                && old.capabilities.can_set_volume
                && sink.capabilities.can_set_volume
                && (old.volume != sink.volume || old.muted != sink.muted);
            if changed {
                feedback_events.push(sink_volume_feedback(sink));
            }
        }
        if is_node_observation
            && let (Some(old), Some(source)) =
                (previous_source.as_ref(), self.audio.default_source())
            && old.id == source.id
            && old.capabilities.can_set_mute
            && source.capabilities.can_set_mute
            && old.muted != source.muted
        {
            feedback_events.push(FeedbackEvent::new(
                FeedbackKind::Microphone,
                if source.muted {
                    "Microphone muted"
                } else {
                    "Microphone on"
                },
            ));
        }

        for event in feedback_events {
            if let Some(input) = self.feedback.offer(event, now) {
                self.arbitration.apply(input, now);
            }
        }
        self.refresh_arbitration_selections();
        self.output_ids().collect()
    }

    /// Validate a requested audio command against current capabilities.
    ///
    /// Returns the typed command to dispatch when permitted. When the request
    /// is not currently possible, emits content-free error feedback and returns
    /// `None` alongside the outputs whose projection changed.
    pub fn audio_command(
        &mut self,
        kind: AudioCommandKind,
        observed_millis: u64,
    ) -> (Option<AudioCommand>, Vec<OutputId>) {
        match self.audio.validate(kind) {
            Ok(command) => (Some(command), Vec::new()),
            Err(error) => {
                let now = self.advance_now(observed_millis);
                let event = FeedbackEvent::new(
                    FeedbackKind::CommandResult,
                    format!("Audio: {}", error.reason()),
                )
                .with_severity(Severity::Warning);
                if let Some(input) = self.feedback.offer(event, now) {
                    self.arbitration.apply(input, now);
                    self.refresh_arbitration_selections();
                    (None, self.output_ids().collect())
                } else {
                    (None, Vec::new())
                }
            }
        }
    }
}

fn audio_update_observed_millis(update: &AudioUpdate) -> u64 {
    match update {
        AudioUpdate::Connecting | AudioUpdate::Degraded | AudioUpdate::Unavailable => 0,
        AudioUpdate::Snapshot {
            observed_millis, ..
        }
        | AudioUpdate::NodeChanged {
            observed_millis, ..
        }
        | AudioUpdate::NodeRemoved {
            observed_millis, ..
        }
        | AudioUpdate::DefaultChanged {
            observed_millis, ..
        }
        | AudioUpdate::CommandOutcome {
            observed_millis, ..
        }
        | AudioUpdate::MovableStreamChanged {
            observed_millis, ..
        } => *observed_millis,
    }
}

fn sink_volume_feedback(sink: &AudioNode) -> FeedbackEvent {
    if sink.muted {
        FeedbackEvent::new(FeedbackKind::Volume, "Volume muted")
            .with_accessible_label("Output volume muted")
    } else {
        let percent = sink.volume.cubic_percent();
        let basis_points = u32::from(percent)
            .saturating_mul(100)
            .min(10_000)
            .try_into()
            .unwrap_or(u16::MAX);
        FeedbackEvent::new(FeedbackKind::Volume, format!("Volume {percent}%"))
            .with_accessible_label(format!("Output volume {percent} percent"))
            .with_progress(basis_points)
    }
}

fn audio_outcome_feedback(outcome: &AudioCommandOutcome) -> FeedbackEvent {
    match outcome.error {
        Some(error) => FeedbackEvent::new(
            FeedbackKind::CommandResult,
            format!("{}: {}", outcome.label, error.reason()),
        )
        .with_severity(Severity::Warning),
        None => FeedbackEvent::new(
            FeedbackKind::CommandResult,
            format!("{} request sent", outcome.label),
        ),
    }
}
