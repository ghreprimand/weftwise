//! Authoritative product and presentation state.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use crate::config::Config;
use crate::context::arbitration::{
    ArbitrationInput, Arbitrator, CandidateAction, CandidateRegion, PresentationKind,
    PresentationProjection, Severity, Timestamp,
};

/// Delay before a pointer at the top edge reveals the Ribbon.
pub const DWELL_DELAY: Duration = Duration::from_millis(240);

/// Delay before a pointer departure collapses the Ribbon.
pub const DISMISS_DELAY: Duration = Duration::from_millis(360);
/// Maximum local workspace marks rendered in the navigation region.
pub const MAX_NAVIGATION_MARKS: usize = 16;
/// Maximum selected activity marks rendered in the center region.
pub const MAX_ACTIVITY_MARKS: usize = 4;
/// Maximum warning/privacy marks rendered in the attention region.
pub const MAX_ATTENTION_MARKS: usize = 4;

/// Process-local identity assigned to a GDK output surface.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OutputId(u64);

impl OutputId {
    /// Construct an output identity from the surface manager's monotonic counter.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Numeric identity used only for stable in-process ordering.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Bounded compositor output name kept out of diagnostics by default.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct OutputName(String);

impl OutputName {
    /// Validate and bound an output name received from GDK or Hyprland.
    #[must_use]
    pub fn new(value: &str) -> Option<Self> {
        if value.chars().count() > 128 {
            return None;
        }
        let value = bounded_text(value, 128);
        (!value.is_empty()).then_some(Self(value))
    }

    /// Borrow the value for exact in-process matching or rendering.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OutputName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted-output>")
    }
}

/// Address-bearing Hyprland client identity.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ClientAddress(String);

impl ClientAddress {
    /// Parse a hexadecimal address with or without Hyprland's `0x` prefix.
    #[must_use]
    pub fn new(value: &str) -> Option<Self> {
        let value = value.strip_prefix("0x").unwrap_or(value);
        if value.is_empty()
            || value.len() > 32
            || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
        Some(Self(value.to_ascii_lowercase()))
    }

    /// Borrow the normalized address for transport correlation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ClientAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted-client>")
    }
}

/// Bounded untrusted display text with redacted debug formatting.
#[derive(Clone, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct DisplayText(String);

impl DisplayText {
    /// Sanitize controls and truncate at a Unicode scalar boundary.
    #[must_use]
    pub fn new(value: &str, maximum_characters: usize) -> Self {
        Self(bounded_text(value, maximum_characters))
    }

    /// Borrow the sanitized text for rendering.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the sanitized value is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for DisplayText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted-text>")
    }
}

fn bounded_text(value: &str, maximum_characters: usize) -> String {
    value
        .chars()
        .filter_map(|character| {
            if character.is_control() {
                (character == '\t').then_some(' ')
            } else {
                Some(character)
            }
        })
        .take(maximum_characters)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Availability of one independently supervised adapter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AdapterAvailability {
    /// No complete snapshot has been received yet.
    #[default]
    Starting,
    /// A complete snapshot and all subsequent events are current.
    Ready,
    /// Retained state may be stale while a new snapshot is acquired.
    Stale,
    /// The adapter has no usable transport or retained snapshot.
    Unavailable,
}

/// Stable Hyprland workspace identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspaceId(i64);

impl WorkspaceId {
    /// Construct an identity from Hyprland JSON or an address-bearing event.
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Numeric value used for deterministic ordering.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Reconciled compositor output state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompositorOutput {
    /// Hyprland's numeric monitor identity.
    pub id: i64,
    /// Connector name used to bind the output to its GDK surface.
    pub name: OutputName,
    /// Whether this is Hyprland's focused output.
    pub focused: bool,
    /// Scale represented in thousandths to avoid non-total floating ordering.
    pub scale_milli: u32,
    /// Active regular workspace on this output.
    pub active_workspace: Option<WorkspaceId>,
    /// Whether an output client is currently fullscreen.
    pub fullscreen: bool,
}

/// Reconciled workspace state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceState {
    /// Stable numeric identity.
    pub id: WorkspaceId,
    /// Bounded display name.
    pub name: DisplayText,
    /// Output affinity when known.
    pub output: Option<OutputName>,
    /// Number of clients reported by the snapshot.
    pub clients: u32,
    /// Whether the workspace contains a fullscreen client.
    pub fullscreen: bool,
}

/// Reconciled client lifecycle state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientState {
    /// Address-bearing identity preferred over title matching.
    pub address: ClientAddress,
    /// Bounded untrusted application class.
    pub class: DisplayText,
    /// Bounded untrusted window title.
    pub title: DisplayText,
    /// Current workspace.
    pub workspace: WorkspaceId,
    /// Current output affinity when known.
    pub output: Option<OutputName>,
    /// Fullscreen state.
    pub fullscreen: bool,
}

