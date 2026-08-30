# Weftwise - Devlog

Public running record of Weftwise development in reverse-chronological order.
Entries describe what changed, what was verified, and what remains unavailable
or unmeasured. Planned work is never presented as implemented behavior.

---

## 2026-08-30 - Restore repository-backed CI checkout

The Arch CI job now installs Git before `actions/checkout`. The previous order
caused checkout to fall back to a source archive, leaving the job without the
repository metadata required by the tracked-tree safety gate. A contract test
now preserves the bootstrap-before-checkout ordering and requires Git in that
bootstrap step.

### Verification

- GitHub Actions run `33314267426` reproduced the original failure at
  `git rev-parse`: `fatal: not a git repository (or any parent up to mount point /)`.
- `bash .github/scripts/check.sh --worktree`: passed locally with 32 tests;
  zero failed, ignored, or measured.
- The complete worktree gate passed with Rust 1.96.0 in the project's
  digest-pinned Arch `base-devel` environment, including formatting,
  deny-warning Clippy, tests, documentation, public-safety, and RustSec.
- A replacement GitHub Actions run remains required before the CI repair is
  considered verified on the hosted runner.

### Next

- Push the reviewed repair and verify the replacement GitHub Actions run.
- Continue the Phase 2 state and Hyprland integration landing after CI is green.

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
- Pointer pass-through, dwell interaction, stacking, fullscreen behavior,
  outside click, and prior-client focus restoration remain unmeasured acceptance
  checks rather than product claims.

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
