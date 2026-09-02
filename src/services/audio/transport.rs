//! Supervised PipeWire transport for the audio adapter.
//!
//! Compiled only with the `audio-transport` feature. This module owns the
//! dedicated PipeWire loop thread, binds registry globals, tracks bounded audio
//! nodes plus movable playback streams, and cooperates with WirePlumber through
//! its `default` metadata for default-node selection and stream movement. It
//! never polls a `wpctl` subprocess and holds no `unsafe` project code.
//!
//! Native runtime behavior against a live PipeWire server is not verified in a
//! headless build environment; the bindings are validated at compile time and
//! the pure decision helpers live beside the model in the parent module.

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
    Generation, MAX_AUDIO_NAME_CHARACTERS, MAX_AUDIO_NODES, MAX_MOVABLE_STREAMS,
    MovableStreamCandidate, MovableStreamState, MoveEnablement, MoveMovingPolicy,
    TARGET_OBJECT_METADATA_KEY, TARGET_OBJECT_METADATA_TYPE, build_props_pod, default_metadata_key,
    default_metadata_value, is_movable_stream_class, metadata_permits_target_write, move_is_fresh,
    parse_allow_moving_streams, parse_props_pod, select_movable_stream,
    should_bind_default_metadata, should_clear_default_metadata, subject_permits_metadata,
    target_object_metadata_value,
};
use crate::services::capture::{
    CaptureKind, CaptureObservation, CaptureTracker, PublishCapture, capture_loss_state,
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
    /// Whether the bound object grants the write and execute permission a
    /// foreign-subject `target.object` write requires. When false, the move
    /// action is disabled rather than presented as if it could succeed.
    writable: bool,
    _listener: pipewire::metadata::MetadataListener,
}

/// Shared slot for the single bound `default` metadata handle.
type DefaultMetadata = Rc<RefCell<Option<DefaultMetadataHandle>>>;

/// The bound `sm-settings` metadata proxy retained to read session policy.
struct SmSettingsHandle {
    global_id: u32,
    _proxy: Metadata,
    _listener: pipewire::metadata::MetadataListener,
}

/// Shared slot for the single bound `sm-settings` metadata handle.
type SmSettings = Rc<RefCell<Option<SmSettingsHandle>>>;

/// A bound movable playback stream tracked for the move action.
struct StreamEntry {
    /// Whether the node reported the running state.
    running: bool,
    /// Whether movement is permitted (not `node.dont-move`).
    movable: bool,
    /// Whether the subject grants `PW_PERM_M` for a `target.object` write.
    metadata_permission: bool,
    _proxy: pipewire::node::Node,
    _listener: pipewire::node::NodeListener,
}

/// Bounded inventory of `Stream/Output/Audio` playback streams.
type Streams = Rc<RefCell<HashMap<u32, StreamEntry>>>;

type Publish = Arc<dyn Fn(AudioUpdate) + Send + Sync + 'static>;

