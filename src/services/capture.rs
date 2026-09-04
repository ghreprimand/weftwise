//! Bounded, privacy-preserving PipeWire capture-evidence graph.
//!
//! This module derives positive microphone and camera capture evidence from the
//! PipeWire object graph without retaining any identifying content. Every record
//! it stores is a numeric registry identity or a small closed enum: device
//! backing API, node role, node running flag, port direction, and link activity.
//! Client names, application names, media names, node and device descriptions,
//! object paths, serials, process arguments, and media samples are not retained.
//! The few properties needed for classification are inspected transiently, and
//! only the resulting enum is kept.
//!
//! Evidence is strictly positive. A complete active capture path proves the
//! source is `Active`. The absence of such a path over a complete graph is
//! reported as `Unknown` rather than `Inactive`, because Weftwise cannot prove
//! from this graph that no capture is happening through a path it does not
//! model. This matches the project's other positive-evidence adapters, which
//! assert activity but never assert inactivity.
//!
//! Scope: only a **direct** capture path is proven. A path is an active link
//! whose input port belongs to a running `Stream/Input/Audio` (or
//! `Stream/Input/Video`) terminal and whose output port belongs to a running,
//! non-virtual, non-monitor `Audio/Source` (or `Video/Source`) node backed by an
//! ALSA or BlueZ5 device (or a V4L2 or libcamera device). Capture that is routed
//! through one or more intermediate filter nodes (for example an echo-cancel or
//! loopback chain that inserts a virtual source between the hardware node and the
//! consumer) is a deliberate known false negative: the intermediate virtual
//! source is excluded, so such a chain stays `Unknown` rather than `Active`. This
//! keeps the module honest about what it proves and avoids classifying playback
//! monitors or virtual chains as real capture.
//!
//! The graph is size-bounded per object kind. When a bound is exceeded the graph
//! is marked overflowed: it can no longer be trusted to be complete, so a source
//! without a proven path is reported as visible uncertainty
//! (`Unavailable`/`Stale`) rather than a silent `Unknown`. Overflow is sticky for
//! the life of one connection and only clears when a fresh connection rebuilds
//! the graph.
//!
//! Readiness and trust are distinct. The graph becomes *ready* to publish only
//! after a second core-sync barrier flushes the node and link info replies that
//! binding requested; a ready graph republishes resolved states on every later
//! change. Trust is a stronger property: the second barrier completed over a
//! *complete* (non-overflowed) graph. A ready-but-incomplete graph still proves
//! an active path as `Active` while reporting absent sources as `Unavailable`,
//! and only a trusted graph treats an absent path as a silent `Unknown` or
//! degrades it to `Stale` once a later overflow makes the graph incomplete. If
//! the second barrier cannot be established at all, the graph is neither ready
//! nor trusted and every source is published as pre-trust `Unavailable`.
//!
//! The pure graph, its classification helpers, its evaluation, and the readiness
//! and reporting decisions are fully deterministic and unit tested. The transport
//! tracker at the bottom (`audio-transport` feature only) feeds real registry
//! globals into the graph and publishes typed observations. Its live behavior
//! against a running PipeWire server is not verified in a headless build and
//! remains a native proof task.

use std::collections::BTreeMap;

use crate::context::privacy::{PrivacyEvidence, PrivacyState, PrivacyUpdate};

/// Maximum capture devices retained from an untrusted server.
pub const MAX_CAPTURE_DEVICES: usize = 64;
/// Maximum capture-relevant nodes retained from an untrusted server.
pub const MAX_CAPTURE_NODES: usize = 512;
/// Maximum ports retained from an untrusted server.
pub const MAX_CAPTURE_PORTS: usize = 2_048;
/// Maximum links retained from an untrusted server.
pub const MAX_CAPTURE_LINKS: usize = 2_048;

/// Backing hardware API of a capture device (numeric classification only).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceApi {
    /// ALSA hardware, the usual local microphone backing.
    Alsa,
    /// BlueZ5 Bluetooth audio backing.
    Bluez5,
    /// Video4Linux2 camera backing.
    V4l2,
    /// libcamera camera backing.
    Libcamera,
    /// Any other or virtual backing that is not real capture hardware.
    Other,
}

impl DeviceApi {
    /// Whether this API backs a real microphone source.
    const fn is_microphone_backing(self) -> bool {
        matches!(self, Self::Alsa | Self::Bluez5)
    }

    /// Whether this API backs a real camera source.
    const fn is_camera_backing(self) -> bool {
        matches!(self, Self::V4l2 | Self::Libcamera)
    }
}

/// Role of a node in the capture graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeRole {
    /// A non-virtual, non-monitor `Audio/Source` hardware capture node.
    AudioSource,
    /// A non-virtual `Video/Source` camera capture node.
    VideoSource,
    /// A `Stream/Input/Audio` consumer terminal (an app recording audio).
    AudioInputStream,
    /// A `Stream/Input/Video` consumer terminal (an app using a camera).
    VideoInputStream,
    /// Any node that is not part of a capture path.
    Other,
}

/// Direction of a port relative to its node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortDirection {
    /// Data flows into the node through this port.
    Input,
    /// Data flows out of the node through this port.
    Output,
}

/// The capture source a graph path concerns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureKind {
    /// Microphone capture.
    Microphone,
    /// Camera capture.
    Camera,
}

impl CaptureKind {
    /// Every capture kind in deterministic order.
    pub const ALL: [Self; 2] = [Self::Microphone, Self::Camera];

    /// The privacy evidence source this kind maps to.
    #[must_use]
    pub const fn evidence(self) -> PrivacyEvidence {
        match self {
            Self::Microphone => PrivacyEvidence::Microphone,
            Self::Camera => PrivacyEvidence::Camera,
        }
    }

    /// The terminal (consumer) node role that proves this capture kind.
    const fn terminal_role(self) -> NodeRole {
        match self {
            Self::Microphone => NodeRole::AudioInputStream,
            Self::Camera => NodeRole::VideoInputStream,
        }
    }

    /// The source (producer) node role that proves this capture kind.
    const fn source_role(self) -> NodeRole {
        match self {
            Self::Microphone => NodeRole::AudioSource,
            Self::Camera => NodeRole::VideoSource,
        }
    }

    /// Whether a device API is an accepted hardware backing for this kind.
    const fn accepts_backing(self, api: DeviceApi) -> bool {
        match self {
            Self::Microphone => api.is_microphone_backing(),
            Self::Camera => api.is_camera_backing(),
        }
    }
}

/// Transient classification inputs for a node, none of which are retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NodeClassInput<'a> {
    /// The node's `media.class` string.
    pub media_class: &'a str,
    /// Whether the node is a monitor of playback rather than real capture.
    pub is_monitor: bool,
    /// Whether the node advertises itself as virtual.
    pub is_virtual: bool,
}

/// Classify a device `device.api` string into a bounded backing enum.
///
/// The match is exact against the documented PipeWire `device.api` tokens.
/// A near match such as `alsa-evil` or a decorated prefix is not accepted; it
/// classifies as `Other` and can never back a capture source.
#[must_use]
pub fn classify_device_api(api: &str) -> DeviceApi {
    match api.trim() {
        "alsa" => DeviceApi::Alsa,
        "bluez5" => DeviceApi::Bluez5,
        "v4l2" => DeviceApi::V4l2,
        "libcamera" => DeviceApi::Libcamera,
        _ => DeviceApi::Other,
    }
}