/// Address-bearing client creation payload from Hyprland's event socket.
///
/// The wire event identifies the workspace by name rather than by the stable
/// numeric identity available in JSON snapshots, so reconciliation resolves
/// this value against the current workspace map before inserting the client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenedClient {
    /// Address-bearing client identity.
    pub address: ClientAddress,
    /// Bounded untrusted application class.
    pub class: DisplayText,
    /// Bounded untrusted window title.
    pub title: DisplayText,
    /// Bounded workspace name supplied by the event socket.
    pub workspace_name: DisplayText,
}

/// Current focused compositor context.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ActiveContext {
    /// Focused output.
    pub output: Option<OutputName>,
    /// Focused workspace.
    pub workspace: Option<WorkspaceId>,
    /// Address-bearing focused client when known.
    pub client: Option<ClientAddress>,
    /// Sanitized class from a legacy non-address event or snapshot.
    pub class: DisplayText,
    /// Sanitized title from a legacy non-address event or snapshot.
    pub title: DisplayText,
}

/// Complete structured Hyprland snapshot applied atomically before events.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HyprlandSnapshot {
    /// Current outputs keyed by connector identity.
    pub outputs: BTreeMap<OutputName, CompositorOutput>,
    /// Current workspaces keyed by stable numeric identity.
    pub workspaces: BTreeMap<WorkspaceId, WorkspaceState>,
    /// Current clients keyed by address.
    pub clients: BTreeMap<ClientAddress, ClientState>,
    /// Current focused context.
    pub active: ActiveContext,
}

/// One parsed newline-delimited Hyprland event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HyprlandEvent {
    /// Focused workspace changed.
    WorkspaceChanged {
        /// Stable workspace identity.
        id: WorkspaceId,
        /// Bounded display name.
        name: DisplayText,
    },
    /// Focused output and its active workspace changed.
    FocusedOutput {
        /// Output connector.
        output: OutputName,
        /// Stable workspace identity.
        workspace: WorkspaceId,
        /// Bounded workspace name.
        workspace_name: DisplayText,
    },
    /// Address-bearing active client changed.
    ActiveClient {
        /// `None` represents no active client.
        address: Option<ClientAddress>,
        /// Optional legacy/snapshot class hint.
        class: DisplayText,
        /// Optional legacy/snapshot title hint.
        title: DisplayText,
    },
    /// A client was created with an address-bearing identity and workspace name.
    ClientOpened(OpenedClient),
    /// A client was removed.
    ClientClosed(ClientAddress),
    /// A client moved to another workspace.
    ClientMoved {
        /// Client identity.
        address: ClientAddress,
        /// Destination workspace.
        workspace: WorkspaceId,
        /// Destination workspace name.
        workspace_name: DisplayText,
    },
    /// An address-bearing client title changed.
    ClientTitleChanged {
        /// Client identity.
        address: ClientAddress,
        /// New bounded title.
        title: DisplayText,
    },
    /// The active client's fullscreen state changed.
    FullscreenChanged(bool),
    /// A workspace was created.
    WorkspaceCreated(WorkspaceState),
    /// A workspace was removed.
    WorkspaceDestroyed(WorkspaceId),
    /// A workspace moved to another output.
    WorkspaceMoved {
        /// Workspace identity.
        workspace: WorkspaceId,
        /// Destination output.
        output: OutputName,
    },
    /// The event cannot be reconciled safely without a fresh snapshot.
    ResnapshotRequired,
}

/// Ordered adapter-to-reducer messages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HyprlandUpdate {
    /// A new connection attempt began after fresh path discovery.
    Connecting,
    /// Complete initial state, always emitted before buffered events.
    Snapshot(HyprlandSnapshot),
    /// One parsed event after snapshot ordering is established.
    Event(HyprlandEvent),
    /// A parse, truncation, or buffering gap requires re-snapshotting.
    Gap,
    /// No current transport is available.
    Unavailable,
}

/// Current root-owned compositor domain state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DesktopState {
    /// Independent adapter availability.
    pub availability: AdapterAvailability,
    /// Current outputs.
    pub outputs: BTreeMap<OutputName, CompositorOutput>,
    /// Current workspaces.
    pub workspaces: BTreeMap<WorkspaceId, WorkspaceState>,
    /// Current clients.
    pub clients: BTreeMap<ClientAddress, ClientState>,
    /// Current focused context.
    pub active: ActiveContext,
}

/// One bounded mark rendered in an output's Selvage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceMark {
    /// Stable workspace identity.
    pub id: WorkspaceId,
    /// Sanitized accessible label.
    pub label: String,
    /// Whether the workspace is active on this output.
    pub active: bool,
    /// Whether it contains any clients.
    pub occupied: bool,
    /// Color-independent geometric state.
    pub shape: MarkShape,
    /// Color-independent fill pattern.
    pub pattern: MarkPattern,
    /// Complete accessible state label.
    pub accessible_label: String,
}

