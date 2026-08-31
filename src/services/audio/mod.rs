//! Direct PipeWire audio adapter and its transport-independent typed model.
//!
//! The pure model at the top of this file (state, nodes, volumes, commands,
//! capability gating, and the SPA `Props` pod codec) is fully deterministic and
//! unit tested. The adapter at the bottom drives a dedicated PipeWire loop on
//! its own OS thread, because `libpipewire` objects are neither `Send` nor
//! `Sync`. A supervisor-owned Tokio task bridges typed commands into that loop
//! and forwards typed updates back out, so no `wpctl` subprocess is polled.
//!
//! WirePlumber remains the session policy owner. The adapter reads its standard
//! PipeWire `default` metadata for current sink and source selection and
//! cooperates with WirePlumber for route mutation by writing its `default`
//! metadata rather than mutating links: default-node selection writes
//! `default.configured.audio.{sink,source}`, and active-stream movement writes
//! the stream's `target.object` key. Every such write only requests a change;
//! WirePlumber owns the resulting relink and republishes the outcome. This
//! crate adds no `wireplumber` dependency and contains no `unsafe` project code.
//!
//! The transport adapter and pod codec compile only with the `audio-transport`
//! feature, which enables the optional `pipewire` dependency. Native runtime
//! behavior against a live PipeWire server is not verified in a headless build
//! environment; the pod codec is verified by a round-trip test, and the pure
//! contracts here are exercised directly.

use std::collections::BTreeMap;

use crate::state::{AdapterAvailability, DisplayText};

/// Maximum audio nodes retained from an untrusted server.
pub const MAX_AUDIO_NODES: usize = 128;
/// Maximum visible characters retained from a device name.
pub const MAX_AUDIO_NAME_CHARACTERS: usize = 128;
/// Fixed-point scale for a linear volume value (1.0 linear == 1000 units).
pub const VOLUME_SCALE: u32 = 1_000;
/// Maximum linear volume accepted from any producer or command (400 percent).
pub const MAX_VOLUME_LINEAR_MILLIS: u32 = 4_000;
/// Bounded command channel depth shared by the root and adapter.
pub const COMMAND_CAPACITY: usize = 16;

/// Direction of an audio node relative to the local machine.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AudioDirection {
    /// A playback sink (speakers, headphones).
    Sink,
    /// A capture source (microphone, line-in).
    Source,
}

/// Stable process-local PipeWire global identity for an audio node.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AudioNodeId(u32);

impl AudioNodeId {
    /// Wrap a PipeWire global identity.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// The underlying PipeWire global identity.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Bounded linear volume stored as fixed-point to keep the state hashable.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct AudioVolume {
    linear_millis: u32,
}

impl AudioVolume {
    /// Construct a clamped volume from a fixed-point linear value.
    #[must_use]
    pub const fn from_linear_millis(value: u32) -> Self {
        Self {
            linear_millis: if value > MAX_VOLUME_LINEAR_MILLIS {
                MAX_VOLUME_LINEAR_MILLIS
            } else {
                value
            },
        }
    }

    /// Construct a clamped volume from a linear float such as a PipeWire value.
    #[must_use]
    pub fn from_linear(value: f32) -> Self {
        if !value.is_finite() || value <= 0.0 {
            return Self { linear_millis: 0 };
        }
        let scaled = (value * VOLUME_SCALE as f32).round();
        let bounded = scaled.clamp(0.0, MAX_VOLUME_LINEAR_MILLIS as f32);
        Self {
            linear_millis: bounded as u32,
        }
    }

    /// The fixed-point linear value.
    #[must_use]
    pub const fn linear_millis(self) -> u32 {
        self.linear_millis
    }

    /// The linear value as a float for the transport boundary.
    #[must_use]
    pub fn linear(self) -> f32 {
        self.linear_millis as f32 / VOLUME_SCALE as f32
    }

    /// Conventional cubic display percentage, bounded to whole percent.
    ///
    /// PipeWire clients conventionally present the cube root of the linear
    /// amplitude. The exact mapping against a live server is unverified here and
    /// remains a native proof task, so this value is display-only.
    #[must_use]
    pub fn cubic_percent(self) -> u16 {
        let cubic = self.linear().cbrt();
        let percent = (cubic * 100.0).round();
        percent.clamp(0.0, f32::from(u16::MAX)) as u16
    }
}

/// Capabilities offered by a single audio node.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AudioCapabilities {
    /// The node exposes a controllable volume.
    pub can_set_volume: bool,
    /// The node exposes a controllable mute.
    pub can_set_mute: bool,
}

/// A bounded, sanitized audio node projected into root state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioNode {
    /// Stable PipeWire global identity.
    pub id: AudioNodeId,
    /// Playback or capture direction.
    pub direction: AudioDirection,
    /// Bounded machine-readable node name used to express defaults.
    pub name: DisplayText,
    /// Bounded human-facing description.
    pub description: DisplayText,
    /// Current linear volume.
    pub volume: AudioVolume,
    /// Whether the node is muted.
    pub muted: bool,
    /// Whether the node is currently routable and available.
    pub available: bool,
    /// Controllable capabilities advertised for this node.
    pub capabilities: AudioCapabilities,
}

impl AudioNode {
    /// Construct a bounded node, clamping untrusted names and volume.
    #[must_use]
    pub fn bounded(
        id: AudioNodeId,
        direction: AudioDirection,
        name: &str,
        description: &str,
        volume: AudioVolume,
        muted: bool,
        available: bool,
    ) -> Self {
        Self {
            id,
            direction,
            name: DisplayText::new(name, MAX_AUDIO_NAME_CHARACTERS),
            description: DisplayText::new(description, MAX_AUDIO_NAME_CHARACTERS),
            volume,
            muted,
            available,
            capabilities: AudioCapabilities {
                can_set_volume: available,
                can_set_mute: available,
            },
        }
    }
}

/// Per-connection generation token.
///
/// Every PipeWire connection attempt is stamped with a fresh, monotonically
/// increasing generation. Because opaque registry global IDs are reused across
/// a PipeWire reconnect or service restart, the generation distinguishes a
/// stream or sink observed on the current connection from an identically
/// numbered object observed on a previous one. It rides inside the published
/// movable-stream selection and back through the validated move command so a move request
/// queued against a stale connection is rejected instead of retargeting a
/// coincidentally matching new object.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Generation(u64);