/// Classify a node into a bounded capture role from transient signals.
///
/// Matching is exact. A source role requires the exact `media.class` of
/// `Audio/Source` or `Video/Source`; decorated classes such as
/// `Audio/Source/Virtual` or `Audio/Source/Internal` are not sources. Monitor
/// and virtual nodes are additionally excluded so that recording of desktop
/// playback or echo-cancel chains is never mistaken for real capture. Consumer
/// stream terminals match the exact `Stream/Input/Audio` and
/// `Stream/Input/Video` classes.
#[must_use]
pub fn classify_node_role(input: NodeClassInput<'_>) -> NodeRole {
    match input.media_class.trim() {
        "Stream/Input/Audio" => NodeRole::AudioInputStream,
        "Stream/Input/Video" => NodeRole::VideoInputStream,
        "Audio/Source" if !input.is_monitor && !input.is_virtual => NodeRole::AudioSource,
        "Video/Source" if !input.is_monitor && !input.is_virtual => NodeRole::VideoSource,
        _ => NodeRole::Other,
    }
}

/// Whether a PipeWire boolean-valued property is enabled.
///
/// PipeWire and SPA serialize booleans as either the literal `true`/`false`
/// spelling or `1`/`0`. Treating only `true` as enabled would let a monitor or
/// virtual source (`stream.monitor` or `node.virtual` set to `"1"`) escape
/// classification and present as an active microphone. The comparison is exact
/// after trimming surrounding whitespace so an unrelated value never enables
/// the flag.
#[must_use]
pub fn pipewire_flag_enabled(value: &str) -> bool {
    matches!(value.trim(), "true" | "1")
}

/// Classify a port `port.direction` string into a bounded direction enum.
#[must_use]
pub fn classify_port_direction(direction: &str) -> Option<PortDirection> {
    match direction.trim() {
        "in" | "input" => Some(PortDirection::Input),
        "out" | "output" => Some(PortDirection::Output),
        _ => None,
    }
}

/// Whether the capture graph should be trusted once its readiness barrier is
/// reached. An overflowed graph is never trusted for that connection: it cannot
/// be shown to be complete, so its emptiness cannot prove inactivity.
#[must_use]
pub const fn should_trust_on_ready(overflowed: bool) -> bool {
    !overflowed
}

/// Resolve the published privacy state for one capture source.
///
/// A proven direct path is always `Active`. Otherwise, over a complete graph the
/// absence of a path is `Unknown` (positive-evidence default); over an overflowed
/// (incomplete) graph the absence is visible uncertainty instead of a silent
/// `Unknown`: `Stale` once the graph has been trusted, `Unavailable` before.
#[must_use]
pub const fn resolve_capture_state(
    path_active: bool,
    overflowed: bool,
    trusted: bool,
) -> PrivacyState {
    if path_active {
        PrivacyState::Active
    } else if overflowed {
        if trusted {
            PrivacyState::Stale
        } else {
            PrivacyState::Unavailable
        }
    } else {
        PrivacyState::Unknown
    }
}

/// Outcome of the capture graph's second readiness barrier.
///
/// `ready` (may publish resolved states) is modeled separately from
/// `trusted`/complete (absence may be reported as `Stale` rather than
/// `Unavailable`). An overflowed-but-flushed snapshot is ready without being
/// trusted: an active path is still shown, but a missing path stays visible
/// uncertainty instead of a silent `Unknown`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarrierOutcome {
    /// The second sync completed over a complete graph: publish and trust it.
    Trusted,
    /// The second sync completed but the graph overflowed: publish resolved
    /// states without treating absence as proof of inactivity.
    ReadyIncomplete,
    /// The second sync could not be established: neither ready nor trusted.
    Failed,
}

impl BarrierOutcome {
    /// Whether the tracker may publish resolved states after this outcome.
    #[must_use]
    pub const fn is_ready(self) -> bool {
        matches!(self, Self::Trusted | Self::ReadyIncomplete)
    }

    /// Whether absence over an incomplete graph may be reported as stale.
    #[must_use]
    pub const fn is_trusted(self) -> bool {
        matches!(self, Self::Trusted)
    }
}

/// Decide the second-barrier outcome from whether the second core sync was
/// established and whether the graph had overflowed by the time it completed.
#[must_use]
pub const fn second_barrier_outcome(sync_ok: bool, overflowed: bool) -> BarrierOutcome {
    if !sync_ok {
        BarrierOutcome::Failed
    } else if overflowed {
        BarrierOutcome::ReadyIncomplete
    } else {
        BarrierOutcome::Trusted
    }
}

/// Whether an overflow flag transitioned from clear to set. This is a reportable
/// change even though no object value was updated, so a previously trusted
/// `Unknown` does not silently persist after the graph becomes incomplete.
#[must_use]
pub const fn overflow_became_set(before: bool, after: bool) -> bool {
    !before && after
}

