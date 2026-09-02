# Weftwise - Devlog

Public running record of Weftwise development in reverse-chronological order.
Entries describe what changed, what was verified, and what remains unavailable
or unmeasured. Planned work is never presented as implemented behavior.

---

## 2026-09-02 - Replace Ribbon pin click-away with a shortcut toggle

A keyboard-pinned Ribbon no longer uses native outside-click dismissal. The
invisible pin-guard popover, the grab-release re-entry suppression state, and
the paired rearm timer are removed. A pinned Ribbon now stays visible until the
reveal shortcut is pressed again, which acts as an explicit toggle: the first
tap glances, a second tap within the pairing window pins, and any later tap
while pinned or while the Panel is open collapses straight to the Selvage. The
reducer gains an `UnpinRibbon` input and an `is_pinned` accessor; the removed
`FocusLost` and `DismissalGuardElapsed` inputs, the `ScheduleDismissalGuard`
effect, the `TimerKind::Rearm` timer, and the dead `AppAction::Quit` variant
are deleted. The keyboard pin now survives a Panel round-trip: opening the Panel
keeps the pin and closing it restores the pinned Ribbon rather than collapsing.

This change removes the trigger for a `gtk_widget_is_ancestor` critical that was
observed firing from the pin-guard popover during a keyboard pin. The operator
selected the no-click-away toggle model; the trade-off is that a pinned Ribbon
is dismissed only by the shortcut (or by opening and toggling), not by clicking
elsewhere. The decision and its rationale are recorded in
`docs/interface-model.md`.

The default and transport-free gates pass, including the new pin-toggle reducer
contracts (glance/pin/toggle-off, unpin rearms only normal dwell, pin survives
the Panel round-trip, output removal while pinned or in Panel, and stale-timer
invalidation after unpin and Panel close). Native Hyprland behavior of the
toggle and the broader Waybar proof remain unmeasured.

## 2026-09-01 - Separate Ribbon pinning from Panel controls

Two `weftwise reveal` requests within 1500 milliseconds now pin only the Ribbon
on the focused output. The keyboard pin has its own root-owned state, ignores
pointer-leave timers, and uses an invisible GTK autohide guard for native
outside-click dismissal. It no longer opens or reopens the Panel popover. An
initial layer-window active-state observer proved unsuitable because an
on-demand layer surface does not acquire compositor focus programmatically;
the guard replaced it after native testing. The guard contains a focusable,
one-logical-pixel transparent child so GTK retains the outside-click grab rather
than closing it immediately. Native key-binding delivery also showed that the
original 500-millisecond recognition window was too narrow once two compositor
`exec` launches and session-bus activation were included; 1500 milliseconds
remains below the 2.5-second single-glance lifetime while accommodating that
transport overhead.

Closing GTK's outside-click grab can synthesize pointer re-entry on the Ribbon
underneath. Root interaction state now disarms pointer reveal for 500
milliseconds after click-away dismissal. A synthetic entry during that guard is
absorbed until the pointer leaves, preventing the ordinary dwell transition
from reopening and pinning the dismissed Ribbon.

The Panel no longer presents proof placeholders. It projects the focused
output's default PipeWire sink as capability-gated ten-percentage-point volume
steps and a mute toggle, alongside any advertised media controls. Requests are
validated by root state and sent through the bounded typed audio command
channel; device identity remains outside the UI and logs.

Deterministic coverage includes Ribbon pin/focus-loss transitions, click-away
re-entry suppression, Panel separation, focused-output audio projection, and
cubic display-volume command conversion. The complete default gate passed with
132 runnable library tests
and one explicitly ignored live-system-bus test; the transport-free gate passed
131 with the same ignore. All integration suites, warnings-denied Clippy,
documentation, file-size and dependency-topology checks, RustSec across 172
dependencies, and worktree public-safety passed. Native click-away, volume, and
mute acceptance remain open.

## 2026-08-31 - Add double-tap keyboard pinning

Two `weftwise reveal` requests received within 500 milliseconds initially
promoted the focused-output keyboard glance into the Panel state. Native
follow-up showed that this conflated a requested Ribbon pin with the early Panel
popover and could reopen after dismissal. The subsequent 2026-09-01 landing
separates those states. A single request retains the 2.5-second Ribbon glance,
and Hyprland continues to own the configurable global key binding.

Deterministic tests cover taps inside and outside the recognition window and
the glance-to-Panel reducer transition. The complete host gate passed with 130
runnable library tests and one explicitly ignored live-system-bus test; the
transport-free suite passed 129 with the same ignore. All integration suites,
warnings-denied Clippy, documentation, file-size and dependency-topology
checks, RustSec across 172 dependencies, and worktree public-safety passed.
Native operator acceptance remains open.

## 2026-08-31 - Preserve focused context and authoritative Panel dismissal

The Panel close button now emits only the typed close action and leaves popover
visibility to the root-owned presentation state. This keeps the closing pointer
gesture from reaching the Ribbon underneath and immediately reopening the
Panel. Escape and outside-click dismissal continue through the same root state.

Passive MPRIS activity no longer replaces a known focused-client title in the
Ribbon context region. The selected player retains its focused-output activity
mark, progress, capability-gated Panel actions, and text fallback when no
focused-client title is available. Higher-severity candidates and temporary
direct feedback retain their arbitration behavior.

The activation table now accepts `reveal_on_entry = true` as an opt-in
alternative to dwell. It applies the existing immediate-entry transition to
every bounded island and side leg without expanding input geometry. Dwell
remains the default to avoid accidental reveals during ordinary top-edge
crossings.

Verified with the complete host default gate and explicit transport-free
Clippy, tests, and documentation. The default library suite passed 128 tests
with one live-system-bus test explicitly ignored; the transport-free suite
passed 127 with the same ignore. All integration suites, file-size and
dependency-topology checks, RustSec across 172 dependencies, and worktree
public-safety passed. Native operator acceptance remains open.

## 2026-08-31 - Mirror the bounded activation island

Native multi-output testing found that immediate entry did not make the narrow
left-side leg as easy to acquire as the wider top island near the right edge.
The collapsed input region now mirrors any non-central bounded top island
across the output center, giving both top corners the same acquisition area and
configured dwell. The existing immediate side legs remain available when an
adjacent output makes a physical top edge hard to hold. Centered and full-width
activation regions are not duplicated.

The geometry uses GDK logical coordinates and contains no connector,
resolution, or scale-specific exception. Automated verification covers
end-anchored, centered, full-width, and expanded presentation states. Native
operator acceptance remains open.

## 2026-08-31 - Localize media and enable native audio by default

Session-global MPRIS state is now projected on the compositor-focused output
only. Unfocused outputs receive neither the media label, its progress mark, nor
its capability actions. This is a deterministic single-output policy because
MPRIS does not reliably expose a window-to-output association; it avoids
inventing one from untrusted titles or application-name heuristics.

Both narrow side-corner targets now reveal immediately on horizontal entry,
while the bounded top island retains its configured dwell. Native follow-up
showed that immediate timing alone did not make the narrow left leg as easy to
acquire as the wider right-side top island; a mirrored island is handled by the
subsequent entry. The logical-pixel rule itself remains independent of output
name, resolution, and scale.

The direct PipeWire transport is now a default Cargo feature on the supported
OdysseyOS/Arch native baseline. CI installs and verifies the corresponding
native interface, while `--no-default-features` retains a transport-free build
for pure-domain development. A live process loaded the PipeWire transport and
retained the existing remote reveal action; operator observation of a volume or
mute change remains part of the open manual audio check. Persistent audio,
network, power, and tray modules remain later Panel and incremental-Waybar work.

