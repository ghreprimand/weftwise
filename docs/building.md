# Building and validation

The supported development baseline is OdysseyOS/Arch on native Wayland with
Hyprland. Other distributions are unverified.

## Native packages

Install the native build baseline through the system package manager:

```sh
sudo pacman -S --needed base-devel rustup gtk4 gtk4-layer-shell pipewire clang
```

Confirm the required native interfaces before invoking Cargo:

```sh
pkg-config --modversion gtk4
pkg-config --modversion gtk4-layer-shell-0
pkg-config --modversion libpipewire-0.3
```

The selected audio boundary links the upstream PipeWire Rust binding to
`libpipewire-0.3`. Arch's `pipewire` package supplies that interface without a
separate development package, while `clang` supplies `libclang` for the
binding's generated native declarations. WirePlumber remains the runtime policy owner;
Weftwise uses PipeWire metadata and graph APIs rather than linking a
WirePlumber Rust binding or polling `wpctl`. The native audio transport is a
default feature. `cargo build --no-default-features` is supported for bounded
transport-free development, but it cannot observe or control live audio.

## Rust dependency baseline

`rust-toolchain.toml` selects Rust 1.96.0 and `Cargo.toml` declares a 1.96 MSRV.
The complete locked gate passes under that exact compiler on the supported Arch
native-library baseline. The lockfile resolves Relm4 0.11, the matching gtk4-rs
line used by gtk4-layer-shell 0.8, Tokio 1.x, zbus 5 with only its Tokio backend,
serde/TOML, tracing, and `thiserror`.

Dependency changes must confirm that the graph contains one GTK major line,
one Tokio runtime, and no unintended async executor:

```sh
cargo tree -d
cargo tree -i gtk4
cargo tree -i tokio
```

## Runtime ownership

The application configures Relm4 for one asynchronous worker thread and four
blocking worker threads before the shared runtime is first used. Long-lived
adapter factories are owned by the supervisor and invoked inside that entered
runtime. Tokio-backed zbus connections must be created inside those factories.
Adapters send typed, non-GTK data to the root model; GTK objects remain on the
main thread.

## Configuration and private files

The versioned user file is `weftwise/config.toml` below the XDG configuration
base, with the standard `.config` home fallback. Cache and state directories use
the XDG cache and state bases. Runtime endpoints require `XDG_RUNTIME_DIR` and
have no shared temporary-directory fallback.

Future directory and file writers must apply modes `0700` and `0600`. The
application currently performs a bounded startup read of this file; an absent
file selects defaults. Unknown keys, unsupported schema versions, files larger
than 64 KiB, unsafe CSS token strings, and out-of-range activation geometry are
errors. Diagnostic formatting redacts resolved paths, desktop text, content
metadata, and process arguments by default.

`[activation]` selects `exposed-edge` or comparison-only `full-width` input and
bounds the island width, height, margin, and alignment in GDK logical pixels.
The height also controls the width of the short left- and right-edge legs used
for horizontal entry on internal top edges. An output surrounded across its
top and both upper sides reveals on bounded entry; physical corners retain the
configured dwell.
`[ribbon]` enables the workspace, selected context, and clock regions.
`[theme]` supplies validated semantic colors, font family, font size, and corner
radius. Restart Weftwise after editing; live reload has not landed.

The local activity listener uses
`${XDG_RUNTIME_DIR}/weftwise/activity-v1.sock`. It starts with the supervised
adapters, requires a private current-user runtime base, applies `0700` to the
application directory and `0600` to the socket, and authenticates Linux peer
credentials. Protocol frames are JSON lines capped at 16 KiB; concurrent
clients, per-client idle time, and per-client message rate are bounded. A safe
owned refused socket is treated as stale, but a regular file, foreign socket,
or live endpoint is never replaced. The CLI verifies that same ownership and
mode boundary before connecting, then waits at most two seconds for the fixed
acknowledgement sent after validation and root-message handoff.

Publish, update, complete, or cancel synthetic tracked activity with typed
arguments:

```sh
weftwise activity publish build.synthetic build "Synthetic build" --progress-bp 2500
weftwise activity update build.synthetic --progress-bp 7500
weftwise activity complete build.synthetic succeeded --label "Synthetic build complete"
weftwise activity cancel build.synthetic
```

Run `weftwise --help` for the complete bounded grammar. Labels are display data;
the CLI has no option for a command, argument vector, environment, or shell
string.

The running GTK application exports a typed `reveal` action on the session bus.
Hyprland can own a configurable global binding without giving GTK a global key
grab. The recommended default is spatially aligned with the top-edge surface:

```ini
bind = SUPER, grave, exec, weftwise reveal
```

`weftwise reveal` targets the compositor-focused output, falls back
deterministically when focus state is unavailable, and dismisses after 2.5
seconds unless pointer or Panel interaction takes ownership.

`config/example.toml` contains synthetic values only. Test fixtures must also
use invented identities, paths, desktop text, metadata, hosts, outputs, and
process arguments. The public-safety script supplements, but does not replace,
manual inspection.

## Phase 1 native proof

Run the selected zone-`-1` policy only from a native Hyprland session:

```sh
cargo run --locked
```

Select the retained zone-`0` comparison or force reduced motion with public-safe
environment switches:

```sh
WEFTWISE_EXCLUSIVE_ZONE=0 cargo run --locked
WEFTWISE_REDUCED_MOTION=1 cargo run --locked
```

`WEFTWISE_EXCLUSIVE_ZONE` accepts only `0` and `-1`.
`WEFTWISE_REDUCED_MOTION` accepts `0`, `1`, `false`, and `true`. Invalid values
produce redacted startup errors. A missing display also exits with a structured
error instead of creating partial surfaces.

Use [the public-safe manual checklist](../tests/phase1_manual_hyprland_waybar_checklist.md)
for native interaction checks. The zone comparison selected `-1` after a
four-output session showed physical-top placement without a new reserved area.
Do not infer pointer pass-through, focus restoration, or stacking from
automated state tests.

## Phase 2 Hyprland adapter

The adapter uses the active session's `XDG_RUNTIME_DIR` and
`HYPRLAND_INSTANCE_SIGNATURE` only to resolve Hyprland's request and event
sockets. Their values and derived paths never enter normal diagnostics. Missing
or invalid values produce an explicit unavailable state. No `hyprctl` polling
process is used for primary state.

The event socket is connected before the initial JSON snapshot. Request
connections are fresh, strictly timed, and limited to 1 MiB each. Event lines
are limited to 64 KiB, while the initial race buffer is limited to 512 known
events and 256 KiB of source data. A parse or truncation gap triggers new path
discovery and a full snapshot. Synthetic parser and reducer tests do not replace
a sanitized native check of transport recovery. A full Hyprland restart changes
`HYPRLAND_INSTANCE_SIGNATURE` and terminates the GTK Wayland connection; verify
that case by starting a fresh Weftwise process after an orderly session cycle,
not by expecting the old process to reconnect.

## Required gates

Run the complete local gate with full output retained below the ignored
`target/gate-logs` directory:

```sh
bash .github/scripts/check.sh --worktree
```

The script runs formatting, deny-warning Clippy, locked tests, documentation,
production Rust file-size and dependency-topology checks, worktree public-safety
checks, and RustSec.
Before a local landing, stage the reviewed content and also run the required
staged-content command:

```sh
bash .github/scripts/public-safety.sh
```

CI runs the same gates in tree mode. A successful automated scan still requires
manual review of every staged byte before a commit.