/// The privacy state each capture source takes when the whole connection is
/// lost: `Stale` if the just-disconnected attempt had reached a complete trusted
/// snapshot, otherwise `Unavailable`. Never `Inactive`.
#[must_use]
pub const fn capture_loss_state(trusted: bool) -> PrivacyState {
    if trusted {
        PrivacyState::Stale
    } else {
        PrivacyState::Unavailable
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DeviceRecord {
    api: DeviceApi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NodeRecord {
    role: NodeRole,
    device_id: Option<u32>,
    running: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PortRecord {
    node_id: u32,
    direction: PortDirection,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LinkRecord {
    output_port: u32,
    input_port: u32,
    active: bool,
}

/// Outcome of a bounded insert into one object map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Insert {
    /// The identity was already present with an identical value.
    Unchanged,
    /// The identity was inserted or its value changed.
    Updated,
    /// The identity was new but the map was full and it was dropped.
    Rejected,
}

/// The per-source result of evaluating the capture graph.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureEvidence {
    /// Microphone state: `Active` when a complete path exists, else `Unknown`.
    pub microphone: PrivacyState,
    /// Camera state: `Active` when a complete path exists, else `Unknown`.
    pub camera: PrivacyState,
}

impl CaptureEvidence {
    /// The state for one capture kind.
    #[must_use]
    pub const fn state(&self, kind: CaptureKind) -> PrivacyState {
        match kind {
            CaptureKind::Microphone => self.microphone,
            CaptureKind::Camera => self.camera,
        }
    }
}

/// Node and link identities released by a cascading [`CaptureGraph::remove`].
///
/// A transport tracker uses the listed ids to drop the corresponding PipeWire
/// proxies so a released dependent cannot linger or reappear as a stale slot.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemovalOutcome {
    /// Node registry ids released by the cascade.
    pub nodes: Vec<u32>,
    /// Link registry ids released by the cascade.
    pub links: Vec<u32>,
    /// Whether any object was removed.
    pub changed: bool,
}

/// Bounded, numeric-only view of the PipeWire graph for capture evidence.
///
/// The graph stores only registry identities and closed enums. It rejects growth
/// past its per-kind bounds so a hostile or runaway server cannot exhaust memory,
/// and records that rejection as overflow so an incomplete graph is never read as
/// proof of inactivity.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CaptureGraph {
    devices: BTreeMap<u32, DeviceRecord>,
    nodes: BTreeMap<u32, NodeRecord>,
    ports: BTreeMap<u32, PortRecord>,
    links: BTreeMap<u32, LinkRecord>,
    overflow: bool,
}

impl CaptureGraph {
    /// Create an empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn bounded_insert<V: PartialEq>(
        map: &mut BTreeMap<u32, V>,
        id: u32,
        value: V,
        bound: usize,
    ) -> Insert {
        match map.get(&id) {
            Some(existing) => {
                if *existing == value {
                    Insert::Unchanged
                } else {
                    map.insert(id, value);
                    Insert::Updated
                }
            }
            None => {
                if map.len() >= bound {
                    Insert::Rejected
                } else {
                    map.insert(id, value);
                    Insert::Updated
                }
            }
        }
    }

    fn record(&mut self, result: Insert) -> bool {
        if result == Insert::Rejected {
            self.overflow = true;
        }
        result == Insert::Updated
    }

    /// Insert or update a device by registry id. Returns whether it changed.
    pub fn upsert_device(&mut self, id: u32, api: DeviceApi) -> bool {
        let result = Self::bounded_insert(
            &mut self.devices,
            id,
            DeviceRecord { api },
            MAX_CAPTURE_DEVICES,
        );
        self.record(result)
    }

    /// Insert or update a node by registry id. Returns whether it changed.
    ///
    /// The running flag is preserved only across an identical re-announcement
    /// (same role and device backing). A semantic change to the role or device
    /// resets the running flag to false so freshly reclassified nodes cannot
    /// inherit a stale positive state before their next info reply.
    pub fn upsert_node(&mut self, id: u32, role: NodeRole, device_id: Option<u32>) -> bool {
        let running = match self.nodes.get(&id) {
            Some(record) if record.role == role && record.device_id == device_id => record.running,
            _ => false,
        };
        let result = Self::bounded_insert(
            &mut self.nodes,
            id,
            NodeRecord {
                role,
                device_id,
                running,
            },
            MAX_CAPTURE_NODES,
        );
        self.record(result)
    }

    /// Update a node's running flag. Returns whether it changed.
    ///
    /// A running signal for a node that has not been announced yet is ignored;
    /// the node must be classified before its activity matters.
    pub fn set_node_running(&mut self, id: u32, running: bool) -> bool {
        match self.nodes.get_mut(&id) {
            Some(record) if record.running != running => {
                record.running = running;
                true
            }
            _ => false,
        }
    }

    /// Insert or update a port by registry id. Returns whether it changed.
    pub fn upsert_port(&mut self, id: u32, node_id: u32, direction: PortDirection) -> bool {
        let result = Self::bounded_insert(
            &mut self.ports,
            id,
            PortRecord { node_id, direction },
            MAX_CAPTURE_PORTS,
        );
        self.record(result)
    }

    /// Insert or update a link by registry id. Returns whether it changed.
    ///
    /// The active flag is preserved only across an identical re-announcement
    /// (same endpoints). A change to either endpoint resets the active flag to
    /// false so a re-routed link cannot inherit a stale positive state before its
    /// next info reply.
    pub fn upsert_link(&mut self, id: u32, output_port: u32, input_port: u32) -> bool {
        let active = match self.links.get(&id) {
            Some(record)
                if record.output_port == output_port && record.input_port == input_port =>
            {
                record.active
            }
            _ => false,
        };
        let result = Self::bounded_insert(
            &mut self.links,
            id,
            LinkRecord {
                output_port,
                input_port,
                active,
            },
            MAX_CAPTURE_LINKS,
        );
        self.record(result)
    }

    /// Update a link's active flag. Returns whether it changed.
    pub fn set_link_active(&mut self, id: u32, active: bool) -> bool {
        match self.links.get_mut(&id) {
            Some(record) if record.active != active => {
                record.active = active;
                true
            }
            _ => false,
        }
    }

    /// Remove any object with this registry id. Returns whether it changed.
    ///
    /// Removal does not clear the overflow flag: once a connection's graph has
    /// been incomplete it stays untrusted until a fresh connection rebuilds it.
    pub fn remove(&mut self, id: u32) -> bool {
        self.remove_cascade(id).changed
    }

    /// Remove an object and every graph identity that depends on it.
    ///
    /// A dangling node, port, or link left after a partial removal could hold a
    /// bounded cap slot or leave a fragment that is later misread, so removal
    /// releases the whole connected capture component reachable from `id`
    /// through device-to-node, node-to-port, and port-to-link edges (and their
    /// reverses through a link's two ports). For a privacy graph an
    /// over-release is conservative: it can only move evidence toward `Unknown`,
    /// never toward a false `Inactive`. The returned [`RemovalOutcome`] lists the
    /// released node and link ids so a transport tracker can drop their proxies.
    /// The overflow flag is intentionally not cleared here.
    pub fn remove_cascade(&mut self, id: u32) -> RemovalOutcome {
        let mut outcome = RemovalOutcome::default();
        let mut queue = vec![id];
        while let Some(current) = queue.pop() {
            if self.devices.remove(&current).is_some() {
                outcome.changed = true;
                for (node_id, node) in &self.nodes {
                    if node.device_id == Some(current) {
                        queue.push(*node_id);
                    }
                }
            }
            if self.nodes.remove(&current).is_some() {
                outcome.changed = true;
                outcome.nodes.push(current);
                for (port_id, port) in &self.ports {
                    if port.node_id == current {
                        queue.push(*port_id);
                    }
                }
            }
            if let Some(port) = self.ports.remove(&current) {
                outcome.changed = true;
                // Reach the owning node and every link touching this port so a
                // capture path is released as a whole rather than in fragments.
                queue.push(port.node_id);
                for (link_id, link) in &self.links {
                    if link.output_port == current || link.input_port == current {
                        queue.push(*link_id);
                    }
                }
            }
            if let Some(link) = self.links.remove(&current) {
                outcome.changed = true;
                outcome.links.push(current);
                queue.push(link.output_port);
                queue.push(link.input_port);
            }
        }
        outcome
    }

    /// Whether a device with this id is currently retained.
    #[must_use]
    pub fn contains_device(&self, id: u32) -> bool {
        self.devices.contains_key(&id)
    }

    /// Whether a node with this id is currently retained.
    #[must_use]
    pub fn contains_node(&self, id: u32) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Whether a link with this id is currently retained.
    #[must_use]
    pub fn contains_link(&self, id: u32) -> bool {
        self.links.contains_key(&id)
    }

    /// Whether the graph dropped an object because a bound was exceeded.
    #[must_use]
    pub const fn overflowed(&self) -> bool {
        self.overflow
    }

    /// Total retained object count, for bound assertions and tests.
    #[must_use]
    pub fn len(&self) -> usize {
        self.devices.len() + self.nodes.len() + self.ports.len() + self.links.len()
    }

    /// Whether the graph retains no objects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Evaluate positive capture evidence for both sources.
    ///
    /// Each source is `Active` when a complete direct path exists and `Unknown`
    /// otherwise. Overflow handling is applied by the reporting layer via
    /// [`resolve_capture_state`], not here, so this stays a pure path predicate.
    #[must_use]
    pub fn evaluate(&self) -> CaptureEvidence {
        CaptureEvidence {
            microphone: self.evaluate_kind(CaptureKind::Microphone),
            camera: self.evaluate_kind(CaptureKind::Camera),
        }
    }

    /// Whether a complete direct path currently proves this capture kind.
    #[must_use]
    pub fn path_active(&self, kind: CaptureKind) -> bool {
        self.links
            .values()
            .filter(|link| link.active)
            .any(|link| self.link_proves_capture(link, kind))
    }

    /// Evaluate one capture kind to `Active` (proven) or `Unknown` (unproven).
    fn evaluate_kind(&self, kind: CaptureKind) -> PrivacyState {
        if self.path_active(kind) {
            PrivacyState::Active
        } else {
            PrivacyState::Unknown
        }
    }

    /// Whether one active link is a complete direct capture path for a kind.
    fn link_proves_capture(&self, link: &LinkRecord, kind: CaptureKind) -> bool {
        // The input side must terminate at a running consumer stream terminal
        // through a real input port.
        let Some(input_port) = self.ports.get(&link.input_port) else {
            return false;
        };
        if input_port.direction != PortDirection::Input {
            return false;
        }
        let Some(terminal) = self.nodes.get(&input_port.node_id) else {
            return false;
        };
        if terminal.role != kind.terminal_role() || !terminal.running {
            return false;
        }
        // The output side must originate at a running, hardware-backed source
        // node through a real output port.
        let Some(output_port) = self.ports.get(&link.output_port) else {
            return false;
        };
        if output_port.direction != PortDirection::Output {
            return false;
        }
        let Some(source) = self.nodes.get(&output_port.node_id) else {
            return false;
        };
        if source.role != kind.source_role() || !source.running {
            return false;
        }
        // A guard against a self-referential link that names the same node on
        // both ends: capture must cross from a source node to a distinct
        // terminal node.
        if input_port.node_id == output_port.node_id {
            return false;
        }
        let Some(device_id) = source.device_id else {
            return false;
        };
        self.devices
            .get(&device_id)
            .is_some_and(|device| kind.accepts_backing(device.api))
    }
}