Verified with the complete host default gate, explicit transport-free Clippy,
tests, and documentation, and the digest-pinned Arch baseline under exact Rust
1.96.0 in both configurations. The default library suite passed 125 tests with
one live-system-bus test explicitly ignored; the transport-free suite passed
124 with the same ignore. File-size and dependency-topology checks, RustSec
across 172 dependencies, and public-safety passed.

## 2026-08-31 - Add typed activity CLI and keyboard Ribbon reveal

The `weftwise` executable now publishes, updates, completes, and cancels typed
Phase 6 activity through the authenticated local endpoint. The CLI constructs
the same bounded protocol values as the service, waits for a fixed
acknowledgement with bounded I/O deadlines, and exposes no shell command,
program argument, or environment surface. Error diagnostics do not echo
untrusted argument values.

`weftwise reveal` now activates a parameter-free GApplication action in the
running primary instance. It reveals the Ribbon on the compositor-focused
output for a 2.5-second glance, leaves an already-open Panel unchanged, and
hands dismissal ownership to the pointer if it enters the Ribbon. Hyprland
continues to own key selection; the maintained example recommends
`SUPER+grave` and invokes the portable command rather than embedding a local
path.

Verified with the complete host gate and native `audio-transport`
Clippy/test/documentation gate, plus the digest-pinned Arch baseline under
exact Rust 1.96.0. Formatting, warnings-denied Clippy, default and feature
tests, documentation, file-size and dependency-topology checks, RustSec across
172 dependencies, and public-safety passed. A first container attempt stopped
before compilation because its transient `/tmp` mount prohibited execution;
the corrected bounded run used an executable transient mount and passed. A
live session accepted all four synthetic activity operations, exposed the
`reveal` action, and accepted `weftwise reveal`. A physical hotkey press and the
broader manual Hyprland proof remain operator-observed checks.

## 2026-08-31 - Mirror collapsed activation at both corners

The collapsed input region now includes matching narrow legs at the top-left
and top-right corners of every output. Either physical corner can enter the
same bounded dwell interaction, including when an output above covers the
horizontal top edge. The visual Selvage remains 3 pixels high and pointer input
still passes through outside the top island and two corner legs.

Topology handling now distinguishes both upper side edges. Immediate reveal is
used only when the top, left, and right entry paths all continue directly into
neighboring outputs; if either corner is a physical boundary, both corner
targets retain the configured dwell. The rule continues to use GDK logical
rectangles with the existing fractional-layout tolerance and contains no
monitor-name or resolution special case.

Verified with the complete host gate: formatting, warnings-denied Clippy, 117
passing library tests with one live-system-bus test explicitly ignored, all
integration tests, documentation, file-size and dependency-topology checks,
public-safety, and RustSec across 172 dependencies passed. Native comfort and
pointer pass-through remain part of the open manual Hyprland proof.

## 2026-08-31 - Bind the authenticated local activity endpoint

The Phase 6 protocol now has a supervised Unix listener beneath the application
XDG runtime directory. Setup rejects symlinked, foreign-owner, or permissive
runtime bases; applies `0700` to the application directory and `0600` to the
socket; authenticates every Linux peer credential against the effective
process user; and refuses to replace regular files or live endpoints. Only an
owned connection-refused socket is removed as stale, with device and inode
checked again before unlinking. Cleanup applies the same identity check.

The transport processes at most eight clients concurrently. Each client has a
30-second idle deadline, a 64-frame-per-second rate limit, and the existing
16 KiB frame boundary. Unknown versions and malformed frames disconnect without
emitting root state or including payloads in diagnostics. Validated events now
flow through typed root messages: up to 128 live identities can publish and
update activity candidates, completion produces bounded temporary feedback,
and cancellation removes the source-scoped identity. The CLI remains pending.
The Phase 5 stale-evidence recovery contract now also confirms that fresh
inactive microphone and camera observations clear their uncertainty marks.

Verified with the complete host gate and the digest-pinned Arch baseline under
exact Rust 1.96.0: formatting, warnings-denied Clippy, default and
`audio-transport` tests, documentation, RustSec across 172 dependencies,
file-size and dependency-topology checks, and public-safety passed. The first
optional-feature container attempt stopped before compiling Weftwise because
`libclang` was unavailable; adding Arch's `clang` native build dependency made
the exact feature gate pass and the maintained package list now records it.

## 2026-08-31 - Define the bounded activity protocol and recovery contracts

Phase 6 now has a transport-independent version 1 JSON-lines schema for timer,
build, download, render, and command-result display activity. Publish, update,
complete, and cancel operations carry protocol-safe identities, sanitized
bounded labels, optional basis-point progress, bounded lifetimes, and typed
terminal outcomes. Frames larger than 16 KiB, unknown fields or versions,
invalid identities, out-of-range progress, invalid lifetimes, and empty updates
are rejected without payload-bearing diagnostics. A synthetic fixture and
round-trip, rejection, redaction, and XDG endpoint-path contracts cover the
boundary. The schema accepts no executable, arguments, or shell command.

The versioned socket leaf is reserved below the Weftwise XDG runtime directory,
but no endpoint is created yet. Authentication, restrictive permissions,
connection/rate limits, supervision, and the CLI remain separate work.

The Hyprland lifecycle contract was also corrected after native analysis. A
full compositor restart changes its instance signature and closes the GTK
Wayland connection, so the existing process cannot provide meaningful
same-process recovery. Maintained prose now limits adapter reconnection claims
to transport loss within the current session and reserves full-session
acceptance for a fresh Weftwise start after an orderly session cycle.

Phase 5 deterministic coverage now also exercises service loss followed by a
fresh audio snapshot, replacement of a removed default by a hotplugged device,
move-capability revocation on route removal, rapid default-sink volume
coalescing through the cubic display mapping, out-of-order capture graph events
and link removal, and partial recovery from stale privacy evidence. These are
synthetic state contracts; live device and service-restart checks remain
separate manual evidence.

Verified with the complete host default gate, the native `audio-transport`
Clippy/test/documentation gate, and the digest-pinned Arch baseline under exact
Rust 1.96.0. Formatting, warnings-denied Clippy, default and feature tests,
documentation, file-size and dependency-topology checks, RustSec across 172
dependencies, and public-safety passed.

## 2026-08-31 - Make the right-corner trigger reachable

Native testing exposed a topology the horizontal activation island could not
solve: one lower output's entire top edge led directly into the outputs above
it. The collapsed input region now combines the monitor-aware top island with a
narrow leg down the right edge of the fixed-height surface. The leg can be
entered horizontally without increasing the full-width application gesture
conflict. The default thickness increases from 8 to 12 GDK logical pixels and
remains bounded and configurable. Collapsed workspace and status marks are now
hidden while the Ribbon or Panel is visible, removing the mark group that
overlaid the top-left Ribbon content.

Follow-up testing on a fractionally scaled output exposed a second topology:
both the selected top edge and the right edge led directly into neighboring
outputs, so GTK delivered entry and departure before the dwell could finish.
Such fully internal corners now reveal on bounded entry. Physical-edge layouts
keep the configured dwell. The decision comes from GDK logical rectangles with
a two-pixel rounding tolerance and contains no output name, resolution, or
machine-specific case.

Verified with the complete host default gate, the native `audio-transport`
Clippy/test/documentation gate, and the digest-pinned Arch baseline under Rust
1.96.0. Formatting, warnings-denied Clippy, default and feature tests,
documentation, file-size and dependency-topology checks, RustSec across 172
dependencies, and public-safety passed. A live four-output session is running
the revised geometry for manual trigger and appearance acceptance.

## 2026-08-31 - Refine exposed-edge activation and the configurable Ribbon