impl Generation {
    /// Wrap a connection generation counter value.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The underlying generation counter value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Whether a move command's generation still matches the live connection.
///
/// A queued move built against connection `command` must be rejected once the
/// adapter has reconnected to a newer `current` generation, because the stream
/// and sink IDs it names may now identify different objects.
#[must_use]
pub const fn move_is_fresh(command: Generation, current: Generation) -> bool {
    command.get() == current.get()
}

/// Maximum movable playback streams retained from an untrusted server.
pub const MAX_MOVABLE_STREAMS: usize = 64;

/// The exact `media.class` of a movable audio playback stream.
pub const MOVABLE_STREAM_MEDIA_CLASS: &str = "Stream/Output/Audio";

/// Whether a `media.class` names a movable audio playback stream.
///
/// The comparison is exact after trimming surrounding whitespace: a decorated
/// or near-match class such as `Stream/Output/Audio/Virtual`,
/// `Stream/Output/Audiofoo`, or `Stream/Output/Video` is rejected, so an
/// unrelated node can never be tracked as a movable stream.
#[must_use]
pub fn is_movable_stream_class(class: &str) -> bool {
    class.trim() == MOVABLE_STREAM_MEDIA_CLASS
}

/// Selection status of the single movable playback stream.
///
/// Stream movement targets exactly one running, movable playback stream. Zero
/// running movable streams is [`Unavailable`](MovableStreamState::Unavailable);
/// more than one is [`Ambiguous`](MovableStreamState::Ambiguous) because there
/// is no unique subject; a bounded-inventory overflow, an explicit session
/// policy denial, or a metadata object without write and execute permission is
/// [`Disabled`](MovableStreamState::Disabled). Only
/// [`Active`](MovableStreamState::Active) offers the move action, and even then
/// a successful write acknowledges only that a request was sent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MovableStreamState {
    /// No running movable playback stream exists; the action is unavailable.
    #[default]
    Unavailable,
    /// Exactly one running movable playback stream; it is the move subject.
    ///
    /// Carries the connection generation the selection was observed on so a
    /// move built against it can be rejected after a reconnect.
    Active {
        /// Opaque global identity of the uniquely movable stream.
        stream: AudioNodeId,
        /// Connection generation this selection was observed on.
        generation: Generation,
    },
    /// More than one running movable stream; no unique subject exists.
    Ambiguous,
    /// Movement is disabled by overflow, session policy, or permission.
    Disabled,
}

impl MovableStreamState {
    /// The uniquely movable stream identity, when exactly one exists.
    #[must_use]
    pub const fn active(self) -> Option<AudioNodeId> {
        match self {
            Self::Active { stream, .. } => Some(stream),
            _ => None,
        }
    }

    /// The uniquely movable stream identity with its connection generation.
    #[must_use]
    pub const fn active_generation(self) -> Option<(AudioNodeId, Generation)> {
        match self {
            Self::Active { stream, generation } => Some((stream, generation)),
            _ => None,
        }
    }

    /// Whether a move action can currently be offered.
    #[must_use]
    pub const fn can_move(self) -> bool {
        matches!(self, Self::Active { .. })
    }
}

/// A bounded, sanitized movable-stream candidate for pure selection.
///
/// Only numeric identity and three booleans are retained; no application name,
/// process identity, or media metadata crosses this boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MovableStreamCandidate {
    /// Opaque PipeWire global identity of the stream node.
    pub id: AudioNodeId,
    /// Whether the stream node is in the running state.
    pub running: bool,
    /// Whether the stream permits movement (not `node.dont-move`).
    pub movable: bool,
    /// Whether the stream object grants the metadata permission a
    /// `target.object` write on this subject requires (`PW_PERM_M`).
    pub has_metadata_permission: bool,
}

/// Session-policy and permission preconditions for offering stream movement.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MoveEnablement {
    /// The session policy does not explicitly deny moving streams.
    pub policy_allows: bool,
    /// The bound `default` metadata object permits writing a foreign subject.
    pub metadata_writable: bool,
    /// The bounded stream inventory overflowed and cannot be trusted.
    pub overflowed: bool,
}

/// Tri-state resolution of the `sm-settings` `linking.allow-moving-streams`.
///
/// The setting is frequently absent because the WirePlumber default is not
/// republished. [`Unknown`](MoveMovingPolicy::Unknown) is therefore treated as
/// permitting an attempt rather than a claim of capability: the move action only
/// ever sends a request and never asserts the graph moved. Only an explicit
/// [`Denied`](MoveMovingPolicy::Denied) disables the action.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MoveMovingPolicy {
    /// The setting was not observed.
    #[default]
    Unknown,
    /// Moving streams is explicitly allowed.
    Allowed,
    /// Moving streams is explicitly denied.
    Denied,
}

impl MoveMovingPolicy {
    /// Whether this policy permits attempting a move (anything but denial).
    #[must_use]
    pub const fn permits_attempt(self) -> bool {
        !matches!(self, Self::Denied)
    }
}

/// Resolve the `linking.allow-moving-streams` setting value into tri-state.
///
/// Accepts the canonical boolean spellings WirePlumber writes; any other or
/// absent value is [`Unknown`](MoveMovingPolicy::Unknown).
#[must_use]
pub fn parse_allow_moving_streams(value: Option<&str>) -> MoveMovingPolicy {
    match value.map(str::trim) {
        Some("true" | "1") => MoveMovingPolicy::Allowed,
        Some("false" | "0") => MoveMovingPolicy::Denied,
        _ => MoveMovingPolicy::Unknown,
    }
}

/// Select the movable-stream state from bounded candidates and enablement.
///
/// Overflow, policy denial, or a non-writable metadata object disables the
/// action outright. Otherwise the running, movable candidates that also grant
/// the subject metadata permission decide: exactly one is
/// [`Active`](MovableStreamState::Active) (stamped with `generation`), several
/// are [`Ambiguous`](MovableStreamState::Ambiguous), and none is
/// [`Unavailable`](MovableStreamState::Unavailable). A running movable stream
/// that lacks the subject permission cannot be moved, so it is not counted.
#[must_use]
pub fn select_movable_stream(
    candidates: &[MovableStreamCandidate],
    enablement: MoveEnablement,
    generation: Generation,
) -> MovableStreamState {
    if enablement.overflowed || !enablement.policy_allows || !enablement.metadata_writable {
        return MovableStreamState::Disabled;
    }
    let mut eligible = candidates
        .iter()
        .filter(|c| c.running && c.movable && c.has_metadata_permission);
    match (eligible.next(), eligible.next()) {
        (Some(one), None) => MovableStreamState::Active {
            stream: one.id,
            generation,
        },
        (Some(_), Some(_)) => MovableStreamState::Ambiguous,
        (None, _) => MovableStreamState::Unavailable,
    }
}

/// PipeWire permission bit granting property writes on an object.
pub const PW_PERM_W: u32 = 0o200;
/// PipeWire permission bit granting method invocation on an object.
pub const PW_PERM_X: u32 = 0o100;
/// PipeWire permission bit granting metadata assignment on an object.
pub const PW_PERM_M: u32 = 0o010;