/// One capture adapter-to-root observation, mirroring the other privacy sources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureObservation {
    /// Typed evidence update for the privacy domain.
    pub update: PrivacyUpdate,
    /// Adapter-relative observation time in milliseconds.
    pub observed_millis: u64,
}

impl CaptureObservation {
    /// A supported/unsupported declaration for one capture source.
    #[must_use]
    pub const fn supported(kind: CaptureKind, supported: bool, observed_millis: u64) -> Self {
        Self {
            update: PrivacyUpdate::Supported {
                evidence: kind.evidence(),
                supported,
            },
            observed_millis,
        }
    }

    /// A concrete observed state for one capture source.
    #[must_use]
    pub const fn observed(kind: CaptureKind, state: PrivacyState, observed_millis: u64) -> Self {
        Self {
            update: PrivacyUpdate::Observed {
                evidence: kind.evidence(),
                state,
            },
            observed_millis,
        }
    }

    /// A single capture source became unobservable.
    #[must_use]
    pub const fn unavailable(kind: CaptureKind, observed_millis: u64) -> Self {
        Self {
            update: PrivacyUpdate::Unavailable(kind.evidence()),
            observed_millis,
        }
    }
}

// ---------------------------------------------------------------------------
// Supervised transport tracker (compiled only with the audio-transport feature).
// ---------------------------------------------------------------------------

#[cfg(feature = "audio-transport")]
pub use transport::{CaptureTracker, PublishCapture};

#[cfg(feature = "audio-transport")]
mod transport {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::{Rc, Weak};
    use std::sync::Arc;
    use std::time::Instant;

    use pipewire::link::{Link, LinkListener, LinkState};
    use pipewire::node::{Node, NodeListener, NodeState};
    use pipewire::registry::{GlobalObject, RegistryRc};
    use pipewire::spa::utils::dict::DictRef;
    use pipewire::types::ObjectType;

    use super::{
        BarrierOutcome, CaptureGraph, CaptureKind, CaptureObservation, NodeClassInput, NodeRole,
        PrivacyState, classify_device_api, classify_node_role, classify_port_direction,
        overflow_became_set, pipewire_flag_enabled, resolve_capture_state, second_barrier_outcome,
    };

    /// Publisher of capture observations back to the supervised task.
    pub type PublishCapture = Arc<dyn Fn(CaptureObservation) + Send + Sync + 'static>;

    struct NodeProxy {
        _proxy: Node,
        _listener: NodeListener,
    }

    struct LinkProxy {
        _proxy: Link,
        _listener: LinkListener,
    }

    /// Owns the capture graph and the proxies whose info drives node and link
    /// state, and publishes coalesced observations once the snapshot is trusted.
    ///
    /// Proxy info listeners hold only a `Weak` reference back to the tracker so
    /// the retained proxies do not form a reference cycle that would leak the
    /// graph across reconnects. A proxy is bound and stored only for an identity
    /// the graph actually retained, so the proxy maps stay within the graph's own
    /// object bounds.
    pub struct CaptureTracker {
        graph: CaptureGraph,
        node_proxies: HashMap<u32, NodeProxy>,
        link_proxies: HashMap<u32, LinkProxy>,
        /// The second barrier has completed, so resolved states may publish.
        ready: bool,
        /// The second barrier completed over a complete graph, so an absent path
        /// may be reported as `Stale` rather than `Unavailable`.
        trusted: bool,
        last: Option<(PrivacyState, PrivacyState)>,
        publish: PublishCapture,
        started: Instant,
    }

    impl CaptureTracker {
        /// Create a tracker sharing the loop-thread publisher and clock.
        #[must_use]
        pub fn new(publish: PublishCapture, started: Instant) -> Rc<RefCell<Self>> {
            Rc::new(RefCell::new(Self {
                graph: CaptureGraph::new(),
                node_proxies: HashMap::new(),
                link_proxies: HashMap::new(),
                ready: false,
                trusted: false,
                last: None,
                publish,
                started,
            }))
        }

        fn elapsed_millis(&self) -> u64 {
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
        }

        /// Observe one registry global, updating the graph and any proxies.
        pub fn observe_global(
            this: &Rc<RefCell<Self>>,
            registry: &RegistryRc,
            global: &GlobalObject<&DictRef>,
        ) {
            let changed = match global.type_ {
                ObjectType::Device => Self::observe_device(this, global),
                ObjectType::Node => Self::observe_node(this, registry, global),
                ObjectType::Port => Self::observe_port(this, global),
                ObjectType::Link => Self::observe_link(this, registry, global),
                _ => false,
            };
            if changed {
                Self::republish(this);
            }
        }

        fn observe_device(this: &Rc<RefCell<Self>>, global: &GlobalObject<&DictRef>) -> bool {
            let Some(props) = global.props else {
                return false;
            };
            let Some(api) = props.get("device.api") else {
                return false;
            };
            let mut tracker = this.borrow_mut();
            let before = tracker.graph.overflowed();
            let changed = tracker
                .graph
                .upsert_device(global.id, classify_device_api(api));
            changed || overflow_became_set(before, tracker.graph.overflowed())
        }

        fn observe_node(
            this: &Rc<RefCell<Self>>,
            registry: &RegistryRc,
            global: &GlobalObject<&DictRef>,
        ) -> bool {
            let Some(props) = global.props else {
                return false;
            };
            let Some(media_class) = props.get("media.class") else {
                return false;
            };
            let is_monitor = props
                .get("stream.monitor")
                .is_some_and(pipewire_flag_enabled)
                || props
                    .get("node.name")
                    .is_some_and(|name| name.ends_with(".monitor"));
            let is_virtual = props.get("node.virtual").is_some_and(pipewire_flag_enabled);
            let role = classify_node_role(NodeClassInput {
                media_class,
                is_monitor,
                is_virtual,
            });
            if role == NodeRole::Other {
                return false;
            }
            let device_id = props
                .get("device.id")
                .and_then(|value| value.trim().parse::<u32>().ok());
            let (changed, retained) = {
                let mut tracker = this.borrow_mut();
                let before = tracker.graph.overflowed();
                let changed = tracker.graph.upsert_node(global.id, role, device_id);
                let changed = changed || overflow_became_set(before, tracker.graph.overflowed());
                (changed, tracker.graph.contains_node(global.id))
            };
            // Bind a proxy only for an identity the bounded graph actually kept,
            // so a rejected overflow node never leaks an unbounded proxy.
            if retained {
                Self::bind_node(this, registry, global.id, global);
            }
            changed
        }

