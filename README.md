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

The project is pre-alpha. The repository contains a buildable Rust/Relm4
scaffold and the design, architecture, and repository-safety documentation.
The executable initializes a hidden root component, but no layer surface or
product interface is implemented yet.

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

The next native proof requires:

1. one non-exclusive top-edge surface per output;
2. a 2-3 pixel pointer-active Selvage with click-through transparent space;
3. animated Ribbon reveal without resizing the layer surface;
4. tested pointer, focus, Escape, and outside-click behavior;
5. active workspace/window state with a clock fallback; and
6. startup and shutdown without orphaned adapter tasks.

Claims in project documentation describe implemented behavior only when they
are accompanied by verification. Planned behavior is labeled as planned.

## Building the scaffold

The documented native baseline is OdysseyOS/Arch with `base-devel`, `rustup`,
`gtk4`, and `gtk4-layer-shell`. PipeWire and WirePlumber development packages
are deferred until their integration boundary is selected. See
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

No license has been selected yet. Until a license is added, the source and
documentation remain protected by their respective copyright holders and are
not granted for reuse or redistribution.