/// Whether a stream subject grants the metadata permission a `target.object`
/// write requires.
///
/// Writing `target.object` names the stream node as the metadata *subject*.
/// The server only accepts that assignment when the client holds the metadata
/// (`PW_PERM_M`) permission on the subject object, so a stream without it is
/// not offered as a move subject rather than presented as if it could move.
#[must_use]
pub const fn subject_permits_metadata(permissions: u32) -> bool {
    permissions & PW_PERM_M != 0
}

/// Whether the bound `default` metadata permissions allow a foreign-subject
/// write.
///
/// Writing `target.object` for another node's subject invokes `set_property`
/// on the metadata object, which requires both write and execute permission.
/// Without both, the move request would be silently rejected by the server, so
/// the action is disabled rather than presented as if it could succeed.
#[must_use]
pub const fn metadata_permits_target_write(permissions: u32) -> bool {
    permissions & PW_PERM_W != 0 && permissions & PW_PERM_X != 0
}

/// The `default` metadata key WirePlumber watches to move a stream's target.
pub const TARGET_OBJECT_METADATA_KEY: &str = "target.object";
/// SPA metadata value type for [`TARGET_OBJECT_METADATA_KEY`]: an object id.
pub const TARGET_OBJECT_METADATA_TYPE: &str = "Spa:Id";

/// Build the `target.object` metadata value: the sink's decimal object serial.
///
/// WirePlumber matches the numeric `object.serial` of the destination sink, not
/// its registry id, so the stream follows the intended device across
/// re-enumeration. The value is a plain decimal string with no interpolation.
#[must_use]
pub fn target_object_metadata_value(object_serial: u64) -> String {
    object_serial.to_string()
}

/// Root-owned PipeWire audio domain state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AudioState {
    /// Independent adapter availability.
    pub availability: AdapterAvailability,
    nodes: BTreeMap<AudioNodeId, AudioNode>,
    default_sink: Option<AudioNodeId>,
    default_source: Option<AudioNodeId>,
    movable_stream: MovableStreamState,
}

impl AudioState {
    /// Replace the node set from a complete bounded snapshot.
    pub fn apply_snapshot(
        &mut self,
        nodes: Vec<AudioNode>,
        default_sink: Option<AudioNodeId>,
        default_source: Option<AudioNodeId>,
    ) {
        self.nodes = nodes
            .into_iter()
            .take(MAX_AUDIO_NODES)
            .map(|node| (node.id, node))
            .collect();
        self.availability = AdapterAvailability::Ready;
        self.default_sink = default_sink.filter(|id| self.nodes.contains_key(id));
        self.default_source = default_source.filter(|id| self.nodes.contains_key(id));
    }

    /// Insert or replace one node, honoring the retained-node bound.
    pub fn upsert_node(&mut self, node: AudioNode) {
        if self.nodes.len() < MAX_AUDIO_NODES || self.nodes.contains_key(&node.id) {
            self.nodes.insert(node.id, node);
            self.availability = AdapterAvailability::Ready;
        }
    }

    /// Remove one node and clear any default that referenced it.
    pub fn remove_node(&mut self, id: AudioNodeId) {
        self.nodes.remove(&id);
        if self.default_sink == Some(id) {
            self.default_sink = None;
        }
        if self.default_source == Some(id) {
            self.default_source = None;
        }
    }

    /// Set the default node for a direction, ignoring unknown identities.
    pub fn set_default(&mut self, direction: AudioDirection, id: Option<AudioNodeId>) {
        let resolved = id.filter(|id| {
            self.nodes
                .get(id)
                .is_some_and(|node| node.direction == direction)
        });
        match direction {
            AudioDirection::Sink => self.default_sink = resolved,
            AudioDirection::Source => self.default_source = resolved,
        }
    }

    /// Replace the movable-stream selection projected from the adapter.
    pub fn set_movable_stream(&mut self, state: MovableStreamState) {
        self.movable_stream = state;
    }

    /// The current movable-stream selection state.
    #[must_use]
    pub fn movable_stream(&self) -> MovableStreamState {
        self.movable_stream
    }

    /// Mark retained state stale while a fresh connection is acquired.
    pub fn mark_stale(&mut self) {
        if !self.nodes.is_empty() {
            self.availability = AdapterAvailability::Stale;
        }
    }

    /// Mark the adapter unavailable, retaining stale state as uncertainty.
    pub fn mark_unavailable(&mut self) {
        self.availability = if self.nodes.is_empty() {
            AdapterAvailability::Unavailable
        } else {
            AdapterAvailability::Stale
        };
    }

    /// The current default playback node.
    #[must_use]
    pub fn default_sink(&self) -> Option<&AudioNode> {
        self.default_sink.and_then(|id| self.nodes.get(&id))
    }

    /// The current default capture node.
    #[must_use]
    pub fn default_source(&self) -> Option<&AudioNode> {
        self.default_source.and_then(|id| self.nodes.get(&id))
    }

    /// Look up a node by identity.
    #[must_use]
    pub fn node(&self, id: AudioNodeId) -> Option<&AudioNode> {
        self.nodes.get(&id)
    }

    /// Number of retained nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether any node is retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Validate a requested command against the current capabilities.
    ///
    /// Returns a typed command ready for the adapter, or a typed error that the
    /// root turns into visible acknowledgement feedback.
    pub fn validate(&self, kind: AudioCommandKind) -> Result<AudioCommand, AudioCommandError> {
        let ok = |kind| Ok(AudioCommand { kind });
        match kind {
            AudioCommandKind::SetVolume { id, .. } => {
                self.require_capability(id, AudioDirection::Sink, |caps| caps.can_set_volume)?;
                ok(kind)
            }
            AudioCommandKind::SetMute { id, .. } | AudioCommandKind::ToggleMute { id } => {
                let node = self.require_node(id)?;
                if !node.capabilities.can_set_mute {
                    return Err(AudioCommandError::Unsupported);
                }
                ok(kind)
            }
            AudioCommandKind::SetDefault { direction, id } => {
                let node = self.require_node(id)?;
                if node.direction != direction {
                    return Err(AudioCommandError::WrongDirection);
                }
                // Only a currently routable node can be made the default; an
                // unavailable node would ask WirePlumber to select a device it
                // cannot use.
                if !node.available {
                    return Err(AudioCommandError::Unsupported);
                }
                ok(kind)
            }
            // The public request carries no generation; validation binds it to
            // the current selection's generation, producing the adapter command.
            AudioCommandKind::MoveStream { stream, target } => {
                self.validate_move(stream, target, None)
            }
            // A pre-generationed adapter command is re-validated against the
            // current selection, and its generation must still match.
            AudioCommandKind::MoveStreamTo {
                stream,
                target,
                generation,
            } => self.validate_move(stream, target, Some(generation)),
        }
    }