/// Geometric signal used when color is unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkShape {
    /// Small point for an empty inactive workspace.
    Dot,
    /// Horizontal bar for ordinary occupied or activity state.
    Bar,
    /// Diamond for warning state.
    Diamond,
    /// Triangle for critical or privacy state.
    Triangle,
}

/// Fill pattern used in addition to shape and color.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkPattern {
    /// Hollow or outlined state.
    Outline,
    /// Solid selected or ordinary active state.
    Solid,
    /// Striped exceptional state.
    Striped,
}

/// One bounded activity or attention signal in a stable Selvage region.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatusMark {
    /// Geometric semantic signal.
    pub shape: MarkShape,
    /// Fill pattern independent of color.
    pub pattern: MarkPattern,
    /// Semantic severity.
    pub severity: Severity,
    /// Whether this mark represents the selected Ribbon candidate.
    pub selected: bool,
    /// Optional progress in basis points.
    pub progress_basis_points: Option<u16>,
    /// Complete accessible state label.
    pub accessible_label: String,
}

/// Immutable projection consumed by one output surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputView {
    /// Interaction state.
    pub presentation: OutputPresentation,
    /// Local ordered workspaces.
    pub workspaces: Vec<WorkspaceMark>,
    /// Stable center-region activity marks.
    pub activity: Vec<StatusMark>,
    /// Stable end-region attention marks.
    pub attention: Vec<StatusMark>,
    /// Active context text, or the clock fallback.
    pub ribbon_label: String,
    /// Complete accessible label for the Ribbon button.
    pub ribbon_accessible_label: String,
    /// Typed actions advertised by the selected candidate.
    pub candidate_actions: Vec<CandidateAction>,
    /// Whether the matched compositor output is focused.
    pub focused: bool,
}

/// Visible top-edge presentation level.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PresentationLevel {
    /// Only the pointer-active 2-3 pixel Selvage is visible.
    #[default]
    Selvage,
    /// The labeled Ribbon is visible and pointer-active.
    Ribbon,
    /// The interactive Panel is explicitly open.
    Panel,
}

/// Generation token that invalidates superseded dwell and dismissal timers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InteractionToken(u64);

/// Deterministic inputs accepted by an output interaction state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionInput {
    /// Pointer entered the active top-edge region.
    PointerEntered,
    /// Pointer left the active visible region.
    PointerLeft,
    /// A previously scheduled dwell timer completed.
    DwellElapsed(InteractionToken),
    /// A previously scheduled dismissal timer completed.
    DismissElapsed(InteractionToken),
    /// The user explicitly requested the Panel from the Ribbon.
    OpenPanel,
    /// The Panel closed through Escape, outside click, or an explicit action.
    ClosePanel,
    /// The effective GTK and application motion preference changed.
    SetReducedMotion(bool),
}

/// Side effects emitted by the pure interaction reducer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionEffect {
    /// Schedule a dwell timer with the supplied generation.
    ScheduleDwell(InteractionToken),
    /// Schedule a dismissal timer with the supplied generation.
    ScheduleDismiss(InteractionToken),
    /// Re-render the output surface from authoritative state.
    Render,
}

/// Root-owned interaction state for one output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputPresentation {
    level: PresentationLevel,
    pointer_inside: bool,
    reduced_motion: bool,
    generation: u64,
}

impl OutputPresentation {
    /// Create a collapsed output presentation.
    #[must_use]
    pub const fn new(reduced_motion: bool) -> Self {
        Self {
            level: PresentationLevel::Selvage,
            pointer_inside: false,
            reduced_motion,
            generation: 0,
        }
    }

    /// Current presentation level.
    #[must_use]
    pub const fn level(&self) -> PresentationLevel {
        self.level
    }

    /// Whether reveal animation must be disabled.
    #[must_use]
    pub const fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    /// Whether the pointer is in the current input region.
    #[must_use]
    pub const fn pointer_inside(&self) -> bool {
        self.pointer_inside
    }

    /// Apply one deterministic input and return required side effects.
    pub fn update(&mut self, input: InteractionInput) -> Vec<InteractionEffect> {
        match input {
            InteractionInput::PointerEntered => self.pointer_entered(),
            InteractionInput::PointerLeft => self.pointer_left(),
            InteractionInput::DwellElapsed(token) => self.dwell_elapsed(token),
            InteractionInput::DismissElapsed(token) => self.dismiss_elapsed(token),
            InteractionInput::OpenPanel => self.open_panel(),
            InteractionInput::ClosePanel => self.close_panel(),
            InteractionInput::SetReducedMotion(reduced) => self.set_reduced_motion(reduced),
        }
    }

    fn pointer_entered(&mut self) -> Vec<InteractionEffect> {
        self.pointer_inside = true;
        let token = self.next_token();
        if self.level == PresentationLevel::Selvage {
            vec![InteractionEffect::ScheduleDwell(token)]
        } else {
            Vec::new()
        }
    }

