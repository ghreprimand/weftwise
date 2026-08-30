# Architecture

Weftwise is a native Wayland process with explicit boundaries between desktop
protocols, application policy, and GTK presentation.

```text
Hyprland sockets ---------+
D-Bus services -----------+
PipeWire/WirePlumber -----+--> service adapters --> typed messages
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

## Surface model

A surface manager owns one top-anchored layer surface per output. Each surface
uses the overlay layer and no exclusive zone. Its visual allocation is tall
enough for the Ribbon, but its collapsed GDK input region covers only the
Selvage. Transparent non-input pixels pass pointer events to the surface below.

The first interactive Panel is an attached GTK popover. Keyboard interactivity
is enabled only while interaction requires it and is returned to none during
collapse. The native proof must verify focus restoration, outside-click
dismissal, Escape, compositor restart, and behavior beside Waybar before the
Panel grows.

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