The collapsed pointer target is now independent from the 3-pixel visual
Selvage. Each output selects the widest portion of its top edge that is not
covered by an adjacent output above it, then places a bounded activation island
inside that segment. The default island is 96 by 12 GDK logical pixels, aligned
near the segment end with a 12-pixel inset. This makes a partially exposed edge
usable in vertically stacked layouts and limits competition with application
top-edge gestures. Full-width activation remains an explicit comparison and
rollback mode. Initial and allocation-time input regions are clamped to the
actual surface width, and output reconciliation recalculates retained and new
surfaces after layout changes.

The revealed Ribbon now has persistent workspace, selected-context, and clock
regions instead of one undifferentiated label. Contextual candidates such as
media, privacy, and temporary feedback still use the deterministic arbiter, but
they no longer displace the persistent output-local workspace and
boundary-aligned clock. The default visual treatment changes from a flat
brown/gold strip to inset charcoal surfaces, restrained coral selection,
subtle borders, rounded corners, and clearer type hierarchy.

The existing versioned XDG configuration is now read at startup with a 64 KiB
bound. Typed `[activation]`, `[ribbon]`, and `[theme]` tables control trigger
geometry, persistent-region visibility, semantic colors, font family, font
size, and radius. Colors accept only exact hex forms and the font family is
restricted before the tokens enter generated GTK CSS. The synthetic example
configuration and maintained architecture, interface, build, README, and
scaffolding prose describe the implemented startup behavior. Configuration
reload still requires an application restart; safe live reload remains later
work.

The native audio integration no longer turns an unobserved PipeWire placeholder
into `Volume 0%`. Snapshots and the first real property observation establish a
baseline without producing temporary feedback. Later changes emit feedback only
when both the previous and current default node have an observed capability.

Verified on the host with the default repository gate and the
`audio-transport` feature gate. Formatting, warnings-denied Clippy, all tests,
documentation, file-size and dependency-topology checks, RustSec across 172
dependencies, and public-safety passed. The default build ran 104 library tests
with 103 passed and one live-logind test ignored, plus all integration suites;
the audio feature ran 105 library tests with 104 passed and the same one ignored,
plus all integration suites. The same default and audio-feature gates passed in
the digest-pinned Arch baseline with Rust 1.96.0; the feature gate included the
native PipeWire and clang build requirements. A live four-output session
confirmed one bounded activation island per surface and no GTK CSS parser
errors. Final trigger comfort, application gesture coexistence, the revised
appearance, fullscreen stacking, and focus restoration remain manual
acceptance work.

## 2026-08-31 - Add WirePlumber-cooperating active-stream movement

The audio adapter can now move the active playback stream to a chosen sink by
cooperating with WirePlumber instead of mutating graph links. A move writes the
`target.object` key of the `default` metadata with the stream's registry
identity as the subject and the destination sink's decimal `object.serial` as a
`Spa:Id` value, the metadata channel WirePlumber watches for stream targets. A
successful write acknowledges only that a request was sent; it never asserts the
graph moved, because WirePlumber owns the relink.

The typed `MoveStream` command is now gated by a deterministic movable-stream
selection instead of being an unconditional transport limitation. The transport
keeps a bounded, numeric inventory of playback streams whose `media.class`
equals `Stream/Output/Audio` exactly after trimming, so a decorated or
near-match class is never tracked. Each entry is keyed by registry identity and
retains only a running flag, a movable flag derived from `node.dont-move`, and
whether the stream subject grants the metadata (`PW_PERM_M`) permission a
`target.object` write on that subject requires. No application, media, or
process identity crosses the boundary. Exactly one running, movable,
metadata-permitted stream is the active subject; zero is unavailable; more than
one is ambiguous; and an inventory overflow, an explicit
`linking.allow-moving-streams` denial in the `sm-settings` metadata, or a
`default` metadata object lacking write and execute permission disables the
action. A running movable stream whose subject lacks the metadata permission is
not counted rather than offered. The root projects this selection through a new
capability update and clears it on reconnect and loss so a stale subject cannot
validate a move to a vanished stream.

Because opaque registry IDs are reused across a PipeWire reconnect or service
restart, every connection attempt is stamped with a monotonic generation. The
generation rides inside the published selection, is adopted by the validated
adapter command, and is rechecked at dispatch; the transport also re-derives the full
selection preconditions there. A move queued against a stale connection fails
the generation recheck instead of retargeting a coincidentally matching new
stream or sink, and a move against an ambiguous or disabled graph is refused.

To hold both the always-compiled pure model and the feature-gated transport
under the file-size limits, the adapter moved from a single `services/audio.rs`
file to a `services/audio/` module: `mod.rs` carries the pure state, selection,
policy, permission, and metadata-value logic, and `transport.rs` carries the
supervised PipeWire loop. Behavior of the split modules is unchanged from the
prior single file.

Verified on the host and in the digest-pinned Arch container with Rust 1.96.0,
GTK 4.22.4, GTK4 Layer Shell 1.3.0, PipeWire 1.6.8, and SPA 0.2. The default
gate passes formatting, Clippy with warnings denied, 99 library tests with one
live-logind test ignored, all integration tests, documentation, file-size and
dependency topology checks, RustSec across 172 dependencies, and public-safety.
The `audio-transport` feature passes Clippy with warnings denied, 100 library
tests with the same live-logind test ignored, all integration tests including
the 10 Phase 5 contracts, and documentation. The live relink against a running
WirePlumber, whether streams reach the running state, and the `object.serial`
and permission values a real server reports remain unmeasured in this headless
environment; the bindings are validated at compile time only. GitHub Actions
run 33403704666 passed commit `4dfd432`.

## 2026-08-31 - Add positive PipeWire microphone and camera capture evidence

A bounded capture-evidence graph now derives positive microphone and camera
activity from the PipeWire object graph, riding the same supervisor-owned loop as
the audio adapter. The graph is numeric-only: it retains device backing API, node
capture role, node running flag, port direction, and link active flag as small
closed enums keyed by registry identity. Client, application, and media names;
node and device descriptions; object paths; serials; and process arguments are
not retained. The few properties needed for classification are inspected
transiently, and only their bounded classification result is kept.
Classification is exact: sources match the exact `Audio/Source` or `Video/Source`
class, terminals the exact `Stream/Input/Audio` or `Stream/Input/Video` class,
and backing devices the exact `alsa`, `bluez5`, `v4l2`, or `libcamera`
`device.api`, so decorated or near-match values are excluded.

A source is reported active only by a complete, running, single-link direct path:
an active link whose input port belongs to a running `Stream/Input/Audio` or
`Stream/Input/Video` terminal and whose output port belongs to a running,
non-virtual, non-monitor `Audio/Source` or `Video/Source` node backed by an ALSA
or BlueZ5 device for microphones or a V4L2 or libcamera device for cameras. Only
this direct hardware-source-to-terminal shape is proven; capture routed through
intermediate filter nodes such as an echo-cancel or loopback chain is a
deliberate known false negative that stays unknown rather than being asserted as
active. The absence of a proven path over a complete graph is unknown rather than
inactive, matching the project's positive-only privacy adapters.

