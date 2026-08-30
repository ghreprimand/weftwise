# Weftwise

**A quiet, contextual desktop surface for OdysseyOS and Hyprland.**

Weftwise is a native Wayland interface that replaces a permanently crowded
status bar with an ambient top edge. It starts as a companion to Waybar and is
intended to grow into a cohesive home for workspace navigation, current
activity, privacy state, transient system feedback, and focused controls.

The project is pre-alpha. The repository currently contains the product,
interaction, architecture, and public-safety foundation. There is no runnable
application yet.

## Interface model

Weftwise uses three levels of presentation:

- **Selvage:** a persistent 2-3 pixel line at the top edge. It communicates
  workspace position, current activity, and exceptional system state without
  reserving vertical space.
- **Ribbon:** a compact labeled surface revealed by deliberate pointer intent
  at the top edge. It provides exact context and immediate controls.
- **Panel:** an interactive view opened by click or a Hyprland binding for
  navigation, search, media, calendar, quick settings, and history.

The default presentation is silent unless information is timely, actionable,
or safety-critical. Exact values belong in the Ribbon or Panel rather than in
permanent status labels.

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

The first executable milestone will prove:

1. one non-exclusive top-edge surface per output;
2. a 2-3 pixel pointer-active Selvage with click-through transparent space;
3. smooth Ribbon reveal without resizing the layer surface;
4. reliable pointer, focus, Escape, and outside-click behavior;
5. active workspace/window state with a clock fallback; and
6. clean startup, shutdown, and adapter degradation.

Claims in project documentation describe implemented behavior only when they
are accompanied by verification. Planned behavior is labeled as planned.

## Contributing and security

The repository is being prepared for public development. Read
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
- [Project scaffolding](scaffolding.md)
- [Public repository safety](docs/public-repository-safety.md)
- [Development log](DEVLOG.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## License

No license has been selected yet. Until a license is added, the source and
documentation remain protected by their respective copyright holders and are
not granted for reuse or redistribution.
