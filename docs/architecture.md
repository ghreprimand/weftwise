# Architecture

Weftwise is a native Wayland process with explicit boundaries between desktop
protocols, application policy, and GTK presentation.

```text
Hyprland sockets ---------+
D-Bus services -----------+
PipeWire graph/metadata --+--> service adapters --> typed messages
process supervision ------+                         |
clock and sensors --------+                         v
                                               root reducer
                                                    |
                                 +------------------+------------------+
                                 v                  v                  v
                           arbitration       surface manager    action dispatch
                                 |                  |                  |
                                 +------------------+------------------+
                                                    v
                                             Relm4 components
```

## Ownership

The root application model is the authoritative owner of product state. It
contains compositor, output, workspace, client, context, media, audio, privacy,
process, notification, configuration, theme, and panel state.

Child components receive immutable projections suitable for rendering and emit
typed actions. They do not retain a second copy of domain state. Temporary GTK
state is acceptable only when it has no meaning outside the widget, such as
pointer hover or an animation fraction.

## Message flow

Service adapters convert transport-specific values into internal messages. The
root reducer applies each message deterministically and then recomputes affected
presentation candidates. Rendering observes the resulting state. User input
travels in the opposite direction as typed actions handled by the dispatcher.

No background task mutates a GTK object. GTK and Relm4 components remain on the
main thread.

## Context arbitration

Presentation producers publish bounded candidates rather than selecting GTK
widgets. A candidate has a protocol-safe source-scoped stable identity, typed source and kind,
semantic severity, normalized creation/update/expiration timestamps, a bounded
minimum-display interval, preemption class, optional basis-point progress,
bounded typed actions, and optional output affinity. Visible and accessible
labels use the existing sanitized display-text boundary.

The root-owned pure reducer deduplicates by source and identity, updates candidates in
place, expires old content, removes stale producers, and retains per-output
selection memory. Ranking is total and deterministic. An active candidate stays
selected through its minimum interval unless a higher preemption class arrives;
privacy-critical content is the highest class. Logically late delivery is
reconciled through normalized timestamps so equal candidate sets do not depend
on insertion order. Expiration always releases stickiness and reveals the next
ranked candidate, including the clock fallback.

## Privacy evidence

Privacy evidence is a transport-independent domain that produces privacy
candidates without performing any I/O; selected evidence adapters feed
observations into it. Each source
(microphone, camera, screen sharing, recording, and idle inhibitor) keeps one
of five distinct states: active, inactive, unknown, unavailable, and stale.
These states never collapse into one another. A confirmed-off source is
inactive; a supported source that has not reported yet is unknown; a source
whose observation failed is unavailable; and a previously observed source that
can no longer be trusted is stale.

A source is unsupported until an adapter declares it, and unsupported detections
are silent rather than reported as inactive. Only supported sources produce
candidates: an active capture is privacy-critical, an active idle inhibitor is
an interruptible warning, and a failure state (unavailable or stale) produces an
uncertainty candidate so a degraded source stays visible instead of silently
implying that nothing is happening.

The selected native audio boundary observes PipeWire registry, node, device,
link, parameter, and default-metadata changes on one supervisor-owned thread.
WirePlumber remains the session policy owner. PipeWire capture graphs provide
scoped positive microphone, camera, and screen-capture evidence; their absence
cannot prove that direct ALSA, V4L2, or libcamera clients are inactive.
Hyprland screencast lifecycle events supplement screen-capture evidence, while
Hyprland client snapshots and logind inhibitor snapshots provide complementary
idle-inhibition evidence. Recording and network sharing cannot be
distinguished from capture by these transports and remain unsupported as
separate detections. Any incomplete, ambiguous, disconnected, or gap-affected
source remains unknown, unavailable, or stale rather than becoming inactive.

Microphone and camera evidence is derived by a bounded capture graph that rides
the same supervisor-owned PipeWire loop as the audio adapter. The graph retains
only numeric registry identities and small closed enums: a device's backing API,
a node's capture role, a node's running flag, a port's direction, and a link's
active flag. Classification is exact, not substring based: a source is the exact
`media.class` `Audio/Source` or `Video/Source`, a terminal is the exact
`Stream/Input/Audio` or `Stream/Input/Video`, and a backing device is the exact
`device.api` `alsa`, `bluez5`, `v4l2`, or `libcamera`, so decorated or near-match
values such as `Audio/Source/Virtual` or `alsa-evil` are excluded. Client,
application, and media names; node and device descriptions; object paths;
serials; and process arguments are not retained. The few properties needed for
classification are inspected transiently, and only their bounded classification
result is kept.