    fn pointer_left(&mut self) -> Vec<InteractionEffect> {
        self.pointer_inside = false;
        let token = self.next_token();
        if self.level == PresentationLevel::Ribbon {
            vec![InteractionEffect::ScheduleDismiss(token)]
        } else {
            Vec::new()
        }
    }

    fn dwell_elapsed(&mut self, token: InteractionToken) -> Vec<InteractionEffect> {
        if token == self.token() && self.pointer_inside && self.level == PresentationLevel::Selvage
        {
            self.level = PresentationLevel::Ribbon;
            vec![InteractionEffect::Render]
        } else {
            Vec::new()
        }
    }

    fn dismiss_elapsed(&mut self, token: InteractionToken) -> Vec<InteractionEffect> {
        if token == self.token() && !self.pointer_inside && self.level == PresentationLevel::Ribbon
        {
            self.level = PresentationLevel::Selvage;
            vec![InteractionEffect::Render]
        } else {
            Vec::new()
        }
    }

    fn open_panel(&mut self) -> Vec<InteractionEffect> {
        if self.level == PresentationLevel::Ribbon {
            self.next_token();
            self.level = PresentationLevel::Panel;
            vec![InteractionEffect::Render]
        } else {
            Vec::new()
        }
    }

    fn close_panel(&mut self) -> Vec<InteractionEffect> {
        if self.level == PresentationLevel::Panel {
            self.next_token();
            self.level = if self.pointer_inside {
                PresentationLevel::Ribbon
            } else {
                PresentationLevel::Selvage
            };
            vec![InteractionEffect::Render]
        } else {
            Vec::new()
        }
    }

    fn set_reduced_motion(&mut self, reduced: bool) -> Vec<InteractionEffect> {
        if self.reduced_motion == reduced {
            Vec::new()
        } else {
            self.reduced_motion = reduced;
            vec![InteractionEffect::Render]
        }
    }

    fn next_token(&mut self) -> InteractionToken {
        self.generation = self.generation.wrapping_add(1);
        self.token()
    }

    const fn token(&self) -> InteractionToken {
        InteractionToken(self.generation)
    }
}

/// Root-owned application state.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AppState {
    /// Validated user configuration.
    pub config: Config,
    outputs: BTreeMap<OutputId, OutputPresentation>,
    output_connectors: BTreeMap<OutputId, Option<OutputName>>,
    /// Root-owned compositor domain state.
    pub desktop: DesktopState,
    clock_label: String,
    arbitration: Arbitrator,
    arbitration_now: Timestamp,
    selected_candidates: BTreeMap<OutputId, PresentationProjection>,
}

impl AppState {
    /// Add and remove output presentation state after shell reconciliation.
    pub fn reconcile_outputs(
        &mut self,
        added: impl IntoIterator<Item = OutputId>,
        removed: impl IntoIterator<Item = OutputId>,
        reduced_motion: bool,
    ) {
        for id in removed {
            if let Some(output) = self
                .output_connectors
                .get(&id)
                .and_then(Option::as_ref)
                .cloned()
            {
                self.arbitration.apply(
                    ArbitrationInput::OutputRemoved(output),
                    self.arbitration_now,
                );
            }
            self.outputs.remove(&id);
            self.output_connectors.remove(&id);
            self.selected_candidates.remove(&id);
        }
        for id in added {
            self.outputs
                .entry(id)
                .or_insert_with(|| OutputPresentation::new(reduced_motion));
            self.output_connectors.entry(id).or_default();
        }
        self.refresh_arbitration_selections();
    }

    /// Replace GDK-to-compositor connector bindings after output reconciliation.
    pub fn bind_outputs(&mut self, bindings: impl IntoIterator<Item = (OutputId, Option<String>)>) {
        for (id, connector) in bindings {
            if self.outputs.contains_key(&id) {
                self.output_connectors
                    .insert(id, connector.as_deref().and_then(OutputName::new));
            }
        }
        self.refresh_arbitration_selections();
    }

    /// Apply one deterministic candidate reducer input and refresh projections.
    pub fn apply_arbitration(&mut self, input: ArbitrationInput, now: Timestamp) -> Vec<OutputId> {
        self.arbitration_now = now;
        let before = self.selected_candidates.clone();
        let state_changed = self.arbitration.apply(input, now);
        self.refresh_arbitration_selections();
        if state_changed || before != self.selected_candidates {
            self.output_ids().collect()
        } else {
            Vec::new()
        }
    }

