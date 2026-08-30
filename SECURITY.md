# Security Policy

Weftwise consumes compositor events, D-Bus metadata, media metadata, process
output, configuration, window titles, notification content, and other data that
may be malformed, hostile, or private. It can also dispatch desktop actions.
These boundaries are security-sensitive even while the project is pre-alpha.

## Supported versions

No release is currently supported. During pre-alpha development, security fixes
apply to the latest revision of the default branch only. This section will be
revised before the first release.

## Reporting a vulnerability

Do not open a public issue, pull request, or discussion for a vulnerability.

Use [GitHub's private vulnerability reporting](https://github.com/ghreprimand/weftwise/security/advisories/new)
from the repository's **Security** tab. Include the affected revision, the
smallest safe reproduction, the expected behavior, and the observed impact. Do
not include unrelated personal data or secrets in the report.

## Security boundaries

Reports are in scope when they involve:

- command or argument injection through window titles, media metadata,
  notifications, configuration, or process tracking;
- unsafe Hyprland socket, D-Bus, PipeWire, or WirePlumber message handling;
- unintended keyboard capture, focus theft, click interception, or action
  dispatch from a layer surface;
- configuration or theme paths escaping their intended locations;
- logs or diagnostics exposing private desktop content;
- unbounded input, allocation, retries, subprocess creation, or task growth;
- privilege-boundary assumptions or unsafe filesystem permissions; or
- privacy indicators that report a protected resource as inactive while known
  active evidence is available.

Visual defects without a security or privacy impact belong in the normal issue
tracker after it becomes available.
