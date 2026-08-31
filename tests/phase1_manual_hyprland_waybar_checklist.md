# Phase 1 manual Hyprland and Waybar proof

Run this checklist only on a native Hyprland session with Waybar already
running. Do not attach screenshots, recordings, compositor logs, window titles,
runtime paths, output names, device names, or copied configuration to a public
result.

Record each row as one of `pass`, `fail`, `unavailable hardware`,
`unsupported`, or `unmeasured`. Record the build revision, command, date, and
the selected exclusive-zone value separately from desktop content.

The initial four-output comparison selected `exclusive_zone = -1`: both `0`
and `-1` preserved the existing reserved work area, but only `-1` placed every
surface at the physical top edge. The other rows remain independent checks.

| Check | Expected observation | Evidence boundary |
| --- | --- | --- |
| Startup | One Weftwise surface appears on every currently connected output. | Count only; do not record output names. |
| Zone selection | The selected `0` or `-1` setting places the visible edge at the physical top while no work area is reserved. | Record selected value and pass/fail only. |
| Waybar coexistence | Waybar remains visible and usable with no duplicate reserved work area. | Do not record Waybar configuration. |
| Collapsed geometry | The Selvage is 2-3 pixels high; pointer events below it pass through. | Do not record application names. |
| Dwell reveal | Holding at the top edge reveals the Ribbon after the configured delay. | Record configured delay only if synthetic/default. |
| Keyboard glance | The configured Hyprland binding reveals the Ribbon on the focused output, auto-dismisses, and does not open the Panel. | Record the symbolic binding and pass/fail only. |
| Dismissal | Leaving the Ribbon collapses it after its delay; rapid re-entry cancels dismissal. | Pass/fail only. |
| Panel | Clicking the revealed Ribbon opens the Panel on that output. | Do not record focused client identity. |
| Keyboard | Tab navigation works; Escape dismisses the Panel and focus returns to the prior client. | Pass/fail only. |
| Outside click | An outside click dismisses the Panel without persistent pointer interception. | Pass/fail only. |
| Reduced motion | Desktop preference and explicit override use immediate transitions while preserving reveal and dismissal semantics. | State only enabled/disabled. |
| Scale | Repeat collapsed, reveal, and Panel checks for every available scale factor. | Record scale values, never output names. |
| Stacking | Repeat beside fullscreen applications; the layer surface behavior matches the selected overlay policy. | Do not capture application content. |
| Shutdown | Closing Weftwise removes surfaces and leaves no visible orphan UI. | Process details remain local. |

If a criterion fails, preserve a private incident record with the smallest safe
reproduction. Do not copy raw desktop data into test fixtures, logs, commits,
issues, or this checklist.
