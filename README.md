# Weftwise

[Development log](DEVLOG.md) |
[Documentation](docs/) |
[Issues](https://github.com/ghreprimand/weftwise/issues) |
[Security](SECURITY.md)

**A minimal top-edge desktop interface for OdysseyOS and Hyprland.**

Weftwise is a native Wayland top bar that keeps a 2-3 pixel status line visible
and expands when used. It starts alongside Waybar. Planned features include
workspace navigation, activity progress, privacy indicators, temporary system
feedback, and system controls.

The project is pre-alpha. The repository contains a buildable Rust/Relm4 native
proof and the design, architecture, and repository-safety documentation. The
proof implements per-output layer surfaces and the Selvage, Ribbon, and Panel
interaction model. It also contains the typed Phase 2 state core, direct
Hyprland socket adapter, local workspace/context projections, and in-process
clock fallback. The root also owns the Phase 3 deterministic presentation
candidate arbiter and color-independent accessible Selvage projections.
The Phase 4 slice discovers MPRIS players over the session bus, sanitizes media
metadata, selects one active player deterministically, renders its progress and
Ribbon label, and exposes only advertised transport actions.
Corrected pointer delivery, stacking, and focus behavior remain unmeasured until
the Hyprland/Waybar checklist passes.

## Interface model

Weftwise uses three levels of presentation:

- **Selvage:** a persistent 2-3 pixel line at the top edge. Marks show workspace
  position, active progress, and warnings without reserving vertical space.
- **Ribbon:** a compact labeled bar revealed by holding the pointer against the
  top edge. It provides labels, values, and immediate controls.
- **Panel:** an interactive view opened by click or a Hyprland binding for
  navigation, search, media, calendar, quick settings, and history.

The Selvage shows workspace position, progress, and warnings. Exact values
appear in the Ribbon or Panel rather than in permanent status labels.

See [the interface model](docs/interface-model.md) for behavior, state encoding,
and initial feature priorities.

## Technical direction

- Rust with the 2024 edition
- GTK4 through `gtk4-rs`
- Relm4 for typed component and message flow
- `gtk4-layer-shell` for native Wayland shell surfaces
- Tokio for supervised asynchronous integrations
- `zbus` for D-Bus services
- TOML configuration under the XDG configuration hierarchy
- `tracing` for structured diagnostics

Weftwise is Linux-, Wayland-, and Hyprland-specific. X11, GNOME Wayland, and
cross-platform desktop support are not current goals.

The architecture keeps compositor, media, audio, process, and other service
integrations outside GTK components. Adapters publish typed events into one
authoritative application state; UI components render state and emit typed
actions. See [the architecture overview](docs/architecture.md) and the
[project scaffolding](scaffolding.md).

## Development status

The Phase 0 repository baseline now includes the retained module boundaries, a
locked dependency graph, bounded Relm4 runtime settings, versioned XDG
configuration types, redacted diagnostic defaults, and local/CI validation
scripts. Rust 1.96.0 is the pinned project toolchain and has passed the complete
locked gate on the supported Arch native-library baseline.

The Phase 1 native proof code provides:

1. one top-edge overlay surface per GDK output;
2. fixed transparent geometry with a 3-pixel collapsed input region;
3. deterministic dwell reveal, delayed dismissal, and explicit Panel state;
4. Ribbon animation with GTK and explicit reduced-motion handling;
5. an attached keyboard-navigable Panel with Escape and outside-click
   dismissal; and
6. owned output signals, UI timers, process shutdown handling, and adapter
   cancellation.

The proof uses `exclusive_zone = -1`. A public-safe native Hyprland comparison
beside Waybar showed that `0` and `-1` preserved the existing work area, while
only `-1` kept every surface at the physical top edge. Pointer pass-through,
stacking, and focus restoration remain manual acceptance checks.

The Phase 2 state slice connects to Hyprland's event socket before requesting
bounded JSON snapshots through fresh request connections. It applies the
snapshot atomically, replays buffered address-bearing events in order, and
re-resolves the instance after parse gaps, disconnects, or compositor restarts.
Root state now owns typed outputs, workspaces, clients, active context, adapter
availability, and GDK connector bindings. Each surface renders only its local
workspace marks; the focused Ribbon renders active context with a
boundary-aligned in-process clock fallback. This code has deterministic
coverage, while a sanitized real-session restart check remains unmeasured.

The Phase 3 context slice deduplicates source-scoped stable candidate identities, applies
normalized timestamp ordering, expiration, bounded stickiness, explicit
preemption, stale-producer removal, and output affinity in a pure root-owned
reducer. Each immutable output projection has stable navigation, activity, and
attention regions. Shape, width, fill pattern, visible text, and accessible
labels distinguish selection and warnings without relying on color alone.
Candidate actions remain typed data rather than command strings.

The Phase 4 media slice installs MPRIS owner and property subscriptions before
its initial bounded player snapshot. Playing players outrank paused and stopped
players; stopped players expire after a bounded recent-activity interval, and
unknown playback states are excluded. Player identity, title, artist, artwork
URL, duration, and position are sanitized or clamped before root-state
insertion. Artwork is not fetched. The selected player produces one activity
mark and Ribbon label, while Play/Pause, Previous, Next, and relative seek
controls are shown and dispatched only when the player advertises the required
capability. Commands are pinned to the advertised unique-owner generation so a
delayed action cannot control a restarted player. Player and session-bus
restarts are handled independently with bounded retry.

Claims in project documentation describe implemented behavior only when they
are accompanied by verification. Planned behavior is labeled as planned.

## Building the scaffold

The documented native baseline is OdysseyOS/Arch with `base-devel`, `rustup`,
`gtk4`, `gtk4-layer-shell`, and `pipewire`. The selected audio boundary uses
direct PipeWire registry and metadata APIs while the installed WirePlumber
service remains the policy owner; Weftwise does not link a WirePlumber Rust
binding. The PipeWire audio transport is compiled behind the optional
`audio-transport` Cargo feature, which enables the `pipewire` dependency and
requires the `libpipewire-0.3` and `libspa-0.2` development headers plus
`clang` for binding generation. The default build and every repository gate
run without that feature; the pure audio domain and typed contracts are always
built and tested. See
[building and validation](docs/building.md) for package checks, configuration
locations, runtime limits, and repository gates.

## Contributing and security

The repository is publicly developed at
[`ghreprimand/weftwise`](https://github.com/ghreprimand/weftwise). Read
[CONTRIBUTING.md](CONTRIBUTING.md) before proposing or implementing a change.
Security-sensitive reports must follow [SECURITY.md](SECURITY.md), not a public
issue.

All tracked content is treated as immediately public. The
[public repository safety policy](docs/public-repository-safety.md) prohibits
secrets, personal data, private infrastructure, machine-local configuration,
and unsanitized runtime captures.

## Documentation

- [Interface model](docs/interface-model.md)
- [Architecture](docs/architecture.md)
- [Building and validation](docs/building.md)
- [Project scaffolding](scaffolding.md)
- [Public repository safety](docs/public-repository-safety.md)
- [Development log](DEVLOG.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## License

Weftwise is licensed under **GPL-3.0-only**. The source may be used, studied,
shared, and modified under that license; distributed modifications must use the
same license. See [LICENSE](LICENSE).