        fn observe_port(this: &Rc<RefCell<Self>>, global: &GlobalObject<&DictRef>) -> bool {
            let Some(props) = global.props else {
                return false;
            };
            let Some(direction) = props
                .get("port.direction")
                .and_then(classify_port_direction)
            else {
                return false;
            };
            let Some(node_id) = props
                .get("node.id")
                .and_then(|value| value.trim().parse::<u32>().ok())
            else {
                return false;
            };
            let mut tracker = this.borrow_mut();
            let before = tracker.graph.overflowed();
            let changed = tracker.graph.upsert_port(global.id, node_id, direction);
            changed || overflow_became_set(before, tracker.graph.overflowed())
        }

        fn observe_link(
            this: &Rc<RefCell<Self>>,
            registry: &RegistryRc,
            global: &GlobalObject<&DictRef>,
        ) -> bool {
            let Some(props) = global.props else {
                return false;
            };
            let Some(output_port) = props
                .get("link.output.port")
                .and_then(|value| value.trim().parse::<u32>().ok())
            else {
                return false;
            };
            let Some(input_port) = props
                .get("link.input.port")
                .and_then(|value| value.trim().parse::<u32>().ok())
            else {
                return false;
            };
            let (changed, retained) = {
                let mut tracker = this.borrow_mut();
                let before = tracker.graph.overflowed();
                let changed = tracker
                    .graph
                    .upsert_link(global.id, output_port, input_port);
                let changed = changed || overflow_became_set(before, tracker.graph.overflowed());
                (changed, tracker.graph.contains_link(global.id))
            };
            // Bind a proxy only for an identity the bounded graph actually kept.
            if retained {
                Self::bind_link(this, registry, global.id, global);
            }
            changed
        }

        fn bind_node(
            this: &Rc<RefCell<Self>>,
            registry: &RegistryRc,
            id: u32,
            global: &GlobalObject<&DictRef>,
        ) {
            if this.borrow().node_proxies.contains_key(&id) {
                return;
            }
            let Ok(proxy) = registry.bind::<Node, _>(global) else {
                return;
            };
            let weak: Weak<RefCell<Self>> = Rc::downgrade(this);
            let listener = proxy
                .add_listener_local()
                .info(move |info| {
                    let Some(this) = weak.upgrade() else {
                        return;
                    };
                    let running = matches!(info.state(), NodeState::Running);
                    let changed = this.borrow_mut().graph.set_node_running(id, running);
                    if changed {
                        Self::republish(&this);
                    }
                })
                .register();
            this.borrow_mut().node_proxies.insert(
                id,
                NodeProxy {
                    _proxy: proxy,
                    _listener: listener,
                },
            );
        }

        fn bind_link(
            this: &Rc<RefCell<Self>>,
            registry: &RegistryRc,
            id: u32,
            global: &GlobalObject<&DictRef>,
        ) {
            if this.borrow().link_proxies.contains_key(&id) {
                return;
            }
            let Ok(proxy) = registry.bind::<Link, _>(global) else {
                return;
            };
            let weak: Weak<RefCell<Self>> = Rc::downgrade(this);
            let listener = proxy
                .add_listener_local()
                .info(move |info| {
                    let Some(this) = weak.upgrade() else {
                        return;
                    };
                    let active = matches!(info.state(), LinkState::Active);
                    let changed = this.borrow_mut().graph.set_link_active(id, active);
                    if changed {
                        Self::republish(&this);
                    }
                })
                .register();
            this.borrow_mut().link_proxies.insert(
                id,
                LinkProxy {
                    _proxy: proxy,
                    _listener: listener,
                },
            );
        }

        /// Observe removal of a registry global by id.
        ///
        /// The cascade releases dependent nodes and links, so their proxies are
        /// dropped here too; a released dependent must not keep a listener alive
        /// or hold a bounded slot.
        pub fn observe_remove(this: &Rc<RefCell<Self>>, id: u32) {
            let changed = {
                let mut tracker = this.borrow_mut();
                let outcome = tracker.graph.remove_cascade(id);
                tracker.node_proxies.remove(&id);
                tracker.link_proxies.remove(&id);
                for node in &outcome.nodes {
                    tracker.node_proxies.remove(node);
                }
                for link in &outcome.links {
                    tracker.link_proxies.remove(link);
                }
                outcome.changed
            };
            if changed {
                Self::republish(this);
            }
        }

        /// Complete the second readiness barrier and report whether the attempt
        /// became trusted.
        ///
        /// This runs only after the first core sync established registry
        /// enumeration and the second core sync flushed the node and link info
        /// replies requested by binding. `sync_ok` is whether that second sync
        /// was established at all. The outcome decides three distinct states:
        ///
        /// - `Failed` (second sync could not be issued): the graph is neither
        ///   ready nor trusted, and every source is published as pre-trust
        ///   `Unavailable` uncertainty so nothing silently implies inactivity.
        /// - `ReadyIncomplete` (flushed but the graph overflowed): the tracker
        ///   becomes ready but not trusted, so it publishes and keeps republishing
        ///   resolved states - an active path stays `Active`, an absent path is
        ///   `Unavailable` - and only a fresh connection can restore trust.
        /// - `Trusted` (flushed over a complete graph): ready and trusted, so an
        ///   absent path is a silent `Unknown` and a later overflow degrades an
        ///   absent path to `Stale`.
        ///
        /// Returns whether the attempt is trusted, so the supervisor can decide
        /// `Stale` versus `Unavailable` if the whole connection is later lost.
        pub fn finish_barrier(this: &Rc<RefCell<Self>>, sync_ok: bool) -> bool {
            let overflowed = this.borrow().graph.overflowed();
            let outcome = second_barrier_outcome(sync_ok, overflowed);
            {
                let mut tracker = this.borrow_mut();
                tracker.ready = outcome.is_ready();
                tracker.trusted = outcome.is_trusted();
                tracker.last = None;
            }
            match outcome {
                BarrierOutcome::Failed => Self::publish_states(
                    this,
                    (PrivacyState::Unavailable, PrivacyState::Unavailable),
                ),
                BarrierOutcome::ReadyIncomplete | BarrierOutcome::Trusted => Self::republish(this),
            }
            outcome.is_trusted()
        }

        /// Publish an explicit per-source state pair regardless of readiness.
        ///
        /// Used for the `Failed` barrier outcome, where the graph is not ready to
        /// evaluate but the sources must still be surfaced as uncertainty.
        fn publish_states(this: &Rc<RefCell<Self>>, states: (PrivacyState, PrivacyState)) {
            let (publish, observations) = {
                let mut tracker = this.borrow_mut();
                let observed_millis = tracker.elapsed_millis();
                let observations = tracker.delta_observations(states, observed_millis);
                (tracker.publish.clone(), observations)
            };
            for observation in observations {
                (publish)(observation);
            }
        }

        fn republish(this: &Rc<RefCell<Self>>) {
            let (publish, observations) = {
                let mut tracker = this.borrow_mut();
                if !tracker.ready {
                    return;
                }
                let overflowed = tracker.graph.overflowed();
                let trusted = tracker.trusted;
                let mic = resolve_capture_state(
                    tracker.graph.path_active(CaptureKind::Microphone),
                    overflowed,
                    trusted,
                );
                let camera = resolve_capture_state(
                    tracker.graph.path_active(CaptureKind::Camera),
                    overflowed,
                    trusted,
                );
                let observed_millis = tracker.elapsed_millis();
                let observations = tracker.delta_observations((mic, camera), observed_millis);
                (tracker.publish.clone(), observations)
            };
            for observation in observations {
                (publish)(observation);
            }
        }

