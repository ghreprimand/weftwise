# Building and validation

The supported development baseline is OdysseyOS/Arch on native Wayland with
Hyprland. Other distributions are unverified.

## Native packages

Install the native build baseline through the system package manager:

```sh
sudo pacman -S --needed base-devel rustup gtk4 gtk4-layer-shell
```

Confirm both required native interfaces before invoking Cargo:

```sh
pkg-config --modversion gtk4
pkg-config --modversion gtk4-layer-shell-0
```

PipeWire and WirePlumber development packages are intentionally deferred until
the audio integration boundary is selected and compiled.

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

Future directory and file writers must apply modes `0700` and `0600`. Unknown
configuration keys and unsupported schema versions are errors. Diagnostic
formatting redacts resolved paths, desktop text, content metadata, and process
arguments by default.

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
