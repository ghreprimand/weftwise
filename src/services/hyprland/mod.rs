//! Direct Hyprland request/event socket adapter.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

use crate::state::{
    ClientAddress, ClientState, CompositorOutput, DisplayText, HyprlandEvent, HyprlandSnapshot,
    HyprlandUpdate, OpenedClient, OutputName, WorkspaceId, WorkspaceState,
};
use crate::supervisor::{Cancellation, ReconnectBackoff};

mod discovery;

pub use discovery::{
    DiscoveryEnvironment, DiscoveryError, InstanceScan, MAX_METADATA_BYTES, MAX_SCANNED_INSTANCES,
    ProcProbe, ProcessLiveness, ProcessProbe, SocketPaths, current_uid, scan_instances,
    valid_signature,
};

/// Maximum bytes accepted from any one JSON request.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// Maximum bytes accepted from one newline-delimited event.
pub const MAX_EVENT_LINE_BYTES: usize = 64 * 1024;
/// Maximum parsed events retained while snapshots are in flight.
pub const MAX_BUFFERED_EVENTS: usize = 512;
/// Maximum source bytes retained by the event-first buffer.
pub const MAX_BUFFERED_EVENT_BYTES: usize = 256 * 1024;

const REQUEST_DEADLINE: Duration = Duration::from_millis(900);
/// Maximum in-place snapshot repairs before a session falls back to reconnect.
pub const MAX_GAP_REPAIRS: u32 = 3;

/// Structured parser failure; raw desktop data is never included.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ParseError {
    /// Snapshot JSON was malformed or had incompatible fields.
    #[error("Hyprland returned an invalid JSON snapshot")]
    InvalidSnapshot,
    /// A recognized event did not match its bounded schema.
    #[error("Hyprland returned a malformed event line")]
    MalformedEvent,
    /// An event-first buffer exceeded its count or byte limit.
    #[error("the Hyprland event buffer exceeded its limit")]
    BufferLimit,
}

/// Public-safe transport or reconciliation failure.
#[derive(Debug, Error)]
pub enum AdapterError {
    /// Socket discovery failed.
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
    /// A socket could not be connected or used.
    #[error("a Hyprland socket operation failed")]
    Socket {
        /// Original I/O failure; never log with debug formatting.
        #[source]
        source: io::Error,
    },
    /// A strict request deadline elapsed.
    #[error("a Hyprland request exceeded its deadline")]
    Deadline,
    /// A bounded response exceeded its byte limit.
    #[error("a Hyprland response exceeded its size limit")]
    ResponseLimit,
    /// A bounded event exceeded its byte limit.
    #[error("a Hyprland event exceeded its size limit")]
    EventLimit,
    /// The event socket ended between delimiters.
    #[error("the Hyprland event stream ended with a truncated line")]
    TruncatedEvent,
    /// The event stream disconnected cleanly.
    #[error("the Hyprland event stream disconnected")]
    Disconnected,
    /// Structured parsing failed.
    #[error(transparent)]
    Parse(#[from] ParseError),
    /// A lifecycle event requires a fresh snapshot.
    #[error("Hyprland state requires a fresh snapshot")]
    SnapshotGap,
}

impl AdapterError {
    fn is_gap(&self) -> bool {
        matches!(
            self,
            Self::ResponseLimit
                | Self::EventLimit
                | Self::TruncatedEvent
                | Self::Parse(_)
                | Self::SnapshotGap
        )
    }
}

/// Bounded ordered events received before the initial snapshot completes.
#[derive(Clone, Debug)]
pub struct EventBuffer {
    events: Vec<HyprlandEvent>,
    source_bytes: usize,
    maximum_events: usize,
    maximum_bytes: usize,
}

impl Default for EventBuffer {
    fn default() -> Self {
        Self::with_limits(MAX_BUFFERED_EVENTS, MAX_BUFFERED_EVENT_BYTES)
    }
}

impl EventBuffer {
    /// Construct a deterministic buffer with explicit bounds.
    #[must_use]
    pub const fn with_limits(maximum_events: usize, maximum_bytes: usize) -> Self {
        Self {
            events: Vec::new(),
            source_bytes: 0,
            maximum_events,
            maximum_bytes,
        }
    }

