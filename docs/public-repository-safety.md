# Public Repository Safety

Every tracked byte, commit, branch, pull request, issue, workflow log, release
artifact, and generated site should be treated as public before publication.
Deletion from a later commit does not remove data from Git history, forks,
caches, logs, or downloaded artifacts.

## Content that must remain private

Never track or publish:

- credentials, tokens, private keys, cookies, session data, or secret-bearing
  environment files;
- private hostnames, addresses, URLs, repository locations, account identifiers,
  or infrastructure details;
- personal email addresses, machine usernames, home-directory paths, device
  serials, monitor identifiers, or identifying machine configuration;
- real clipboard data, notification bodies, media history, window titles,
  command history, process arguments, or desktop event captures;
- raw logs, crash dumps, screenshots, recordings, profiles, or traces that have
  not been deliberately sanitized;
- local development instructions, internal workflow records, audit findings,
  scratch reports, or workflow metadata; or
- configuration copied from a live desktop when a synthetic example can express
  the same behavior.

Public fixtures use synthetic names, paths, services, metadata, and event
streams. Values should use reserved examples such as `example.com`, neutral
application names, and non-identifying paths.

## Before a local landing

1. Inspect `git status --short` and account for every path.
2. Inspect the complete staged diff, including new files and generated output.
3. Run `bash .github/scripts/public-safety.sh`.
4. Review documentation, fixtures, snapshots, logs, and images manually. Pattern
   checks supplement review; they do not establish safety.
5. Run the required formatting, lint, test, and dependency gates applicable to
   the change.
6. Record exact results and limitations in `DEVLOG.md` when the change advances
   project state.
7. Commit with the configured GitHub no-reply identity and an impersonal project
   voice. Do not add internal workflow or automation attribution.

Before a push, inspect the commit author, committer, complete message, and final
diff again. Publishing requires explicit maintainer authorization.

## Public prose and evidence

Documentation and commit history describe what changed and why. They do not
narrate assignments, internal review roles, automation, or approval mechanics.

Evidence terms remain precise:

- **pass** means the stated check ran and succeeded;
- **fail** means it ran and did not meet its criterion;
- **skip** and **ignore** preserve the test harness's exact distinction;
- **unsupported** describes a declared product boundary;
- **unmeasured** means no qualifying observation exists; and
- **unavailable hardware** means the required apparatus was absent.

Expected output is never regenerated to conceal a regression. Scaffolding,
mocks, and synthetic demonstrations are never promoted into product claims.

## GitHub Actions

Values derived from refs, inputs, tags, or other GitHub expressions route
through workflow `env:` entries and are referenced as quoted shell variables.
The literal GitHub expression opener must never appear inside a `run:` block,
including comments. Workflow permissions start read-only and expand only for a
job with a documented need.

Secrets are never printed, transformed into command tracing, accepted from
untrusted pull-request execution, or passed broadly to jobs that do not require
them.

## If private data is discovered

Stop publication immediately. If a credential may have been exposed, revoke or
rotate it before attempting repository cleanup. Preserve a private incident
record, determine every publication surface, and choose history remediation
based on the exposure. Removing a file in a new commit is not sufficient.