    /// Validate a stream move and bind it to the current connection generation.
    ///
    /// `required_generation` is `None` for a fresh public request (the current
    /// selection's generation is adopted) and `Some(g)` when re-validating an
    /// already-stamped adapter command (the generation must still match, so a
    /// move queued against a stale connection is rejected).
    fn validate_move(
        &self,
        stream: AudioNodeId,
        target: AudioNodeId,
        required_generation: Option<Generation>,
    ) -> Result<AudioCommand, AudioCommandError> {
        // The target must be a currently routable sink endpoint.
        let target_node = self.require_node(target)?;
        if target_node.direction != AudioDirection::Sink {
            return Err(AudioCommandError::WrongDirection);
        }
        if !target_node.available {
            return Err(AudioCommandError::Unsupported);
        }
        // The stream identity is not an endpoint in this model; it must match
        // the single active movable stream the adapter selected. Zero,
        // ambiguous, or disabled movement is not a supported move.
        let (active, generation) = self
            .movable_stream
            .active_generation()
            .ok_or(AudioCommandError::Unsupported)?;
        if active != stream {
            return Err(AudioCommandError::Unsupported);
        }
        // A re-validated command must still target the live generation.
        if let Some(required) = required_generation
            && !move_is_fresh(required, generation)
        {
            return Err(AudioCommandError::Unsupported);
        }
        Ok(AudioCommand {
            kind: AudioCommandKind::MoveStreamTo {
                stream,
                target,
                generation,
            },
        })
    }

    fn require_node(&self, id: AudioNodeId) -> Result<&AudioNode, AudioCommandError> {
        self.nodes.get(&id).ok_or(AudioCommandError::UnknownNode)
    }

    fn require_capability(
        &self,
        id: AudioNodeId,
        direction: AudioDirection,
        capable: impl Fn(&AudioCapabilities) -> bool,
    ) -> Result<(), AudioCommandError> {
        let node = self.require_node(id)?;
        if node.direction != direction {
            return Err(AudioCommandError::WrongDirection);
        }
        if capable(&node.capabilities) {
            Ok(())
        } else {
            Err(AudioCommandError::Unsupported)
        }
    }
}

/// Ordered adapter-to-root update.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioUpdate {
    /// A new PipeWire connection attempt began.
    Connecting,
    /// Complete node set captured after the initial sync.
    Snapshot {
        /// Bounded node snapshots.
        nodes: Vec<AudioNode>,
        /// Default playback node, if resolved.
        default_sink: Option<AudioNodeId>,
        /// Default capture node, if resolved.
        default_source: Option<AudioNodeId>,
        /// Adapter-relative observation time.
        observed_millis: u64,
    },
    /// One node appeared or changed a relevant property.
    NodeChanged {
        /// Complete replacement snapshot for the node.
        node: AudioNode,
        /// Adapter-relative observation time.
        observed_millis: u64,
    },
    /// One node was removed from the graph.
    NodeRemoved {
        /// Stable global identity.
        id: AudioNodeId,
        /// Adapter-relative observation time.
        observed_millis: u64,
    },
    /// The default node for a direction changed.
    DefaultChanged {
        /// Affected direction.
        direction: AudioDirection,
        /// Resolved default node, if any.
        node: Option<AudioNodeId>,
        /// Adapter-relative observation time.
        observed_millis: u64,
    },
    /// The local transport accepted or rejected a dispatched command.
    CommandOutcome {
        /// Command outcome for visible feedback.
        outcome: AudioCommandOutcome,
        /// Adapter-relative observation time.
        observed_millis: u64,
    },
    /// The movable playback stream selection changed.
    MovableStreamChanged {
        /// Resolved movable-stream selection.
        state: MovableStreamState,
        /// Adapter-relative observation time.
        observed_millis: u64,
    },
    /// The PipeWire connection is unavailable; retained state is stale.
    Unavailable,
}

/// A capability-gated command sent from the root dispatcher to the adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AudioCommand {
    /// Validated request with no shell-string interpretation.
    pub kind: AudioCommandKind,
}

/// Typed audio request vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioCommandKind {
    /// Set a sink's linear volume across all channels.
    SetVolume {
        /// Target sink identity.
        id: AudioNodeId,
        /// Requested clamped volume.
        volume: AudioVolume,
    },
    /// Set a node's mute state.
    SetMute {
        /// Target node identity.
        id: AudioNodeId,
        /// Requested mute state.
        muted: bool,
    },
    /// Toggle a node's mute state.
    ToggleMute {
        /// Target node identity.
        id: AudioNodeId,
    },
    /// Select the default node for a direction.
    SetDefault {
        /// Affected direction.
        direction: AudioDirection,
        /// Requested default node.
        id: AudioNodeId,
    },
    /// Move the active playback stream to a target sink.
    ///
    /// `stream` must equal the single active movable stream the adapter
    /// currently reports through [`MovableStreamState::Active`]; the caller
    /// reads it from state rather than nominating an arbitrary node. The move
    /// cooperates with WirePlumber through `target.object` metadata and only
    /// ever acknowledges that a request was sent.
    MoveStream {
        /// The active movable stream to move.
        stream: AudioNodeId,
        /// Destination sink node.
        target: AudioNodeId,
    },
    /// Adapter-internal generation-stamped form of [`MoveStream`].
    ///
    /// Produced only by [`AudioState::validate`]; never constructed by callers.
    /// The `generation` binds the request to the connection whose selection
    /// authorized it, so the transport can reject a move queued across a
    /// reconnect where the stream and sink IDs may now identify different
    /// objects.
    ///
    /// [`MoveStream`]: AudioCommandKind::MoveStream
    MoveStreamTo {
        /// The active movable stream to move.
        stream: AudioNodeId,
        /// Destination sink node.
        target: AudioNodeId,
        /// Connection generation the authorizing selection was observed on.
        generation: Generation,
    },
}

/// Why a requested command could not be issued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioCommandError {
    /// The target node is not present in current state.
    UnknownNode,
    /// The target node is the wrong direction for the request.
    WrongDirection,
    /// The target node does not advertise the capability.
    Unsupported,
    /// The adapter transport rejected or failed the request.
    Transport,
}

impl AudioCommandError {
    /// A short, content-free reason suitable for accessible feedback.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::UnknownNode => "device unavailable",
            Self::WrongDirection => "wrong device type",
            Self::Unsupported => "control unsupported",
            Self::Transport => "audio service error",
        }
    }
}