    /// Parse and append one line. Unknown events are tolerated and discarded.
    pub fn push_line(&mut self, line: &str) -> Result<(), ParseError> {
        let next_bytes = self.source_bytes.saturating_add(line.len());
        if next_bytes > self.maximum_bytes {
            return Err(ParseError::BufferLimit);
        }
        let event = parse_event_line(line)?;
        self.source_bytes = next_bytes;
        if let Some(event) = event {
            if self.events.len() >= self.maximum_events {
                return Err(ParseError::BufferLimit);
            }
            self.events.push(event);
        }
        Ok(())
    }

    /// Buffer one already-decoded record with per-record tolerance. A malformed
    /// individual line is skipped rather than ending the session; only the
    /// count and byte bounds return `BufferLimit`. Used while snapshotting.
    pub fn observe_record(&mut self, line: &str) -> Result<(), ParseError> {
        let next_bytes = self.source_bytes.saturating_add(line.len());
        if next_bytes > self.maximum_bytes {
            return Err(ParseError::BufferLimit);
        }
        self.source_bytes = next_bytes;
        match parse_event_line(line) {
            Ok(Some(event)) => {
                if self.events.len() >= self.maximum_events {
                    return Err(ParseError::BufferLimit);
                }
                self.events.push(event);
                Ok(())
            }
            // Unknown and individually malformed lines are tolerated.
            Ok(None) | Err(ParseError::MalformedEvent) => Ok(()),
            Err(other) => Err(other),
        }
    }

    /// Number of parsed known events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether no known events are buffered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Replay parsed events in wire order.
    pub fn drain(&mut self) -> impl Iterator<Item = HyprlandEvent> + '_ {
        self.source_bytes = 0;
        self.events.drain(..)
    }
}

/// Workspace identities known to one event-socket session.
///
/// Hyprland's `openwindow` event supplies a workspace name rather than the
/// stable numeric identity used by snapshots and the remaining v2 events. A
/// name that cannot be reconciled is an identity gap and must end the session
/// so the adapter can discover the active instance and take a fresh snapshot.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceCatalog {
    by_name: BTreeMap<DisplayText, WorkspaceId>,
}

impl WorkspaceCatalog {
    /// Build the catalog from an atomic snapshot before buffered events replay.
    #[must_use]
    pub fn from_snapshot(snapshot: &HyprlandSnapshot) -> Self {
        let mut catalog = Self::default();
        for workspace in snapshot.workspaces.values() {
            catalog.upsert(workspace.id, workspace.name.clone());
        }
        catalog
    }

    /// Reconcile one ordered event or report that a fresh snapshot is required.
    pub fn observe(&mut self, event: &HyprlandEvent) -> Result<(), AdapterError> {
        match event {
            HyprlandEvent::ClientOpened(opened) => {
                if !self.by_name.contains_key(&opened.workspace_name) {
                    return Err(AdapterError::SnapshotGap);
                }
            }
            HyprlandEvent::WorkspaceChanged { id, name } => {
                self.upsert(*id, name.clone());
            }
            HyprlandEvent::FocusedOutput {
                workspace,
                workspace_name,
                ..
            } if !workspace_name.is_empty() => {
                self.upsert(*workspace, workspace_name.clone());
            }
            HyprlandEvent::ClientMoved {
                workspace,
                workspace_name,
                ..
            } => {
                self.upsert(*workspace, workspace_name.clone());
            }
            HyprlandEvent::WorkspaceCreated(workspace) => {
                self.upsert(workspace.id, workspace.name.clone());
            }
            HyprlandEvent::WorkspaceDestroyed(id) => {
                self.by_name.retain(|_, candidate| candidate != id);
            }
            _ => {}
        }
        Ok(())
    }

    fn upsert(&mut self, id: WorkspaceId, name: DisplayText) {
        self.by_name.retain(|_, candidate| *candidate != id);
        self.by_name.insert(name, id);
    }
}

