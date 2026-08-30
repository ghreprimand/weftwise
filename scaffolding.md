# Weftwise - Project Scaffolding

## Status

This document records the product and implementation decisions used to scaffold
the public repository. Implemented behavior, once present, takes precedence over
planned behavior and must remain synchronized with this document.

The Phase 0 repository baseline is implemented and verified under the pinned
Rust toolchain. Phase 1 native-proof code now owns per-output layer surfaces,
fixed input geometry, deterministic reveal/dismissal state, and an attached
Panel. A native Hyprland/Waybar comparison selected `exclusive_zone = -1` for
physical-edge placement without a new reserved area. Pointer, stacking, and
focus behavior remain unmeasured. Phase 2 now provides root-owned typed
compositor state, event-first direct Hyprland socket reconciliation, local
workspace and active-context rendering, and a boundary-aligned in-process clock.
Phase 3 adds the root-owned deterministic presentation candidate reducer plus
stable navigation/activity/attention projections whose shape, fill pattern,
text, and accessible labels do not rely on color alone. Compositor restart and
native accessibility behavior remain session checks. Phase 4 now provides
subscribed MPRIS discovery, bounded media state, deterministic active-player
selection, progress and Ribbon projections, typed capability-gated controls,
and independent player/session-bus restart recovery.

## Name

**Weftwise** is a textile term meaning “in the direction of the weft”: horizontally across a piece of fabric, from selvage to selvage. The name fits a horizontal desktop surface spanning the screen while also suggesting a system that is context-aware or “wise” about what it displays.

Working technical names:

- Display name: `Weftwise`
- Repository and package: `weftwise`
- Executable: `weftwise`
- Application ID: `io.unfinished_works.weftwise`

The exact crates.io, npm, PyPI, Arch/AUR, and `ghreprimand/weftwise` namespaces appeared unused when checked on 2026-08-29. That was an availability check, not formal trademark clearance.

## Product concept

Weftwise is a native desktop interface for OdysseyOS on Hyprland. Its collapsed
state is a 2-3 pixel top-edge line that runs alongside Waybar. Later phases can
replace Waybar modules and add desktop-shell functions.

The goal is not to reproduce Waybar with different styling. Waybar already handles independent status labels well. Weftwise should provide capabilities that are awkward in Waybar's module-and-stdout model:

- Shared, typed state across widgets.
- Context-sensitive content instead of a permanently crowded status strip.
- Panels, popovers, lists, search, and keyboard interaction.
- Transitions from status marks to labeled controls.
- Event-driven integrations rather than many polling scripts and subprocesses.
- Precise control over layer surfaces, input regions, monitor behavior, and animations.
- A unified design language for the bar, OSDs, notifications, quick settings, and related shell components.

The bar uses a central priority policy. Warnings and active work replace the
fallback display when necessary, and the bar expands to show controls when the
user opens it.

## Initial user experience

The first version uses a minimal top-edge surface alongside the existing Waybar
configuration. Its interaction model has three levels:

- **Selvage:** a persistent 2-3 pixel status line with no exclusive zone.
- **Ribbon:** a compact labeled surface revealed by holding the pointer against
  the top edge.
- **Panel:** an interactive surface opened by click or a Hyprland binding.

One surface should be owned per output from the initial native proof. The
focused or pointer-active output receives detailed contextual presentation.
Waybar should continue providing its existing modules during early development.
Weftwise should show contextual information rather than a permanent row of
generic hardware statistics.

Potential contextual content, in rough priority order:

1. Privacy-critical state: microphone use, screen sharing, recording, camera, or an active idle inhibitor.
2. Timed activity: timer, build, download, render, update, or other process progress.
3. Active media: title, artist, progress, and compact playback controls.
4. Active workspace/window context: application, title, project, Git branch, and dirty state where available.
5. Temporary feedback: volume, brightness, screenshot, clipboard, or command results.
6. Fallback: time and date.