/// A local command-dispatch result projected into visible feedback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioCommandOutcome {
    /// Short label describing the attempted action.
    pub label: String,
    /// Present when the command failed.
    pub error: Option<AudioCommandError>,
}

/// SPA metadata value type used for the WirePlumber default-node keys.
pub const DEFAULT_METADATA_TYPE: &str = "Spa:String:JSON";

/// The `default` metadata key WirePlumber watches for a configured default.
///
/// Weftwise writes the persistent *configured* preference key rather than the
/// runtime `default.audio.*` output key that WirePlumber itself owns. Writing
/// the configured key is exactly how `wpctl set-default` cooperates with the
/// session policy: WirePlumber validates the request, applies it, and then
/// republishes the resulting `default.audio.*` selection back to the graph.
#[must_use]
pub const fn default_metadata_key(direction: AudioDirection) -> &'static str {
    match direction {
        AudioDirection::Sink => "default.configured.audio.sink",
        AudioDirection::Source => "default.configured.audio.source",
    }
}

/// Build the JSON value written to a configured default-node metadata key.
///
/// Returns the `{"name":"<node.name>"}` document WirePlumber expects, or `None`
/// for an empty or NUL-bearing node name. `serde_json` escapes every control
/// byte, so the produced value never carries an interior NUL into the C string
/// boundary of the metadata transport.
#[must_use]
pub fn default_metadata_value(name: &str) -> Option<String> {
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    serde_json::to_string(&serde_json::json!({ "name": name })).ok()
}

/// Whether a newly observed `default` metadata global should be bound.
///
/// Only one `default` metadata object is bound at a time. `current` is the
/// registry global ID of the handle already retained, if any. A second
/// concurrent object is a session-policy anomaly and is ignored until the
/// retained one is removed, which prevents a silent route-target swap.
#[must_use]
pub const fn should_bind_default_metadata(current: Option<u32>) -> bool {
    current.is_none()
}

/// Whether a `global_remove` for `removed_id` clears the retained metadata.
///
/// The retained handle is keyed by its registry global ID so that a destroyed
/// `default` metadata object releases the stale proxy. Without this a recreated
/// object would be ignored by [`should_bind_default_metadata`] and route writes
/// would target a proxy whose backing global no longer exists.
#[must_use]
pub const fn should_clear_default_metadata(current: Option<u32>, removed_id: u32) -> bool {
    matches!(current, Some(id) if id == removed_id)
}

// ---------------------------------------------------------------------------
// SPA Props pod codec (verified by round-trip test).
// ---------------------------------------------------------------------------

/// Parsed subset of a node `Props` parameter.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedProps {
    /// Per-channel linear volumes, if present.
    pub channel_volumes: Vec<AudioVolume>,
    /// Mute state, if present.
    pub muted: Option<bool>,
}

impl ParsedProps {
    /// Mean linear volume across channels, if any were reported.
    #[must_use]
    pub fn mean_volume(&self) -> Option<AudioVolume> {
        if self.channel_volumes.is_empty() {
            return None;
        }
        let total: u64 = self
            .channel_volumes
            .iter()
            .map(|volume| u64::from(volume.linear_millis()))
            .sum();
        let mean = total / self.channel_volumes.len() as u64;
        Some(AudioVolume::from_linear_millis(
            u32::try_from(mean).unwrap_or(MAX_VOLUME_LINEAR_MILLIS),
        ))
    }
}

/// Build a serialized SPA `Props` pod that sets channel volumes and/or mute.
#[cfg(feature = "audio-transport")]
#[must_use]
pub fn build_props_pod(volume: Option<AudioVolume>, muted: Option<bool>) -> Option<Vec<u8>> {
    use pipewire::spa::pod::serialize::PodSerializer;
    use pipewire::spa::pod::{Object, Property, PropertyFlags, Value, ValueArray};
    use std::io::Cursor;

    let mut properties = Vec::new();
    if let Some(volume) = volume {
        properties.push(Property {
            key: pipewire::spa::sys::SPA_PROP_channelVolumes,
            flags: PropertyFlags::empty(),
            value: Value::ValueArray(ValueArray::Float(vec![volume.linear(); 2])),
        });
    }
    if let Some(muted) = muted {
        properties.push(Property {
            key: pipewire::spa::sys::SPA_PROP_mute,
            flags: PropertyFlags::empty(),
            value: Value::Bool(muted),
        });
    }
    if properties.is_empty() {
        return None;
    }
    let object = Value::Object(Object {
        type_: pipewire::spa::sys::SPA_TYPE_OBJECT_Props,
        id: pipewire::spa::sys::SPA_PARAM_Props,
        properties,
    });
    let (cursor, _len) = PodSerializer::serialize(Cursor::new(Vec::new()), &object).ok()?;
    Some(cursor.into_inner())
}

/// Parse a serialized SPA `Props` pod into bounded channel volumes and mute.
#[cfg(feature = "audio-transport")]
#[must_use]
pub fn parse_props_pod(bytes: &[u8]) -> Option<ParsedProps> {
    use pipewire::spa::pod::deserialize::PodDeserializer;
    use pipewire::spa::pod::{Value, ValueArray};

    let (_rest, value) = PodDeserializer::deserialize_any_from(bytes).ok()?;
    let Value::Object(object) = value else {
        return None;
    };
    if object.type_ != pipewire::spa::sys::SPA_TYPE_OBJECT_Props {
        return None;
    }
    let mut parsed = ParsedProps::default();
    for property in object.properties {
        if property.key == pipewire::spa::sys::SPA_PROP_channelVolumes {
            if let Value::ValueArray(ValueArray::Float(values)) = property.value {
                parsed.channel_volumes = values.into_iter().map(AudioVolume::from_linear).collect();
            }
        } else if property.key == pipewire::spa::sys::SPA_PROP_mute
            && let Value::Bool(muted) = property.value
        {
            parsed.muted = Some(muted);
        }
    }
    Some(parsed)
}

// ---------------------------------------------------------------------------
// Supervised transport adapter (compiled only with the audio-transport feature).
// ---------------------------------------------------------------------------

#[cfg(feature = "audio-transport")]
pub use transport::{command_channel, run};

#[cfg(feature = "audio-transport")]
mod transport;

#[cfg(test)]
mod tests {
    use super::*;

    fn sink(id: u32, name: &str) -> AudioNode {
        AudioNode::bounded(
            AudioNodeId::new(id),
            AudioDirection::Sink,
            name,
            "Synthetic sink",
            AudioVolume::from_linear_millis(500),
            false,
            true,
        )
    }

    fn source(id: u32, name: &str) -> AudioNode {
        AudioNode::bounded(
            AudioNodeId::new(id),
            AudioDirection::Source,
            name,
            "Synthetic source",
            AudioVolume::from_linear_millis(800),
            false,
            true,
        )
    }