The running and active flags are preserved only across an identical
re-announcement; a role, backing, or endpoint change resets the flag so a
reclassified object cannot inherit a stale positive state. Exceeding a per-kind
size bound marks the graph overflowed, which is sticky for the connection and is
reported as a change so a previously trusted source does not silently persist as
`Unknown`. Readiness and trust are modeled separately. Two core-sync barriers are
used: the first establishes registry enumeration and drives the audio snapshot
and audio ready signal, and the second flushes the node and link info replies
that binding requested. The graph becomes ready when the second barrier completes
and then republishes resolved states on every later change; it is trusted only
when that barrier completed over a complete graph. A ready-but-incomplete graph
still proves an active path as `Active` while reporting an absent source as
`Unavailable`, and a later disappearance of that path becomes `Unavailable`
immediately; a trusted graph reports an absent path as `Unknown` and degrades it
to `Stale` on a later overflow. If the second sync cannot be issued, the barrier
fails: the graph is neither ready nor trusted and every source is published as
pre-trust `Unavailable`. Attempt trust is derived only from the capture second
barrier (not the audio first barrier) and is reset on every reconnect, so a loss
before a trusted barrier stays `Unavailable` and a loss after it becomes `Stale`.
A proxy is bound and retained only for an identity the bounded graph kept, so the
proxy maps stay within the graph's object bounds, and proxy info listeners hold
only weak references back to the tracker so the retained node and link proxies
cannot leak the graph across reconnects.

### Verification

- Thirty deterministic synthetic unit tests cover exact device-API, node-role,
  and port-direction classification including rejection of decorated and
  near-match values, complete and incomplete capture paths, the running
  requirement on both endpoints, virtual and monitor exclusion, missing device
  backing, reversed ports, cross-source isolation, self-looping links, link
  removal, per-kind size bounds with non-retention of rejected identities, sticky
  overflow across removal, the pre-announcement running guard, identical
  re-announcement preservation, running-flag reset on reclassification, active-flag
  reset on link re-route, the overflow-and-trust reporting decision, the
  ready-versus-trusted second-barrier outcome, the overflow rising-edge change
  signal, the connection-loss state decision, an active path proven over an
  incomplete graph, and the observation builders. The pure graph builds and is
  tested without the `audio-transport` feature.
- The complete default worktree gate passes formatting, deny-warning Clippy,
  89 library tests with one explicitly ignored live-logind test, all 58
  integration tests, documentation, file-size and dependency-topology checks,
  the RustSec audit over 172 locked dependencies, and the public-safety scan.
- The `audio-transport` feature gate passes deny-warning Clippy, 90 library
  tests with the same explicitly ignored live-logind test, all 59 integration
  tests, and documentation, exercising the PipeWire node, link, port, device,
  registry, and double-sync binding usage at compile time.
- Both configurations pass in the digest-pinned Arch Linux container with
  `rustc 1.96.0`, `cargo 1.96.0`, GTK 4.22.4, gtk4-layer-shell 1.3.0,
  PipeWire 1.6.8, and SPA 0.2 metadata.
- GitHub Actions run 33362131874 passed commit `34c6e26`.
- The live capture-graph behavior against a running PipeWire server, including
  whether node and link info reach the running and active states as modeled and
  whether the two-barrier readiness orders correctly against real info replies,
  is unmeasured here; no PipeWire session is available in this environment and it
  remains a native proof task.

---

## 2026-08-30 - Add WirePlumber-cooperating default-route selection

The direct PipeWire adapter now implements capability-gated default-route
selection. Choosing a default sink or source writes the persistent
`default.configured.audio.sink` or `default.configured.audio.source` key of the
bound `default` metadata object as a `Spa:String:JSON` `{"name":"<node.name>"}`
value, the same cooperation path `wpctl set-default` uses. WirePlumber remains
the policy owner: it validates and applies the request and republishes the
resulting `default.audio.*` selection, which returns through the existing
metadata property listener as a default-changed update. Weftwise never writes
the runtime output key directly, and it binds only the first `default` metadata
object.

The retained metadata handle is now keyed by its registry global ID. A
`global_remove` for that object releases the proxy and its property listener so
a recreated `default` metadata object rebinds on its next global, instead of the
adapter writing route requests through a stale proxy whose backing global no
longer exists. The single-bind and clear-on-removal decisions are pure,
transport-independent functions with their own unit tests.

The root capability gate now rejects a default-route request to an unavailable
node or a node of the wrong direction before dispatch. At this landing,
per-stream movement remained an explicit transport limitation: it targeted a
stream node this endpoint-only model did not represent, and its native contract
was unverified.

### Verification

- New unit tests cover the configured-preference metadata keys, the bounded
  JSON value builder with empty, NUL-bearing, and quote/backslash names, the
  default-route capability and availability gate, and the metadata single-bind
  and clear-on-global-removal lifecycle decisions.
- The pure metadata key and value builders are transport-independent and build
  without the `audio-transport` feature.
- The live metadata round-trip against a running WirePlumber is unmeasured here;
  no PipeWire session is available in this environment.
- The complete default worktree gate passes formatting, deny-warning Clippy,
  tests, documentation, file-size and dependency-topology checks, and
  public-safety automation, with RustSec clean over 172 locked dependencies on
  the host. The `audio-transport` gate passes deny-warning Clippy and 60 tests
  with one explicit live-only ignore.
- The same default and feature gates pass with exact Rust 1.96.0 in the
  digest-pinned Arch environment, including the RustSec audit over the same 172
  locked dependencies.
- GitHub Actions run 33351987274 passed commit `4ac1fe4`.

---

## 2026-08-30 - Add Hyprland screen-sharing evidence

The direct Hyprland event adapter now accepts the address-free
`screencastv2` lifecycle event as positive screen-sharing evidence. It validates
the state and monitor, window, or region owner category, discards the shared
target name, and ignores the paired legacy event. Root state keeps a bounded
concurrent-client count so one stopping client cannot hide another active
client.

The event socket has no initial screencast snapshot, so a zero count remains
unknown rather than inactive. A positive count is active, and parse gaps,
compositor restart, or socket loss produces stale or unavailable uncertainty
before the independently supervised adapter reconnects. A fresh compositor
snapshot resets the count to unknown before buffered v2 events replay.

### Verification

- Parser contracts cover active and stopped v2 events, comma-containing target
  names, legacy suppression, invalid states, invalid owner categories, and
  truncated fields.
- Root contracts cover two concurrent clients, partial and complete teardown,
  stale gap handling, and fresh-snapshot recovery without false inactive state.
- The complete host gate passes formatting, deny-warning Clippy, 112 default
  tests with one explicit live-only ignore, documentation, file-size and
  dependency-topology checks, public-safety automation, and RustSec over 172
  locked dependencies. The feature gate passes deny-warning Clippy, 114 tests
  with the same explicit ignore, and documentation.
- The same complete default and feature gates pass with exact Rust 1.96.0 in
  the digest-pinned Arch environment against PipeWire 1.6.8, `libspa-0.2`, GTK
  4.22.4, and gtk4-layer-shell 1.3.0.
- GitHub Actions run 33345014212 passed commit `60d46db`.

### Next

- Add positive PipeWire microphone and camera capture evidence, and retain
  recording as unsupported until a selected source can distinguish it.

## 2026-08-30 - Add bounded logind idle-inhibitor evidence

A supervised systemd-logind adapter now feeds typed idle-inhibitor evidence to
the root privacy domain. It opens the system-bus connection inside the Relm4
Tokio runtime, subscribes to login1 owner and manager-property changes before
its initial `ListInhibitors` snapshot, and reconnects with bounded jittered
backoff after service or bus loss. Each snapshot is count-bounded. The adapter
discards inhibitor owner, reason, mode, user, and process fields without logging
them.

An exact `idle` target is positive active evidence. An empty logind snapshot is
unknown rather than inactive because Hyprland's Wayland idle-inhibit protocol
is outside logind. Oversized snapshots and transport failures are unavailable,
so neither an incomplete source nor a failed source can become false inactive
state.

### Verification

- Unit tests cover exact compound-target matching, substring rejection, empty
  and unrelated snapshots remaining unknown, and oversized responses becoming
  uncertain rather than false inactive.
