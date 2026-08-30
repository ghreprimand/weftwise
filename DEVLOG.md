# Weftwise - Devlog

Public running record of Weftwise development in reverse-chronological order.
Entries describe what changed, what was verified, and what remains unavailable
or unmeasured. Planned work is never presented as implemented behavior.

---

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
no `unsafe` code. Default and stream routing through the WirePlumber-owned
policy metadata is reported as a transport limitation pending native verification
rather than claimed as complete.

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