    #[test]
    fn volume_is_clamped_and_finite() {
        assert_eq!(
            AudioVolume::from_linear_millis(9_999).linear_millis(),
            MAX_VOLUME_LINEAR_MILLIS
        );
        assert_eq!(AudioVolume::from_linear(f32::NAN).linear_millis(), 0);
        assert_eq!(AudioVolume::from_linear(-1.0).linear_millis(), 0);
        assert_eq!(AudioVolume::from_linear(1.0).linear_millis(), 1_000);
        assert_eq!(
            AudioVolume::from_linear(100.0).linear_millis(),
            MAX_VOLUME_LINEAR_MILLIS
        );
    }

    #[test]
    fn cubic_percent_is_bounded_display_only() {
        // Unity linear maps to 100 percent cubic.
        assert_eq!(AudioVolume::from_linear(1.0).cubic_percent(), 100);
        assert_eq!(AudioVolume::from_linear(0.0).cubic_percent(), 0);
    }

    #[test]
    fn snapshot_bounds_nodes_and_filters_unknown_defaults() {
        let mut state = AudioState::default();
        let nodes = (0..(MAX_AUDIO_NODES as u32 + 5))
            .map(|id| sink(id, &format!("sink-{id}")))
            .collect::<Vec<_>>();
        state.apply_snapshot(
            nodes,
            Some(AudioNodeId::new(0)),
            Some(AudioNodeId::new(9_999)),
        );
        assert_eq!(state.len(), MAX_AUDIO_NODES);
        assert!(state.default_sink().is_some());
        // The unknown default source identity is dropped.
        assert!(state.default_source().is_none());
        assert_eq!(state.availability, AdapterAvailability::Ready);
    }

    #[test]
    fn removing_a_node_clears_defaults_referencing_it() {
        let mut state = AudioState::default();
        state.apply_snapshot(
            vec![sink(1, "a"), source(2, "b")],
            Some(AudioNodeId::new(1)),
            Some(AudioNodeId::new(2)),
        );
        state.remove_node(AudioNodeId::new(1));
        assert!(state.default_sink().is_none());
        assert!(state.default_source().is_some());
    }

    #[test]
    fn set_default_rejects_wrong_direction_and_unknown() {
        let mut state = AudioState::default();
        state.apply_snapshot(vec![sink(1, "a"), source(2, "b")], None, None);
        state.set_default(AudioDirection::Sink, Some(AudioNodeId::new(2)));
        assert!(state.default_sink().is_none());
        state.set_default(AudioDirection::Sink, Some(AudioNodeId::new(1)));
        assert_eq!(
            state.default_sink().map(|node| node.id),
            Some(AudioNodeId::new(1))
        );
    }

    #[test]
    fn stale_and_unavailable_preserve_state_semantics() {
        let mut state = AudioState::default();
        state.mark_unavailable();
        assert_eq!(state.availability, AdapterAvailability::Unavailable);
        state.apply_snapshot(vec![sink(1, "a")], Some(AudioNodeId::new(1)), None);
        state.mark_stale();
        assert_eq!(state.availability, AdapterAvailability::Stale);
        state.mark_unavailable();
        // Retained nodes downgrade to Stale, never a false empty Unavailable.
        assert_eq!(state.availability, AdapterAvailability::Stale);
    }

    #[test]
    fn command_validation_is_capability_gated() {
        let mut state = AudioState::default();
        state.apply_snapshot(vec![sink(1, "a"), source(2, "b")], None, None);

        assert!(
            state
                .validate(AudioCommandKind::SetVolume {
                    id: AudioNodeId::new(1),
                    volume: AudioVolume::from_linear_millis(500),
                })
                .is_ok()
        );
        // Setting volume on a source is the wrong direction.
        assert_eq!(
            state.validate(AudioCommandKind::SetVolume {
                id: AudioNodeId::new(2),
                volume: AudioVolume::from_linear_millis(500),
            }),
            Err(AudioCommandError::WrongDirection)
        );
        // Muting the microphone source is allowed.
        assert!(
            state
                .validate(AudioCommandKind::SetMute {
                    id: AudioNodeId::new(2),
                    muted: true,
                })
                .is_ok()
        );
        // Unknown node.
        assert_eq!(
            state.validate(AudioCommandKind::ToggleMute {
                id: AudioNodeId::new(99),
            }),
            Err(AudioCommandError::UnknownNode)
        );
        // Move stream requires a sink target.
        assert_eq!(
            state.validate(AudioCommandKind::MoveStream {
                stream: AudioNodeId::new(1),
                target: AudioNodeId::new(2),
            }),
            Err(AudioCommandError::WrongDirection)
        );
        assert_eq!(
            state.validate(AudioCommandKind::MoveStream {
                stream: AudioNodeId::new(2),
                target: AudioNodeId::new(1),
            }),
            Err(AudioCommandError::Unsupported)
        );
    }

    #[test]
    fn unavailable_capability_blocks_control() {
        let mut state = AudioState::default();
        let mut node = sink(1, "a");
        node.available = false;
        node.capabilities = AudioCapabilities::default();
        state.upsert_node(node);
        assert_eq!(
            state.validate(AudioCommandKind::SetVolume {
                id: AudioNodeId::new(1),
                volume: AudioVolume::from_linear_millis(100),
            }),
            Err(AudioCommandError::Unsupported)
        );
    }

    #[cfg(feature = "audio-transport")]
    #[test]
    fn props_pod_round_trips_volume_and_mute() {
        let volume = AudioVolume::from_linear_millis(250);
        let bytes = build_props_pod(Some(volume), Some(true)).expect("pod");
        let parsed = parse_props_pod(&bytes).expect("parsed");
        assert_eq!(parsed.muted, Some(true));
        let mean = parsed.mean_volume().expect("mean volume");
        assert_eq!(mean.linear_millis(), volume.linear_millis());
    }

    #[test]
    fn command_error_reasons_are_content_free() {
        assert_eq!(
            AudioCommandError::UnknownNode.reason(),
            "device unavailable"
        );
        assert_eq!(AudioCommandError::Transport.reason(), "audio service error");
    }