- The live, ignored logind contract was run explicitly against the active
  system bus and passed without printing or retaining inhibitor details.
- The complete host gate passes formatting, deny-warning Clippy, 110 default
  tests with one explicit live-only ignore, documentation, file-size and
  dependency-topology checks, public-safety automation, and RustSec over 172
  locked dependencies. The feature gate passes deny-warning Clippy, 112 tests
  with the same explicit ignore, and documentation.
- The same complete default and feature gates pass with exact Rust 1.96.0 in
  the digest-pinned Arch environment against PipeWire 1.6.8, `libspa-0.2`, GTK
  4.22.4, and gtk4-layer-shell 1.3.0.
- GitHub Actions run 33344660172 passed commit `88e7fb3`.

### Next

- Add complementary Hyprland idle-inhibit and positive PipeWire capture
  evidence before claiming complete privacy-source coverage.

## 2026-08-30 - Strengthen Phase 5 audio recovery contracts

The Phase 5 integration contract suite now exercises the audio domain alongside
privacy and temporary feedback. Synthetic snapshots verify node-count, name,
volume, and default-identity bounds before later deltas are accepted. Transport
loss retains previously observed state as stale, and a fresh snapshot restores
ready state with the new topology. Command tests verify capability rejection and
visible content-free feedback for unsupported stream routing. Under the optional
`audio-transport` feature, a retained command sender keeps the bounded receiver
open until adapter ownership ends.

### Verification

- The default Phase 5 contract target passes 8 tests.
- The `audio-transport` Phase 5 contract target passes 9 tests, including the
  retained-command-channel regression.
- The complete host gate passes formatting, deny-warning Clippy, 107 default
  tests, documentation, file-size and dependency-topology checks,
  public-safety automation, and RustSec over 172 locked dependencies. The
  feature gate passes deny-warning Clippy, 109 tests, and documentation.
- The same complete default and feature gates pass with exact Rust 1.96.0 in
  the digest-pinned Arch environment against PipeWire 1.6.8, `libspa-0.2`, GTK
  4.22.4, and gtk4-layer-shell 1.3.0.
- GitHub Actions run 33344298746 passed commit `6a21082`.
- Live PipeWire recovery, device hotplug, route removal, and hardware behavior
  remain unmeasured and are not satisfied by these synthetic contracts.

### Next

- Validate the remaining recovery and hardware cases in a native user session.

## 2026-08-30 - Add the direct PipeWire audio adapter and typed audio control

A transport-independent audio domain now models bounded sink and source nodes
with fixed-point linear volume, mute, availability, and per-node capabilities,
plus the resolved default sink and source. Snapshots are node-count bounded,
untrusted names are length-clamped, volume is clamped and finite, and unknown
default identities are dropped. Removing a node clears any default that
referenced it, and a failed transport downgrades retained state to stale rather
than a false empty inactive.

Typed volume, mute, toggle-mute, default-route, and move-stream commands are
capability-gated by the root before dispatch. A rejected request emits
content-free error feedback through the shared emitter; volume and
microphone-mute changes on the default nodes emit temporary volume and
microphone feedback.

The direct PipeWire adapter runs one supervisor-owned loop thread, because
`libpipewire` objects are neither `Send` nor `Sync`. It binds node and
default-metadata globals, subscribes to each node's `Props` parameter, parses
channel volumes and mute from the SPA pod, and resolves the default sink and
source from the standard `default` metadata that WirePlumber owns. A Tokio task
bridges typed commands into the loop thread and forwards typed updates out. The
root retains the command sender for the adapter's lifetime. Registry changes
remain buffered until PipeWire Core sync completes and one bounded initial
snapshot is published; only then does the adapter publish deltas and reset its
bounded exponential reconnect backoff. Cancellation remains owned. No `wpctl`
subprocess is polled, no `wireplumber` crate is linked, and the project contains
no `unsafe` code. At this landing, default and stream routing through the
WirePlumber-owned policy metadata was reported as a transport limitation pending
native verification rather than claimed as complete.

The optional `pipewire` dependency and the adapter compile only behind a new
`audio-transport` Cargo feature. The pure domain, typed contracts, and SPA pod
codec round-trip are always available; the transport requires the
`libpipewire-0.3` and `libspa-0.2` development headers and `clang`.

### Verification

- New audio unit tests cover volume clamping and finiteness, cubic display
  bounds, snapshot node bounds and unknown-default filtering, default clearing
  on node removal, wrong-direction and unknown-node command rejection,
  capability-gated control, stale/unavailable semantics, and content-free error
  reasons. The SPA `Props` pod build/parse round-trip is verified under the
  feature.
- A stricter tester contract for feedback TTL expiry was found failing against
  the landed `flush_feedback`; it now advances the arbitration clock so an
  elapsed TTL is released even with no pending event. The Phase 5 privacy and
  feedback contract suite passes.
- Default gate: formatting, deny-warning Clippy, 104 tests, documentation,
  file-size and dependency-topology checks (no second async runtime),
  public-safety automation, and RustSec over 172 locked dependencies. The root
  state file is 1,662 lines after audio, media, and context integration moved to
  retained submodules; the audio adapter is 1,335 lines.
- Feature gate (`--features audio-transport`): deny-warning Clippy, 105 tests
  including the pod round-trip, and documentation all pass.
- The complete default and feature gates pass with exact Rust 1.96.0 in the
  digest-pinned Arch environment against PipeWire 1.6.8, `libspa-0.2`, GTK
  4.22.4, and gtk4-layer-shell 1.3.0.
- GitHub Actions run 33340784731 passed the repository gate for commit
  `0aa89a4`.
- Live adapter behavior, route mutation, and audio hardware validation remain
  unmeasured in this slice.

### Next

- Verify volume, mute, default-route, and move-stream against a live PipeWire
  and WirePlumber session without recording or committing real device names.
- Wire Panel-driven audio actions and render audio state in the interface.

## 2026-08-30 - Add Phase 5 privacy and temporary-feedback domain

A new privacy evidence domain models microphone, camera, screen-sharing,
recording, and idle-inhibitor sources with five distinct states: active,
inactive, unknown, unavailable, and stale. The states never collapse into one
another. Each source is unsupported until an adapter declares it, and
unsupported detections stay silent rather than reporting inactive. Only
supported sources produce candidates: active capture is privacy-critical, an
active idle inhibitor is an interruptible warning, and a failure state
(unavailable or stale) produces an uncertainty candidate so a degraded source
stays visible instead of implying nothing is happening. Adapter degradation
marks supported observations stale, never inactive, and does not overwrite a
source already recorded as unavailable. The domain performs no D-Bus, PipeWire,
or process work.

The audio and capture research boundary is now selected: a dedicated,
supervisor-owned thread will observe direct PipeWire registry, graph, parameter,
and default-metadata APIs while WirePlumber remains the policy owner. The
unusable crates.io WirePlumber stub is not adopted, `wpctl` is not a refresh
path, and no PipeWire object crosses the thread boundary. PipeWire evidence is
explicitly scoped: capture absence cannot prove that direct device clients are
inactive, and the selected transports cannot distinguish recording from
network sharing. Hyprland and logind provide complementary screen-capture and
idle-inhibitor evidence. These adapters remain unimplemented in this landing.

A new temporary-feedback emitter turns discrete volume, microphone, brightness,
screenshot, and command-result events into bounded, self-expiring feedback
candidates. Each kind maps to one stable identity for deduplication and
update-in-place, carries an explicit expiration, and bounds its untrusted text
and progress before emission. A per-kind minimum interval rate-limits rapid
streams by coalescing to the latest value and flushing once the interval
elapses, so the final value is never lost. Only already-defined typed actions
are used, and result-style feedback offers explicit dismissal. Both producers
are root-owned and feed the existing arbitration reducer through the same typed
inputs the media domain uses. No transport adapter or message route is added,
and no selected evidence source is presented as working.

