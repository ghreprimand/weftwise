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
//! PipeWire `default` metadata for current sink and source selection; route
//! mutation remains an explicit transport limitation. This crate adds no
//! `wireplumber` dependency and contains no `unsafe` project code.
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

/// Root-owned PipeWire audio domain state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AudioState {
    /// Independent adapter availability.
    pub availability: AdapterAvailability,
    nodes: BTreeMap<AudioNodeId, AudioNode>,
    default_sink: Option<AudioNodeId>,
    default_source: Option<AudioNodeId>,
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
            AudioCommandKind::MoveStream { stream, target } => {
                self.require_node(stream)?;
                let target_node = self.require_node(target)?;
                if target_node.direction != AudioDirection::Sink {
                    return Err(AudioCommandError::WrongDirection);
                }
                Err(AudioCommandError::Unsupported)
            }
        }
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
    /// Move a stream node to a target sink.
    MoveStream {
        /// Stream node to move.
        stream: AudioNodeId,
        /// Destination sink node.
        target: AudioNodeId,
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
mod transport {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Instant;

    use pipewire::context::ContextRc;
    use pipewire::main_loop::MainLoopRc;
    use pipewire::metadata::Metadata;
    use pipewire::registry::{GlobalObject, RegistryRc};
    use pipewire::spa::param::ParamType;
    use pipewire::spa::pod::Pod;
    use pipewire::spa::utils::dict::DictRef;
    use pipewire::types::ObjectType;
    use tokio::sync::mpsc;

    use super::{
        AudioCapabilities, AudioCommand, AudioCommandError, AudioCommandKind, AudioCommandOutcome,
        AudioDirection, AudioNode, AudioNodeId, AudioUpdate, AudioVolume, DEFAULT_METADATA_TYPE,
        MAX_AUDIO_NAME_CHARACTERS, MAX_AUDIO_NODES, build_props_pod, default_metadata_key,
        default_metadata_value, parse_props_pod, should_bind_default_metadata,
        should_clear_default_metadata,
    };
    use crate::supervisor::{Cancellation, ReconnectBackoff};

    /// The bound `default` metadata proxy retained for route mutation.
    ///
    /// Keyed by the registry global ID so a `global_remove` for that object can
    /// release the proxy and let a recreated one rebind, instead of writing
    /// through a stale handle. The property listener is retained alongside the
    /// proxy so both drop together when the handle is cleared.
    struct DefaultMetadataHandle {
        global_id: u32,
        proxy: Metadata,
        _listener: pipewire::metadata::MetadataListener,
    }

    /// Shared slot for the single bound `default` metadata handle.
    type DefaultMetadata = Rc<RefCell<Option<DefaultMetadataHandle>>>;

    type Publish = Arc<dyn Fn(AudioUpdate) + Send + Sync + 'static>;

    /// Create the bounded command channel owned by the root and adapter.
    #[must_use]
    pub fn command_channel() -> (mpsc::Sender<AudioCommand>, mpsc::Receiver<AudioCommand>) {
        mpsc::channel(super::COMMAND_CAPACITY)
    }

    /// Control messages delivered into the PipeWire loop thread.
    enum Control {
        Command(AudioCommand),
        Quit,
    }

    /// Run the supervised PipeWire adapter, reconnecting until cancellation.
    pub async fn run(
        publish: impl Fn(AudioUpdate) + Send + Sync + 'static,
        mut commands: mpsc::Receiver<AudioCommand>,
        mut cancellation: Cancellation,
    ) {
        let publish: Publish = Arc::new(publish);
        let mut backoff = ReconnectBackoff::default();
        loop {
            (publish)(AudioUpdate::Connecting);
            let (control_tx, control_rx) = pipewire::channel::channel::<Control>();
            let (done_tx, mut done_rx) = mpsc::unbounded_channel::<()>();
            let (ready_tx, mut ready_rx) = mpsc::unbounded_channel::<()>();
            let thread_publish = Arc::clone(&publish);
            let handle = std::thread::Builder::new()
                .name("weftwise-pipewire".to_owned())
                .spawn(move || {
                    thread_main(thread_publish, control_rx, ready_tx);
                    let _ = done_tx.send(());
                });
            let Ok(handle) = handle else {
                (publish)(AudioUpdate::Unavailable);
                let delay = backoff.next_delay();
                tokio::select! {
                    _ = cancellation.cancelled() => return,
                    _ = tokio::time::sleep(delay) => {}
                }
                continue;
            };

            let mut ready = false;
            loop {
                tokio::select! {
                    _ = cancellation.cancelled() => {
                        let _ = control_tx.send(Control::Quit);
                        break;
                    }
                    _ = done_rx.recv() => break,
                    initial_sync = ready_rx.recv(), if !ready => {
                        ready = true;
                        if initial_sync.is_some() {
                            backoff.reset();
                        }
                    }
                    command = commands.recv() => {
                        match command {
                            Some(command) => {
                                if control_tx.send(Control::Command(command)).is_err() {
                                    break;
                                }
                            }
                            None => {
                                let _ = control_tx.send(Control::Quit);
                                break;
                            }
                        }
                    }
                }
            }

            let _ = handle.join();
            if cancellation.is_cancelled() {
                return;
            }
            (publish)(AudioUpdate::Unavailable);
            let delay = backoff.next_delay();
            tokio::select! {
                _ = cancellation.cancelled() => return,
                _ = tokio::time::sleep(delay) => {}
            }
        }
    }

    struct NodeEntry {
        node: AudioNode,
        name: String,
        proxy: pipewire::node::Node,
        _listener: pipewire::node::NodeListener,
    }

    #[derive(Default)]
    struct Defaults {
        sink_name: Option<String>,
        source_name: Option<String>,
    }

    type Nodes = Rc<RefCell<HashMap<u32, NodeEntry>>>;

    fn thread_main(
        publish: Publish,
        control_rx: pipewire::channel::Receiver<Control>,
        ready_tx: mpsc::UnboundedSender<()>,
    ) {
        pipewire::init();
        let Ok(main_loop) = MainLoopRc::new(None) else {
            return;
        };
        let Ok(context) = ContextRc::new(&main_loop, None) else {
            return;
        };
        let Ok(core) = context.connect_rc(None) else {
            return;
        };
        let Ok(registry) = core.get_registry_rc() else {
            return;
        };

        let nodes: Nodes = Rc::new(RefCell::new(HashMap::new()));
        let defaults = Rc::new(RefCell::new(Defaults::default()));
        let synced = Rc::new(Cell::new(false));
        let default_metadata: DefaultMetadata = Rc::new(RefCell::new(None));
        let started = Instant::now();

        let quit_loop = main_loop.clone();
        let command_nodes = Rc::clone(&nodes);
        let command_metadata = Rc::clone(&default_metadata);
        let command_publish = Arc::clone(&publish);
        let _control = control_rx.attach(main_loop.loop_(), move |control| match control {
            Control::Quit => quit_loop.quit(),
            Control::Command(command) => {
                apply_command(
                    &command_nodes,
                    &command_metadata,
                    &command,
                    &command_publish,
                    started,
                );
            }
        });

        let registry_for_cb = registry.clone();
        let global_nodes = Rc::clone(&nodes);
        let global_defaults = Rc::clone(&defaults);
        let global_synced = Rc::clone(&synced);
        let global_metadata = Rc::clone(&default_metadata);
        let global_publish = Arc::clone(&publish);
        let remove_nodes = Rc::clone(&nodes);
        let remove_synced = Rc::clone(&synced);
        let remove_publish = Arc::clone(&publish);
        let remove_metadata = Rc::clone(&default_metadata);
        let _registry_listener = registry
            .add_listener_local()
            .global(move |global| {
                on_global(
                    &registry_for_cb,
                    &global_nodes,
                    &global_defaults,
                    &global_synced,
                    &global_metadata,
                    &global_publish,
                    started,
                    global,
                );
            })
            .global_remove(move |id| {
                let current_id = remove_metadata
                    .borrow()
                    .as_ref()
                    .map(|handle| handle.global_id);
                if should_clear_default_metadata(current_id, id) {
                    // Release the stale proxy and listener so a recreated
                    // `default` metadata object can rebind on its next global.
                    *remove_metadata.borrow_mut() = None;
                }
                if remove_nodes.borrow_mut().remove(&id).is_some() && remove_synced.get() {
                    (remove_publish)(AudioUpdate::NodeRemoved {
                        id: AudioNodeId::new(id),
                        observed_millis: elapsed_millis(started),
                    });
                }
            })
            .register();

        let Ok(pending) = core.sync(0) else {
            return;
        };
        let sync_nodes = Rc::clone(&nodes);
        let sync_defaults = Rc::clone(&defaults);
        let sync_synced = Rc::clone(&synced);
        let sync_publish = Arc::clone(&publish);
        let error_loop = main_loop.clone();
        let _core_listener = core
            .add_listener_local()
            .done(move |id, sequence| {
                if id != pipewire::core::PW_ID_CORE || sequence != pending || sync_synced.get() {
                    return;
                }
                sync_synced.set(true);
                publish_snapshot(&sync_nodes, &sync_defaults, &sync_publish, started);
                let _ = ready_tx.send(());
            })
            .error(move |id, _sequence, _result, _message| {
                if id == pipewire::core::PW_ID_CORE {
                    error_loop.quit();
                }
            })
            .register();

        main_loop.run();
    }

    #[allow(clippy::too_many_arguments)]
    fn on_global(
        registry: &RegistryRc,
        nodes: &Nodes,
        defaults: &Rc<RefCell<Defaults>>,
        synced: &Rc<Cell<bool>>,
        metadata: &DefaultMetadata,
        publish: &Publish,
        started: Instant,
        global: &GlobalObject<&DictRef>,
    ) {
        match global.type_ {
            ObjectType::Node => {
                on_node_global(registry, nodes, defaults, synced, publish, started, global)
            }
            ObjectType::Metadata => on_metadata_global(
                registry, nodes, defaults, synced, metadata, publish, started, global,
            ),
            _ => {}
        }
    }

    fn on_node_global(
        registry: &RegistryRc,
        nodes: &Nodes,
        defaults: &Rc<RefCell<Defaults>>,
        synced: &Rc<Cell<bool>>,
        publish: &Publish,
        started: Instant,
        global: &GlobalObject<&DictRef>,
    ) {
        let Some(props) = global.props else {
            return;
        };
        let Some(direction) = audio_direction(props) else {
            return;
        };
        let id = global.id;
        if nodes.borrow().len() >= MAX_AUDIO_NODES && !nodes.borrow().contains_key(&id) {
            return;
        }
        let name = props
            .get("node.name")
            .unwrap_or_default()
            .chars()
            .take(MAX_AUDIO_NAME_CHARACTERS)
            .collect::<String>();
        let description = props
            .get("node.description")
            .or_else(|| props.get("node.nick"))
            .unwrap_or(&name)
            .to_owned();
        let Ok(proxy) = registry.bind::<pipewire::node::Node, _>(global) else {
            return;
        };
        proxy.subscribe_params(&[ParamType::Props]);
        let param_nodes = Rc::clone(nodes);
        let param_synced = Rc::clone(synced);
        let param_publish = Arc::clone(publish);
        let listener = proxy
            .add_listener_local()
            .param(move |_seq, _kind, _index, _next, pod| {
                if let Some(pod) = pod {
                    on_node_params(
                        &param_nodes,
                        &param_synced,
                        &param_publish,
                        started,
                        id,
                        pod,
                    );
                }
            })
            .register();
        let mut node = AudioNode::bounded(
            AudioNodeId::new(id),
            direction,
            &name,
            &description,
            AudioVolume::default(),
            false,
            true,
        );
        // Runtime control stays disabled until the server exposes the
        // corresponding writable Props value for this node.
        node.capabilities = AudioCapabilities::default();
        nodes.borrow_mut().insert(
            id,
            NodeEntry {
                node: node.clone(),
                name,
                proxy,
                _listener: listener,
            },
        );
        if synced.get() {
            (publish)(AudioUpdate::NodeChanged {
                node,
                observed_millis: elapsed_millis(started),
            });
            resolve_defaults(nodes, defaults, publish, started);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn on_metadata_global(
        registry: &RegistryRc,
        nodes: &Nodes,
        defaults: &Rc<RefCell<Defaults>>,
        synced: &Rc<Cell<bool>>,
        metadata_handle: &DefaultMetadata,
        publish: &Publish,
        started: Instant,
        global: &GlobalObject<&DictRef>,
    ) {
        let is_default = global
            .props
            .and_then(|props| props.get("metadata.name"))
            .is_some_and(|name| name == "default");
        if !is_default {
            return;
        }
        // Bind only the first `default` metadata object; a second one would be a
        // policy anomaly and must not silently replace the route target.
        let current_id = metadata_handle
            .borrow()
            .as_ref()
            .map(|handle| handle.global_id);
        if !should_bind_default_metadata(current_id) {
            return;
        }
        let global_id = global.id;
        let Ok(metadata) = registry.bind::<pipewire::metadata::Metadata, _>(global) else {
            return;
        };
        let property_defaults = Rc::clone(defaults);
        let property_nodes = Rc::clone(nodes);
        let property_synced = Rc::clone(synced);
        let property_publish = Arc::clone(publish);
        let listener = metadata
            .add_listener_local()
            .property(move |_subject, key, _type, value| {
                on_default_metadata(
                    &property_defaults,
                    &property_nodes,
                    &property_synced,
                    &property_publish,
                    started,
                    key,
                    value,
                );
                0
            })
            .register();
        // The proxy must outlive the loop so route commands can write to it. It
        // is retained with its global ID and listener so a matching
        // `global_remove` releases both and a recreated object can rebind.
        *metadata_handle.borrow_mut() = Some(DefaultMetadataHandle {
            global_id,
            proxy: metadata,
            _listener: listener,
        });
    }

    fn on_node_params(
        nodes: &Nodes,
        synced: &Rc<Cell<bool>>,
        publish: &Publish,
        started: Instant,
        id: u32,
        pod: &Pod,
    ) {
        let Some(parsed) = parse_props_pod(pod.as_bytes()) else {
            return;
        };
        let mut borrow = nodes.borrow_mut();
        let Some(entry) = borrow.get_mut(&id) else {
            return;
        };
        if let Some(volume) = parsed.mean_volume() {
            entry.node.volume = volume;
            entry.node.capabilities.can_set_volume = true;
        }
        if let Some(muted) = parsed.muted {
            entry.node.muted = muted;
            entry.node.capabilities.can_set_mute = true;
        }
        let node = entry.node.clone();
        drop(borrow);
        if synced.get() {
            (publish)(AudioUpdate::NodeChanged {
                node,
                observed_millis: elapsed_millis(started),
            });
        }
    }

    fn on_default_metadata(
        defaults: &Rc<RefCell<Defaults>>,
        nodes: &Nodes,
        synced: &Rc<Cell<bool>>,
        publish: &Publish,
        started: Instant,
        key: Option<&str>,
        value: Option<&str>,
    ) {
        let Some(key) = key else {
            return;
        };
        let name = value.and_then(parse_default_name);
        match key {
            "default.audio.sink" => defaults.borrow_mut().sink_name = name,
            "default.audio.source" => defaults.borrow_mut().source_name = name,
            _ => return,
        }
        if synced.get() {
            resolve_defaults(nodes, defaults, publish, started);
        }
    }

    fn publish_snapshot(
        nodes: &Nodes,
        defaults: &Rc<RefCell<Defaults>>,
        publish: &Publish,
        started: Instant,
    ) {
        let defaults = defaults.borrow();
        let borrow = nodes.borrow();
        let mut snapshot = borrow
            .values()
            .map(|entry| entry.node.clone())
            .collect::<Vec<_>>();
        snapshot.sort_by_key(|node| node.id);
        let resolve = |direction, wanted: Option<&str>| {
            wanted.and_then(|wanted| {
                borrow
                    .values()
                    .find(|entry| entry.name == wanted && entry.node.direction == direction)
                    .map(|entry| entry.node.id)
            })
        };
        (publish)(AudioUpdate::Snapshot {
            nodes: snapshot,
            default_sink: resolve(AudioDirection::Sink, defaults.sink_name.as_deref()),
            default_source: resolve(AudioDirection::Source, defaults.source_name.as_deref()),
            observed_millis: elapsed_millis(started),
        });
    }

    fn resolve_defaults(
        nodes: &Nodes,
        defaults: &Rc<RefCell<Defaults>>,
        publish: &Publish,
        started: Instant,
    ) {
        let defaults = defaults.borrow();
        let borrow = nodes.borrow();
        for (direction, wanted) in [
            (AudioDirection::Sink, defaults.sink_name.as_deref()),
            (AudioDirection::Source, defaults.source_name.as_deref()),
        ] {
            let resolved = wanted.and_then(|wanted| {
                borrow
                    .values()
                    .find(|entry| entry.name == wanted && entry.node.direction == direction)
                    .map(|entry| entry.node.id)
            });
            (publish)(AudioUpdate::DefaultChanged {
                direction,
                node: resolved,
                observed_millis: elapsed_millis(started),
            });
        }
    }

    fn apply_command(
        nodes: &Nodes,
        metadata: &DefaultMetadata,
        command: &AudioCommand,
        publish: &Publish,
        started: Instant,
    ) {
        let (label, result) = dispatch(nodes, metadata, command);
        (publish)(AudioUpdate::CommandOutcome {
            outcome: AudioCommandOutcome {
                label,
                error: result.err(),
            },
            observed_millis: elapsed_millis(started),
        });
    }

    fn dispatch(
        nodes: &Nodes,
        metadata: &DefaultMetadata,
        command: &AudioCommand,
    ) -> (String, Result<(), AudioCommandError>) {
        let borrow = nodes.borrow();
        match command.kind {
            AudioCommandKind::SetVolume { id, volume } => (
                "Volume".to_owned(),
                set_node_props(&borrow, id.get(), Some(volume), None),
            ),
            AudioCommandKind::SetMute { id, muted } => (
                "Mute".to_owned(),
                set_node_props(&borrow, id.get(), None, Some(muted)),
            ),
            AudioCommandKind::ToggleMute { id } => {
                match borrow.get(&id.get()).map(|entry| !entry.node.muted) {
                    Some(muted) => (
                        "Mute".to_owned(),
                        set_node_props(&borrow, id.get(), None, Some(muted)),
                    ),
                    None => ("Mute".to_owned(), Err(AudioCommandError::UnknownNode)),
                }
            }
            AudioCommandKind::SetDefault { direction, id } => (
                "Route".to_owned(),
                set_default_node(&borrow, metadata, direction, id.get()),
            ),
            AudioCommandKind::MoveStream { .. } => (
                "Route".to_owned(),
                // Per-stream movement targets a stream node this endpoint-only
                // model does not represent, and its native contract is not
                // verified. It stays an explicit transport limitation.
                Err(AudioCommandError::Transport),
            ),
        }
    }

    fn set_node_props(
        nodes: &HashMap<u32, NodeEntry>,
        id: u32,
        volume: Option<AudioVolume>,
        muted: Option<bool>,
    ) -> Result<(), AudioCommandError> {
        let entry = nodes.get(&id).ok_or(AudioCommandError::UnknownNode)?;
        let bytes = build_props_pod(volume, muted).ok_or(AudioCommandError::Transport)?;
        let pod = Pod::from_bytes(&bytes).ok_or(AudioCommandError::Transport)?;
        entry.proxy.set_param(ParamType::Props, 0, pod);
        Ok(())
    }

    /// Ask WirePlumber to make a node the configured default for its direction.
    ///
    /// This writes the persistent `default.configured.audio.{sink,source}` key
    /// of the `default` metadata, the same cooperation path `wpctl set-default`
    /// uses. WirePlumber validates and applies the request, then republishes the
    /// resulting `default.audio.*` selection, which arrives back through the
    /// metadata property listener as a `DefaultChanged` update. Weftwise never
    /// writes the runtime output key directly.
    fn set_default_node(
        nodes: &HashMap<u32, NodeEntry>,
        metadata: &DefaultMetadata,
        direction: AudioDirection,
        id: u32,
    ) -> Result<(), AudioCommandError> {
        let entry = nodes.get(&id).ok_or(AudioCommandError::UnknownNode)?;
        if entry.node.direction != direction {
            return Err(AudioCommandError::WrongDirection);
        }
        if !entry.node.available {
            return Err(AudioCommandError::Unsupported);
        }
        let value = default_metadata_value(&entry.name).ok_or(AudioCommandError::Transport)?;
        let handle = metadata.borrow();
        let handle = handle.as_ref().ok_or(AudioCommandError::Transport)?;
        handle.proxy.set_property(
            0,
            default_metadata_key(direction),
            Some(DEFAULT_METADATA_TYPE),
            Some(&value),
        );
        Ok(())
    }

    fn audio_direction(props: &DictRef) -> Option<AudioDirection> {
        let class = props.get("media.class")?;
        if class.contains("Audio/Sink") {
            Some(AudioDirection::Sink)
        } else if class.contains("Audio/Source") {
            Some(AudioDirection::Source)
        } else {
            None
        }
    }

    fn parse_default_name(value: &str) -> Option<String> {
        let object = serde_json::from_str::<serde_json::Value>(value).ok()?;
        Some(
            object
                .get("name")?
                .as_str()?
                .chars()
                .take(MAX_AUDIO_NAME_CHARACTERS)
                .collect(),
        )
    }

    fn elapsed_millis(started: Instant) -> u64 {
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

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
}