A source is proven active only by a complete, running, single-link direct path:
an active link whose input port belongs to a running `Stream/Input/Audio` or
`Stream/Input/Video` terminal and whose output port belongs to a running,
non-virtual, non-monitor `Audio/Source` or `Video/Source` node backed by an ALSA
or BlueZ5 device for microphones or a V4L2 or libcamera device for cameras. Only
this direct hardware-source-to-terminal shape is proven; capture routed through
intermediate filter nodes such as an echo-cancel or loopback chain that inserts a
virtual source is a deliberate known false negative and stays unknown rather than
being asserted as active. The absence of a proven path over a complete graph is
reported as unknown rather than inactive, because the graph cannot prove a client
is not capturing through a path it does not model.

The graph is size-bounded per object kind and preserves a node's running flag or
a link's active flag only across an identical re-announcement; a role, backing,
or endpoint change resets that flag so a reclassified object cannot inherit a
stale positive state. Exceeding a bound marks the graph overflowed, which is
sticky for the connection and reported as a change so a previously trusted source
does not silently persist. Readiness and trust are modeled separately. Readiness
uses two core-sync barriers: the first establishes registry enumeration and
drives the audio snapshot, and the second flushes the node and link info replies
that binding requested. A graph is ready once the second barrier completes, and a
ready graph republishes resolved states on every later change; it is trusted only
when that barrier completed over a complete, non-overflowed graph. A ready but
incomplete graph still proves an active path as active while reporting an absent
source as unavailable, and a later disappearance of that path becomes unavailable
immediately. A trusted graph reports an absent path as unknown and degrades it to
stale if a later overflow makes the graph incomplete. If the second barrier
cannot be established, the graph is neither ready nor trusted and every source is
published as pre-trust unavailable. Attempt trust is reset on every reconnect;
a connection lost before the second barrier makes microphone and camera
unavailable, and a loss after a trusted barrier makes them stale, without
degrading the other privacy sources.

The Hyprland adapter consumes address-free `screencastv2` lifecycle events and
counts concurrent screencopy clients with bounded state. It validates the
monitor, window, or region owner category but discards the shared target name.
Only a positive count is active. Zero remains unknown because socket2 provides
no initial screencast snapshot, and a parse gap or socket loss changes the
source to stale or unavailable before reconnect.

The implemented logind adapter subscribes to login1 service-owner and manager
property changes before taking a bounded `ListInhibitors` snapshot. It retains
none of the returned owner, reason, mode, user, or process fields and publishes
only whether an exact `idle` target is present. Positive evidence is active; an
empty snapshot is unknown rather than inactive because logind cannot observe
Hyprland's Wayland idle-inhibit protocol. Oversized responses and transport
loss are unavailable, and service-owner changes force a fresh connection and
snapshot with bounded reconnect backoff.

## Audio control

The audio domain is transport-independent typed state: bounded sink and source
nodes with fixed-point linear volume, mute, availability, and per-node
capabilities, plus the resolved default sink and source. The direct PipeWire
adapter runs one supervisor-owned loop thread because `libpipewire` objects are
neither `Send` nor `Sync`. It binds node and default-metadata globals, parses
each node's `Props` parameter for channel volumes and mute, and resolves the
default sink and source from the standard `default` metadata that WirePlumber
owns. Initial registry changes remain private to the adapter until a PipeWire
Core sync completes, then one bounded snapshot establishes root state before
incremental updates are published. A Tokio task bridges typed commands into the
loop thread and forwards typed updates back out; the retained command sender
keeps that supervised transport alive. No `wpctl` subprocess is polled, and no
`wireplumber` crate is used.

Volume, mute, and default-route commands are typed and capability-gated by the
root before dispatch. A rejected request produces content-free error feedback
rather than a silent failure. Volume and microphone-mute changes on the default
nodes produce temporary feedback candidates through the shared emitter. The
displayed volume percentage uses the conventional cubic mapping and remains
display-only pending native verification; the pod codec that reads and writes
volume is verified by a serialize/parse round-trip, while live-server behavior
is unmeasured.