/// Parse five structured request snapshots into one atomic state value.
pub fn parse_snapshot_json(
    monitors: &str,
    workspaces: &str,
    clients: &str,
    active_workspace: &str,
    active_window: &str,
) -> Result<HyprlandSnapshot, ParseError> {
    let raw_monitors: Vec<RawMonitor> =
        serde_json::from_str(monitors).map_err(|_| ParseError::InvalidSnapshot)?;
    let raw_workspaces: Vec<RawWorkspace> =
        serde_json::from_str(workspaces).map_err(|_| ParseError::InvalidSnapshot)?;
    let raw_clients: Vec<RawClient> =
        serde_json::from_str(clients).map_err(|_| ParseError::InvalidSnapshot)?;
    let raw_active_workspace: RawWorkspaceReference =
        serde_json::from_str(active_workspace).map_err(|_| ParseError::InvalidSnapshot)?;
    let raw_active_window: RawActiveWindow =
        serde_json::from_str(active_window).map_err(|_| ParseError::InvalidSnapshot)?;

    let mut snapshot = HyprlandSnapshot::default();
    for raw in raw_monitors {
        let Some(name) = OutputName::new(&raw.name) else {
            return Err(ParseError::InvalidSnapshot);
        };
        let active_workspace = raw
            .active_workspace
            .id
            .filter(|id| *id != 0)
            .map(WorkspaceId::new);
        snapshot.outputs.insert(
            name.clone(),
            CompositorOutput {
                id: raw.id,
                name: name.clone(),
                focused: raw.focused,
                scale_milli: scale_to_milli(raw.scale),
                active_workspace,
                fullscreen: false,
            },
        );
        if raw.focused {
            snapshot.active.output = Some(name);
        }
    }

    for raw in raw_workspaces {
        let id = WorkspaceId::new(raw.id);
        snapshot.workspaces.insert(
            id,
            WorkspaceState {
                id,
                name: DisplayText::new(&raw.name, 128),
                output: OutputName::new(&raw.monitor),
                clients: raw.windows.min(u64::from(u32::MAX)) as u32,
                fullscreen: raw.has_fullscreen,
            },
        );
    }

    for raw in raw_clients {
        let Some(address) = ClientAddress::new(&raw.address) else {
            return Err(ParseError::InvalidSnapshot);
        };
        let workspace = WorkspaceId::new(raw.workspace.id.unwrap_or_default());
        let output = snapshot
            .workspaces
            .get(&workspace)
            .and_then(|workspace| workspace.output.clone())
            .or_else(|| {
                raw.monitor.and_then(|monitor_id| {
                    snapshot
                        .outputs
                        .values()
                        .find(|output| output.id == monitor_id)
                        .map(|output| output.name.clone())
                })
            });
        let fullscreen = raw.fullscreen.as_bool();
        if fullscreen
            && let Some(output) = output
                .as_ref()
                .and_then(|name| snapshot.outputs.get_mut(name))
        {
            output.fullscreen = true;
        }
        snapshot.clients.insert(
            address.clone(),
            ClientState {
                address,
                class: DisplayText::new(&raw.class, 128),
                title: DisplayText::new(&raw.title, 256),
                workspace,
                output,
                fullscreen,
            },
        );
    }

    snapshot.active.workspace = raw_active_workspace
        .id
        .filter(|id| *id != 0)
        .map(WorkspaceId::new)
        .or_else(|| {
            snapshot
                .active
                .output
                .as_ref()
                .and_then(|name| snapshot.outputs.get(name))
                .and_then(|output| output.active_workspace)
        });
    snapshot.active.class = DisplayText::new(raw_active_window.class.as_deref().unwrap_or(""), 128);
    snapshot.active.title = DisplayText::new(raw_active_window.title.as_deref().unwrap_or(""), 256);
    snapshot.active.client = raw_active_window
        .address
        .as_deref()
        .and_then(ClientAddress::new);
    Ok(snapshot)
}

