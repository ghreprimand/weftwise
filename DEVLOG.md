# Weftwise - Devlog

Public running record of Weftwise development in reverse-chronological order.
Entries describe what changed, what was verified, and what remains unavailable
or unmeasured. Planned work is never presented as implemented behavior.

---

## 2026-08-29 - Establish the product and public repository foundation

The initial product direction defines Weftwise as an ambient desktop control
surface rather than a restyled collection of status modules. Its interface has
three levels: a 2-3 pixel Selvage for quiet state, a compact Ribbon revealed by
pointer intent, and an interactive Panel opened explicitly. The collapsed
surface reserves no compositor work area.

The architecture selects Rust, GTK4, Relm4, `gtk4-layer-shell`, Tokio, `zbus`,
serde, TOML, and structured tracing. Service adapters remain independent from
GTK components and publish typed events into one authoritative application
state. Output surfaces are managed per monitor from the first native proof,
while comprehensive hotplug behavior remains later work.

The repository foundation adds public project, architecture, interface,
contribution, security, and data-safety documentation. Local development
instructions remain ignored, while public policy is tracked. A staged-content
safety check rejects likely secrets, private home-directory paths, dangerous
filenames, and non-GitHub commit email identities before a local landing.

The reviewed foundation was published as the root commit of the public
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