Default-route selection cooperates with WirePlumber policy rather than
overriding it. Selecting a default sink or source writes the persistent
`default.configured.audio.sink` or `default.configured.audio.source` key of the
`default` metadata as a `Spa:String:JSON` `{"name":"<node.name>"}` value, the
same mechanism `wpctl set-default` uses. WirePlumber validates the request,
applies it, and republishes the resulting `default.audio.*` selection, which
returns through the metadata property listener as a default-changed update.
Weftwise never writes the runtime `default.audio.*` output key directly. The
selection is capability-gated to an available node of the requested direction,
and the metadata key and JSON value builders are verified by unit tests; the
live metadata round-trip against a running WirePlumber remains unmeasured here.

Moving the active playback stream to a chosen sink also cooperates with
WirePlumber rather than mutating links. The transport keeps a bounded, numeric
inventory of playback streams whose `media.class` equals `Stream/Output/Audio`
exactly after trimming, so a decorated or near-match class is never tracked as a
movable stream. Each entry is keyed by registry identity and retains only a
running flag, a movable flag derived from `node.dont-move`, and whether the
stream subject grants the metadata (`PW_PERM_M`) permission a `target.object`
write requires; no application, media, or process identity is kept. Selection is
deterministic: exactly one running, movable, metadata-permitted stream is the
active subject, zero is unavailable, more than one is ambiguous, and an
inventory overflow, an explicit `linking.allow-moving-streams` denial in the
`sm-settings` metadata, or a `default` metadata object without write and execute
permission disables the action. A running movable stream whose subject lacks the
metadata permission cannot be moved, so it is not counted rather than offered.
Only the active state offers a move. A move writes the `target.object` key of
the `default` metadata with the stream's identity as the subject and the
destination sink's decimal `object.serial` as a `Spa:Id` value; WirePlumber owns
the resulting relink, so a successful write acknowledges only that a request was
sent and never asserts the graph moved. Because opaque registry IDs are reused
across a PipeWire reconnect or service restart, every connection attempt is
stamped with a monotonic generation carried inside the published selection and
back through the validated move command; a move queued against a stale
connection fails the generation recheck at dispatch instead of retargeting a coincidentally matching
new stream or sink. The selection rules, exact class match, subject and metadata
permission gates, tri-state policy parsing, generation freshness check, and
target value builder are verified by unit tests; the live relink against a
running WirePlumber remains unmeasured here. The adapter and its optional
`pipewire` dependency compile with the `audio-transport` feature, which is
enabled by default on the supported native baseline. A transport-free
`--no-default-features` build retains the pure domain and typed contracts.
The transport for this adapter lives in a dedicated `audio/transport.rs` module
so the always-compiled pure model stays well under the file-size limits.

## Local activity protocol

Protocol version 1 is a transport-independent newline-delimited JSON schema for
timer, build, download, render, and command-result display activity. Each frame
is limited to 16 KiB before parsing. It carries a protocol-safe stable identity,
sanitized bounded labels, optional progress in zero through 10,000 basis
points, and an optional lifetime no longer than 24 hours. Publish, update,
complete, and cancel are the only operations; completion outcomes are typed as
succeeded or failed. Unknown fields, operations, enum values, and schema
versions are rejected with payload-free diagnostics.

The supervised endpoint is `activity-v1.sock` beneath the application XDG
runtime directory. It refuses a symlinked or group/world-accessible runtime
base, requires that base to match the effective process user, verifies every
peer through Linux socket credentials, and applies `0700` to the application
directory and `0600` to the socket. At most eight clients are processed at
once. Each client has a 30-second idle deadline and may submit 64 frames per
one-second window. Existing regular files and live sockets are never replaced;
an owned refused socket is removed as stale, with its device and inode checked
again before unlinking. Endpoint cleanup also verifies the created socket's
device and inode. After validating and handing off a frame, the endpoint writes
a fixed acknowledgement. The synchronous CLI verifies the private endpoint,
uses bounded read and write deadlines, and treats a missing or invalid
acknowledgement as failure. The protocol deliberately has no executable,
argument vector, shell command, environment, output text, or arbitrary metadata
field.