/// Parse one newline-delimited event, splitting only at the first delimiter.
pub fn parse_event_line(line: &str) -> Result<Option<HyprlandEvent>, ParseError> {
    let line = line.trim_end_matches(['\r', '\n']);
    let Some((name, data)) = line.split_once(">>") else {
        return Err(ParseError::MalformedEvent);
    };
    match name {
        "workspacev2" => {
            let (id, workspace_name) = two_fields(data)?;
            Ok(Some(HyprlandEvent::WorkspaceChanged {
                id: parse_workspace_id(id)?,
                name: DisplayText::new(workspace_name, 128),
            }))
        }
        "focusedmonv2" => {
            let (output, workspace) = two_fields(data)?;
            Ok(Some(HyprlandEvent::FocusedOutput {
                output: parse_output(output)?,
                workspace: parse_workspace_id(workspace)?,
                workspace_name: DisplayText::default(),
            }))
        }
        "activewindowv2" => Ok(Some(HyprlandEvent::ActiveClient {
            address: if data.is_empty() {
                None
            } else {
                Some(parse_address(data)?)
            },
            class: DisplayText::default(),
            title: DisplayText::default(),
        })),
        "activewindow" => {
            let (class, title) = data.split_once(',').unwrap_or((data, ""));
            Ok(Some(HyprlandEvent::ActiveClient {
                address: None,
                class: DisplayText::new(class, 128),
                title: DisplayText::new(title, 256),
            }))
        }
        "openwindow" => {
            let mut fields = data.splitn(4, ',');
            let address = parse_address(required_field(&mut fields)?)?;
            let workspace_name = required_field(&mut fields)?;
            let class = fields.next().ok_or(ParseError::MalformedEvent)?;
            let title = fields.next().ok_or(ParseError::MalformedEvent)?;
            Ok(Some(HyprlandEvent::ClientOpened(OpenedClient {
                address,
                class: DisplayText::new(class, 128),
                title: DisplayText::new(title, 256),
                workspace_name: DisplayText::new(workspace_name, 128),
            })))
        }
        "closewindow" | "closewindowv2" => {
            Ok(Some(HyprlandEvent::ClientClosed(parse_address(data)?)))
        }
        "movewindowv2" => {
            let mut fields = data.splitn(3, ',');
            let address = parse_address(required_field(&mut fields)?)?;
            let workspace = parse_workspace_id(required_field(&mut fields)?)?;
            let name = required_field(&mut fields)?;
            Ok(Some(HyprlandEvent::ClientMoved {
                address,
                workspace,
                workspace_name: DisplayText::new(name, 128),
            }))
        }
        "fullscreen" => match data {
            "0" => Ok(Some(HyprlandEvent::FullscreenChanged(false))),
            "1" => Ok(Some(HyprlandEvent::FullscreenChanged(true))),
            _ => Err(ParseError::MalformedEvent),
        },
        "screencastv2" => {
            let mut fields = data.splitn(3, ',');
            let state = required_field(&mut fields)?;
            let owner = required_field(&mut fields)?;
            let _name = fields.next().ok_or(ParseError::MalformedEvent)?;
            if !matches!(owner, "monitor" | "window" | "region") {
                return Err(ParseError::MalformedEvent);
            }
            match state {
                "0" => Ok(Some(HyprlandEvent::ScreencastChanged(false))),
                "1" => Ok(Some(HyprlandEvent::ScreencastChanged(true))),
                _ => Err(ParseError::MalformedEvent),
            }
        }
        "createworkspacev2" => {
            let (id, name) = two_fields(data)?;
            let id = parse_workspace_id(id)?;
            Ok(Some(HyprlandEvent::WorkspaceCreated(WorkspaceState {
                id,
                name: DisplayText::new(name, 128),
                output: None,
                clients: 0,
                fullscreen: false,
            })))
        }
        "destroyworkspacev2" => {
            let (id, _) = two_fields(data)?;
            Ok(Some(HyprlandEvent::WorkspaceDestroyed(parse_workspace_id(
                id,
            )?)))
        }
        "moveworkspacev2" => {
            let mut fields = data.splitn(3, ',');
            let workspace = parse_workspace_id(required_field(&mut fields)?)?;
            let _name = required_field(&mut fields)?;
            let output = parse_output(required_field(&mut fields)?)?;
            Ok(Some(HyprlandEvent::WorkspaceMoved { workspace, output }))
        }
        "windowtitlev2" => {
            let (address, title) = data.split_once(',').ok_or(ParseError::MalformedEvent)?;
            Ok(Some(HyprlandEvent::ClientTitleChanged {
                address: parse_address(address)?,
                title: DisplayText::new(title, 256),
            }))
        }
        "monitoraddedv2" | "monitorremoved" | "monitoradded" | "monitorremovedv2"
        | "renameworkspace" | "configreloaded" | "activespecial" | "activespecialv2" | "kill" => {
            Ok(Some(HyprlandEvent::ResnapshotRequired))
        }
        "workspace" | "focusedmon" | "movewindow" | "windowtitle" | "screencast" => Ok(None),
        _ => Ok(None),
    }
}

fn required_field<'a>(fields: &mut impl Iterator<Item = &'a str>) -> Result<&'a str, ParseError> {
    fields
        .next()
        .filter(|field| !field.is_empty())
        .ok_or(ParseError::MalformedEvent)
}

fn two_fields(data: &str) -> Result<(&str, &str), ParseError> {
    let (first, second) = data.split_once(',').ok_or(ParseError::MalformedEvent)?;
    if first.is_empty() || second.is_empty() {
        Err(ParseError::MalformedEvent)
    } else {
        Ok((first, second))
    }
}

