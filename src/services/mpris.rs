//! Bounded MPRIS session-bus adapter.
//!
//! The adapter subscribes to owner and property changes before its initial
//! snapshot. It owns D-Bus values and publishes only typed, sanitized data to
//! the root model.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::fdo::DBusProxy;
use zbus::message::Type as MessageType;
use zbus::names::BusName;
use zbus::zvariant::OwnedValue;
use zbus::{Connection, MatchRule, MessageStream, Proxy};

use crate::state::{
    MEDIA_RECENT_ACTIVITY_MILLIS, MediaCapabilities, MediaMetadata, MediaPlaybackStatus,
    MediaPlayer, MediaPlayerId,
};
use crate::supervisor::{Cancellation, ReconnectBackoff};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT_INTERFACE: &str = "org.mpris.MediaPlayer2";
const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";
/// Maximum simultaneously tracked MPRIS players.
///
/// The cap is enforced identically on the initial snapshot, the owner-change
/// path, and the root projection: the lexicographically lowest identities are
/// retained so admission and eviction are deterministic regardless of arrival
/// order. The root imports this constant rather than repeating a literal.
pub const MAX_PLAYERS: usize = 32;
const COMMAND_CAPACITY: usize = 16;
const CALL_TIMEOUT: Duration = Duration::from_secs(2);
const INITIAL_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
const PROGRESS_REFRESH: Duration = Duration::from_secs(1);
const REFRESH_CONCURRENCY: usize = 4;

/// Deterministic outcome of admitting one player identity under [`MAX_PLAYERS`].
///
/// The decision keeps the lexicographically lowest identities: an already
/// tracked or under-cap identity is admitted; at the cap a strictly-lower
/// newcomer evicts the current greatest; a larger newcomer is rejected. Both
/// the owner-change path and the root projection route admission through
/// [`media_admission`] so a bounded inventory can never grow past the cap or
/// drop deterministically-retained players.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaAdmission {
    /// Insert or update in place; the inventory is under the cap or the
    /// identity is already tracked.
    Admit,
    /// At the cap: evict this greatest identity, then admit the newcomer.
    Evict(MediaPlayerId),
    /// At the cap and not strictly lower than the greatest: reject the newcomer.
    Reject,
}

/// Maximum accepted encoded body size of an MPRIS property signal.
pub const MPRIS_SIGNAL_MAX_BYTES: usize = 64 * 1024;
/// Maximum accepted encoded body size of an MPRIS metadata reply.
pub const MPRIS_METADATA_MAX_BYTES: usize = 64 * 1024;
/// Maximum accepted encoded body size of a bus `ListNames` reply.
pub const MPRIS_LIST_NAMES_MAX_BYTES: usize = 256 * 1024;
/// Maximum accepted encoded body size of a scalar string property reply.
pub const MPRIS_STRING_PROP_MAX_BYTES: usize = 4 * 1024;

/// Whether a D-Bus message body is within an encoded-size budget.
///
/// The check runs on the raw encoded body length before any deserialization,
/// so an oversized or hostile reply is rejected before it can allocate a large
/// typed value. A caller pairs this with the decode step it guards; the decode
/// closure must not run when this returns `false`.
#[must_use]
pub fn dbus_body_within_cap(body_len: usize, cap: usize) -> bool {
    body_len <= cap
}

/// Classify one admission under [`MAX_PLAYERS`].
///
/// `under_cap_or_present` is `players.len() < MAX_PLAYERS || players.contains_key(&id)`
/// evaluated by the caller against its own inventory, and `greatest` is the
/// current largest tracked identity (the last key of an ordered map).
#[must_use]
pub fn media_admission(
    under_cap_or_present: bool,
    greatest: Option<&MediaPlayerId>,
    id: &MediaPlayerId,
) -> MediaAdmission {
    if under_cap_or_present {
        MediaAdmission::Admit
    } else if greatest.is_some_and(|greatest| greatest > id) {
        MediaAdmission::Evict(greatest.expect("greatest present").clone())
    } else {
        MediaAdmission::Reject
    }
}