### Verification

- New privacy and feedback unit tests cover unsupported silence,
  privacy-critical active capture, idle-inhibitor warning classing,
  inactive and not-yet-reported sources, failure states remaining visible as
  uncertainty, degradation-to-stale semantics, deduplication, rate-limit
  coalescing and flush, per-kind channel isolation, action and text bounds, and
  kind-scoped dismissal.
- `cargo test --locked`: passed 90 tests; zero failed or ignored.
- The complete worktree gate passes: formatting, deny-warning Clippy, 90 tests,
  documentation, file-size and dependency-topology checks, public-safety
  automation, and RustSec over 148 dependencies and 1,226 advisories.
- The same complete gate passes with exact Rust 1.96.0 in the digest-pinned
  Arch environment with PipeWire 1.6.8 available through `libpipewire-0.3`.
- GitHub Actions run 33336931461 passed the repository gate for commit
  `824f0e0`.
- Native behavior and audio/PipeWire integration remain unmeasured in this
  landing.

### Next

- Implement the selected direct-PipeWire audio and capture boundary, then add
  Hyprland and logind evidence adapters without weakening unknown, unavailable,
  stale, and unsupported states.

## 2026-08-30 - Add bounded MPRIS media integration

The independently supervised MPRIS adapter now opens its session-bus connection
inside Relm4's Tokio runtime. It installs owner-change and player-property
subscriptions before listing a bounded player set and publishing the initial
snapshot. Signals are correlated through unique owners to bounded well-known
player identities and owner generations. Player appearance, disappearance,
property changes, and restarts refresh the affected player; session-bus loss
marks retained state stale and reconnects with bounded jittered backoff.

Raw D-Bus values are converted to typed snapshots before reaching root state.
Player identity, display identity, title, artists, artwork URL, duration,
position, playback state, and capabilities are sanitized or clamped. Root state
selects playing, paused, then recently stopped players, excluding unknown
playback states and using playback activity plus stable identity for
deterministic ties. The selected player produces an activity mark, bounded
progress, accessible Ribbon title/artist text, and only the transport actions
its capabilities allow. Panel buttons emit typed Play/Pause, Previous, Next,
and signed seek requests through a bounded channel. Each request carries the
advertised owner generation, so a delayed action cannot target a restarted
player. The adapter invokes explicit MPRIS methods without shell interpretation
or GTK objects.

### Verification

- Focused media unit and contract tests cover malformed or extreme metadata,
  stable multi-player selection, recent-state expiry, unknown playback state,
  progress bounds, disappearing and restarting players, stale owner
  generations, unsupported actions, session-bus loss state, and recovery.
- `cargo test --locked`: passed 77 tests; zero failed or ignored.
- The complete worktree gate passes with exact Rust 1.96.0 in the digest-pinned
  Arch environment: formatting, deny-warning Clippy, 77 tests, documentation,
  file-size and dependency-topology checks, public-safety automation, and
  RustSec over 148 dependencies and 1,226 advisories.
- GitHub Actions run 33331999506 passed the same repository gate for commit
  `3ba2977`.

### Next

- Validate the complete media milestone in a native Hyprland session beside
  Waybar, including a real player and session-bus restart without retaining
  media metadata in evidence.

## 2026-08-30 - Add deterministic context arbitration and accessible projections

The root state now owns a pure presentation candidate arbiter. Candidates carry
a protocol-safe source-scoped stable identity, typed source and semantic kind, severity,
normalized creation/update/expiration time, bounded minimum display duration,
explicit preemption class, optional basis-point progress, bounded typed actions,
and optional output affinity. Visible and accessible labels pass through the
existing bounded display-text type.

Source-scoped identity deduplicates rapid producer updates in place without
allowing unrelated adapters to collide. Selection uses a
total deterministic order, per-output affinity, expiration, stale-source
removal, and minimum-duration stickiness. A higher preemption class interrupts
sticky content, with privacy-critical state above every ordinary class.
Normalized late arrivals are re-ranked without making equivalent candidate sets
depend on insertion order, while a genuinely newer passive candidate waits for
the active minimum interval. Expiration always releases a selection to the next
candidate, including a fallback. Candidate storage, producer text, progress,
actions, and media seek payloads are bounded before entering authoritative
state. Older or conflicting equal-time producer revisions cannot replace the
accepted value.

Immutable output projections now divide the Selvage into homogeneous navigation,
activity, and attention regions. Local workspace selection uses point/bar shape,
width, and outline/solid fill. Warning and privacy content uses distinct
diamond/triangle geometry and striped fill in addition to semantic colors.
Every compact mark is a GTK label carrying the complete accessible state text;
the Ribbon receives the selected label and bounded typed actions without storing
domain or arbitration state inside widgets. Global detail appears on the
focused output, or on the lowest stable output when no output is focused, while
compact global state remains visible everywhere. Opening the Panel takes focus
once per transition rather than resetting keyboard focus during later renders.
Existing reduced-motion and keyboard interaction semantics are unchanged.

### Verification

- Ten focused arbitration unit tests cover insertion-order independence,
  minimum-duration stickiness, privacy-critical preemption, expiration fallback,
  update-in-place bounds, candidate-set limits, output affinity, stale-source
  removal, cross-source identity isolation, conflicting revisions, and backward
  clock input.
- Eight independent Phase 3 contract tests cover priority inversions, equal
  ties, rapid and stale updates, clock and output changes, privacy preemption,
  non-color attention semantics, bounded typed actions, keyboard Panel inputs,
  and reduced motion.
- The complete worktree gate passes with exact Rust 1.96.0 in the digest-pinned
  Arch environment: formatting, deny-warning Clippy, 69 tests, documentation,
  file-size and dependency-topology checks, public-safety automation, and
  RustSec over 148 dependencies and 1,226 advisories.
- GitHub Actions run `33323159589` passed the same repository gate for commit
  `e95899a` after the Phase 3 landing was pushed.

### Next

- Perform native accessibility inspection; automated projection, keyboard, and
  reduced-motion contracts do not constitute assistive-technology evidence.
- Begin MPRIS discovery and sanitization without changing the established
  arbitration ownership boundary.

## 2026-08-30 - Restore repository-backed CI checkout

The Arch CI job now installs Git before `actions/checkout` and explicitly marks
only `$GITHUB_WORKSPACE` as a safe Git directory after checkout. The previous
order caused checkout to fall back to a source archive, leaving the job without
the repository metadata required by the tracked-tree safety gate. The container
then rejected the checked-out repository because its mounted ownership differed
from the container user. A contract test now preserves both requirements and
rejects a wildcard safe-directory exception.

### Verification

- GitHub Actions run `33314267426` reproduced the original failure at
  `git rev-parse`: `fatal: not a git repository (or any parent up to mount point /)`.
- GitHub Actions run `33314837489` confirmed checkout retained repository
  metadata, then reproduced Git's ownership rejection for the mounted workspace.
- `bash .github/scripts/check.sh --worktree`: passed locally with 32 tests;
  zero failed, ignored, or measured.
- The complete worktree gate passed with Rust 1.96.0 in the project's
  digest-pinned Arch `base-devel` environment, including formatting,
  deny-warning Clippy, tests, documentation, public-safety, and RustSec.
- GitHub Actions run `33314950838` passed all steps on commit `ebabd62`,
  including the complete repository gate with Rust 1.96.0.

