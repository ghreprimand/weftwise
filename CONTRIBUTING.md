# Contributing to Weftwise

Weftwise is an early, maintainer-led project. Changes should remain small,
reviewable, evidence-backed, and consistent with the interface model.

## Before changing code

Read the [interface model](docs/interface-model.md),
[architecture overview](docs/architecture.md), and
[public repository safety policy](docs/public-repository-safety.md).

Large feature proposals should first establish:

- the user state or action they represent;
- whether they belong in the Selvage, Ribbon, or Panel;
- their behavior when their backing service is absent or restarts;
- their focused-output and multi-output behavior;
- their keyboard and reduced-motion behavior; and
- why existing typed state and actions cannot express them.

## Standing engineering gates

- **Behavior preservation:** changes are externally behavior-neutral unless
  their scope explicitly assigns a behavior change.
- **Production-file limit:** every tracked handwritten Rust file compiled into
  a non-test target stays under 2,000 physical lines. Refactored files target
  fewer than 1,700 lines.
- **Required local gate:** each landing runs `cargo fmt --check`,
  `cargo clippy --all-targets --locked -- -D warnings`, and
  `cargo test --locked`. Full test output is retained in an ignored local log.
  Dependency, parser, transport, input, storage, and release changes also run
  the repository's RustSec audit script.
- **Platform truthfulness:** Linux on native Wayland with Hyprland is the only
  supported platform. Behavior on another compositor is never inferred from
  Hyprland results, and unsupported environments remain explicitly labeled.
- **MSRV lockstep:** `rust-toolchain.toml` and `Cargo.toml` `rust-version` move
  together, only when required, and only after the declared floor is verified.
- **Heavy-job safety:** mutation, fuzz, sanitizer, and benchmark jobs run one at
  a time inside a bounded transient cgroup, or they do not run. See the local
  contributor instructions for the current resource envelope.
- **Evidence integrity:** pass, fail, skip, ignore, unsupported, unmeasured, and
  unavailable-hardware are distinct outcomes.
- **Sibling paths:** a new guard or fix must search for equivalent
  implementations and cover them in the same change or document each exemption.
- **Prose consistency:** behavior-changing work updates comments and maintained
  documentation describing the previous behavior in the same change.

## State and UI rules

- GTK remains on its main thread.
- Product state has one authoritative owner.
- Child UI components receive state projections and emit typed actions.
- External integrations do not mutate widgets.
- Adapters prefer event subscriptions, establish an initial snapshot, reconnect
  with bounded backoff, and degrade independently.
- Shell commands are never constructed by interpolating untrusted strings.
  External programs receive an explicit executable and argument vector.
- Arbitration changes include deterministic tests for priority, expiration,
  preemption, minimum display duration, and tie-breaking.

## Public data and project voice

Do not include secrets, private infrastructure, personal data, real home paths,
machine-local configuration, unsanitized logs, clipboard contents, window
titles, media metadata, or screenshots containing private information.
Fixtures use synthetic public-safe values.

Commits, documentation, source comments, and the development log use an
impersonal project voice. They describe what changed and why. They do not
mention internal workflow roles, automation attribution, approvals, or work
assignments.

## Pull requests

A change should include:

- a concise explanation of behavior and motivation;
- tests appropriate to the changed boundary;
- exact verification results with limitations;
- documentation updates for user-visible behavior; and
- confirmation that tracked content passed the public-repository safety check.

Security vulnerabilities must follow [SECURITY.md](SECURITY.md) and must not be
opened as public issues or pull requests.

## Scaffold validation

The native package baseline, pinned toolchain status, runtime bounds, XDG
locations, and complete local command are documented in
[building and validation](docs/building.md). The gate retains full output under
the ignored `target/gate-logs` directory. `cargo-audit` must be installed for
dependency, parser, transport, input, storage, or release work.