/// Ordered adapter-to-root update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MediaUpdate {
    /// A new session-bus connection attempt began.
    Connecting,
    /// Complete player set captured after subscriptions were installed.
    Snapshot {
        /// Bounded player snapshots.
        players: Vec<MediaPlayer>,
        /// Adapter-relative observation time.
        observed_millis: u64,
    },
    /// One player appeared, restarted, or changed a relevant property.
    PlayerChanged {
        /// Complete replacement snapshot for the player.
        player: MediaPlayer,
        /// Adapter-relative observation time.
        observed_millis: u64,
    },
    /// One player disappeared from the bus.
    PlayerRemoved {
        /// Stable well-known bus identity.
        id: MediaPlayerId,
        /// Adapter-relative observation time.
        observed_millis: u64,
    },
    /// Advance recent-state expiry and position-derived projections.
    Tick {
        /// Adapter-relative observation time.
        observed_millis: u64,
    },
    /// The session bus is unavailable; retained player state is stale.
    Unavailable,
}

/// Capability-gated command sent from the root dispatcher.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaCommand {
    /// Target player selected by root-owned arbitration.
    pub player: MediaPlayerId,
    /// Unique-owner generation selected when the action was advertised.
    pub owner_generation: u64,
    /// Method request with no shell-string interpretation.
    pub kind: MediaCommandKind,
}

/// Supported MPRIS player methods.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaCommandKind {
    /// Toggle play and pause.
    PlayPause,
    /// Select the previous item.
    Previous,
    /// Select the next item.
    Next,
    /// Seek by a signed, bounded number of milliseconds.
    SeekMillis(i64),
}

/// Create the bounded command channel owned by the root and adapter.
#[must_use]
pub fn command_channel() -> (mpsc::Sender<MediaCommand>, mpsc::Receiver<MediaCommand>) {
    mpsc::channel(COMMAND_CAPACITY)
}