Validated activity is emitted as typed root messages. Root state retains at
most 128 live identities and projects publish/update state through the activity
arbitration source. Completion becomes bounded temporary success or warning
feedback, while cancellation removes the source-scoped identity. The publishing
CLI exposes all four operations through typed positional and named arguments.

The separate `weftwise reveal` command uses GApplication's session-bus remote
action mechanism rather than the activity schema. The primary GTK application
exports a parameter-free action and maps it to a root message. Root state
selects the Hyprland-focused output and applies a generation-checked 2.5-second
Ribbon glance. A second request within 1500 milliseconds applies a distinct
typed Ribbon-pin transition and invalidates the glance dismissal without
opening the Panel. A pinned Ribbon has no click-away guard: it is collapsed
only by a further `reveal` request, which the reducer maps to an `UnpinRibbon`
transition. The remote handler treats a pinned Ribbon or an open Panel as
held-open and sends `UnpinRibbon` on the next request, so the shortcut toggles
the surface closed rather than re-pinning it. Pointer entry still invalidates an
ordinary glance dismissal generation and returns ownership to the interaction
reducer. Because there is no focus-grab surface, no invisible popover, and no
rearm timer, an ordinary click never steals focus from the client beneath the
Ribbon, and the pin state is described entirely by the reducer. The keyboard pin
survives a Panel round-trip: opening the Panel preserves the pin and closing it
restores the pinned Ribbon. Hyprland remains the global shortcut owner, so key
selection is configuration rather than a GTK grab.

## Temporary feedback

Temporary feedback confirms a discrete change: a volume or brightness step, a
microphone toggle, a screenshot, or a launched command's result. A pure emitter
turns those events into bounded, self-expiring feedback candidates. Each kind
maps to one stable identity, so repeated events update a single candidate in
place. Every candidate carries an explicit expiration, and its untrusted text
and progress are bounded before emission. A per-kind minimum interval
rate-limits rapid streams such as a dragged volume slider: events inside the
interval are coalesced to the latest value and flushed once the interval
elapses, so the final value is never lost. Only already-defined typed actions
appear; result-style feedback offers explicit dismissal.

## Async supervision

One application supervisor owns long-lived adapter tasks and their cancellation
tokens. Each adapter:

- establishes an initial snapshot before relying on incremental events;
- prefers subscriptions over polling where the service supports them;
- tolerates unknown messages and malformed individual events;
- reconnects with bounded exponential backoff and jitter;
- reports availability as explicit state;
- stops promptly during application shutdown; and
- cannot terminate unrelated adapters.

CPU, memory, temperature, and similar measurements may require bounded sampling.
They use in-process system interfaces where practical rather than spawning a
command for each refresh. Sampling frequency follows presentation need and does
not turn hidden exact values into continuous UI work.

### Hyprland ordering

The Hyprland adapter resolves the active instance below `XDG_RUNTIME_DIR` on
every connection attempt and never logs the instance identifier, PID, Wayland
display, or socket paths. The environment signature is trusted only for the
first connection; every reconnect rescans `XDG_RUNTIME_DIR/hypr` so a compositor
restart that mints a fresh signature is followed to its new instance rather than
retried on a stale socket path. Scanning is bounded and validates each candidate
directory as an owned, non-symlink directory whose `.socket.sock` and
`.socket2.sock` are owned, non-symlink Unix sockets; a definitively dead PID
(parsed defensively from the instance lock or log and confirmed absent under
`/proc`) excludes a candidate, while an unparseable or unknown PID is retained.
Candidates are ranked deterministically by Wayland-display affinity, signature
timestamp, lock or directory recency, then lexically. Wayland-display affinity
reads the display token from the instance lock's second line, which the
compositor writes directly after the PID, and falls back to scanning the log
only when the lock declares no display. Ranking is only an
ordering hint: the authoritative liveness proof is a successful event-socket
connection plus a complete five-request snapshot, so a stale directory that
ranks first is skipped when it fails to answer. No empty request-socket probe is
used. The adapter connects the newline-delimited event socket first; while five
JSON snapshots are requested through fresh, strictly timed, size-bounded request
connections, parsed events enter a count- and byte-bounded buffer, and the root
receives one atomic snapshot before those events replay in wire order. If a
known event loses its payload while the snapshot is being taken, the snapshot is
retaken exactly once: a tracked change was observed but its content is
unrecoverable, so the first read may already be stale.

