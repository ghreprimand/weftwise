//! Root-state integration for ordered MPRIS updates.

use crate::context::arbitration::{ArbitrationInput, CandidateSource, Timestamp};
use crate::services::mpris::{MAX_PLAYERS, MediaAdmission, MediaUpdate, media_admission};

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
                    .take(MAX_PLAYERS)
                    .map(|player| (player.id.clone(), player))
                    .collect();
                // A complete snapshot is the authoritative baseline, so the
                // resurrection fence is cleared: no earlier removal can veto a
                // player the adapter just re-listed.
                self.media.removed_generation.clear();
                self.media.availability = AdapterAvailability::Ready;
                observed_millis
            }
            MediaUpdate::PlayerChanged {
                player,
                observed_millis,
            } => {
                self.admit_media_change(player);
                self.media.availability = AdapterAvailability::Ready;
                observed_millis
            }
            MediaUpdate::PlayerRemoved {
                id,
                observed_millis,
            } => {
                if let Some(removed) = self.media.players.remove(&id) {
                    self.fence_removed_player(id, removed.owner_generation);
                }
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

    /// Admit one `PlayerChanged` under the shared cap and resurrection fence.
    ///
    /// An update to a currently tracked player is applied only for the same or
    /// a newer owner generation. An absent identity is admitted only when its
    /// generation is strictly newer than any recorded removal, so a late
    /// refresh that began before the owner vanished cannot reintroduce it. A
    /// genuine reappearance clears the fence and is bounded by [`MAX_PLAYERS`].
    fn admit_media_change(&mut self, player: MediaPlayer) {
        let id = player.id.clone();
        if let Some(current) = self.media.players.get(&id) {
            if player.owner_generation >= current.owner_generation {
                self.media.players.insert(id, player);
            }
            return;
        }
        if self
            .media
            .removed_generation
            .get(&id)
            .is_some_and(|fenced| player.owner_generation <= *fenced)
        {
            return;
        }
        let under_cap_or_present =
            self.media.players.len() < MAX_PLAYERS || self.media.players.contains_key(&id);
        match media_admission(
            under_cap_or_present,
            self.media.players.keys().next_back(),
            &id,
        ) {
            MediaAdmission::Reject => {}
            MediaAdmission::Evict(evicted) => {
                if let Some(removed) = self.media.players.remove(&evicted) {
                    self.fence_removed_player(evicted, removed.owner_generation);
                }
                self.media.removed_generation.remove(&id);
                self.media.players.insert(id, player);
            }
            MediaAdmission::Admit => {
                self.media.removed_generation.remove(&id);
                self.media.players.insert(id, player);
            }
        }
    }

    /// Record the generation at which a player left, bounding the fence map.
    fn fence_removed_player(&mut self, id: super::MediaPlayerId, generation: u64) {
        if self.media.removed_generation.len() >= MAX_PLAYERS
            && !self.media.removed_generation.contains_key(&id)
            && let Some(oldest) = self.media.removed_generation.keys().next().cloned()
        {
            self.media.removed_generation.remove(&oldest);
        }
        self.media.removed_generation.insert(id, generation);
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