### Next

- Continue the Phase 2 state and Hyprland integration landing after CI is green.

## 2026-08-30 - Add the Phase 2 state and Hyprland core

The root reducer now owns typed compositor outputs, workspaces, address-bearing
clients, active context, adapter availability, GDK connector bindings, and the
existing presentation state. Untrusted connector names, client addresses,
classes, workspace names, and window titles are validated, bounded, sanitized,
and redacted from default debug formatting. Retained snapshots remain visible
as explicitly stale state during reconnect rather than being presented as
current or discarded without distinction.

The Hyprland adapter uses the request and event sockets directly. Each attempt
re-resolves the active instance without logging its identifier or paths,
connects the event socket first, and buffers parsed events while five bounded
JSON snapshots are read through fresh request connections with strict
deadlines. The root receives the atomic snapshot before buffered events replay
in wire order. Event records split only at the first `>>`; address-bearing v2
forms are preferred where available. Hyprland's address-bearing `openwindow`
event is parsed directly, and its workspace name is reconciled to the stable
numeric identity from the current snapshot. Unknown events are ignored, while
malformed recognized events, truncated or oversized reads, buffer overflow,
identity gaps, and monitor changes force a new snapshot after bounded
exponential backoff with jitter.

GDK connector identity maps each layer surface to local Hyprland state. The
Selvage renders bounded local workspace marks, and the focused Ribbon renders
the active workspace plus bounded window title. A separate supervised clock
adapter publishes an immediate value and then aligns updates to wall-clock
minute boundaries without a subprocess. Both adapters receive owned
cancellation, and failure or reconnect in one does not stop the other.
Synchronous shutdown broadcasts cancellation, waits for a bounded
100-millisecond cooperative grace, and aborts only unfinished stragglers.

### Verification

- Eighteen new Phase 2 parser, reducer, clock, backoff, and cancellation tests
  pass. The supervisor cancellation regression passed ten consecutive focused
  runs after the bounded cooperative shutdown fix.
- The Tokio macros feature adds only `tokio-macros`; the graph retains one Tokio
  runtime and one gtk4-rs line. The complete worktree gate passes with the exact
  Rust 1.96.0 toolchain, including formatting, deny-warning Clippy, 50 tests,
  documentation, dependency topology, public-safety automation, and RustSec.
- GitHub Actions run `33316075710` passed the same repository gate for commit
  `dd8c300` after the Phase 2 landing was pushed.
- Synthetic tests cover structured snapshots, first-delimiter event parsing,
  unknown and malformed events, bounded event buffering and ordered replay,
  unsafe instance leaves, redacted errors, atomic reducer updates, local output
  projections, client lifecycle counts, stale availability, deterministic
  backoff, cooperative cancellation, minute-boundary calculations, paired
  legacy/v2 events, negative special-workspace identities, and forced
  re-snapshot when an `openwindow` workspace name cannot be reconciled.
- A native Hyprland run connected through the direct adapter but repeatedly
  requested fresh snapshots because paired legacy events were treated as gaps
  even when their address-bearing v2 counterparts followed. Paired legacy
  workspace, focused-output, move, and title events are now ignored; genuine
  monitor or identity gaps still force a snapshot. Stable native delivery and
  compositor restart remain unmeasured. No runtime paths, output identifiers,
  window text, or client addresses were captured.

### Next

- Exercise event-first startup and compositor restart in a native session using
  only sanitized counts and availability transitions as evidence.
- Complete the corrected pointer-reveal and remaining Phase 1 manual checklist
  before treating the surface interaction proof as accepted.
- Begin the pure context arbitration reducer after the Phase 2 landing is
  reviewed.

## 2026-08-30 - Implement the Phase 1 native surface proof

The application now owns one top-anchored overlay-layer window per current GDK
output. Every surface sets its monitor, layer, top/left/right anchors,
`weftwise` namespace, keyboard mode, and candidate exclusive zone before it is
presented. Its allocation remains 30 logical pixels high while the collapsed
GDK input region covers only the 3-pixel Selvage. Input regions are first
applied after realization and recomputed after allocation and scale changes.

The root model owns deterministic Selvage, Ribbon, and Panel state for each
output. Generation-checked dwell and dismissal timers prevent stale or
interrupted transitions from changing presentation. The Ribbon animates inside
fixed surface geometry, and reduced motion combines GTK's desktop setting with
an explicit proof override. The attached GTK popover provides focusable actions,
Escape and outside-click dismissal, and returns layer-shell keyboard mode to
`None` when closed.

The surface manager watches GDK monitor changes and reconciles create/remove
lifecycle without exposing monitor metadata. GLib timer sources, the monitor
signal, layer windows, the GTK animation preference signal, the Tokio shutdown
listener, and adapter handles all have explicit owners and shutdown paths. GTK
objects remain on the main thread; background work sends typed messages only.

A later native run exposed a reveal regression even though compositor geometry
and stacking were correct. The initial render installed the required empty
pre-layout input region, but the GDK layout callback discarded its authoritative
width and deferred refresh through the root message queue. Region installation
now occurs synchronously from each layout callback's logical width, with the
current presentation level retained for reconfiguration. Motion observation is
attached in capture phase to the fixed-height root widget rather than the layer
window. Public-safe debug diagnostics record only the process-local output ID,
interaction kind, region dimensions, level, and update source. A native rerun
of the corrected binary confirmed that holding the pointer at the physical edge
reveals the Ribbon. The remaining pointer pass-through, dismissal, focus, and
stacking rows are independent checks.

The native Hyprland/Waybar comparison selected zone `-1`, which is now the
default. Both `0` and `-1` left the existing reserved work area unchanged, but
only `-1` placed all four surfaces at the physical top edge. Zone `0` remains
available through the public-safe `WEFTWISE_EXCLUSIVE_ZONE` switch for
diagnostics. `WEFTWISE_REDUCED_MOTION` selects the explicit motion override.
Invalid switch values produce redacted startup errors.

### Verification

- `bash .github/scripts/check.sh --worktree`: passed with Rust 1.97.1 and 32
  tests; zero failed, ignored, or measured.
- The same complete worktree gate passed with Rust 1.96.0 in the project's
  digest-pinned Arch `base-devel` environment. GTK4 4.22.4 and
  gtk4-layer-shell 1.3.0 were available there.
- `cargo clippy --all-targets --locked -- -D warnings`, documentation,
  dependency topology, production file size, public-safety, and RustSec passed
  as part of that gate.
- `timeout 10s cargo run --locked` without a display exited with status 1 and
  the structured error `GTK could not initialize the active display backend`.
- Deterministic tests cover dwell reveal, dismissal cancellation, stale timers,
  Panel pin/close behavior, reduced motion, output-state reconciliation, both
  zone values, and collapsed/expanded input geometry.
- Native Hyprland/Waybar zone proof: passed on four outputs across scale factors
  1.00, 1.25, and 1.67. Zone `-1` placed all four surfaces at the physical edge;
  zone `0` placed none there. Neither changed the reserved-area fingerprint.
  Corrected surfaces spanned every output in logical coordinates and shut down
  without GTK child-finalization warnings.
- Corrected physical-edge dwell reveal: passed in the native session that
  reproduced the failure.
- Pointer pass-through, stacking, fullscreen behavior, outside click, and
  prior-client focus restoration remain unmeasured acceptance checks rather
  than product claims.
- The post-landing synchronous region/capture-phase correction passes the
  deterministic and deny-warning local gates; operator confirmation closed the
  reveal regression.

### Next

- Complete the remaining pointer, keyboard, focus, and stacking rows in the
  public-safe Hyprland/Waybar checklist.