    /// Apply one ordered adapter update and return outputs whose projection changed.
    pub fn apply_hyprland_update(&mut self, update: HyprlandUpdate) -> Vec<OutputId> {
        match update {
            HyprlandUpdate::Connecting => {
                self.desktop.availability = if self.desktop.outputs.is_empty()
                    && self.desktop.workspaces.is_empty()
                    && self.desktop.clients.is_empty()
                {
                    AdapterAvailability::Starting
                } else {
                    AdapterAvailability::Stale
                };
            }
            HyprlandUpdate::Snapshot(snapshot) => {
                self.desktop = DesktopState {
                    availability: AdapterAvailability::Ready,
                    outputs: snapshot.outputs,
                    workspaces: snapshot.workspaces,
                    clients: snapshot.clients,
                    active: snapshot.active,
                };
            }
            HyprlandUpdate::Event(event) => self.apply_hyprland_event(event),
            HyprlandUpdate::Gap => self.desktop.availability = AdapterAvailability::Stale,
            HyprlandUpdate::Unavailable => {
                self.desktop.availability = if self.desktop.outputs.is_empty()
                    && self.desktop.workspaces.is_empty()
                    && self.desktop.clients.is_empty()
                {
                    AdapterAvailability::Unavailable
                } else {
                    AdapterAvailability::Stale
                };
            }
        }
        self.output_ids().collect()
    }

    /// Update the formatted local clock fallback.
    pub fn set_clock_label(&mut self, label: String) -> Vec<OutputId> {
        if self.clock_label == label {
            Vec::new()
        } else {
            self.clock_label = label;
            self.output_ids().collect()
        }
    }