fn parse_workspace_id(value: &str) -> Result<WorkspaceId, ParseError> {
    value
        .parse::<i64>()
        .map(WorkspaceId::new)
        .map_err(|_| ParseError::MalformedEvent)
}

fn parse_address(value: &str) -> Result<ClientAddress, ParseError> {
    ClientAddress::new(value).ok_or(ParseError::MalformedEvent)
}

fn parse_output(value: &str) -> Result<OutputName, ParseError> {
    OutputName::new(value).ok_or(ParseError::MalformedEvent)
}

fn scale_to_milli(scale: f64) -> u32 {
    if scale.is_finite() && scale > 0.0 {
        (scale.clamp(0.1, 16.0) * 1000.0).round() as u32
    } else {
        1000
    }
}

#[derive(Deserialize)]
struct RawMonitor {
    id: i64,
    name: String,
    #[serde(default)]
    focused: bool,
    #[serde(default = "default_scale")]
    scale: f64,
    #[serde(default, rename = "activeWorkspace")]
    active_workspace: RawWorkspaceReference,
}

const fn default_scale() -> f64 {
    1.0
}

#[derive(Default, Deserialize)]
struct RawWorkspaceReference {
    #[serde(default)]
    id: Option<i64>,
}

#[derive(Deserialize)]
struct RawWorkspace {
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    monitor: String,
    #[serde(default)]
    windows: u64,
    #[serde(default, rename = "hasfullscreen")]
    has_fullscreen: bool,
}

#[derive(Deserialize)]
struct RawClient {
    address: String,
    #[serde(default)]
    class: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    workspace: RawWorkspaceReference,
    #[serde(default)]
    monitor: Option<i64>,
    #[serde(default)]
    fullscreen: Boolish,
}

#[derive(Default, Deserialize)]
struct RawActiveWindow {
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(untagged)]
enum Boolish {
    Boolean(bool),
    Integer(i64),
    #[default]
    Missing,
}

impl Boolish {
    const fn as_bool(self) -> bool {
        match self {
            Self::Boolean(value) => value,
            Self::Integer(value) => value != 0,
            Self::Missing => false,
        }
    }
}

/// Run the independently reconnecting adapter until cancellation.
///
/// The environment signature is trusted only for the first connection; every
/// reconnect rescans the runtime directory so a compositor restart is followed
/// to its new instance rather than retrying a stale socket path.
pub async fn run<Emit>(emit: Emit, mut cancellation: Cancellation)
where
    Emit: Fn(HyprlandUpdate) + Send + Sync + 'static,
{
    let mut backoff = ReconnectBackoff::default();
    let mut prefer_environment = true;
    loop {
        emit(HyprlandUpdate::Connecting);
        let connection = run_connection(&emit, prefer_environment);
        let (error, initialized) = tokio::select! {
            result = connection => result,
            () = cancellation.cancelled() => return,
        };
        prefer_environment = false;
        if initialized {
            backoff.reset();
        }
        if error.is_gap() || initialized {
            emit(HyprlandUpdate::Gap);
        } else {
            emit(HyprlandUpdate::Unavailable);
        }
        tracing::warn!(reason = %error, "Hyprland adapter will reconnect");

        let delay = backoff.next_delay();
        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = cancellation.cancelled() => return,
        }
    }
}

/// Resolve ranked instance candidates for one connection attempt.
fn resolve_candidates(prefer_environment: bool) -> Result<Vec<SocketPaths>, DiscoveryError> {
    let scan = InstanceScan::from_environment()?;
    let uid = current_uid().ok_or(DiscoveryError::OwnerUnavailable)?;
    match scan_instances(&scan, uid, prefer_environment, &ProcProbe) {
        Ok(candidates) => Ok(candidates),
        // First-connection fast path when the runtime directory cannot be
        // scanned but the environment signature is present and valid.
        Err(DiscoveryError::NoLiveInstance) if prefer_environment => {
            SocketPaths::discover().map(|paths| vec![paths])
        }
        Err(error) => Err(error),
    }
}