/// Run the independently supervised MPRIS adapter until cancellation.
pub async fn run(
    publish: impl Fn(MediaUpdate) + Send + Sync + 'static,
    mut commands: mpsc::Receiver<MediaCommand>,
    mut cancellation: Cancellation,
) {
    let started = Instant::now();
    let mut backoff = ReconnectBackoff::default();
    loop {
        publish(MediaUpdate::Connecting);
        let result = run_connection(
            &publish,
            &mut commands,
            &mut cancellation,
            &mut backoff,
            started,
        )
        .await;
        if cancellation.is_cancelled() || result.is_ok() {
            return;
        }
        if result.is_err() {
            tracing::warn!("MPRIS session-bus adapter unavailable");
        }
        publish(MediaUpdate::Unavailable);
        let delay = backoff.next_delay();
        tokio::select! {
            _ = cancellation.cancelled() => return,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

async fn run_connection(
    publish: &(impl Fn(MediaUpdate) + Send + Sync + 'static),
    commands: &mut mpsc::Receiver<MediaCommand>,
    cancellation: &mut Cancellation,
    backoff: &mut ReconnectBackoff,
    started: Instant,
) -> zbus::Result<()> {
    let connection = tokio::time::timeout(CALL_TIMEOUT, Connection::session())
        .await
        .map_err(|_| zbus::Error::Failure("MPRIS session-bus connection timed out".to_owned()))??;
    let bus = tokio::time::timeout(CALL_TIMEOUT, DBusProxy::new(&connection))
        .await
        .map_err(|_| zbus::Error::Failure("MPRIS bus proxy timed out".to_owned()))??;
    let mut owner_changes = tokio::time::timeout(CALL_TIMEOUT, bus.receive_name_owner_changed())
        .await
        .map_err(|_| zbus::Error::Failure("MPRIS owner subscription timed out".to_owned()))??;
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(PROPERTIES_INTERFACE)?
        .member("PropertiesChanged")?
        .path(MPRIS_PATH)?
        .build();
    let mut property_changes = tokio::time::timeout(
        CALL_TIMEOUT,
        MessageStream::for_match_rule(rule, &connection, Some(64)),
    )
    .await
    .map_err(|_| zbus::Error::Failure("MPRIS property subscription timed out".to_owned()))??;

    let mut owners = BTreeMap::new();
    let mut destinations = BTreeMap::new();
    let mut next_generation = 1_u64;
    let mut players = tokio::time::timeout(
        INITIAL_SNAPSHOT_TIMEOUT,
        initial_snapshot(
            &connection,
            &bus,
            &mut owners,
            &mut destinations,
            &mut next_generation,
        ),
    )
    .await
    .map_err(|_| zbus::Error::Failure("MPRIS initial snapshot timed out".to_owned()))??;
    publish(MediaUpdate::Snapshot {
        players: players.values().cloned().collect(),
        observed_millis: elapsed_millis(started),
    });
    backoff.reset();
    let mut dirty_players = BTreeSet::new();
    let mut recent_stopped_active = players.values().any(|player| {
        player.status == MediaPlaybackStatus::Stopped
            && player.activity_sequence > 0
            && elapsed_millis(started).saturating_sub(player.activity_sequence)
                < MEDIA_RECENT_ACTIVITY_MILLIS
    });
    let mut progress_refresh = tokio::time::interval(PROGRESS_REFRESH);
    progress_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = cancellation.cancelled() => return Ok(()),
            command = commands.recv() => {
                let Some(command) = command else {
                    return Ok(());
                };
                if dispatch_command(&connection, &players, &destinations, command).await.is_err() {
                    tracing::warn!("MPRIS player command failed");
                }
            }
            _ = progress_refresh.tick() => {
                let observed_millis = elapsed_millis(started);
                let has_recent_stopped = players.values().any(|player| {
                    player.status == MediaPlaybackStatus::Stopped
                        && player.activity_sequence > 0
                        && observed_millis.saturating_sub(player.activity_sequence)
                            < MEDIA_RECENT_ACTIVITY_MILLIS
                });
                if has_recent_stopped || recent_stopped_active {
                    publish(MediaUpdate::Tick { observed_millis });
                }
                recent_stopped_active = has_recent_stopped;
                dirty_players.extend(
                    players
                        .values()
                        .filter(|player| player.status == MediaPlaybackStatus::Playing)
                        .map(|player| player.id.clone()),
                );
                let ids = std::mem::take(&mut dirty_players)
                    .into_iter()
                    .take(MAX_PLAYERS)
                    .collect::<Vec<_>>();
                let refreshed = futures_util::stream::iter(ids)
                    .filter_map(|id| {
                        let destination = destinations.get(&id).cloned();
                        async move { destination.map(|destination| (id, destination)) }
                    })
                    .map(|(id, destination)| {
                        let connection = &connection;
                        async move {
                        let player = read_player(
                            connection,
                            &destination.unique_owner,
                            id.clone(),
                            observed_millis,
                            destination.generation,
                        )
                        .await;
                        (id, player)
                        }
                    })
                    .buffer_unordered(REFRESH_CONCURRENCY)
                    .collect::<Vec<_>>()
                    .await;
                let mut changed = false;
                for (id, player) in refreshed {
                    if let Some(mut player) = player {
                        if let Some(previous) = players.get(&id)
                            && previous.status == player.status
                            && previous.owner_generation == player.owner_generation
                        {
                            player.activity_sequence = previous.activity_sequence;
                        }
                        changed |= players.insert(id, player.clone()).as_ref() != Some(&player);
                    }
                }
                if changed {
                    publish(MediaUpdate::Snapshot {
                        players: players.values().cloned().collect(),
                        observed_millis,
                    });
                }
            }
            signal = owner_changes.next() => {
                let Some(signal) = signal else {
                    return Err(zbus::Error::Failure("MPRIS owner-change stream ended".to_owned()));
                };
                let args = signal.args()?;
                let name = args.name().as_str();
                if !name.starts_with(MPRIS_PREFIX) {
                    continue;
                }
                let Some(id) = MediaPlayerId::new(name) else {
                    continue;
                };
                dirty_players.remove(&id);
                owners.retain(|_, candidate| candidate != &id);
                destinations.remove(&id);
                if let Some(owner) = args.new_owner().as_ref() {
                    // Deterministic MAX_PLAYERS admission on the owner-change
                    // path: an already-tracked or under-cap identity is
                    // admitted; at the cap a strictly-lower newcomer evicts the
                    // greatest and a larger one is rejected, so owner and
                    // destination bookkeeping cannot grow past the cap.
                    let admitted = players.len() < MAX_PLAYERS || players.contains_key(&id);
                    let decision = media_admission(admitted, players.keys().next_back(), &id);
                    if matches!(decision, MediaAdmission::Reject) {
                        // A larger newcomer at the cap: the id's tentative
                        // bookkeeping was already cleared above, so tracking
                        // nothing keeps the inventory bounded and deterministic.
                    } else {
                        let generation = next_owner_generation(&mut next_generation);
                        players.remove(&id);
                        if let Some(player) = read_player(
                            &connection,
                            owner.as_str(),
                            id.clone(),
                            elapsed_millis(started),
                            generation,
                        ).await {
                            // Only a successful read consumes a slot, so evict
                            // the greatest just before inserting the newcomer.
                            if let MediaAdmission::Evict(evicted) = decision {
                                players.remove(&evicted);
                                destinations.remove(&evicted);
                                owners.retain(|_, candidate| candidate != &evicted);
                                dirty_players.remove(&evicted);
                                publish(MediaUpdate::PlayerRemoved {
                                    id: evicted,
                                    observed_millis: elapsed_millis(started),
                                });
                            }
                            owners.insert(owner.as_str().to_owned(), id.clone());
                            destinations.insert(id.clone(), OwnerDestination {
                                unique_owner: owner.as_str().to_owned(),
                                generation,
                            });
                            players.insert(id.clone(), player.clone());
                            publish(MediaUpdate::PlayerChanged {
                                player,
                                observed_millis: elapsed_millis(started),
                            });
                        } else {
                            // A failed read admits nothing; the id's bookkeeping
                            // was already cleared above.
                            publish(MediaUpdate::PlayerRemoved {
                                id,
                                observed_millis: elapsed_millis(started),
                            });
                        }
                    }
                } else {
                    players.remove(&id);
                    publish(MediaUpdate::PlayerRemoved {
                        id,
                        observed_millis: elapsed_millis(started),
                    });
                }
            }
            message = property_changes.next() => {
                let Some(message) = message else {
                    return Err(zbus::Error::Failure("MPRIS property stream ended".to_owned()));
                };
                let message = message?;
                let header = message.header();
                let Some(sender) = header.sender() else {
                    continue;
                };
                let Some(id) = owners.get(sender.as_str()).cloned() else {
                    continue;
                };
                // Reject an oversized signal body before it deserializes and
                // allocates its property map, but still mark the player dirty so
                // a bounded refresh re-reads its real state.
                if !dbus_body_within_cap(message.body().len(), MPRIS_SIGNAL_MAX_BYTES) {
                    dirty_players.insert(id);
                    continue;
                }
                let Ok((interface, _, _)) = message.body().deserialize::<(
                    String,
                    HashMap<String, OwnedValue>,
                    Vec<String>,
                )>() else {
                    continue;
                };
                if interface != PLAYER_INTERFACE {
                    continue;
                }
                dirty_players.insert(id);
            }
        }
    }
}

async fn initial_snapshot(
    connection: &Connection,
    bus: &DBusProxy<'_>,
    owners: &mut BTreeMap<String, MediaPlayerId>,
    destinations: &mut BTreeMap<MediaPlayerId, OwnerDestination>,
    next_generation: &mut u64,
) -> zbus::Result<BTreeMap<MediaPlayerId, MediaPlayer>> {
    // Read the bus name list through a raw call so the encoded reply is
    // rejected by size before it deserializes into an owned name vector.
    let reply = bus.inner().call_method("ListNames", &()).await?;
    if !dbus_body_within_cap(reply.body().len(), MPRIS_LIST_NAMES_MAX_BYTES) {
        return Err(zbus::Error::Failure(
            "MPRIS bus name list exceeded its byte bound".to_owned(),
        ));
    }
    let mut names = reply
        .body()
        .deserialize::<Vec<String>>()?
        .into_iter()
        .filter_map(|name| MediaPlayerId::new(name.as_str()))
        .collect::<Vec<_>>();
    names.sort();
    // Read identities in lexical order and admit only successful reads, up to
    // the cap. Truncating names first would let a leading unreadable name
    // displace a readable lower-priority one; instead the lowest MAX_PLAYERS
    // identities that actually read are retained.
    let mut players = BTreeMap::new();
    for id in names {
        if players.len() >= MAX_PLAYERS {
            break;
        }
        let Ok(name) = BusName::try_from(id.as_str()) else {
            continue;
        };
        let Ok(owner) = bus.get_name_owner(name).await else {
            continue;
        };
        let generation = next_owner_generation(next_generation);
        let Some(player) = read_player(connection, owner.as_str(), id.clone(), 0, generation).await
        else {
            continue;
        };
        owners.insert(owner.as_str().to_owned(), id.clone());
        destinations.insert(
            id.clone(),
            OwnerDestination {
                unique_owner: owner.as_str().to_owned(),
                generation,
            },
        );
        players.insert(id, player);
    }
    Ok(players)
}

async fn read_player(
    connection: &Connection,
    destination: &str,
    id: MediaPlayerId,
    activity_sequence: u64,
    owner_generation: u64,
) -> Option<MediaPlayer> {
    match tokio::time::timeout(CALL_TIMEOUT, read_player_inner(connection, destination, id)).await {
        Ok(Ok(mut player)) => {
            player.activity_sequence = activity_sequence;
            player.owner_generation = owner_generation;
            Some(player)
        }
        Ok(Err(_)) => {
            tracing::debug!("ignored unusable MPRIS player snapshot");
            None
        }
        Err(_) => {
            tracing::debug!("ignored timed-out MPRIS player snapshot");
            None
        }
    }
}

async fn read_player_inner(
    connection: &Connection,
    destination: &str,
    id: MediaPlayerId,
) -> zbus::Result<MediaPlayer> {
    // Every property is read through a raw Properties.Get whose encoded reply
    // is rejected by size before it deserializes, so a hostile player can
    // neither allocate without bound nor inject partial state. A missing or
    // oversized reply falls back to a safe default for that field.
    let properties = Proxy::new(connection, destination, MPRIS_PATH, PROPERTIES_INTERFACE).await?;
    let identity = get_property_bounded::<String>(
        &properties,
        ROOT_INTERFACE,
        "Identity",
        MPRIS_STRING_PROP_MAX_BYTES,
    )
    .await
    .unwrap_or_else(|| "Media player".to_owned());
    let status = get_property_bounded::<String>(
        &properties,
        PLAYER_INTERFACE,
        "PlaybackStatus",
        MPRIS_STRING_PROP_MAX_BYTES,
    )
    .await
    .unwrap_or_else(|| "Unknown".to_owned());
    let metadata = get_property_bounded::<HashMap<String, OwnedValue>>(
        &properties,
        PLAYER_INTERFACE,
        "Metadata",
        MPRIS_METADATA_MAX_BYTES,
    )
    .await
    .unwrap_or_default();
    let position = get_property_bounded::<i64>(
        &properties,
        PLAYER_INTERFACE,
        "Position",
        MPRIS_STRING_PROP_MAX_BYTES,
    )
    .await
    .unwrap_or(0);
    let capability = |name: &'static str| {
        get_property_bounded::<bool>(
            &properties,
            PLAYER_INTERFACE,
            name,
            MPRIS_STRING_PROP_MAX_BYTES,
        )
    };
    let capabilities = MediaCapabilities {
        can_control: capability("CanControl").await.unwrap_or(false),
        can_play: capability("CanPlay").await.unwrap_or(false),
        can_pause: capability("CanPause").await.unwrap_or(false),
        can_previous: capability("CanGoPrevious").await.unwrap_or(false),
        can_next: capability("CanGoNext").await.unwrap_or(false),
        can_seek: capability("CanSeek").await.unwrap_or(false),
    };
    Ok(MediaPlayer::bounded(
        id,
        0,
        &identity,
        MediaPlaybackStatus::parse(&status),
        metadata_from_values(&metadata, position),
        capabilities,
        0,
    ))
}

/// Read one property through a size-bounded raw `Properties.Get`.
///
/// The encoded reply body is rejected against `cap` before it deserializes,
/// then the `v` payload is decoded to the inner `OwnedValue` (matching
/// `Proxy::get_property`) and converted to the requested type. Any failure,
/// oversize, or decode error yields `None` so the caller applies a safe
/// default rather than trusting an unbounded or partial value.
async fn get_property_bounded<T>(
    properties: &Proxy<'_>,
    interface: &str,
    property: &str,
    cap: usize,
) -> Option<T>
where
    T: TryFrom<OwnedValue>,
{
    let reply = properties
        .call_method("Get", &(interface, property))
        .await
        .ok()?;
    if !dbus_body_within_cap(reply.body().len(), cap) {
        return None;
    }
    let owned = reply.body().deserialize::<OwnedValue>().ok()?;
    T::try_from(owned).ok()
}

fn metadata_from_values(values: &HashMap<String, OwnedValue>, position: i64) -> MediaMetadata {
    let title = values
        .get("xesam:title")
        .and_then(|value| value.downcast_ref::<&str>().ok())
        .unwrap_or_default();
    let artists = values
        .get("xesam:artist")
        .and_then(|value| value.try_clone().ok())
        .map(zbus::zvariant::Value::from)
        .and_then(|value| value.downcast::<Vec<String>>().ok())
        .unwrap_or_default();
    let art_url = values
        .get("mpris:artUrl")
        .and_then(|value| value.downcast_ref::<&str>().ok());
    let duration = values
        .get("mpris:length")
        .and_then(|value| value.downcast_ref::<i64>().ok());
    MediaMetadata::bounded(title, &artists, art_url, duration, position)
}

async fn dispatch_command(
    connection: &Connection,
    players: &BTreeMap<MediaPlayerId, MediaPlayer>,
    destinations: &BTreeMap<MediaPlayerId, OwnerDestination>,
    command: MediaCommand,
) -> zbus::Result<()> {
    let Some(destination) = command_destination(players, destinations, &command) else {
        return Ok(());
    };
    tokio::time::timeout(CALL_TIMEOUT, async {
        let proxy = Proxy::new(connection, destination, MPRIS_PATH, PLAYER_INTERFACE).await?;
        match command.kind {
            MediaCommandKind::PlayPause => proxy.call::<_, _, ()>("PlayPause", &()).await,
            MediaCommandKind::Previous => proxy.call::<_, _, ()>("Previous", &()).await,
            MediaCommandKind::Next => proxy.call::<_, _, ()>("Next", &()).await,
            MediaCommandKind::SeekMillis(delta) => {
                let microseconds = delta.clamp(-86_400_000, 86_400_000).saturating_mul(1_000);
                proxy.call::<_, _, ()>("Seek", &(microseconds,)).await
            }
        }
    })
    .await
    .map_err(|_| zbus::Error::Failure("MPRIS command timed out".to_owned()))?
}

fn command_destination<'a>(
    players: &BTreeMap<MediaPlayerId, MediaPlayer>,
    destinations: &'a BTreeMap<MediaPlayerId, OwnerDestination>,
    command: &MediaCommand,
) -> Option<&'a str> {
    let player = players.get(&command.player)?;
    if !player.capabilities.allows(player.status, command.kind) {
        return None;
    }
    let destination = destinations.get(&command.player)?;
    (player.owner_generation == command.owner_generation
        && destination.generation == command.owner_generation)
        .then_some(destination.unique_owner.as_str())
}