        /// Re-affirm the current per-source states unconditionally.
        ///
        /// Unlike the delta republish, this emits an observation for every
        /// supported source even when the state is unchanged, so the root's
        /// evidence age is refreshed on a bounded interval and a continuously
        /// active or unknown source is never aged to false uncertainty. The
        /// root treats an unchanged state as a no-op re-affirm. Nothing is
        /// emitted before the graph is ready, matching the pre-trust states,
        /// which are failure states the root does not age.
        pub fn reaffirm(this: &Rc<RefCell<Self>>) {
            let (publish, observations) = {
                let tracker = this.borrow();
                if !tracker.ready {
                    return;
                }
                let overflowed = tracker.graph.overflowed();
                let trusted = tracker.trusted;
                let observed_millis = tracker.elapsed_millis();
                let observation = |kind: CaptureKind| {
                    CaptureObservation::observed(
                        kind,
                        resolve_capture_state(tracker.graph.path_active(kind), overflowed, trusted),
                        observed_millis,
                    )
                };
                (
                    tracker.publish.clone(),
                    vec![
                        observation(CaptureKind::Microphone),
                        observation(CaptureKind::Camera),
                    ],
                )
            };
            for observation in observations {
                (publish)(observation);
            }
        }

        /// Compute the per-source observations that changed since the last
        /// publish and record the new states.
        fn delta_observations(
            &mut self,
            states: (PrivacyState, PrivacyState),
            observed_millis: u64,
        ) -> Vec<CaptureObservation> {
            let mut observations = Vec::new();
            let (mic, camera) = states;
            let previous = self.last;
            if previous.map(|(value, _)| value) != Some(mic) {
                observations.push(CaptureObservation::observed(
                    CaptureKind::Microphone,
                    mic,
                    observed_millis,
                ));
            }
            if previous.map(|(_, value)| value) != Some(camera) {
                observations.push(CaptureObservation::observed(
                    CaptureKind::Camera,
                    camera,
                    observed_millis,
                ));
            }
            self.last = Some((mic, camera));
            observations
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "audio-transport")]
    #[test]
    fn reaffirm_emits_current_evidence_for_both_sources() {
        use std::sync::{Arc, Mutex};

        let sink: Arc<Mutex<Vec<CaptureObservation>>> = Arc::new(Mutex::new(Vec::new()));
        let collected = Arc::clone(&sink);
        let publish: PublishCapture =
            Arc::new(move |observation| collected.lock().unwrap().push(observation));
        let tracker = CaptureTracker::new(publish, std::time::Instant::now());
        // Reach a trusted, ready graph, then clear the barrier's own emissions.
        CaptureTracker::finish_barrier(&tracker, true);
        sink.lock().unwrap().clear();

        CaptureTracker::reaffirm(&tracker);
        let observations = sink.lock().unwrap();
        // The re-affirm tick re-emits an observation for each supported source
        // even though nothing changed, refreshing the root's evidence age.
        assert_eq!(observations.len(), 2);
        let kinds: Vec<_> = observations
            .iter()
            .map(|observation| match observation.update {
                PrivacyUpdate::Observed { evidence, .. } => evidence,
                other => panic!("expected an Observed re-affirm, got {other:?}"),
            })
            .collect();
        assert!(kinds.contains(&CaptureKind::Microphone.evidence()));
        assert!(kinds.contains(&CaptureKind::Camera.evidence()));
    }

    fn microphone_ready_graph() -> CaptureGraph {
        // A complete, running microphone path:
        // ALSA device 10 -> source node 11 (out port 12) -> active link 13
        //   -> input port 14 -> Stream/Input/Audio terminal node 15.
        let mut graph = CaptureGraph::new();
        assert!(graph.upsert_device(10, DeviceApi::Alsa));
        assert!(graph.upsert_node(11, NodeRole::AudioSource, Some(10)));
        assert!(graph.upsert_port(12, 11, PortDirection::Output));
        assert!(graph.upsert_node(15, NodeRole::AudioInputStream, None));
        assert!(graph.upsert_port(14, 15, PortDirection::Input));
        assert!(graph.upsert_link(13, 12, 14));
        graph.set_node_running(11, true);
        graph.set_node_running(15, true);
        graph.set_link_active(13, true);
        graph
    }

    #[test]
    fn classify_device_api_maps_exact_backings_only() {
        assert_eq!(classify_device_api("alsa"), DeviceApi::Alsa);
        assert_eq!(classify_device_api(" bluez5 "), DeviceApi::Bluez5);
        assert_eq!(classify_device_api("v4l2"), DeviceApi::V4l2);
        assert_eq!(classify_device_api("libcamera"), DeviceApi::Libcamera);
        // Near matches, prefixes, and decorated variants are not hardware.
        assert_eq!(classify_device_api("alsa-evil"), DeviceApi::Other);
        assert_eq!(classify_device_api("bluez"), DeviceApi::Other);
        assert_eq!(classify_device_api("v4l2loopback-fake"), DeviceApi::Other);
        assert_eq!(classify_device_api("null-audio"), DeviceApi::Other);
    }

    #[test]
    fn pipewire_flag_accepts_true_and_one_only() {
        for enabled in ["true", "1", " true ", "\t1\n"] {
            assert!(pipewire_flag_enabled(enabled), "{enabled:?} must enable");
        }
        for disabled in ["false", "0", "", "yes", "TRUE", "11", "1.0"] {
            assert!(
                !pipewire_flag_enabled(disabled),
                "{disabled:?} must not enable"
            );
        }
    }

    #[test]
    fn monitor_source_with_string_one_is_not_a_microphone() {
        // A monitor source advertising `stream.monitor = "1"` must classify as
        // Other, never an audio source, so it cannot present as a live mic.
        assert_eq!(
            classify_node_role(NodeClassInput {
                media_class: "Audio/Source",
                is_monitor: pipewire_flag_enabled("1"),
                is_virtual: false,
            }),
            NodeRole::Other
        );
    }

    #[test]
    fn classify_node_role_requires_exact_source_class() {
        let base = NodeClassInput {
            media_class: "Audio/Source",
            is_monitor: false,
            is_virtual: false,
        };
        assert_eq!(classify_node_role(base), NodeRole::AudioSource);
        // Monitor and virtual flags exclude an otherwise-exact source.
        assert_eq!(
            classify_node_role(NodeClassInput {
                is_monitor: true,
                ..base
            }),
            NodeRole::Other
        );
        assert_eq!(
            classify_node_role(NodeClassInput {
                is_virtual: true,
                ..base
            }),
            NodeRole::Other
        );
        // Decorated and near-match classes are not sources.
        for class in [
            "Audio/Source/Virtual",
            "Audio/Source/Internal",
            "Audio/Source/",
            "XAudio/Source",
            "Audio/SourceX",
            "Audio/Sink",
        ] {
            assert_eq!(
                classify_node_role(NodeClassInput {
                    media_class: class,
                    is_monitor: false,
                    is_virtual: false,
                }),
                NodeRole::Other,
                "class {class} must not be a source"
            );
        }
    }

    #[test]
    fn classify_node_role_matches_exact_stream_terminals() {
        assert_eq!(
            classify_node_role(NodeClassInput {
                media_class: "Stream/Input/Audio",
                is_monitor: false,
                is_virtual: false,
            }),
            NodeRole::AudioInputStream
        );
        assert_eq!(
            classify_node_role(NodeClassInput {
                media_class: "Stream/Input/Video",
                is_monitor: false,
                is_virtual: false,
            }),
            NodeRole::VideoInputStream
        );
        // An output stream (playback) and a decorated terminal are not capture
        // terminals.
        for class in [
            "Stream/Output/Audio",
            "Stream/Input/Audio/Extra",
            "Stream/Input/Midi",
        ] {
            assert_eq!(
                classify_node_role(NodeClassInput {
                    media_class: class,
                    is_monitor: false,
                    is_virtual: false,
                }),
                NodeRole::Other,
                "class {class} must not be a terminal"
            );
        }
    }