Event records split only at the first `>>` and are tolerated per record. An
over-long line is discarded to the next newline and skipped rather than ending
the session; a non-UTF-8 line, an unknown event name, or a line without the
delimiter is skipped because nothing tracked can have changed; paired legacy
workspace, focused-output, move, and title events are ignored in favor of their
address-bearing v2 counterparts. A genuine state gap - a monitor or workspace
lifecycle event that invalidates cached identity, a known event whose payload
fails to parse, or an unresolved workspace name - triggers an in-place
resnapshot on the same event socket,
bounded to three consecutive repairs before the session falls back to a full
reconnect and rescan. A clean event resets the repair budget, and the first
repair emits a single transient gap signal. Buffer overflow, a truncated read,
or a clean disconnect ends the session for reconnect. Bounded exponential
backoff includes jitter.

The IPC adapter therefore recovers across a compositor restart within the same
process: on reconnect it rediscovers the new instance and re-establishes state.
GTK's Wayland connection is still closed when the compositor exits, so the
layer-shell surfaces themselves do not claim same-process survival of a full
restart; native acceptance for that lifecycle starts a fresh process after an
orderly session cycle. A public discovery seam accepts an injected runtime scan
and process-liveness probe, so synthetic transport tests rotate safe instance
directories and exercise the exact reconnect, ordering, and per-record tolerance
path without a live compositor and without implying GTK survival.
The clock is a separate supervised in-process adapter and aligns each update to
the next wall-clock minute rather than spawning or periodically drifting.
Application shutdown first broadcasts cooperative cancellation, then joins the
owned tasks under a bounded 100-millisecond grace on a dedicated runtime off the
GTK thread rather than busy-waiting the caller. Tasks still running at the
deadline are aborted, and every join result is reaped so an adapter panic is
observed as a redacted category count instead of being silently dropped. The
PipeWire loop-thread join runs on the blocking pool for the same reason. This
preserves prompt GTK shutdown without racing every cancellation receiver.

### MPRIS ordering and ownership

The MPRIS adapter creates its session-bus connection inside the entered Relm4
Tokio runtime. It subscribes to `NameOwnerChanged` and player
`PropertiesChanged` signals before listing players and publishing the initial
snapshot. Unique-owner identity and a local owner generation map property
signals and commands back to bounded well-known MPRIS names. A player restart
replaces or removes only that player, and stale commands cannot cross the owner
generation. Session-bus loss marks retained media state stale, removes its
presentation candidate, and reconnects with bounded backoff without affecting
other adapters.

Raw D-Bus values do not enter GTK or root state. The adapter bounds player
identity, title, artists, artwork URL, duration, position, and capabilities,
then publishes complete typed player snapshots. Artwork URLs are retained only
after scheme and length checks and are not fetched. Root state chooses playing,
paused, then recently stopped players, excludes unknown playback states, and
uses playback activity plus stable identity for ties. It derives one media
candidate and capability-gated typed commands. Commands return through a
bounded channel to the adapter and invoke explicit MPRIS methods; no shell
strings are accepted.

## Surface model

A surface manager owns one top-anchored overlay-layer surface per GDK output.
Monitor, layer, anchors, namespace, keyboard mode, and the candidate exclusive
zone are set before presentation. The fixed 30-pixel visual allocation is tall
enough for the Ribbon. The 3-pixel visual Selvage is distinct from its collapsed
GDK input region: a bounded island selected from the widest exposed top-edge
segment across the current GDK output layout. The region is first applied after
realization and recomputed directly from the GDK surface layout callback's
logical width and after output or scale notifications. The pre-layout region is
deliberately empty; the first positive layout replaces it synchronously so
root-message latency cannot leave a stale empty region. Pointer entry and exit
are observed in capture phase on the fixed-height root widget.

The collapsed region also includes matching narrow legs down the left and right
edges of the fixed 30-pixel surface. Either can be entered horizontally, so a
lower output whose entire top edge leads into another monitor does not require
an impossible physical-edge dwell. The legs disappear with the rest of the
collapsed input region when the Ribbon opens. When the target's top, left, and
right entry edges all adjoin other outputs, the surface emits an
immediate-entry action rather than scheduling a dwell that pointer traversal
cannot complete. GDK logical
rectangles and a two-pixel fractional-layout tolerance determine adjacency, so
the rule has no connector, resolution, or machine-specific cases. Collapsed
workspace and status marks are hidden at the same transition instead of
overlaying Ribbon content.

