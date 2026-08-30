//! Root-state integration for ordered MPRIS updates.

use crate::context::arbitration::{ArbitrationInput, CandidateSource, Timestamp};
use crate::services::mpris::MediaUpdate;

use super::{
    AdapterAvailability, AppState, MediaPlayer, OutputId, media_candidate, select_active_player,
};

impl AppState {
    /// Apply one ordered MPRIS update and refresh its arbitration candidate.
    pub fn apply_media_update(&mut self, update: MediaUpdate) -> Vec<OutputId> {
        let observed_millis = match update {
            MediaUpdate::Connecting => {
                self.media.availability = if self.media.players.is_empty() {
                    AdapterAvailability::Starting
                } else {
                    AdapterAvailability::Stale
                };
                return Vec::new();
            }
            MediaUpdate::Snapshot {
                players,
                observed_millis,
            } => {
                self.media.players = players
                    .into_iter()
                    .take(32)
                    .map(|player| (player.id.clone(), player))
                    .collect();
                self.media.availability = AdapterAvailability::Ready;
                observed_millis
            }
            MediaUpdate::PlayerChanged {
                player,
                observed_millis,
            } => {
                if self.media.players.len() < 32 || self.media.players.contains_key(&player.id) {
                    self.media.players.insert(player.id.clone(), player);
                }
                self.media.availability = AdapterAvailability::Ready;
                observed_millis
            }
            MediaUpdate::PlayerRemoved {
                id,
                observed_millis,
            } => {
                self.media.players.remove(&id);
                observed_millis
            }
            MediaUpdate::Tick { observed_millis } => observed_millis,
            MediaUpdate::Unavailable => {
                self.media.availability = if self.media.players.is_empty() {
                    AdapterAvailability::Unavailable
                } else {
                    AdapterAvailability::Stale
                };
                self.media.active = None;
                return self.apply_arbitration(
                    ArbitrationInput::SourceStale(CandidateSource::Media),
                    self.arbitration_now,
                );
            }
        };

        self.media.active = select_active_player(&self.media.players, observed_millis)
            .map(|player| player.id.clone());
        let observed_millis =
            observed_millis.max(self.arbitration_now.as_millis().saturating_add(1));
        let now = Timestamp::from_millis(observed_millis);
        self.arbitration_now = now;
        if let Some(player) = self.active_media_player() {
            self.arbitration
                .apply(ArbitrationInput::Upsert(media_candidate(player, now)), now);
        } else {
            self.arbitration
                .apply(ArbitrationInput::SourceStale(CandidateSource::Media), now);
        }
        self.refresh_arbitration_selections();
        self.output_ids().collect()
    }

    /// Return the selected media player for a capability-gated output action.
    #[must_use]
    pub fn selected_media_player(&self, output: OutputId) -> Option<&MediaPlayer> {
        let selected = self.selected_candidates.get(&output)?;
        (selected.source == CandidateSource::Media)
            .then(|| self.active_media_player())
            .flatten()
    }

    fn active_media_player(&self) -> Option<&MediaPlayer> {
        self.media
            .active
            .as_ref()
            .and_then(|id| self.media.players.get(id))
    }
}