    #[test]
    fn classify_port_direction_accepts_known_words() {
        assert_eq!(classify_port_direction("in"), Some(PortDirection::Input));
        assert_eq!(classify_port_direction("out"), Some(PortDirection::Output));
        assert_eq!(classify_port_direction("sideways"), None);
    }

    #[test]
    fn complete_running_path_is_active() {
        let graph = microphone_ready_graph();
        assert_eq!(graph.evaluate().microphone, PrivacyState::Active);
        // No camera path exists, so the camera stays Unknown, never Inactive.
        assert_eq!(graph.evaluate().camera, PrivacyState::Unknown);
        assert!(graph.path_active(CaptureKind::Microphone));
        assert!(!graph.path_active(CaptureKind::Camera));
        assert!(!graph.overflowed());
    }

    #[test]
    fn absent_path_is_unknown_not_inactive() {
        let graph = CaptureGraph::new();
        assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
        assert_eq!(graph.evaluate().camera, PrivacyState::Unknown);
    }

    #[test]
    fn inactive_link_does_not_prove_capture() {
        let mut graph = microphone_ready_graph();
        assert!(graph.set_link_active(13, false));
        assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
    }

    #[test]
    fn source_must_be_running() {
        let mut graph = microphone_ready_graph();
        assert!(graph.set_node_running(11, false));
        assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
    }

    #[test]
    fn terminal_must_be_running() {
        let mut graph = microphone_ready_graph();
        assert!(graph.set_node_running(15, false));
        assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
    }

    #[test]
    fn virtual_backed_source_is_not_capture() {
        let mut graph = microphone_ready_graph();
        // Repoint the source device to a non-hardware backing.
        assert!(graph.upsert_device(10, DeviceApi::Other));
        assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
    }

    #[test]
    fn source_without_device_backing_is_not_capture() {
        let mut graph = CaptureGraph::new();
        graph.upsert_node(11, NodeRole::AudioSource, None);
        graph.upsert_port(12, 11, PortDirection::Output);
        graph.upsert_node(15, NodeRole::AudioInputStream, None);
        graph.upsert_port(14, 15, PortDirection::Input);
        graph.upsert_link(13, 12, 14);
        graph.set_node_running(11, true);
        graph.set_node_running(15, true);
        graph.set_link_active(13, true);
        assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
    }

    #[test]
    fn reversed_port_directions_do_not_prove_capture() {
        let mut graph = CaptureGraph::new();
        graph.upsert_device(10, DeviceApi::Alsa);
        graph.upsert_node(11, NodeRole::AudioSource, Some(10));
        graph.upsert_node(15, NodeRole::AudioInputStream, None);
        // Ports deliberately reversed: source has an input port, terminal an
        // output port, so the link cannot be a capture path.
        graph.upsert_port(12, 11, PortDirection::Input);
        graph.upsert_port(14, 15, PortDirection::Output);
        graph.upsert_link(13, 12, 14);
        graph.set_node_running(11, true);
        graph.set_node_running(15, true);
        graph.set_link_active(13, true);
        assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
    }

    #[test]
    fn wrong_kind_terminal_does_not_cross_sources() {
        // A running video path must not surface as microphone activity.
        let mut graph = CaptureGraph::new();
        graph.upsert_device(20, DeviceApi::V4l2);
        graph.upsert_node(21, NodeRole::VideoSource, Some(20));
        graph.upsert_port(22, 21, PortDirection::Output);
        graph.upsert_node(25, NodeRole::VideoInputStream, None);
        graph.upsert_port(24, 25, PortDirection::Input);
        graph.upsert_link(23, 22, 24);
        graph.set_node_running(21, true);
        graph.set_node_running(25, true);
        graph.set_link_active(23, true);
        let evidence = graph.evaluate();
        assert_eq!(evidence.camera, PrivacyState::Active);
        assert_eq!(evidence.microphone, PrivacyState::Unknown);
    }

    #[test]
    fn bluetooth_backed_microphone_is_active() {
        let mut graph = microphone_ready_graph();
        assert!(graph.upsert_device(10, DeviceApi::Bluez5));
        assert_eq!(graph.evaluate().microphone, PrivacyState::Active);
    }

    #[test]
    fn removal_of_link_releases_active_evidence() {
        let mut graph = microphone_ready_graph();
        assert_eq!(graph.evaluate().microphone, PrivacyState::Active);
        assert!(graph.remove(13));
        assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
    }

    #[test]
    fn removing_any_component_member_releases_the_whole_path() {
        // device 1 - node 2 - port 4 -> link 6 -> port 5 - node 3
        let build = || {
            let mut graph = CaptureGraph::new();
            graph.upsert_device(1, DeviceApi::Alsa);
            graph.upsert_node(2, NodeRole::AudioSource, Some(1));
            graph.upsert_node(3, NodeRole::AudioInputStream, None);
            graph.upsert_port(4, 2, PortDirection::Output);
            graph.upsert_port(5, 3, PortDirection::Input);
            graph.upsert_link(6, 4, 5);
            graph
        };
        // Removing the backing device releases the whole path.
        let mut graph = build();
        let outcome = graph.remove_cascade(1);
        assert!(outcome.changed);
        assert!(graph.is_empty(), "removing the device must empty the path");
        let mut nodes = outcome.nodes.clone();
        nodes.sort_unstable();
        assert_eq!(nodes, vec![2, 3]);
        assert_eq!(outcome.links, vec![6]);

        // Removing a non-device member releases the connected stream chain
        // (both nodes, ports, and the link) but leaves the backing device,
        // which may still front other nodes.
        for member in [2, 3, 4, 5, 6] {
            let mut graph = build();
            assert!(
                graph.remove(member),
                "removing {member} must change the graph"
            );
            assert!(
                graph.contains_device(1),
                "removing {member} must retain the backing device"
            );
            assert!(
                !graph.contains_node(2) && !graph.contains_node(3) && !graph.contains_link(6),
                "removing {member} must release the whole stream chain"
            );
        }
    }

    #[test]
    fn repeated_hotplug_below_cap_never_marks_overflow() {
        let mut graph = CaptureGraph::new();
        for _ in 0..8 {
            graph.upsert_device(1, DeviceApi::Alsa);
            graph.upsert_node(2, NodeRole::AudioSource, Some(1));
            graph.upsert_port(4, 2, PortDirection::Output);
            assert!(graph.remove(1));
            assert!(graph.is_empty());
        }
        assert!(!graph.overflowed());
    }

    #[test]
    fn self_looping_link_is_rejected() {
        // A link whose ports both resolve to the same node is not a crossing
        // capture path and must not prove activity.
        let mut graph = CaptureGraph::new();
        graph.upsert_device(10, DeviceApi::Alsa);
        graph.upsert_node(11, NodeRole::AudioInputStream, Some(10));
        graph.upsert_port(12, 11, PortDirection::Output);
        graph.upsert_port(14, 11, PortDirection::Input);
        graph.upsert_link(13, 12, 14);
        graph.set_node_running(11, true);
        graph.set_link_active(13, true);
        assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
    }

    #[test]
    fn device_bound_is_enforced_and_marks_overflow() {
        let mut graph = CaptureGraph::new();
        for id in 0..MAX_CAPTURE_DEVICES as u32 {
            assert!(graph.upsert_device(id, DeviceApi::Alsa));
        }
        assert!(!graph.overflowed());
        // One past the bound with a fresh id is rejected and records overflow.
        assert!(!graph.upsert_device(MAX_CAPTURE_DEVICES as u32, DeviceApi::Alsa));
        assert!(graph.overflowed());
        // Updating an existing id in place is still allowed.
        assert!(graph.upsert_device(0, DeviceApi::Bluez5));
    }