The implemented display policy is explicit and centralized. Producers publish
candidate content into a root-owned arbitration system rather than fighting over
layout directly. Candidates include source-scoped stable identity, semantic kind,
severity, normalized creation/update/expiration time, bounded minimum display
duration, preemption class, optional progress, bounded typed actions, and output
affinity. Priority alone is insufficient: deterministic ties, update-in-place,
stickiness, expiration, stale-source removal, and explicit preemption prevent
flicker and stale content.

## Platform and technology choices

The selected stack is:

- **Language:** Rust
- **UI:** GTK4 via `gtk4-rs`
- **Wayland shell integration:** `gtk4-layer-shell`
- **Asynchronous work:** Tokio where it is useful for sockets, process supervision, and service listeners
- **D-Bus:** `zbus`
- **Serialization/configuration:** `serde`, `serde_json`, and likely TOML for user configuration
- **Logging/diagnostics:** `tracing` and `tracing-subscriber`

Relm4 is the selected component and message architecture over GTK4. The root
model remains the authoritative owner of product state; child components receive
state projections and emit typed actions.

The project should be native Wayland software. X11 and non-Hyprland compositor compatibility are not initial requirements, though compositor-specific code should remain behind an adapter boundary where practical.

## Core architecture

```text
Hyprland request socket ─┐
Hyprland event socket ───┤
D-Bus services ──────────┤
PipeWire/WirePlumber ────┼── service adapters ──> typed application state
system sensors ──────────┤                              │
timers/processes ────────┘                              ▼
                                               context arbitration
                                                        │
                                                        ▼
                                             GTK component rendering
                                                        │
                                                        ▼
                                                action dispatcher
```

### UI process

GTK must remain on its main thread. Background socket, D-Bus, and process listeners should send typed messages into the GTK application rather than mutating widgets directly.

The initial application owns one top-anchored layer-shell surface per output.
Every output renders its local workspace state. Detailed global context appears
on the focused or pointer-active output unless configuration requests otherwise.
A surface manager owns creation, removal, scale changes, and compositor
reconciliation.

### State store

Maintain one authoritative, typed application state. UI components render state and emit actions. Integrations update state through messages. Avoid hidden state inside individual widgets when that state is useful elsewhere.

Likely top-level domains:

- compositor and monitor state
- workspaces and clients
- active context
- media sessions
- audio and privacy state
- timers and tracked processes
- notifications
- power and connectivity
- configuration and theme
- panel visibility/focus state

### Service adapters

Each external integration should be isolated behind a trait or module and should:

- Prefer event subscriptions over polling.
- Establish an initial snapshot before consuming events.
- Reconnect with bounded backoff after a service or Hyprland restart.
- Degrade independently so one broken service does not take down the shell.
- Convert raw protocol data into internal typed messages.
- Avoid leaking transport details into GTK components.

### Action dispatcher

Clicks, scrolls, keyboard commands, and expanded-panel actions should emit typed actions. Avoid interpolated shell command strings. Where an external process is required, invoke it with an explicit program and argument vector.

### Configuration and theme

Use versioned TOML under `$XDG_CONFIG_HOME/weftwise/config.toml`, falling back to
`~/.config/weftwise/config.toml`. Do not begin by creating a general plugin
language. Stable typed configuration is preferable while the product model is
still forming.

GTK CSS should provide OdysseyOS's dark/gold visual identity. Keep semantic design tokens—backgrounds, surfaces, text, accent, warning, critical, radii, spacing, and motion durations—separate from widget selectors so themes can evolve cleanly.

## Hyprland integration

Hyprland exposes two relevant Unix sockets under its runtime directory:

- A request socket for queries and dispatches.
- An event socket that emits newline-delimited events for workspaces, windows, monitors, fullscreen state, and other compositor changes.

The Hyprland adapter should consume the event socket continuously and use structured JSON queries for initial snapshots or reconciliation. Do not make periodic `hyprctl` subprocess calls the primary state mechanism.

Initial events of interest include:

- workspace changes
- focused monitor changes
- active window changes
- window open/close/move events
- monitor add/remove events
- fullscreen changes
- layer-surface changes where useful

The parser should tolerate unknown future events and malformed individual lines without terminating the listener.

## Wayland layout constraint

Layer shell can reserve an exclusive zone from an output edge, but it cannot describe an irregular work area around separated islands. Transparent space in a full-width reserved strip is still unavailable to tiled windows.

Therefore:

- The companion Selvage and Ribbon use `exclusive_zone = -1` to reach the physical edge while overlaying applications and Waybar without reserving work area.
- A future full replacement must choose between reserving the entire top band, overlaying application windows, or collapsing/revealing dynamically.
- Separate island surfaces can improve rendering and pointer input regions, but they do not make Hyprland reserve an island-shaped tiling region.

This is a protocol/compositor geometry constraint, not a Waybar-only limitation.

## Suggested source layout

```text
weftwise/
├── Cargo.toml
├── README.md
├── LICENSE
├── rustfmt.toml
├── src/
│   ├── main.rs
│   ├── app.rs
│   ├── message.rs
│   ├── state.rs
│   ├── action.rs
│   ├── config.rs
│   ├── supervisor.rs
│   ├── shell/
│   │   ├── mod.rs
│   │   ├── surface.rs
│   │   └── outputs.rs
│   ├── services/
│   │   ├── mod.rs
│   │   ├── hyprland.rs
│   │   ├── mpris.rs
│   │   ├── clock.rs
│   │   └── process.rs
│   ├── context/
│   │   ├── mod.rs
│   │   └── arbitration.rs
│   └── widgets/
│       ├── mod.rs
│       ├── selvage.rs
│       ├── ribbon.rs
│       ├── panel.rs
│       ├── active_context.rs
│       ├── media.rs
│       └── clock.rs
├── assets/
│   └── style.css
├── config/
│   └── example.toml
└── tests/
```

Preserve these conceptual boundaries even when initial files are small. Relm4
components do not replace the domain, adapter, or surface ownership layers.

## Phased implementation

### Phase 0 - Repository and dependency baseline

- Resolve and lock the GTK4, Relm4, layer-shell, Tokio, zbus, serialization,
  tracing, and error-handling dependency graph.
- Pin the candidate toolchain/MSRV pair in lockstep and verify it under the
  exact declared compiler before calling the floor supported.
- Retain the application, state, message, action, configuration, supervisor,
  shell, service, arbitration, and widget module boundaries.
- Bound shared runtime workers before first use and keep zbus construction
  inside the entered Tokio runtime.
- Define versioned XDG paths, private file modes, redacted diagnostics, and
  synthetic fixture policy.
- Run formatting, linting, tests, documentation, dependency topology, RustSec,
  file-size, and public-safety gates locally and in Linux CI.

### Phase 1 - Native shell proof

- Initialize the Rust project and development tooling.
- Open one GTK4 layer-shell surface per output.
- Use the overlay layer and a transparent fixed-height surface whose collapsed
  input region covers only the 2-3 pixel Selvage. The native A/B test selected
  zone `-1` for the physical-edge, non-reserving policy.
- Reveal a styled Ribbon without resizing the layer surface.
- Add structured logging and clean shutdown.
- Verify pointer pass-through, dwell reveal, dismissal, focus restoration,
  reduced motion, and stacking behavior while Waybar is running.

### Phases 2-4 - Contextual Ribbon and Panel

- Typed application state and message flow are implemented for outputs,
  workspaces, clients, active context, adapter availability, and presentation.
- The direct Hyprland adapter connects events first, applies bounded JSON
  snapshots, and replays buffered address-bearing events before steady state.
- Local workspace marks, focused active context, and the boundary-aligned clock
  fallback are implemented as immutable surface projections.
- MPRIS media discovery, subscribed playback state, bounded metadata, and
  capability-gated controls are implemented over D-Bus.
