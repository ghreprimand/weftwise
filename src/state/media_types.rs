//! MPRIS media value types: bounded, sanitized player identity and metadata.

use std::collections::BTreeMap;
use std::fmt;

use super::{AdapterAvailability, DisplayText};

/// Stable, bounded MPRIS well-known bus identity.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MediaPlayerId(String);

impl MediaPlayerId {
    /// Accept only protocol-valid MPRIS well-known names.
    #[must_use]
    pub fn new(value: &str) -> Option<Self> {
        const PREFIX: &str = "org.mpris.MediaPlayer2.";
        if value.len() > 255
            || !value.starts_with(PREFIX)
            || value.len() == PREFIX.len()
            || zbus::names::WellKnownName::try_from(value).is_err()
        {
            return None;
        }
        Some(Self(value.to_owned()))
    }

    /// Borrow the exact identity for D-Bus correlation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for MediaPlayerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted-media-player>")
    }
}

/// Normalized MPRIS playback state.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum MediaPlaybackStatus {
    /// The player returned an unsupported or malformed state.
    #[default]
    Unknown,
    /// No active playback item.
    Stopped,
    /// Playback is paused.
    Paused,
    /// Playback is active.
    Playing,
}

impl MediaPlaybackStatus {
    /// Parse the three MPRIS values without treating unknown input as activity.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value {
            "Playing" => Self::Playing,
            "Paused" => Self::Paused,
            "Stopped" => Self::Stopped,
            _ => Self::Unknown,
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Stopped => "stopped",
            Self::Paused => "paused",
            Self::Playing => "playing",
        }
    }
}

/// Bounded, sanitized media metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MediaMetadata {
    /// Display title, at most 256 characters.
    pub title: DisplayText,
    /// Joined artists, at most 256 characters.
    pub artist: DisplayText,
    /// Safe bounded artwork URL retained for future rendering.
    pub art_url: Option<DisplayText>,
    /// Duration in microseconds, clamped to seven days.
    pub duration_micros: u64,
    /// Position in microseconds, clamped to the duration when known.
    pub position_micros: u64,
}

impl MediaMetadata {
    /// Bound metadata received from an untrusted session-bus peer.
    #[must_use]
    pub fn bounded(
        title: &str,
        artists: &[String],
        art_url: Option<&str>,
        duration_micros: Option<i64>,
        position_micros: i64,
    ) -> Self {
        const MAX_DURATION_MICROS: u64 = 7 * 24 * 60 * 60 * 1_000_000;
        let duration_micros = duration_micros
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or_default()
            .min(MAX_DURATION_MICROS);
        let position_micros =
            u64::try_from(position_micros)
                .unwrap_or_default()
                .min(if duration_micros == 0 {
                    MAX_DURATION_MICROS
                } else {
                    duration_micros
                });
        let artist = artists
            .iter()
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .take(16)
            .collect::<Vec<_>>()
            .join(", ");
        let art_url = art_url.and_then(|value| {
            let bounded = DisplayText::new(value, 2_048);
            let safe_scheme = ["https://", "http://"]
                .iter()
                .any(|prefix| bounded.as_str().starts_with(prefix));
            safe_scheme.then_some(bounded)
        });
        Self {
            title: DisplayText::new(title, 256),
            artist: DisplayText::new(&artist, 256),
            art_url,
            duration_micros,
            position_micros,
        }
    }
}

/// MPRIS methods advertised by one player.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MediaCapabilities {
    /// The player accepts control methods.
    pub can_control: bool,
    /// The player can begin or resume playback.
    pub can_play: bool,
    /// The player can pause playback.
    pub can_pause: bool,
    /// The player can select a previous item.
    pub can_previous: bool,
    /// The player can select a next item.
    pub can_next: bool,
    /// The player can seek relative to its current position.
    pub can_seek: bool,
}

/// Complete root-owned snapshot for one player.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaPlayer {
    /// Stable well-known bus identity.
    pub id: MediaPlayerId,
    /// Adapter-owned generation for the current unique D-Bus owner.
    pub owner_generation: u64,
    /// Sanitized human-readable player identity.
    pub identity: DisplayText,
    /// Current playback state.
    pub status: MediaPlaybackStatus,
    /// Sanitized metadata and bounded position.
    pub metadata: MediaMetadata,
    /// Advertised control capabilities.
    pub capabilities: MediaCapabilities,
    /// Monotonic adapter sequence used for deterministic recent activity.
    pub activity_sequence: u64,
}

impl MediaPlayer {
    /// Construct a player whose display fields remain bounded.
    #[must_use]
    pub fn bounded(
        id: MediaPlayerId,
        owner_generation: u64,
        identity: &str,
        status: MediaPlaybackStatus,
        metadata: MediaMetadata,
        capabilities: MediaCapabilities,
        activity_sequence: u64,
    ) -> Self {
        let capabilities = if capabilities.can_control {
            capabilities
        } else {
            MediaCapabilities::default()
        };
        Self {
            id,
            owner_generation,
            identity: DisplayText::new(identity, 128),
            status,
            metadata,
            capabilities,
            activity_sequence,
        }
    }
}

/// Root-owned MPRIS domain state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MediaState {
    /// Independent adapter availability.
    pub availability: AdapterAvailability,
    /// Current players keyed by well-known name.
    pub players: BTreeMap<MediaPlayerId, MediaPlayer>,
    /// Deterministically selected active player.
    pub active: Option<MediaPlayerId>,
    /// Owner generation at which each recently removed or evicted player left.
    ///
    /// A late `PlayerChanged` that began before an owner vanished carries the
    /// same or an older generation and must not resurrect the player, while a
    /// genuine reappearance always carries a strictly newer one. Bounded to
    /// `MAX_PLAYERS` entries so the fence cannot grow without limit.
    pub(crate) removed_generation: BTreeMap<MediaPlayerId, u64>,
}