    /// Build the immutable view consumed by one GTK surface.
    #[must_use]
    pub fn output_view(&self, id: OutputId) -> Option<OutputView> {
        let presentation = self.outputs.get(&id)?.clone();
        let connector = self.output_connectors.get(&id).and_then(Option::as_ref);
        let compositor_output = connector.and_then(|name| self.desktop.outputs.get(name));
        let focused = compositor_output.is_some_and(|output| output.focused);

        let mut workspaces = connector
            .into_iter()
            .flat_map(|name| {
                self.desktop
                    .workspaces
                    .values()
                    .filter(move |workspace| workspace.output.as_ref() == Some(name))
            })
            .map(|workspace| WorkspaceMark {
                id: workspace.id,
                label: workspace.name.as_str().to_owned(),
                active: self.desktop.active.workspace == Some(workspace.id),
                occupied: workspace.clients > 0,
                shape: if self.desktop.active.workspace == Some(workspace.id)
                    || workspace.clients > 0
                {
                    MarkShape::Bar
                } else {
                    MarkShape::Dot
                },
                pattern: if self.desktop.active.workspace == Some(workspace.id) {
                    MarkPattern::Solid
                } else {
                    MarkPattern::Outline
                },
                accessible_label: workspace_accessible_label(
                    workspace.name.as_str(),
                    self.desktop.active.workspace == Some(workspace.id),
                    workspace.clients > 0,
                ),
            })
            .collect::<Vec<_>>();
        workspaces.sort_by_key(|workspace| workspace.id);
        workspaces.truncate(MAX_NAVIGATION_MARKS);

        let fallback_label = if focused {
            self.active_context_label()
        } else {
            compositor_output
                .and_then(|output| output.active_workspace)
                .and_then(|workspace| self.desktop.workspaces.get(&workspace))
                .map(|workspace| workspace.name.as_str().to_owned())
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| self.clock_fallback())
        };
        let candidate = self.selected_candidates.get(&id);
        let detail_output = self.global_detail_output();
        let detailed_candidate = candidate
            .filter(|projection| projection.output_affinity.is_some() || detail_output == Some(id));
        let ribbon_label = detailed_candidate
            .map(|projection| projection.label.clone())
            .filter(|label| !label.is_empty())
            .unwrap_or(fallback_label);
        let ribbon_accessible_label = detailed_candidate
            .map(|projection| projection.accessible_label.clone())
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| format!("Ribbon: {ribbon_label}"));
        let (mut activity, mut attention) =
            candidate.map(candidate_status_marks).unwrap_or_default();
        activity.truncate(MAX_ACTIVITY_MARKS);
        attention.truncate(MAX_ATTENTION_MARKS);

        Some(OutputView {
            presentation,
            workspaces,
            activity,
            attention,
            ribbon_label,
            ribbon_accessible_label,
            candidate_actions: detailed_candidate
                .map(|projection| projection.actions.clone())
                .unwrap_or_default(),
            focused,
        })
    }

    /// Look up an output presentation.
    #[must_use]
    pub fn output(&self, id: OutputId) -> Option<&OutputPresentation> {
        self.outputs.get(&id)
    }

    /// Mutably look up an output presentation.
    pub fn output_mut(&mut self, id: OutputId) -> Option<&mut OutputPresentation> {
        self.outputs.get_mut(&id)
    }

    /// Iterate over output identities without exposing shell objects.
    pub fn output_ids(&self) -> impl Iterator<Item = OutputId> + '_ {
        self.outputs.keys().copied()
    }

    fn refresh_arbitration_selections(&mut self) {
        let bindings = self
            .outputs
            .keys()
            .map(|id| {
                (
                    *id,
                    self.output_connectors
                        .get(id)
                        .and_then(Option::as_ref)
                        .cloned(),
                )
            })
            .collect::<Vec<_>>();
        self.selected_candidates.clear();
        for (id, output) in bindings {
            if let Some(projection) = self
                .arbitration
                .select_for(output.as_ref(), self.arbitration_now)
            {
                self.selected_candidates.insert(id, projection);
            }
        }
    }

    fn global_detail_output(&self) -> Option<OutputId> {
        self.outputs
            .keys()
            .copied()
            .find(|id| {
                self.output_connectors
                    .get(id)
                    .and_then(Option::as_ref)
                    .and_then(|name| self.desktop.outputs.get(name))
                    .is_some_and(|output| output.focused)
            })
            .or_else(|| self.outputs.keys().next().copied())
    }

    fn active_context_label(&self) -> String {
        let title = self
            .desktop
            .active
            .client
            .as_ref()
            .and_then(|address| self.desktop.clients.get(address))
            .map(|client| client.title.as_str())
            .filter(|title| !title.is_empty())
            .or_else(|| {
                (!self.desktop.active.title.is_empty())
                    .then_some(self.desktop.active.title.as_str())
            });
        let workspace = self
            .desktop
            .active
            .workspace
            .and_then(|id| self.desktop.workspaces.get(&id))
            .map(|workspace| workspace.name.as_str())
            .filter(|name| !name.is_empty());

        match (workspace, title) {
            (Some(workspace), Some(title)) => format!("{workspace} · {title}"),
            (Some(workspace), None) => workspace.to_owned(),
            (None, Some(title)) => title.to_owned(),
            (None, None) => self.clock_fallback(),
        }
    }

    fn clock_fallback(&self) -> String {
        if self.clock_label.is_empty() {
            "--:--".to_owned()
        } else {
            self.clock_label.clone()
        }
    }

    fn apply_hyprland_event(&mut self, event: HyprlandEvent) {
        match event {
            HyprlandEvent::WorkspaceChanged { id, name } => {
                self.desktop.active.workspace = Some(id);
                self.desktop
                    .workspaces
                    .entry(id)
                    .and_modify(|workspace| workspace.name = name.clone())
                    .or_insert(WorkspaceState {
                        id,
                        name,
                        output: self.desktop.active.output.clone(),
                        clients: 0,
                        fullscreen: false,
                    });
                if let Some(output) = self.desktop.active.output.as_ref()
                    && let Some(output) = self.desktop.outputs.get_mut(output)
                {
                    output.active_workspace = Some(id);
                }
            }
            HyprlandEvent::FocusedOutput {
                output,
                workspace,
                workspace_name,
            } => {
                for candidate in self.desktop.outputs.values_mut() {
                    candidate.focused = candidate.name == output;
                }
                self.desktop.active.output = Some(output.clone());
                self.desktop.active.workspace = Some(workspace);
                if let Some(candidate) = self.desktop.outputs.get_mut(&output) {
                    candidate.active_workspace = Some(workspace);
                }
                self.desktop
                    .workspaces
                    .entry(workspace)
                    .and_modify(|candidate| {
                        if !workspace_name.is_empty() {
                            candidate.name = workspace_name.clone();
                        }
                        candidate.output = Some(output.clone());
                    })
                    .or_insert(WorkspaceState {
                        id: workspace,
                        name: if workspace_name.is_empty() {
                            DisplayText::new(&workspace.get().to_string(), 128)
                        } else {
                            workspace_name
                        },
                        output: Some(output),
                        clients: 0,
                        fullscreen: false,
                    });
            }
            HyprlandEvent::ActiveClient {
                address,
                class,
                title,
            } => {
                if address.is_some() || (class.is_empty() && title.is_empty()) {
                    self.desktop.active.client = address;
                }
                self.desktop.active.class = class;
                self.desktop.active.title = title;
            }
            HyprlandEvent::ClientOpened(opened) => {
                let Some(workspace) = self
                    .desktop
                    .workspaces
                    .values()
                    .find(|workspace| workspace.name == opened.workspace_name)
                    .map(|workspace| workspace.id)
                else {
                    self.desktop.availability = AdapterAvailability::Stale;
                    return;
                };
                let client = ClientState {
                    address: opened.address,
                    class: opened.class,
                    title: opened.title,
                    workspace,
                    output: self
                        .desktop
                        .workspaces
                        .get(&workspace)
                        .and_then(|workspace| workspace.output.clone()),
                    fullscreen: false,
                };
                let previous = self
                    .desktop
                    .clients
                    .insert(client.address.clone(), client.clone());
                if previous.as_ref().map(|old| old.workspace) != Some(client.workspace) {
                    if let Some(old) = previous
                        && let Some(workspace) = self.desktop.workspaces.get_mut(&old.workspace)
                    {
                        workspace.clients = workspace.clients.saturating_sub(1);
                    }
                    if let Some(workspace) = self.desktop.workspaces.get_mut(&client.workspace) {
                        workspace.clients = workspace.clients.saturating_add(1);
                    }
                }
            }
            HyprlandEvent::ClientClosed(address) => {
                if let Some(client) = self.desktop.clients.remove(&address)
                    && let Some(workspace) = self.desktop.workspaces.get_mut(&client.workspace)
                {
                    workspace.clients = workspace.clients.saturating_sub(1);
                }
                if self.desktop.active.client.as_ref() == Some(&address) {
                    self.desktop.active.client = None;
                    self.desktop.active.class = DisplayText::default();
                    self.desktop.active.title = DisplayText::default();
                }
            }
            HyprlandEvent::ClientMoved {
                address,
                workspace,
                workspace_name,
            } => {
                if let Some(client) = self.desktop.clients.get_mut(&address) {
                    let previous = client.workspace;
                    client.workspace = workspace;
                    client.output = self
                        .desktop
                        .workspaces
                        .get(&workspace)
                        .and_then(|candidate| candidate.output.clone());
                    if previous == workspace {
                        if let Some(candidate) = self.desktop.workspaces.get_mut(&workspace) {
                            candidate.name = workspace_name;
                        }
                    } else {
                        if let Some(candidate) = self.desktop.workspaces.get_mut(&previous) {
                            candidate.clients = candidate.clients.saturating_sub(1);
                        }
                        self.desktop
                            .workspaces
                            .entry(workspace)
                            .and_modify(|candidate| {
                                candidate.clients = candidate.clients.saturating_add(1);
                                candidate.name = workspace_name.clone();
                            })
                            .or_insert(WorkspaceState {
                                id: workspace,
                                name: workspace_name,
                                output: None,
                                clients: 1,
                                fullscreen: false,
                            });
                    }
                }
            }
            HyprlandEvent::ClientTitleChanged { address, title } => {
                if let Some(client) = self.desktop.clients.get_mut(&address) {
                    client.title = title.clone();
                }
                if self.desktop.active.client.as_ref() == Some(&address) {
                    self.desktop.active.title = title;
                }
            }
            HyprlandEvent::FullscreenChanged(fullscreen) => {
                if let Some(address) = self.desktop.active.client.as_ref()
                    && let Some(client) = self.desktop.clients.get_mut(address)
                {
                    client.fullscreen = fullscreen;
                }
            }
            HyprlandEvent::WorkspaceCreated(workspace) => {
                self.desktop
                    .workspaces
                    .entry(workspace.id)
                    .and_modify(|existing| existing.name = workspace.name.clone())
                    .or_insert(workspace);
            }
            HyprlandEvent::WorkspaceDestroyed(workspace) => {
                self.desktop.workspaces.remove(&workspace);
                if self.desktop.active.workspace == Some(workspace) {
                    self.desktop.active.workspace = None;
                }
            }
            HyprlandEvent::WorkspaceMoved { workspace, output } => {
                if let Some(workspace) = self.desktop.workspaces.get_mut(&workspace) {
                    workspace.output = Some(output);
                }
            }
            HyprlandEvent::ResnapshotRequired => {
                self.desktop.availability = AdapterAvailability::Stale;
            }
        }
    }
}