/// Try each ranked candidate in order until one connects and answers a complete
/// snapshot, then run its event loop until an error requires reconnecting.
async fn run_connection<Emit>(emit: &Emit, prefer_environment: bool) -> (AdapterError, bool)
where
    Emit: Fn(HyprlandUpdate) + Send + Sync,
{
    let candidates = match resolve_candidates(prefer_environment) {
        Ok(candidates) => candidates,
        Err(error) => return (error.into(), false),
    };
    let mut last_error = AdapterError::from(DiscoveryError::NoLiveInstance);
    for paths in candidates {
        match establish_session(&paths, emit).await {
            Ok((reader, workspaces, buffered)) => {
                return run_event_loop(&paths, reader, workspaces, buffered, emit).await;
            }
            Err(error) => last_error = error,
        }
    }
    (last_error, false)
}

/// Connect the event socket, take the full snapshot, and emit it. Success here
/// is the authoritative liveness proof for a candidate instance.
async fn establish_session<Emit>(
    paths: &SocketPaths,
    emit: &Emit,
) -> Result<(BufReader<UnixStream>, WorkspaceCatalog, Vec<HyprlandEvent>), AdapterError>
where
    Emit: Fn(HyprlandUpdate) + Send + Sync,
{
    let events = UnixStream::connect(paths.events())
        .await
        .map_err(|source| AdapterError::Socket { source })?;
    let mut reader = BufReader::new(events);
    let mut buffer = EventBuffer::default();
    let snapshot = take_snapshot(paths, &mut reader, &mut buffer).await?;
    let workspaces = WorkspaceCatalog::from_snapshot(&snapshot);
    emit(HyprlandUpdate::Snapshot(snapshot));
    Ok((reader, workspaces, buffer.drain().collect()))
}

/// Drive one established session: replay buffered events, then read live records
/// with per-record tolerance and bounded in-place gap repair.
async fn run_event_loop<Emit>(
    paths: &SocketPaths,
    mut reader: BufReader<UnixStream>,
    mut workspaces: WorkspaceCatalog,
    buffered: Vec<HyprlandEvent>,
    emit: &Emit,
) -> (AdapterError, bool)
where
    Emit: Fn(HyprlandUpdate) + Send + Sync,
{
    let mut repairs: u32 = 0;
    let mut tainted = false;

    for event in buffered {
        if let Err(error) = reconcile_event(
            paths,
            &mut reader,
            &mut workspaces,
            emit,
            &mut repairs,
            &mut tainted,
            event,
        )
        .await
        {
            return (error, true);
        }
    }

    loop {
        let record = match read_event_record(&mut reader).await {
            Ok(record) => record,
            Err(error) => return (error, true),
        };
        let outcome = match record {
            EventRecord::Complete(bytes) => match classify_line(&bytes) {
                LineClass::Event(event) => {
                    reconcile_event(
                        paths,
                        &mut reader,
                        &mut workspaces,
                        emit,
                        &mut repairs,
                        &mut tainted,
                        event,
                    )
                    .await
                }
                LineClass::Benign => Ok(()),
                LineClass::Gap => {
                    handle_gap(
                        paths,
                        &mut reader,
                        &mut workspaces,
                        emit,
                        &mut repairs,
                        &mut tainted,
                    )
                    .await
                }
            },
            EventRecord::OversizeDiscarded => Ok(()),
            EventRecord::Eof => return (AdapterError::Disconnected, true),
            EventRecord::Truncated => return (AdapterError::TruncatedEvent, true),
        };
        if let Err(error) = outcome {
            return (error, true);
        }
    }
}

/// Reconcile one parsed event and emit it, reporting an identity gap.
fn observe_event<Emit>(
    workspaces: &mut WorkspaceCatalog,
    event: HyprlandEvent,
    emit: &Emit,
) -> Result<(), ()>
where
    Emit: Fn(HyprlandUpdate) + Send + Sync,
{
    if event == HyprlandEvent::ResnapshotRequired {
        return Err(());
    }
    if workspaces.observe(&event).is_err() {
        return Err(());
    }
    emit(HyprlandUpdate::Event(event));
    Ok(())
}

/// Reconcile one event, repairing in place on an identity gap. A clean event
/// resets the repair budget so isolated gaps do not accumulate toward reconnect.
#[allow(clippy::too_many_arguments)]
async fn reconcile_event<Emit>(
    paths: &SocketPaths,
    reader: &mut BufReader<UnixStream>,
    workspaces: &mut WorkspaceCatalog,
    emit: &Emit,
    repairs: &mut u32,
    tainted: &mut bool,
    event: HyprlandEvent,
) -> Result<(), AdapterError>
where
    Emit: Fn(HyprlandUpdate) + Send + Sync,
{
    if observe_event(workspaces, event, emit).is_ok() {
        *repairs = 0;
        Ok(())
    } else {
        handle_gap(paths, reader, workspaces, emit, repairs, tainted).await
    }
}