- Begin typed compositor state and direct Hyprland socket integration after the
  native surface proof is accepted.

## 2026-08-30 - Adopt GPL-3.0-only

The repository now licenses Weftwise under GPL-3.0-only, matching OdyTTY. Cargo
metadata and the README identify the license, and the canonical GPL version 3
text is tracked in `LICENSE`. Contributions use the Developer Certificate of
Origin 1.1 instead of a Contributor License Agreement; contributors retain
copyright in their work.

### Verification

- The first worktree gate passed 17 tests, then the public-safety check rejected
  a non-allowlisted instructional email placeholder. The example now uses the
  synthetic GitHub no-reply address format.
- `bash .github/scripts/check.sh --worktree`: passed with Rust 1.97.1 and 17
  tests; zero failed, ignored, or measured.
- The same complete tree gate passed with Rust 1.96.0 in the digest-pinned Arch
  environment.
- The tracked `LICENSE` file is byte-for-byte identical to OdyTTY's canonical
  GPL version 3 license text.
- README, Cargo metadata, and contribution terms consistently identify
  `GPL-3.0-only`.

## 2026-08-30 - Harden Phase 0 CI inputs

The Linux gate now starts from a reviewed Arch `base-devel` image digest instead
of a mutable image tag. The only external action remains checkout at a reviewed
full commit SHA, and its credentials are removed after the source is fetched.
Container commands use Bash explicitly. Rust and native GTK versions are
reported before the repository gate runs.

### Verification

- The first worktree gate stopped at `cargo fmt --check` after the CI contract
  test was added. Formatting was applied before the complete gate reran.
- `bash .github/scripts/check.sh --worktree`: passed with Rust 1.97.1 and 16
  tests; zero failed, ignored, or measured.
- The pinned Arch image, package sequence, Rust 1.96.0 toolchain, native-library
  preflight, and `bash .github/scripts/check.sh --tree` completed successfully in
  an isolated container.
- Checkout source and release metadata identify the pinned commit as the signed
  v7.0.1 release.
- Workflow `run:` blocks contain no GitHub expression opener.
- Linux CI remains unmeasured until the workflow runs on GitHub-hosted
  infrastructure.

## 2026-08-30 - Add the Phase 0 Rust scaffold

The repository now contains a Rust 2024 crate with the retained application,
domain, adapter, surface, arbitration, and Relm4 presentation boundaries. The
initial hidden root component does not create a layer surface or product UI.
Relm4's shared Tokio runtime is bounded to one asynchronous worker and four
blocking workers before first use. The supervisor owns spawned adapter handles
and constructs adapter futures inside the entered runtime.

The lockfile resolves Relm4 0.11, gtk4-layer-shell 0.8, one gtk4-rs 0.11 line,
one Tokio 1.53 line, zbus 5 with its Tokio backend, serde/TOML, tracing, and
`thiserror`. Rust 1.96.0 and a 1.96 Cargo MSRV are pinned as the project pair.
An isolated Arch `base-devel` environment completed the full locked gate with
Rust 1.96.0. The available Rust 1.97.1 compiler also passed the same gate.

Versioned configuration parsing rejects unknown keys, invalid values, and
unsupported schemas. XDG configuration, cache, state, and runtime paths are
resolved without logging their values. Runtime absence remains explicit.
Private directory and file modes are defined as `0700` and `0600`, and default
diagnostic policy redacts user paths, desktop text, content metadata, and
process arguments.

Local and Linux CI gates now cover formatting, deny-warning Clippy, locked
tests, documentation, dependency topology, production Rust file size,
public-safety checks, and RustSec. Worktree safety uses an isolated temporary
Git index and does not change the real staging index. Test fixtures are required
to contain synthetic public-safe data.

### Verification

- `pkg-config --modversion gtk4 gtk4-layer-shell-0`: passed; the reference
  environment reported GTK4 4.22.4 and gtk4-layer-shell 1.3.0.
- `bash .github/scripts/check.sh --tree`: passed under Rust 1.96.0 in an
  isolated Arch `base-devel` environment against the shared lockfile and
  supported native-library baseline. This included formatting, deny-warning
  Clippy, 15 tests, documentation, topology, file size, public-safety, and
  RustSec.
- `cargo fmt --check`: passed with Rust 1.97.1.
- `cargo clippy --all-targets --locked -- -D warnings`: passed with Rust 1.97.1.
- `cargo test --locked`: passed 15 tests; zero failed, ignored, or measured.
- `cargo doc --no-deps --locked`: passed.
- Dependency topology: one gtk4-rs version (`0.11.4`), one Tokio version
  (`1.53.1`), no GTK3 binding, and no second async runtime detected.
- Production Rust file-size gate: passed; every file is below 2,000 lines.
- `bash .github/scripts/public-safety.sh --worktree`: passed; manual review is
  still required before any landing.
- `cargo audit --deny warnings --file Cargo.lock`: passed while scanning 147
  dependencies against 1,226 loaded RustSec advisories.
- Shell syntax validation for repository scripts: passed.
- Linux CI: configured but unmeasured until a workflow run occurs.
- Native layer-shell behavior, input regions, output ownership, focus, and
  Waybar coexistence: unimplemented and unmeasured.

### Next

- Select the project license in a separate complete landing.
- Implement and measure the native Selvage, Ribbon, and Panel proof beside
  Waybar.

## 2026-08-30 - Replace vague product language

The README and interface documents now describe dimensions, states, triggers,
and controls directly. The project summary identifies Weftwise as a minimal
top-edge interface. Terms that described tone instead of behavior were removed.

No product behavior changed. The repository still contains documentation only.

### Verification

- The removed terminology no longer appears in current tracked files.
- Documentation safety and formatting checks passed before publication.
- Rust gates remain unavailable because no Rust crate exists yet.

---

## 2026-08-29 - Define the interface and repository rules

Weftwise is defined as a top-edge interface rather than a restyled collection of
status modules. It has three levels: a 2-3 pixel Selvage for persistent state, a
compact Ribbon revealed from the top edge, and an interactive Panel opened by a
click or key binding. The collapsed surface reserves no compositor work area.

The architecture selects Rust, GTK4, Relm4, `gtk4-layer-shell`, Tokio, `zbus`,
serde, TOML, and structured tracing. Service adapters remain independent from
GTK components and publish typed events into one authoritative application
state. Output surfaces are managed per monitor from the first native proof,
while comprehensive hotplug behavior remains later work.

The initial repository adds project, architecture, interface, contribution,
security, and data-safety documentation. Local development
instructions remain ignored, while public policy is tracked. A staged-content
safety check rejects likely secrets, private home-directory paths, dangerous
filenames, and non-GitHub commit email identities before a local landing.

The reviewed files were published as the root commit of the public
`ghreprimand/weftwise` repository. GitHub secret scanning and push protection
are enabled. Private vulnerability reporting, vulnerability alerts, and
automated dependency security fixes are enabled, and the unused wiki is
disabled so maintained Markdown remains the documentation source of truth.

### Verification

- All maintained relative documentation links resolve to tracked files.
- The automated staged-content public-safety gate and a manual review for local
  paths, personal identifiers, private addresses, email addresses, and internal
  workflow language passed on 2026-08-29.
- The public `main` branch was verified against the reviewed local commit.
- The safety script passes Bash syntax validation. ShellCheck is unavailable in
  the current development environment and is not claimed.
- Rust formatting, linting, tests, and dependency audit: unavailable because no
  Rust crate exists yet.
- Native Wayland behavior: unimplemented and unmeasured.

### Next

- Select the project license.
- Create the Rust workspace and lock the verified toolchain/MSRV pair.
- Implement the bounded native layer-surface proof.