- Deterministic content arbitration is implemented.
- Add click-to-open Panel behavior and a Hyprland keybinding entry point.
- Hyprland, media-player, and session-bus restart recovery is implemented;
  sanitized native-session restart evidence remains a manual check.

### Phase 5 - System feedback

- Volume and microphone state through direct PipeWire graph, parameter, and
  metadata APIs while WirePlumber retains policy ownership.
- Temporary volume and brightness OSDs.
- Recording, screen-sharing, camera, and privacy indicators.
- Timers and supervised process/build progress.
- Notification summaries where useful.

### Phase 6 - Activities, workflow mode, and system health

- A versioned, size-bounded local event protocol for timers and tracked work.
- Explicit program-and-argument process supervision with cancellation and
  output bounds.
- OdysseyOS workflow profile state behind an adapter boundary.
- Threshold-based in-process system health sampling without permanent exact
  value labels.

### Phases 7 and 9 - Daily-use tools and hardening

- Multi-monitor hotplug, scale-change, renaming, and lifecycle hardening.
- Configuration loading and live theme reload where safe.
- Keyboard-accessible expanded panels.
- Quick settings and richer media controls.
- Performance, accessibility, reconnection, and lifecycle hardening.

### Phase 8 - Incremental Waybar replacement

- Native workspaces and window controls.
- Battery, power, connectivity, and audio controls.
- Notifications and history.
- System tray implementation last; it is one of the most protocol-heavy pieces.
- Decide whether Weftwise remains a group of cooperating surfaces or becomes a broader desktop-shell process.

## Explicit non-goals for the first version

- Full Waybar feature parity.
- A system tray.
- NetworkManager or Bluetooth management UI.
- A notification daemon.
- Cross-compositor support.
- A third-party plugin ecosystem.
- Arbitrary shell-script modules.
- Reclaiming an irregularly shaped tiling area around visual islands.

These can be revisited after the top-edge surface proves the state and interaction model.

## Quality expectations

- Keep idle CPU usage negligible through event-driven updates.
- Avoid launching a subprocess for each status refresh.
- Never block the GTK main thread on I/O.
- Recover from Hyprland, D-Bus service, and media-player restarts.
- Log useful state transitions without recording secrets or excessive noise.
- Unit-test parsers, reducers/state transitions, arbitration, and configuration.
- Add integration tests around service adapters where practical.
- Make missing optional services a supported state, not a fatal error.
- Use `cargo fmt`, `clippy`, and tests as standard validation gates.

## First milestone acceptance criteria

The first milestone is complete when:

1. `weftwise` launches native GTK4 layer-shell surfaces on Hyprland.
2. A 2-3 pixel Selvage appears on each output while Waybar remains active and
   reserves no work area.
3. Holding the pointer at the top edge reveals a Ribbon without intercepting
   pointer input in transparent collapsed space.
4. It shows the active window/workspace and falls back to the clock.
5. Playing media causes MPRIS information to take priority according to a documented policy.
6. Clicking the Ribbon opens a small interactive Panel that dismisses reliably
   with Escape and outside click while restoring prior focus.
7. Hyprland or the media player can restart without permanently breaking the application.
8. Idle wakeups and CPU usage meet a documented measurement threshold when no
   state changes occur.
9. The core parsers and arbitration logic have automated tests.

## Resolved scaffolding decisions

- Use Relm4 over GTK4 with one authoritative root state model.
- Use versioned TOML under the XDG configuration hierarchy.
- Create a surface per output; show detailed global context on the focused or
  pointer-active output.
- Animate content inside a fixed transparent layer surface and provide explicit
  reduced-motion behavior.
- Begin with an attached GTK popover for the Panel and verify focus behavior in
  the native proof.
- Pin the Rust toolchain and `Cargo.toml` MSRV in lockstep after the dependency
  set is compiled and the actual minimum is verified.

## Licensing decision

- Weftwise source and documentation use `GPL-3.0-only`.
- Contributions use the Developer Certificate of Origin 1.1 instead of a
  Contributor License Agreement.