    #[test]
    fn node_and_link_bounds_reject_and_do_not_retain() {
        let mut nodes = CaptureGraph::new();
        for id in 0..MAX_CAPTURE_NODES as u32 {
            assert!(nodes.upsert_node(id, NodeRole::AudioSource, Some(0)));
        }
        let overflow_node = MAX_CAPTURE_NODES as u32;
        assert!(!nodes.upsert_node(overflow_node, NodeRole::AudioSource, Some(0)));
        assert!(!nodes.contains_node(overflow_node));
        assert!(nodes.overflowed());

        let mut links = CaptureGraph::new();
        for id in 0..MAX_CAPTURE_LINKS as u32 {
            assert!(links.upsert_link(id, 1, 2));
        }
        let overflow_link = MAX_CAPTURE_LINKS as u32;
        assert!(!links.upsert_link(overflow_link, 1, 2));
        assert!(!links.contains_link(overflow_link));
        assert!(links.overflowed());
    }

    #[test]
    fn overflow_is_sticky_across_removal() {
        let mut graph = CaptureGraph::new();
        for id in 0..MAX_CAPTURE_DEVICES as u32 {
            graph.upsert_device(id, DeviceApi::Alsa);
        }
        assert!(!graph.upsert_device(MAX_CAPTURE_DEVICES as u32, DeviceApi::Alsa));
        assert!(graph.overflowed());
        // Removing objects does not restore trust within the connection.
        assert!(graph.remove(0));
        assert!(graph.overflowed());
    }

    #[test]
    fn running_signal_before_announcement_is_ignored() {
        let mut graph = CaptureGraph::new();
        // No node with this id yet.
        assert!(!graph.set_node_running(11, true));
        // After announcement the running flag applies.
        graph.upsert_node(11, NodeRole::AudioSource, Some(10));
        assert!(graph.set_node_running(11, true));
    }

    #[test]
    fn identical_node_reannounce_preserves_running_flag() {
        let mut graph = microphone_ready_graph();
        // A duplicate registry announcement with identical classification does
        // not clobber the previously observed running flag.
        assert!(!graph.upsert_node(11, NodeRole::AudioSource, Some(10)));
        assert_eq!(graph.evaluate().microphone, PrivacyState::Active);
    }

    #[test]
    fn node_reclassification_resets_running_flag() {
        let mut graph = microphone_ready_graph();
        assert_eq!(graph.evaluate().microphone, PrivacyState::Active);
        // A second valid hardware backing exists for the re-pointed source.
        assert!(graph.upsert_device(99, DeviceApi::Alsa));
        // Reusing the source id for a different device backing must not inherit
        // the old running flag as a false Active before fresh info arrives.
        assert!(graph.upsert_node(11, NodeRole::AudioSource, Some(99)));
        assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
        // A fresh running signal re-establishes the path.
        assert!(graph.set_node_running(11, true));
        assert_eq!(graph.evaluate().microphone, PrivacyState::Active);
    }

    #[test]
    fn link_reroute_resets_active_flag() {
        let mut graph = microphone_ready_graph();
        assert_eq!(graph.evaluate().microphone, PrivacyState::Active);
        // Re-route the link to different ports; the active flag must reset so a
        // moved link cannot carry its old positive state.
        graph.upsert_port(16, 11, PortDirection::Output);
        assert!(graph.upsert_link(13, 16, 14));
        assert_eq!(graph.evaluate().microphone, PrivacyState::Unknown);
        assert!(graph.set_link_active(13, true));
        assert_eq!(graph.evaluate().microphone, PrivacyState::Active);
    }

    #[test]
    fn resolve_capture_state_covers_overflow_and_trust() {
        // A proven path is always active regardless of overflow or trust.
        assert_eq!(
            resolve_capture_state(true, true, false),
            PrivacyState::Active
        );
        // Complete graph, no path: silent Unknown is correct.
        assert_eq!(
            resolve_capture_state(false, false, true),
            PrivacyState::Unknown
        );
        // Overflowed graph, no path: visible uncertainty, not Unknown.
        assert_eq!(
            resolve_capture_state(false, true, true),
            PrivacyState::Stale
        );
        assert_eq!(
            resolve_capture_state(false, true, false),
            PrivacyState::Unavailable
        );
    }

    #[test]
    fn should_trust_on_ready_refuses_incomplete_snapshot() {
        assert!(should_trust_on_ready(false));
        assert!(!should_trust_on_ready(true));
    }

    #[test]
    fn second_barrier_outcome_separates_ready_from_trusted() {
        // A failed second sync is neither ready nor trusted.
        let failed = second_barrier_outcome(false, false);
        assert_eq!(failed, BarrierOutcome::Failed);
        assert!(!failed.is_ready());
        assert!(!failed.is_trusted());
        // A failed sync stays failed regardless of the overflow flag.
        assert_eq!(second_barrier_outcome(false, true), BarrierOutcome::Failed);
        // A flushed but overflowed graph is ready but not trusted.
        let incomplete = second_barrier_outcome(true, true);
        assert_eq!(incomplete, BarrierOutcome::ReadyIncomplete);
        assert!(incomplete.is_ready());
        assert!(!incomplete.is_trusted());
        // A flushed complete graph is ready and trusted.
        let trusted = second_barrier_outcome(true, false);
        assert_eq!(trusted, BarrierOutcome::Trusted);
        assert!(trusted.is_ready());
        assert!(trusted.is_trusted());
    }

    #[test]
    fn overflow_became_set_reports_only_the_rising_edge() {
        assert!(overflow_became_set(false, true));
        assert!(!overflow_became_set(false, false));
        assert!(!overflow_became_set(true, true));
        assert!(!overflow_became_set(true, false));
    }

    #[test]
    fn capture_loss_state_is_stale_only_after_trust() {
        assert_eq!(capture_loss_state(true), PrivacyState::Stale);
        assert_eq!(capture_loss_state(false), PrivacyState::Unavailable);
    }

    #[test]
    fn incomplete_ready_graph_still_proves_an_active_path() {
        // A path can be proven even while the graph is overflowed; the reporting
        // layer keeps that Active while surfacing absent sources as uncertainty.
        let mut graph = microphone_ready_graph();
        for id in 100..(100 + MAX_CAPTURE_DEVICES as u32) {
            graph.upsert_device(id, DeviceApi::Other);
        }
        assert!(graph.overflowed());
        assert!(graph.path_active(CaptureKind::Microphone));
        // ReadyIncomplete + trusted=false: the active mic stays Active, the
        // absent camera is Unavailable (visible uncertainty), never Unknown.
        assert_eq!(
            resolve_capture_state(graph.path_active(CaptureKind::Microphone), true, false),
            PrivacyState::Active
        );
        assert_eq!(
            resolve_capture_state(graph.path_active(CaptureKind::Camera), true, false),
            PrivacyState::Unavailable
        );
    }

    #[test]
    fn observation_builders_map_to_privacy_updates() {
        let supported = CaptureObservation::supported(CaptureKind::Microphone, true, 5);
        assert_eq!(
            supported.update,
            PrivacyUpdate::Supported {
                evidence: PrivacyEvidence::Microphone,
                supported: true,
            }
        );
        let observed = CaptureObservation::observed(CaptureKind::Camera, PrivacyState::Active, 6);
        assert_eq!(
            observed.update,
            PrivacyUpdate::Observed {
                evidence: PrivacyEvidence::Camera,
                state: PrivacyState::Active,
            }
        );
        let unavailable = CaptureObservation::unavailable(CaptureKind::Camera, 7);
        assert_eq!(
            unavailable.update,
            PrivacyUpdate::Unavailable(PrivacyEvidence::Camera)
        );
    }
}