    #[test]
    fn set_default_validation_is_capability_and_availability_gated() {
        let mut state = AudioState::default();
        let mut unavailable = sink(3, "offline");
        unavailable.available = false;
        state.apply_snapshot(vec![sink(1, "a"), source(2, "b"), unavailable], None, None);

        // An available sink can become the default sink.
        assert!(
            state
                .validate(AudioCommandKind::SetDefault {
                    direction: AudioDirection::Sink,
                    id: AudioNodeId::new(1),
                })
                .is_ok()
        );
        // A source cannot become the default sink.
        assert_eq!(
            state.validate(AudioCommandKind::SetDefault {
                direction: AudioDirection::Sink,
                id: AudioNodeId::new(2),
            }),
            Err(AudioCommandError::WrongDirection)
        );
        // An unavailable node cannot be selected as a default.
        assert_eq!(
            state.validate(AudioCommandKind::SetDefault {
                direction: AudioDirection::Sink,
                id: AudioNodeId::new(3),
            }),
            Err(AudioCommandError::Unsupported)
        );
        // An unknown node is rejected.
        assert_eq!(
            state.validate(AudioCommandKind::SetDefault {
                direction: AudioDirection::Source,
                id: AudioNodeId::new(99),
            }),
            Err(AudioCommandError::UnknownNode)
        );
    }

    #[test]
    fn default_metadata_key_targets_the_configured_preference() {
        // Weftwise writes the persistent configured key, never the runtime
        // output key WirePlumber owns.
        assert_eq!(
            default_metadata_key(AudioDirection::Sink),
            "default.configured.audio.sink"
        );
        assert_eq!(
            default_metadata_key(AudioDirection::Source),
            "default.configured.audio.source"
        );
    }