/// Repair a genuine gap in place on the same event reader, bounded by
/// `MAX_GAP_REPAIRS`. The taint flag surfaces transient uncertainty once.
async fn handle_gap<Emit>(
    paths: &SocketPaths,
    reader: &mut BufReader<UnixStream>,
    workspaces: &mut WorkspaceCatalog,
    emit: &Emit,
    repairs: &mut u32,
    tainted: &mut bool,
) -> Result<(), AdapterError>
where
    Emit: Fn(HyprlandUpdate) + Send + Sync,
{
    if *repairs >= MAX_GAP_REPAIRS {
        // Repeated gaps in one session fall back to a full reconnect + rescan.
        return Err(AdapterError::SnapshotGap);
    }
    *repairs += 1;
    if !*tainted {
        *tainted = true;
        emit(HyprlandUpdate::Gap);
    }
    tracing::debug!(attempt = *repairs, "repairing Hyprland state gap in place");
    let mut buffer = EventBuffer::default();
    let snapshot = take_snapshot(paths, reader, &mut buffer).await?;
    *workspaces = WorkspaceCatalog::from_snapshot(&snapshot);
    emit(HyprlandUpdate::Snapshot(snapshot));
    for event in buffer.drain().collect::<Vec<_>>() {
        observe_event(workspaces, event, emit).map_err(|()| AdapterError::SnapshotGap)?;
    }
    Ok(())
}

/// A decoded event record. Malformed and oversize records are tolerated
/// individually rather than ending the session.
enum EventRecord {
    /// One complete newline-terminated record, delimiter stripped.
    Complete(Vec<u8>),
    /// A record exceeded the byte limit and was discarded to the next newline.
    OversizeDiscarded,
    /// The stream ended cleanly on a record boundary.
    Eof,
    /// The stream ended between delimiters.
    Truncated,
}

/// Classification of one complete record.
enum LineClass {
    /// A known event to reconcile and emit.
    Event(HyprlandEvent),
    /// An unknown event, non-UTF-8 bytes, or a line with no `>>` delimiter.
    /// Nothing tracked can have changed, so the line is skipped.
    Benign,
    /// A genuine state gap requiring an in-place resnapshot: either an explicit
    /// lifecycle event or a known event whose payload failed to parse, which
    /// means a tracked state change was observed but its content is lost.
    Gap,
}

/// Classify one complete record without ending the session on bad input.
fn classify_line(bytes: &[u8]) -> LineClass {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return LineClass::Benign;
    };
    if !text.contains(">>") {
        return LineClass::Benign;
    }
    match parse_event_line(text) {
        Ok(Some(HyprlandEvent::ResnapshotRequired)) | Err(_) => LineClass::Gap,
        Ok(Some(event)) => LineClass::Event(event),
        Ok(None) => LineClass::Benign,
    }
}

async fn take_snapshot(
    paths: &SocketPaths,
    reader: &mut BufReader<UnixStream>,
    buffer: &mut EventBuffer,
) -> Result<HyprlandSnapshot, AdapterError> {
    let monitors = request_while_buffering(paths, "j/monitors", reader, buffer).await?;
    let workspaces = request_while_buffering(paths, "j/workspaces", reader, buffer).await?;
    let clients = request_while_buffering(paths, "j/clients", reader, buffer).await?;
    let active_workspace =
        request_while_buffering(paths, "j/activeworkspace", reader, buffer).await?;
    let active_window = request_while_buffering(paths, "j/activewindow", reader, buffer).await?;
    parse_snapshot_json(
        &monitors,
        &workspaces,
        &clients,
        &active_workspace,
        &active_window,
    )
    .map_err(Into::into)
}

async fn request_while_buffering(
    paths: &SocketPaths,
    command: &'static str,
    reader: &mut BufReader<UnixStream>,
    buffer: &mut EventBuffer,
) -> Result<String, AdapterError> {
    let request = request_json(paths.request(), command);
    tokio::pin!(request);
    loop {
        tokio::select! {
            response = &mut request => return response,
            record = read_event_record(reader) => {
                match record? {
                    EventRecord::Complete(bytes) => {
                        if let Ok(text) = std::str::from_utf8(&bytes) {
                            buffer.observe_record(text)?;
                        }
                    }
                    EventRecord::OversizeDiscarded => {}
                    EventRecord::Eof => return Err(AdapterError::Disconnected),
                    EventRecord::Truncated => return Err(AdapterError::TruncatedEvent),
                }
            }
        }
    }
}