/// Shared loop handles for movable-stream tracking and the move action.
///
/// Grouped so the registry callbacks and the command dispatcher can share the
/// same bounded stream inventory, session policy, metadata handle, and last
/// published selection without an unwieldy argument list.
#[derive(Clone)]
struct MoveContext {
    streams: Streams,
    metadata: DefaultMetadata,
    sm_settings: SmSettings,
    policy: Rc<Cell<MoveMovingPolicy>>,
    /// Set once the bounded stream inventory rejects an id; disables the action
    /// for this connection so an incomplete inventory cannot look unambiguous.
    overflow: Rc<Cell<bool>>,
    /// The last published selection, used to publish only on change.
    last: Rc<Cell<MovableStreamState>>,
    synced: Rc<Cell<bool>>,
    /// The generation of this connection attempt. Every published selection is
    /// stamped with it, and a dispatched move must still match it to apply.
    generation: Generation,
    publish: Publish,
}

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
    publish_capture: impl Fn(CaptureObservation) + Send + Sync + 'static,
    mut commands: mpsc::Receiver<AudioCommand>,
    mut cancellation: Cancellation,
) {
    let publish: Publish = Arc::new(publish);
    let capture_publish: PublishCapture = Arc::new(publish_capture);
    let capture_started = Instant::now();
    // Capture observability is declared up front, like the other positive
    // privacy adapters, so a pre-trust loss can surface as uncertainty.
    for kind in CaptureKind::ALL {
        (capture_publish)(CaptureObservation::supported(
            kind,
            true,
            elapsed_millis(capture_started),
        ));
    }
    let mut backoff = ReconnectBackoff::default();
    // Monotonic per-connection generation. Every attempt gets a fresh value so
    // a move queued against a prior connection is rejected once IDs may have
    // been reused after a restart.
    let mut generation_counter: u64 = 0;
    loop {
        (publish)(AudioUpdate::Connecting);
        generation_counter = generation_counter.wrapping_add(1);
        let generation = Generation::new(generation_counter);
        // Attempt trust is reset every reconnect. It becomes true only when
        // the capture second barrier reports a trusted (complete) graph, so a
        // failure before that barrier is published as `Unavailable`, never a
        // false `Stale`.
        let mut capture_trusted = false;
        let (control_tx, control_rx) = pipewire::channel::channel::<Control>();
        let (done_tx, mut done_rx) = mpsc::unbounded_channel::<()>();
        let (ready_tx, mut ready_rx) = mpsc::unbounded_channel::<()>();
        let (capture_ready_tx, mut capture_ready_rx) = mpsc::unbounded_channel::<bool>();
        let thread_publish = Arc::clone(&publish);
        let thread_capture = Arc::clone(&capture_publish);
        let handle = std::thread::Builder::new()
            .name("weftwise-pipewire".to_owned())
            .spawn(move || {
                thread_main(
                    thread_publish,
                    thread_capture,
                    control_rx,
                    ready_tx,
                    capture_ready_tx,
                    generation,
                );
                let _ = done_tx.send(());
            });
        let Ok(handle) = handle else {
            (publish)(AudioUpdate::Unavailable);
            publish_capture_loss(&capture_publish, capture_trusted, capture_started);
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
                trusted = capture_ready_rx.recv() => {
                    if let Some(trusted) = trusted {
                        // Trust for this attempt is driven only by the capture
                        // second barrier, not the audio first barrier.
                        capture_trusted = trusted;
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

        // Join the PipeWire loop thread off the Tokio worker so a slow or stuck
        // teardown never blocks the async runtime; the loop was already asked to
        // quit above, so this only awaits its exit.
        let _ = tokio::task::spawn_blocking(move || handle.join()).await;
        if cancellation.is_cancelled() {
            return;
        }
        (publish)(AudioUpdate::Unavailable);
        publish_capture_loss(&capture_publish, capture_trusted, capture_started);
        let delay = backoff.next_delay();
        tokio::select! {
            _ = cancellation.cancelled() => return,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

/// Publish capture loss for every source without degrading other privacy
/// sources: `Stale` once the attempt reached a complete trusted snapshot,
/// `Unavailable` before. Never `Inactive`.
fn publish_capture_loss(
    capture_publish: &PublishCapture,
    capture_trusted: bool,
    capture_started: Instant,
) {
    let state = capture_loss_state(capture_trusted);
    for kind in CaptureKind::ALL {
        (capture_publish)(CaptureObservation::observed(
            kind,
            state,
            elapsed_millis(capture_started),
        ));
    }
}

struct NodeEntry {
    node: AudioNode,
    name: String,
    /// The node's PipeWire `object.serial`, used as a stable move target.
    object_serial: Option<u64>,
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
    capture_publish: PublishCapture,
    control_rx: pipewire::channel::Receiver<Control>,
    ready_tx: mpsc::UnboundedSender<()>,
    capture_ready_tx: mpsc::UnboundedSender<bool>,
    generation: Generation,
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
    // The capture tracker shares this loop's registry and clock and drives
    // microphone and camera evidence off the same globals as audio nodes.
    let capture = CaptureTracker::new(Arc::clone(&capture_publish), started);

    // Movable-stream tracking shares the same loop. The bounded stream
    // inventory, session policy, metadata handle, and last published selection
    // are grouped so registry callbacks and the command dispatcher agree.
    let streams: Streams = Rc::new(RefCell::new(HashMap::new()));
    let sm_settings: SmSettings = Rc::new(RefCell::new(None));
    let policy = Rc::new(Cell::new(MoveMovingPolicy::default()));
    let stream_overflow = Rc::new(Cell::new(false));
    let movable_last = Rc::new(Cell::new(MovableStreamState::Unavailable));
    let move_ctx = MoveContext {
        streams: Rc::clone(&streams),
        metadata: Rc::clone(&default_metadata),
        sm_settings: Rc::clone(&sm_settings),
        policy: Rc::clone(&policy),
        overflow: Rc::clone(&stream_overflow),
        last: Rc::clone(&movable_last),
        synced: Rc::clone(&synced),
        generation,
        publish: Arc::clone(&publish),
    };

    let quit_loop = main_loop.clone();
    let command_nodes = Rc::clone(&nodes);
    let command_move = move_ctx.clone();
    let command_publish = Arc::clone(&publish);
    let _control = control_rx.attach(main_loop.loop_(), move |control| match control {
        Control::Quit => quit_loop.quit(),
        Control::Command(command) => {
            apply_command(
                &command_nodes,
                &command_move,
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
    let global_capture = Rc::clone(&capture);
    let global_capture_registry = registry.clone();
    let global_move = move_ctx.clone();
    let global_move_registry = registry.clone();
    let remove_nodes = Rc::clone(&nodes);
    let remove_synced = Rc::clone(&synced);
    let remove_publish = Arc::clone(&publish);
    let remove_metadata = Rc::clone(&default_metadata);
    let remove_capture = Rc::clone(&capture);
    let remove_move = move_ctx.clone();
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
            CaptureTracker::observe_global(&global_capture, &global_capture_registry, global);
            on_move_global(&global_move_registry, &global_move, started, global);
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
            CaptureTracker::observe_remove(&remove_capture, id);
            on_move_remove(&remove_move, id, started);
        })
        .register();

    let Ok(pending) = core.sync(0) else {
        return;
    };
    let sync_nodes = Rc::clone(&nodes);
    let sync_defaults = Rc::clone(&defaults);
    let sync_synced = Rc::clone(&synced);
    let sync_publish = Arc::clone(&publish);
    let sync_capture = Rc::clone(&capture);
    let sync_move = move_ctx.clone();
    // The capture graph uses a second readiness barrier. The first sync
    // establishes registry enumeration and drives the audio snapshot and
    // audio ready signal, preserving audio snapshot ordering. Binding each
    // capture node and link during enumeration requests its info; the second
    // sync flushes those info replies, and only its completion trusts the
    // capture graph so a Node/Link running/active reply cannot land after
    // trust.
    let capture_pending: Rc<Cell<Option<pipewire::spa::utils::result::AsyncSeq>>> =
        Rc::new(Cell::new(None));
    let sync_core = core.clone();
    let error_loop = main_loop.clone();
    let _core_listener = core
        .add_listener_local()
        .done(move |id, sequence| {
            if id != pipewire::core::PW_ID_CORE {
                return;
            }
            if !sync_synced.get() && sequence == pending {
                sync_synced.set(true);
                publish_snapshot(&sync_nodes, &sync_defaults, &sync_publish, started);
                // Publish the initial movable-stream selection once enumeration
                // is complete, gated on the same first barrier as the snapshot.
                refresh_movable_stream(&sync_move, started);
                let _ = ready_tx.send(());
                // Issue the second sync only after the audio snapshot; its
                // completion marks the capture graph ready. If the second
                // sync cannot be issued, the barrier fails: publish pre-trust
                // uncertainty and report untrusted so a loss stays Unavailable.
                match sync_core.sync(0) {
                    Ok(second) => capture_pending.set(Some(second)),
                    Err(_) => {
                        let trusted = CaptureTracker::finish_barrier(&sync_capture, false);
                        let _ = capture_ready_tx.send(trusted);
                    }
                }
                return;
            }
            if capture_pending.get() == Some(sequence) {
                capture_pending.set(None);
                let trusted = CaptureTracker::finish_barrier(&sync_capture, true);
                let _ = capture_ready_tx.send(trusted);
            }
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
    let object_serial = props
        .get("object.serial")
        .and_then(|s| s.parse::<u64>().ok());
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
            object_serial,
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
    // A foreign-subject `target.object` write needs write and execute
    // permission on this object; capture it so the move action is only offered
    // when the server would actually accept the request.
    let writable = metadata_permits_target_write(global.permissions.bits());
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
        writable,
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

/// Route a registry global to the movable-stream tracker.
///
/// Playback streams feed the bounded inventory; the `sm-settings` metadata
/// carries the session movement policy. Every relevant global recomputes the
/// selection so a newly bound `default` metadata (write permission) or an
/// updated policy value takes effect immediately.
fn on_move_global(
    registry: &RegistryRc,
    mv: &MoveContext,
    started: Instant,
    global: &GlobalObject<&DictRef>,
) {
    match global.type_ {
        ObjectType::Node => on_stream_global(registry, mv, started, global),
        ObjectType::Metadata => {
            bind_sm_settings(registry, mv, started, global);
            // A `default` metadata global may have just bound with write
            // permission; recompute so the action reflects current writability.
            refresh_movable_stream(mv, started);
        }
        _ => {}
    }
}

/// Track one `Stream/Output/Audio` playback stream in the bounded inventory.
fn on_stream_global(
    registry: &RegistryRc,
    mv: &MoveContext,
    started: Instant,
    global: &GlobalObject<&DictRef>,
) {
    let Some(props) = global.props else {
        return;
    };
    let Some(class) = props.get("media.class") else {
        return;
    };
    if !is_movable_stream_class(class) {
        return;
    }
    let id = global.id;
    if mv.streams.borrow().len() >= MAX_MOVABLE_STREAMS && !mv.streams.borrow().contains_key(&id) {
        // A rejected id makes the inventory incomplete; disable the action
        // rather than risk a false unambiguous selection over a partial set.
        mv.overflow.set(true);
        refresh_movable_stream(mv, started);
        return;
    }
    let movable = stream_is_movable(props);
    // The subject must grant PW_PERM_M for the `target.object` write to be
    // accepted; capture it so a stream we cannot actually move is not offered.
    let metadata_permission = subject_permits_metadata(global.permissions.bits());
    let Ok(proxy) = registry.bind::<pipewire::node::Node, _>(global) else {
        return;
    };
    // Movement permission is a static node property; running state is dynamic
    // and arrives through the node info reply.
    let info_mv = mv.clone();
    let listener = proxy
        .add_listener_local()
        .info(move |info| {
            let running = matches!(info.state(), pipewire::node::NodeState::Running);
            let mut changed = false;
            {
                let mut streams = info_mv.streams.borrow_mut();
                if let Some(entry) = streams.get_mut(&id)
                    && entry.running != running
                {
                    entry.running = running;
                    changed = true;
                }
            }
            if changed {
                refresh_movable_stream(&info_mv, started);
            }
        })
        .register();
    mv.streams.borrow_mut().insert(
        id,
        StreamEntry {
            running: false,
            movable,
            metadata_permission,
            _proxy: proxy,
            _listener: listener,
        },
    );
    refresh_movable_stream(mv, started);
}

/// Whether a stream node permits movement (not `node.dont-move`).
fn stream_is_movable(props: &DictRef) -> bool {
    !matches!(
        props.get("node.dont-move").map(str::trim),
        Some("true" | "1")
    )
}

/// Bind the `sm-settings` metadata and follow `linking.allow-moving-streams`.
fn bind_sm_settings(
    registry: &RegistryRc,
    mv: &MoveContext,
    started: Instant,
    global: &GlobalObject<&DictRef>,
) {
    let is_sm = global
        .props
        .and_then(|props| props.get("metadata.name"))
        .is_some_and(|name| name == "sm-settings");
    if !is_sm || mv.sm_settings.borrow().is_some() {
        return;
    }
    let global_id = global.id;
    let Ok(metadata) = registry.bind::<pipewire::metadata::Metadata, _>(global) else {
        return;
    };
    let listener_mv = mv.clone();
    let listener = metadata
        .add_listener_local()
        .property(move |_subject, key, _type, value| {
            if key == Some("linking.allow-moving-streams") {
                listener_mv.policy.set(parse_allow_moving_streams(value));
                refresh_movable_stream(&listener_mv, started);
            }
            0
        })
        .register();
    *mv.sm_settings.borrow_mut() = Some(SmSettingsHandle {
        global_id,
        _proxy: metadata,
        _listener: listener,
    });
}

/// Release movable-stream state for a removed global and recompute.
fn on_move_remove(mv: &MoveContext, id: u32, started: Instant) {
    mv.streams.borrow_mut().remove(&id);
    let sm_id = mv
        .sm_settings
        .borrow()
        .as_ref()
        .map(|handle| handle.global_id);
    if sm_id == Some(id) {
        // The policy source vanished; revert to Unknown, which permits an
        // attempt but never asserts capability.
        *mv.sm_settings.borrow_mut() = None;
        mv.policy.set(MoveMovingPolicy::Unknown);
    }
    // Any removal can change a precondition (a lost `default` metadata clears
    // writability); recompute so a lost precondition disables the action.
    refresh_movable_stream(mv, started);
}

/// Build the movable-stream candidates from the bounded inventory.
fn movable_candidates(mv: &MoveContext) -> Vec<MovableStreamCandidate> {
    mv.streams
        .borrow()
        .iter()
        .map(|(id, entry)| MovableStreamCandidate {
            id: AudioNodeId::new(*id),
            running: entry.running,
            movable: entry.movable,
            has_metadata_permission: entry.metadata_permission,
        })
        .collect()
}

/// Recompute and publish the movable-stream selection, only when it changes.
fn refresh_movable_stream(mv: &MoveContext, started: Instant) {
    // Selection is only trustworthy once registry enumeration completed.
    if !mv.synced.get() {
        return;
    }
    let candidates = movable_candidates(mv);
    let metadata_writable = mv
        .metadata
        .borrow()
        .as_ref()
        .is_some_and(|handle| handle.writable);
    let enablement = MoveEnablement {
        policy_allows: mv.policy.get().permits_attempt(),
        metadata_writable,
        overflowed: mv.overflow.get(),
    };
    let state = select_movable_stream(&candidates, enablement, mv.generation);
    if mv.last.get() != state {
        mv.last.set(state);
        (mv.publish)(AudioUpdate::MovableStreamChanged {
            state,
            observed_millis: elapsed_millis(started),
        });
    }
}

/// Ask WirePlumber to move the active stream to a target sink.
///
/// Writes the `target.object` key of the `default` metadata with the stream id
/// as the subject and the destination sink's decimal `object.serial` as an
/// `Spa:Id` value; links are never mutated directly. WirePlumber owns the
/// resulting relink, so a successful write means only that the request was
/// sent, never that the graph moved. The command's `generation` must still
/// match this connection, and the stream must still be the single active
/// movable stream with every enablement precondition holding, both re-derived
/// here so a move queued across a reconnect or against an ambiguous or disabled
/// graph cannot retarget a coincidentally matching new object.
fn move_stream(
    nodes: &HashMap<u32, NodeEntry>,
    mv: &MoveContext,
    stream: u32,
    target: u32,
    generation: Generation,
) -> Result<(), AudioCommandError> {
    // Reject a move built against a stale connection before touching any state.
    if !move_is_fresh(generation, mv.generation) {
        return Err(AudioCommandError::Unsupported);
    }
    let handle = mv.metadata.borrow();
    let handle = handle.as_ref().ok_or(AudioCommandError::Transport)?;
    let candidates = movable_candidates(mv);
    let enablement = MoveEnablement {
        policy_allows: mv.policy.get().permits_attempt(),
        metadata_writable: handle.writable,
        overflowed: mv.overflow.get(),
    };
    match select_movable_stream(&candidates, enablement, mv.generation).active() {
        Some(active) if active.get() == stream => {}
        _ => return Err(AudioCommandError::Unsupported),
    }
    let target_node = nodes.get(&target).ok_or(AudioCommandError::UnknownNode)?;
    if target_node.node.direction != AudioDirection::Sink {
        return Err(AudioCommandError::WrongDirection);
    }
    let serial = target_node
        .object_serial
        .ok_or(AudioCommandError::Transport)?;
    let value = target_object_metadata_value(serial);
    handle.proxy.set_property(
        stream,
        TARGET_OBJECT_METADATA_KEY,
        Some(TARGET_OBJECT_METADATA_TYPE),
        Some(&value),
    );
    Ok(())
}

fn apply_command(
    nodes: &Nodes,
    mv: &MoveContext,
    command: &AudioCommand,
    publish: &Publish,
    started: Instant,
) {
    let (label, result) = dispatch(nodes, mv, command);
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
    mv: &MoveContext,
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
            set_default_node(&borrow, &mv.metadata, direction, id.get()),
        ),
        // A raw, ungenerationed move never reaches the transport from the root,
        // which only dispatches the validated `MoveStreamTo` form. Without a
        // generation its freshness cannot be verified, so it is refused.
        AudioCommandKind::MoveStream { .. } => {
            ("Route".to_owned(), Err(AudioCommandError::Unsupported))
        }
        AudioCommandKind::MoveStreamTo {
            stream,
            target,
            generation,
        } => (
            "Route".to_owned(),
            move_stream(&borrow, mv, stream.get(), target.get(), generation),
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