    #[test]
    fn default_metadata_value_is_bounded_json_or_none() {
        assert_eq!(
            default_metadata_value("synthetic-sink"),
            Some(r#"{"name":"synthetic-sink"}"#.to_owned())
        );
        // Empty and NUL-bearing names produce no value.
        assert_eq!(default_metadata_value(""), None);
        assert_eq!(default_metadata_value("bad\0name"), None);
        // Quotes and backslashes are JSON-escaped, never injected raw.
        assert_eq!(
            default_metadata_value(r#"a"b\c"#),
            Some(r#"{"name":"a\"b\\c"}"#.to_owned())
        );
    }

    #[test]
    fn default_metadata_binds_one_object_until_it_is_removed() {
        // No handle retained yet: the first `default` metadata object binds.
        assert!(should_bind_default_metadata(None));
        // A handle is already retained: a second concurrent object is ignored
        // so the route target cannot be silently swapped.
        assert!(!should_bind_default_metadata(Some(41)));
    }

    #[test]
    fn default_metadata_clears_only_on_its_own_global_removal() {
        // A `global_remove` for the retained object's ID releases the handle.
        assert!(should_clear_default_metadata(Some(41), 41));
        // A removal for any other global leaves the handle intact.
        assert!(!should_clear_default_metadata(Some(41), 42));
        // With nothing retained there is nothing to clear.
        assert!(!should_clear_default_metadata(None, 41));
        // After clearing, the slot is empty and the next object may rebind.
        assert!(should_bind_default_metadata(None));
    }

    fn candidate(id: u32, running: bool, movable: bool) -> MovableStreamCandidate {
        MovableStreamCandidate {
            id: AudioNodeId::new(id),
            running,
            movable,
            has_metadata_permission: true,
        }
    }

    const GEN: Generation = Generation::new(1);

    fn ready_enablement() -> MoveEnablement {
        MoveEnablement {
            policy_allows: true,
            metadata_writable: true,
            overflowed: false,
        }
    }

    #[test]
    fn allow_moving_streams_is_tri_state() {
        assert_eq!(
            parse_allow_moving_streams(Some("true")),
            MoveMovingPolicy::Allowed
        );
        assert_eq!(
            parse_allow_moving_streams(Some(" 1 ")),
            MoveMovingPolicy::Allowed
        );
        assert_eq!(
            parse_allow_moving_streams(Some("false")),
            MoveMovingPolicy::Denied
        );
        assert_eq!(
            parse_allow_moving_streams(Some("0")),
            MoveMovingPolicy::Denied
        );
        // Absent or unrecognized values are Unknown, which permits an attempt
        // but never claims capability.
        assert_eq!(parse_allow_moving_streams(None), MoveMovingPolicy::Unknown);
        assert_eq!(
            parse_allow_moving_streams(Some("maybe")),
            MoveMovingPolicy::Unknown
        );
        assert!(MoveMovingPolicy::Unknown.permits_attempt());
        assert!(MoveMovingPolicy::Allowed.permits_attempt());
        assert!(!MoveMovingPolicy::Denied.permits_attempt());
    }

    #[test]
    fn metadata_write_requires_write_and_execute() {
        // Both W (0o200) and X (0o100) are required.
        assert!(metadata_permits_target_write(PW_PERM_W | PW_PERM_X));
        assert!(metadata_permits_target_write(0o700));
        assert!(!metadata_permits_target_write(PW_PERM_W));
        assert!(!metadata_permits_target_write(PW_PERM_X));
        assert!(!metadata_permits_target_write(0o400));
        assert!(!metadata_permits_target_write(0));
    }

    #[test]
    fn selection_requires_exactly_one_running_movable_stream() {
        // Zero running movable streams is unavailable.
        assert_eq!(
            select_movable_stream(&[], ready_enablement(), GEN),
            MovableStreamState::Unavailable
        );
        // A single running movable stream is the unique active subject and
        // carries the connection generation it was selected on.
        assert_eq!(
            select_movable_stream(&[candidate(7, true, true)], ready_enablement(), GEN),
            MovableStreamState::Active {
                stream: AudioNodeId::new(7),
                generation: GEN,
            }
        );
        // A non-running or non-movable stream does not count.
        assert_eq!(
            select_movable_stream(
                &[candidate(7, false, true), candidate(8, true, false)],
                ready_enablement(),
                GEN,
            ),
            MovableStreamState::Unavailable
        );
        // Two running movable streams are ambiguous.
        assert_eq!(
            select_movable_stream(
                &[candidate(7, true, true), candidate(8, true, true)],
                ready_enablement(),
                GEN,
            ),
            MovableStreamState::Ambiguous
        );
    }

    #[test]
    fn selection_ignores_a_subject_without_metadata_permission() {
        // The only running movable stream lacks PW_PERM_M on its subject, so it
        // cannot be moved and the action is unavailable rather than offered.
        let no_perm = MovableStreamCandidate {
            has_metadata_permission: false,
            ..candidate(7, true, true)
        };
        assert_eq!(
            select_movable_stream(&[no_perm], ready_enablement(), GEN),
            MovableStreamState::Unavailable
        );
        // When one of two running movable streams lacks the permission, the
        // other is the unique movable subject rather than an ambiguous pair.
        assert_eq!(
            select_movable_stream(
                &[no_perm, candidate(8, true, true)],
                ready_enablement(),
                GEN
            ),
            MovableStreamState::Active {
                stream: AudioNodeId::new(8),
                generation: GEN,
            }
        );
        assert!(subject_permits_metadata(PW_PERM_M));
        assert!(subject_permits_metadata(0o710));
        assert!(!subject_permits_metadata(PW_PERM_W | PW_PERM_X));
        assert!(!subject_permits_metadata(0));
    }

    #[test]
    fn movable_stream_class_requires_exact_trimmed_equality() {
        assert!(is_movable_stream_class("Stream/Output/Audio"));
        // Surrounding whitespace is trimmed before the exact comparison.
        assert!(is_movable_stream_class("  Stream/Output/Audio\n"));
        // Decorated, prefixed, suffixed, or unrelated classes are rejected.
        assert!(!is_movable_stream_class("Stream/Output/Audio/Virtual"));
        assert!(!is_movable_stream_class("Stream/Output/Audiofoo"));
        assert!(!is_movable_stream_class("xStream/Output/Audio"));
        assert!(!is_movable_stream_class("Stream/Output/Video"));
        assert!(!is_movable_stream_class("Stream/Input/Audio"));
        assert!(!is_movable_stream_class(""));
        assert_eq!(MOVABLE_STREAM_MEDIA_CLASS, "Stream/Output/Audio");
    }

    #[test]
    fn stale_generation_move_is_rejected() {
        // A move built against the live generation is fresh; the same command
        // after a reconnect to a newer generation is stale.
        assert!(move_is_fresh(Generation::new(3), Generation::new(3)));
        assert!(!move_is_fresh(Generation::new(3), Generation::new(4)));
        assert!(!move_is_fresh(Generation::new(4), Generation::new(3)));
    }

    #[test]
    fn selection_disables_on_policy_permission_or_overflow() {
        let one = [candidate(7, true, true)];
        // Explicit policy denial disables even a unique candidate.
        assert_eq!(
            select_movable_stream(
                &one,
                MoveEnablement {
                    policy_allows: false,
                    ..ready_enablement()
                },
                GEN,
            ),
            MovableStreamState::Disabled
        );
        // A metadata object without write permission disables the action.
        assert_eq!(
            select_movable_stream(
                &one,
                MoveEnablement {
                    metadata_writable: false,
                    ..ready_enablement()
                },
                GEN,
            ),
            MovableStreamState::Disabled
        );
        // Inventory overflow disables rather than guessing over a partial set.
        assert_eq!(
            select_movable_stream(
                &one,
                MoveEnablement {
                    overflowed: true,
                    ..ready_enablement()
                },
                GEN,
            ),
            MovableStreamState::Disabled
        );
    }

    #[test]
    fn target_object_value_is_the_decimal_serial() {
        assert_eq!(target_object_metadata_value(0), "0");
        assert_eq!(target_object_metadata_value(4_294_967_297), "4294967297");
        assert_eq!(TARGET_OBJECT_METADATA_KEY, "target.object");
        assert_eq!(TARGET_OBJECT_METADATA_TYPE, "Spa:Id");
    }

    #[test]
    fn move_stream_validates_against_active_selection() {
        let mut state = AudioState::default();
        state.apply_snapshot(vec![sink(1, "a"), source(2, "b")], None, None);

        // With no movable stream, the move is unsupported regardless of subject.
        assert_eq!(
            state.validate(AudioCommandKind::MoveStream {
                stream: AudioNodeId::new(5),
                target: AudioNodeId::new(1),
            }),
            Err(AudioCommandError::Unsupported)
        );

        // With an active movable stream, moving it to an available sink is ok,
        // and validation binds the request to the selection's generation.
        state.set_movable_stream(MovableStreamState::Active {
            stream: AudioNodeId::new(5),
            generation: Generation::new(7),
        });
        assert_eq!(
            state.validate(AudioCommandKind::MoveStream {
                stream: AudioNodeId::new(5),
                target: AudioNodeId::new(1),
            }),
            Ok(AudioCommand {
                kind: AudioCommandKind::MoveStreamTo {
                    stream: AudioNodeId::new(5),
                    target: AudioNodeId::new(1),
                    generation: Generation::new(7),
                },
            })
        );
        // A stamped adapter command whose generation still matches re-validates,
        // but one built against a stale generation is rejected.
        assert!(
            state
                .validate(AudioCommandKind::MoveStreamTo {
                    stream: AudioNodeId::new(5),
                    target: AudioNodeId::new(1),
                    generation: Generation::new(7),
                })
                .is_ok()
        );
        assert_eq!(
            state.validate(AudioCommandKind::MoveStreamTo {
                stream: AudioNodeId::new(5),
                target: AudioNodeId::new(1),
                generation: Generation::new(6),
            }),
            Err(AudioCommandError::Unsupported)
        );
        // A subject that is not the active stream is unsupported.
        assert_eq!(
            state.validate(AudioCommandKind::MoveStream {
                stream: AudioNodeId::new(6),
                target: AudioNodeId::new(1),
            }),
            Err(AudioCommandError::Unsupported)
        );
        // A source target is the wrong direction even with an active stream.
        assert_eq!(
            state.validate(AudioCommandKind::MoveStream {
                stream: AudioNodeId::new(5),
                target: AudioNodeId::new(2),
            }),
            Err(AudioCommandError::WrongDirection)
        );
        // An unknown target node is rejected.
        assert_eq!(
            state.validate(AudioCommandKind::MoveStream {
                stream: AudioNodeId::new(5),
                target: AudioNodeId::new(99),
            }),
            Err(AudioCommandError::UnknownNode)
        );
        // Ambiguous or disabled selection offers no move.
        state.set_movable_stream(MovableStreamState::Ambiguous);
        assert_eq!(
            state.validate(AudioCommandKind::MoveStream {
                stream: AudioNodeId::new(5),
                target: AudioNodeId::new(1),
            }),
            Err(AudioCommandError::Unsupported)
        );
    }

    #[test]
    fn movable_stream_state_accessors() {
        let active = MovableStreamState::Active {
            stream: AudioNodeId::new(3),
            generation: Generation::new(9),
        };
        assert_eq!(active.active(), Some(AudioNodeId::new(3)));
        assert_eq!(
            active.active_generation(),
            Some((AudioNodeId::new(3), Generation::new(9)))
        );
        assert!(active.can_move());
        assert_eq!(MovableStreamState::Unavailable.active(), None);
        assert_eq!(MovableStreamState::Unavailable.active_generation(), None);
        assert!(!MovableStreamState::Ambiguous.can_move());
        assert!(!MovableStreamState::Disabled.can_move());
        // Default is the safe Unavailable.
        assert_eq!(
            MovableStreamState::default(),
            MovableStreamState::Unavailable
        );
        // The generation newtype round-trips its counter value.
        assert_eq!(Generation::new(42).get(), 42);
    }
}