/// Send one bounded command through a fresh request connection.
pub async fn request_json(path: &Path, command: &str) -> Result<String, AdapterError> {
    tokio::time::timeout(REQUEST_DEADLINE, async {
        let mut stream = UnixStream::connect(path)
            .await
            .map_err(|source| AdapterError::Socket { source })?;
        stream
            .write_all(command.as_bytes())
            .await
            .map_err(|source| AdapterError::Socket { source })?;
        stream
            .shutdown()
            .await
            .map_err(|source| AdapterError::Socket { source })?;

        let mut response = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            let read = stream
                .read(&mut chunk)
                .await
                .map_err(|source| AdapterError::Socket { source })?;
            if read == 0 {
                break;
            }
            if response.len().saturating_add(read) > MAX_RESPONSE_BYTES {
                return Err(AdapterError::ResponseLimit);
            }
            response.extend_from_slice(&chunk[..read]);
        }
        String::from_utf8(response).map_err(|_| ParseError::InvalidSnapshot.into())
    })
    .await
    .map_err(|_| AdapterError::Deadline)?
}

/// Read one newline-delimited record from the event socket with per-record
/// tolerance. An over-long record is discarded to the next newline and reported
/// as `OversizeDiscarded` rather than ending the session.
async fn read_event_record(
    reader: &mut BufReader<UnixStream>,
) -> Result<EventRecord, AdapterError> {
    let mut line: Vec<u8> = Vec::new();
    let mut oversize = false;
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|source| AdapterError::Socket { source })?;
        if available.is_empty() {
            if oversize {
                return Ok(EventRecord::Truncated);
            }
            return Ok(if line.is_empty() {
                EventRecord::Eof
            } else {
                EventRecord::Truncated
            });
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |position| position + 1);
        if !oversize {
            if line.len().saturating_add(take) > MAX_EVENT_LINE_BYTES {
                oversize = true;
                line.clear();
            } else {
                line.extend_from_slice(&available[..take]);
            }
        }
        reader.consume(take);
        if newline.is_some() {
            if oversize {
                return Ok(EventRecord::OversizeDiscarded);
            }
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            return Ok(EventRecord::Complete(line));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_recognizes_events_gaps_and_benign_records() {
        assert!(matches!(
            classify_line(b"workspacev2>>1,one"),
            LineClass::Event(_)
        ));
        // A lifecycle event that invalidates cached identity is a genuine gap.
        assert!(matches!(
            classify_line(b"monitoraddedv2>>SYNTH-1"),
            LineClass::Gap
        ));
        // A known event with an unparseable payload lost a tracked change.
        assert!(matches!(
            classify_line(b"workspacev2>>notanid,one"),
            LineClass::Gap
        ));
        // Unknown, legacy, delimiter-less, and non-UTF-8 are benign.
        assert!(matches!(
            classify_line(b"futureevent>>opaque"),
            LineClass::Benign
        ));
        assert!(matches!(classify_line(b"workspace>>1"), LineClass::Benign));
        assert!(matches!(classify_line(b"nodelimiter"), LineClass::Benign));
        assert!(matches!(
            classify_line(&[0xff, 0xfe, 0x00]),
            LineClass::Benign
        ));
    }

    #[test]
    fn observe_record_tolerates_malformed_lines_but_bounds_growth() {
        let mut buffer = EventBuffer::with_limits(8, 4096);
        // A malformed line is skipped rather than ending the session.
        buffer
            .observe_record("workspacev2>>bad")
            .expect("malformed skipped");
        assert!(buffer.is_empty());
        buffer
            .observe_record("workspacev2>>1,one")
            .expect("known event");
        assert_eq!(buffer.len(), 1);
        // Unknown events are tolerated and not stored.
        buffer
            .observe_record("futureevent>>opaque")
            .expect("unknown skipped");
        assert_eq!(buffer.len(), 1);
    }

    #[test]
    fn observe_record_enforces_the_byte_bound() {
        let mut buffer = EventBuffer::with_limits(8, 8);
        assert_eq!(
            buffer.observe_record("workspacev2>>1,one"),
            Err(ParseError::BufferLimit)
        );
    }
}