fn elapsed_millis(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

impl MediaCapabilities {
    fn allows(&self, status: MediaPlaybackStatus, command: MediaCommandKind) -> bool {
        if !self.can_control {
            return false;
        }
        match command {
            MediaCommandKind::PlayPause => match status {
                MediaPlaybackStatus::Playing => self.can_pause,
                MediaPlaybackStatus::Paused | MediaPlaybackStatus::Stopped => self.can_play,
                MediaPlaybackStatus::Unknown => false,
            },
            MediaCommandKind::Previous => self.can_previous,
            MediaCommandKind::Next => self.can_next,
            MediaCommandKind::SeekMillis(_) => self.can_seek,
        }
    }
}

#[derive(Clone)]
struct OwnerDestination {
    unique_owner: String,
    generation: u64,
}

fn next_owner_generation(next: &mut u64) -> u64 {
    let generation = *next;
    *next = next.saturating_add(1);
    generation
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    #[test]
    fn dbus_body_cap_rejects_only_over_the_budget() {
        // The guard runs on the raw encoded length before any deserialization.
        assert!(dbus_body_within_cap(0, MPRIS_SIGNAL_MAX_BYTES));
        assert!(dbus_body_within_cap(
            MPRIS_SIGNAL_MAX_BYTES,
            MPRIS_SIGNAL_MAX_BYTES
        ));
        assert!(!dbus_body_within_cap(
            MPRIS_SIGNAL_MAX_BYTES + 1,
            MPRIS_SIGNAL_MAX_BYTES
        ));

        // A decode closure paired with the guard must not run over the cap.
        let mut decoded = false;
        let body_len = MPRIS_METADATA_MAX_BYTES + 1;
        if dbus_body_within_cap(body_len, MPRIS_METADATA_MAX_BYTES) {
            decoded = true;
        }
        assert!(!decoded, "decode must be skipped for an over-cap body");
    }

    #[test]
    fn metadata_values_are_bounded_and_invalid_artwork_is_dropped() {
        let mut values = HashMap::new();
        values.insert(
            "xesam:title".to_owned(),
            OwnedValue::try_from(Value::new(Value::from("title\nwith controls"))).expect("value"),
        );
        values.insert(
            "xesam:artist".to_owned(),
            OwnedValue::try_from(Value::from(vec!["artist"; 80])).expect("value"),
        );
        values.insert(
            "mpris:artUrl".to_owned(),
            OwnedValue::try_from(Value::from("javascript:unsafe")).expect("value"),
        );
        values.insert(
            "mpris:length".to_owned(),
            OwnedValue::try_from(Value::from(i64::MAX)).expect("value"),
        );
        let metadata = metadata_from_values(&values, i64::MAX);
        assert_eq!(metadata.title.as_str(), "title with controls");
        assert!(metadata.artist.as_str().chars().count() <= 256);
        assert!(metadata.art_url.is_none());
        assert_eq!(metadata.position_micros, metadata.duration_micros);
    }

    #[test]
    fn invalid_bus_names_never_become_player_identities() {
        assert!(MediaPlayerId::new("org.mpris.MediaPlayer2.synthetic").is_some());
        assert!(MediaPlayerId::new("org.example.synthetic").is_none());
        assert!(MediaPlayerId::new(&format!("{MPRIS_PREFIX}{}", "x".repeat(256))).is_none());
    }

    #[test]
    fn stale_owner_generations_cannot_target_a_restarted_player() {
        let id = MediaPlayerId::new("org.mpris.MediaPlayer2.synthetic").expect("player id");
        let capabilities = MediaCapabilities {
            can_control: true,
            can_play: true,
            can_pause: true,
            ..MediaCapabilities::default()
        };
        let player = MediaPlayer::bounded(
            id.clone(),
            7,
            "Synthetic player",
            MediaPlaybackStatus::Playing,
            MediaMetadata::default(),
            capabilities,
            1,
        );
        let players = BTreeMap::from([(id.clone(), player)]);
        let destinations = BTreeMap::from([(
            id.clone(),
            OwnerDestination {
                unique_owner: ":1.42".to_owned(),
                generation: 7,
            },
        )]);

        let current = MediaCommand {
            player: id.clone(),
            owner_generation: 7,
            kind: MediaCommandKind::PlayPause,
        };
        assert_eq!(
            command_destination(&players, &destinations, &current),
            Some(":1.42")
        );
        let stale = MediaCommand {
            player: id,
            owner_generation: 6,
            kind: MediaCommandKind::PlayPause,
        };
        assert!(command_destination(&players, &destinations, &stale).is_none());
    }
}