fn workspace_accessible_label(label: &str, active: bool, occupied: bool) -> String {
    let state = match (active, occupied) {
        (true, true) => "active, occupied",
        (true, false) => "active, empty",
        (false, true) => "inactive, occupied",
        (false, false) => "inactive, empty",
    };
    format!("Workspace {label}: {state}")
}

fn candidate_status_marks(
    projection: &PresentationProjection,
) -> (Vec<StatusMark>, Vec<StatusMark>) {
    let shape = match (projection.kind, projection.severity) {
        (PresentationKind::Privacy, _) | (_, Severity::Critical) => MarkShape::Triangle,
        (PresentationKind::Warning, _) | (_, Severity::Warning) => MarkShape::Diamond,
        _ => MarkShape::Bar,
    };
    let pattern = match projection.kind {
        PresentationKind::Privacy | PresentationKind::Warning => MarkPattern::Striped,
        _ if projection.severity >= Severity::Warning => MarkPattern::Striped,
        _ => MarkPattern::Solid,
    };
    let mark = StatusMark {
        shape,
        pattern,
        severity: projection.severity,
        selected: true,
        progress_basis_points: projection.progress.map(|progress| progress.basis_points()),
        accessible_label: projection.accessible_label.clone(),
    };
    match projection.kind.region() {
        CandidateRegion::None => (Vec::new(), Vec::new()),
        CandidateRegion::Activity => (vec![mark], Vec::new()),
        CandidateRegion::Attention => (Vec::new(), vec![mark]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::arbitration::{
        CandidateId, CandidateSource, PreemptionClass, PresentationCandidate,
    };

    fn scheduled_dwell(state: &mut OutputPresentation) -> InteractionToken {
        let effects = state.update(InteractionInput::PointerEntered);
        let [InteractionEffect::ScheduleDwell(token)] = effects.as_slice() else {
            panic!("pointer entry must schedule dwell");
        };
        *token
    }

    #[test]
    fn dwell_reveals_only_while_pointer_remains_inside() {
        let mut state = OutputPresentation::new(false);
        let token = scheduled_dwell(&mut state);
        state.update(InteractionInput::PointerLeft);
        assert!(
            state
                .update(InteractionInput::DwellElapsed(token))
                .is_empty()
        );
        assert_eq!(state.level(), PresentationLevel::Selvage);
    }

    #[test]
    fn completed_dwell_reveals_the_ribbon() {
        let mut state = OutputPresentation::new(false);
        let token = scheduled_dwell(&mut state);
        assert_eq!(
            state.update(InteractionInput::DwellElapsed(token)),
            vec![InteractionEffect::Render]
        );
        assert_eq!(state.level(), PresentationLevel::Ribbon);
    }

    #[test]
    fn interrupted_dismissal_cannot_collapse_the_ribbon() {
        let mut state = OutputPresentation::new(false);
        let dwell = scheduled_dwell(&mut state);
        state.update(InteractionInput::DwellElapsed(dwell));
        let effects = state.update(InteractionInput::PointerLeft);
        let [InteractionEffect::ScheduleDismiss(dismiss)] = effects.as_slice() else {
            panic!("pointer leave must schedule dismissal");
        };
        state.update(InteractionInput::PointerEntered);
        assert!(
            state
                .update(InteractionInput::DismissElapsed(*dismiss))
                .is_empty()
        );
        assert_eq!(state.level(), PresentationLevel::Ribbon);
    }

    #[test]
    fn panel_is_pinned_until_explicitly_closed() {
        let mut state = OutputPresentation::new(false);
        let dwell = scheduled_dwell(&mut state);
        state.update(InteractionInput::DwellElapsed(dwell));
        state.update(InteractionInput::OpenPanel);
        assert_eq!(state.level(), PresentationLevel::Panel);

        state.update(InteractionInput::PointerLeft);
        assert_eq!(state.level(), PresentationLevel::Panel);
        state.update(InteractionInput::ClosePanel);
        assert_eq!(state.level(), PresentationLevel::Selvage);
    }

    #[test]
    fn reduced_motion_changes_render_without_changing_level() {
        let mut state = OutputPresentation::new(false);
        assert_eq!(
            state.update(InteractionInput::SetReducedMotion(true)),
            vec![InteractionEffect::Render]
        );
        assert!(state.reduced_motion());
        assert_eq!(state.level(), PresentationLevel::Selvage);
    }

    #[test]
    fn global_candidate_detail_appears_only_on_the_focused_output() {
        let first_id = OutputId::new(1);
        let second_id = OutputId::new(2);
        let first_name = OutputName::new("SYNTH-1").expect("output name");
        let second_name = OutputName::new("SYNTH-2").expect("output name");
        let mut state = AppState::default();
        state.reconcile_outputs([first_id, second_id], [], false);
        state.bind_outputs([
            (first_id, Some("SYNTH-1".to_owned())),
            (second_id, Some("SYNTH-2".to_owned())),
        ]);
        state.desktop.outputs.insert(
            first_name.clone(),
            CompositorOutput {
                id: 1,
                name: first_name,
                focused: false,
                scale_milli: 1_000,
                active_workspace: None,
                fullscreen: false,
            },
        );
        state.desktop.outputs.insert(
            second_name.clone(),
            CompositorOutput {
                id: 2,
                name: second_name,
                focused: true,
                scale_milli: 1_000,
                active_workspace: None,
                fullscreen: false,
            },
        );
        state.set_clock_label("12:00".to_owned());
        state.apply_arbitration(
            ArbitrationInput::Upsert(PresentationCandidate {
                id: CandidateId::new("global-build").expect("candidate id"),
                source: CandidateSource::Activity,
                kind: PresentationKind::Activity,
                severity: Severity::Normal,
                label: DisplayText::new("Build complete", 64),
                accessible_label: DisplayText::new("Build complete", 128),
                created_at: Timestamp::from_millis(1),
                updated_at: Timestamp::from_millis(1),
                expires_at: None,
                minimum_display: Duration::ZERO,
                preemption: PreemptionClass::Passive,
                progress: None,
                actions: vec![CandidateAction::RevealDetails],
                output_affinity: None,
            }),
            Timestamp::from_millis(1),
        );

        let first = state.output_view(first_id).expect("first view");
        let second = state.output_view(second_id).expect("second view");
        assert_eq!(first.ribbon_label, "12:00");
        assert!(first.candidate_actions.is_empty());
        assert_eq!(first.activity.len(), 1);
        assert_eq!(second.ribbon_label, "Build complete");
        assert_eq!(
            second.candidate_actions,
            vec![CandidateAction::RevealDetails]
        );
        assert_eq!(second.activity.len(), 1);
    }
}
