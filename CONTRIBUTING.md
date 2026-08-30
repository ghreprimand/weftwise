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

## Developer Certificate of Origin

Weftwise uses the Developer Certificate of Origin 1.1 instead of a Contributor
License Agreement. A commit sign-off certifies that the contributor wrote the
patch or has the right to submit it under the project's license.

Sign off a commit with `-s`:

```sh
git commit -s -m "your commit message"
```

This appends a line in this form:

```text
Signed-off-by: Contributor Name <123456+contributor@users.noreply.github.com>
```

The sign-off identity must match the commit author. Contributions are accepted
under **GPL-3.0-only**, and contributors retain copyright in their work.

The complete DCO 1.1 text follows:

```text
Developer Certificate of Origin
Version 1.1

Copyright (C) 2004, 2006 The Linux Foundation and its contributors.

Everyone is permitted to copy and distribute verbatim copies of this
license document, but changing it is not allowed.


Developer's Certificate of Origin 1.1

By making a contribution to this project, I certify that:

(a) The contribution was created in whole or in part by me and I
    have the right to submit it under the open source license
    indicated in the file; or

(b) The contribution is based upon previous work that, to the best
    of my knowledge, is covered under an appropriate open source
    license and I have the right under that license to submit that
    work with modifications, whether created in whole or in part
    by me, under the same open source license (unless I am
    permitted to submit under a different license), as indicated
    in the file; or

(c) The contribution was provided directly to me by some other
    person who certified (a), (b) or (c) and I have not modified
    it.

(d) I understand and agree that this project and the contribution
    are public and that a record of the contribution (including all
    personal information I submit with it, including my sign-off) is
    maintained indefinitely and may be redistributed consistent with
    this project or the open source license(s) involved.
```

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
