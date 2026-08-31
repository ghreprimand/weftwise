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

The Hyprland adapter consumes address-free `screencastv2` lifecycle events and
counts concurrent screencopy clients with bounded state. It validates the
monitor, window, or region owner category but discards the shared target name.
Only a positive count is active. Zero remains unknown because socket2 provides
no initial screencast snapshot, and a parse gap, compositor restart, or socket
loss changes the source to stale or unavailable before reconnect.

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
Per-stream movement targets a stream node this endpoint-only model does not yet
represent and stays an explicit transport limitation until its native contract
is verified. The adapter and its optional `pipewire` dependency compile only
with the `audio-transport` feature; the pure domain and typed contracts build
without it.

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
every connection attempt and never logs the instance identifier or socket
paths. It connects the newline-delimited event socket first. While five JSON
snapshots are requested through fresh, strictly timed, size-bounded request
connections, parsed events enter a count- and byte-bounded buffer. The root
receives one atomic snapshot before those events replay in wire order.

Event records split only at the first `>>`. Address-bearing v2 forms are used
where Hyprland provides them. Unknown events are ignored; malformed recognized
events, overlong or truncated reads, monitor lifecycle, legacy events without
stable identity that cannot be paired safely, unresolved workspace identity,
and buffer overflow mark retained state stale and force fresh discovery plus a
new snapshot. Paired legacy workspace, focused-output, move, and title events
are ignored in favor of their stable-identity counterparts. Bounded exponential
backoff includes jitter.
The clock is a separate supervised in-process adapter and aligns each update to
the next wall-clock minute rather than spawning or periodically drifting.
Synchronous application shutdown first broadcasts cooperative cancellation,
allows a bounded 100-millisecond grace for adapters to finish, and aborts only
stragglers. This preserves prompt GTK shutdown without racing every cancellation
receiver.

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
enough for the Ribbon, but its collapsed GDK input region covers only the
3-pixel Selvage. The region is first applied after realization and recomputed
directly from the GDK surface layout callback's logical width and after scale
notifications. The pre-layout region is deliberately empty; the first positive
layout replaces it synchronously so root-message latency cannot leave a stale
empty region. Pointer entry and exit are observed in capture phase on the
fixed-height root widget.

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
addition to color; GTK widgets do not retain arbitration state.

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

## Module boundaries

The initial source layout retains dedicated modules for:

- application lifecycle, messages, state, actions, and configuration;
- shell surfaces and output ownership;
- Hyprland, MPRIS, clock, and process adapters;
- context candidates and arbitration; and
- Selvage, Ribbon, Panel, active-context, media, and clock components.

Files may begin small. Boundaries are established early because each subsystem
has distinct ownership, failure, and testing requirements.