The native proof defaults to `exclusive_zone = -1` and retains `0` as a manual
comparison value. A native four-output Hyprland session with Waybar showed that
neither value changed the existing reserved work area, while only `-1` placed
every Weftwise surface at the physical top edge. Pointer pass-through, stacking,
and focus behavior remain separate manual checks.

The first interactive Panel is an attached GTK popover. Keyboard interactivity
is `OnDemand` only while the Panel is open and returns to `None` on dismissal.
Dropping keyboard interactivity and GTK focus is the implemented restoration
request; actual restoration to the prior Hyprland client, outside-click
dismissal, Escape, stacking, and behavior beside Waybar still require the
native checklist.

Root state owns each output's presentation level, pointer state, reduced-motion
projection, and timer generation. GTK callbacks emit typed actions. Stale dwell
and dismissal timers cannot change state, and all GLib sources, output signals,
surfaces, and supervised tasks have explicit shutdown owners.

GDK connector names bind process-local surfaces to Hyprland outputs without
entering diagnostics. The root reducer owns compositor outputs, workspaces,
address-bearing clients, active context, and explicit starting, ready, stale,
or unavailable adapter state. GTK surfaces receive immutable local projections:
bounded workspace marks, bounded activity and attention marks, selected typed
actions, and an active candidate label or compositor/clock fallback. The
Selvage uses stable navigation/activity/attention thirds. Shape, width, fill
pattern, visible text after reveal, and accessible labels encode state in
addition to color; GTK widgets do not retain arbitration state. Each Selvage
region reconciles its mark widgets against the new projection with a pure
key-diff rather than rebuilding them every render: workspace marks are keyed by
workspace id and status marks by their stable region slot, so a matched widget
is updated in place, only genuinely new marks are created, only vanished marks
are removed, and a widget is recreated only when its accessible role changes.
This retains tooltips and accessible objects across renders and avoids
widget-lifetime churn during rapid state updates.

## Configuration

Configuration is versioned TOML loaded from
`$XDG_CONFIG_HOME/weftwise/config.toml`, falling back to
`~/.config/weftwise/config.toml`. System defaults may use the XDG configuration
directories. Unknown keys produce useful diagnostics; invalid user values do
not silently replace valid defaults.

Cache and persistent state use the corresponding XDG cache and state bases.
Per-login sockets or transient state require `XDG_RUNTIME_DIR`; absence is an
explicit unavailable state rather than a fallback to a shared temporary
directory. Application directories and private files use modes `0700` and
`0600` respectively when writers are introduced. Default diagnostics redact
user paths, desktop text, content metadata, and process arguments.

GTK CSS uses semantic tokens for backgrounds, surfaces, text, accent, warning,
critical state, spacing, radii, and motion. User CSS cannot alter protocol or
action-dispatch safety boundaries.

The current startup loader accepts validated semantic theme tokens rather than
arbitrary CSS. Hex colors, a restricted font-family string, font size, and
radius are converted into application-priority GTK CSS. The same versioned
configuration controls exposed-edge activation geometry and visibility of the
workspace, context, and clock Ribbon regions. Entry reveal can be enabled as an
explicit alternative to dwell without changing the visual Selvage geometry.
Live reload remains planned.

## Module boundaries

The source layout retains dedicated modules for:

- application lifecycle, messages, state, actions, and configuration;
- shell surfaces and output ownership;
- Hyprland, MPRIS, clock, local-activity, and process adapters;
- context candidates and arbitration; and
- Selvage, Ribbon, and Panel widget construction.

The widget layer is `widgets/mod.rs` (the root `TopEdgeWidgets` that owns each
output surface and renders authoritative projections) plus three construction
submodules: `ribbon.rs` builds the revealer, activation button, and navigation,
context, and status labels; `panel.rs` builds the attached popover with its
audio, media transport, and close controls together with Escape dismissal and
focus restoration; and `selvage.rs` holds the pure mark diff used to reconcile
Selvage marks in place. Active-context, media, and clock have no standalone
widget: active context and the clock are rendered into Ribbon labels, and media
transport is rendered into the Panel controls, so no separate module is
retained for them.

Boundaries are established where each subsystem has distinct ownership, failure,
and testing requirements.
