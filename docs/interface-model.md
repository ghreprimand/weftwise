# Interface Model

Weftwise uses three UI states at the top edge. Pointer input and current system
state determine which one is visible.

## Presentation levels

### Selvage

The resting surface is a 2-3 pixel line anchored to the top edge. Its bounded
activation island accepts pointer input at an exposed screen boundary without
reserving application space.
The native proof uses layer-shell zone `-1`. A native comparison against `0`
showed that both values preserved the existing work area, while only `-1`
retained physical-edge placement beside Waybar. Value `0` remains available as
a diagnostic comparison.

Its three stable regions are:

- **Navigation:** local workspace marks. Active, occupied, empty, and urgent
  states remain visually distinct without requiring labels.
- **Activity:** progress for the single currently selected media session, timer,
  build, download, render, or other timed activity.
- **Attention:** privacy and exceptional system state such as recording, active
  microphone or camera use, disconnection, mute, critical temperature, or a
  blocked operation.

The Selvage does not display text or exact values. It uses mark position,
length, color, and pattern. Labels and numbers appear after reveal.

### Ribbon

The pointer trigger is separate from the visual line. By default it is a small,
bounded island near the end of the widest top-edge segment not covered by an
output above it. This avoids requiring pointer precision at an internal edge in
a vertically stacked layout and limits conflicts with application-owned
top-edge reveal gestures. Holding in the island for the configured dwell time
reveals a 26-30 pixel Ribbon. Moving into the Ribbon keeps it open; leaving
starts the dismissal timer. Full-width activation remains an explicit
comparison and rollback mode.

A narrow leg extends down the right edge of the fixed-height surface. This
second way into the same dwell state can be entered horizontally and remains
reachable when another output covers the entire top edge. If both the top edge
and this right edge are internal to the monitor layout, entry reveals
immediately because pointer motion into a neighboring output cannot sustain a
dwell. Physical-edge layouts retain the configured dwell. Adjacency is derived
from GDK logical output rectangles with a small rounding tolerance, not output
names or resolutions. Once the Ribbon is visible, the collapsed workspace and
status marks are hidden so they cannot overlay its three text regions.

The Ribbon has persistent workspace, context, and clock regions. The context
region labels the selected candidate or active client and exposes its actions.
The implemented Hyprland projection shows the active workspace and bounded
window title on the focused output, and uses a boundary-aligned local clock when
compositor context is absent. A volume change selects audio temporarily. The implemented MPRIS
projection shows bounded title and artist text for the selected player, with
Previous, Play/Pause, Next, and 10-second seek controls present only when the
player advertises each capability.
A privacy event can replace ordinary content. The fallback is time, date,
current workspace, and active application.

Right-click-only and hover-tooltip-only essential actions are avoided. Scroll
actions become active only after reveal so accidental top-edge scrolling cannot
change system state.

### Panel

Clicking the Ribbon opens the proof Panel on that output. A focused-output
Hyprland binding is planned with compositor state in Phase 2. The Panel provides
keyboard navigation; Escape and outside click dismiss it. The proof returns
layer-shell keyboard interactivity to `None`, while real restoration to the
prior client remains a native-session acceptance check.

Initial Panel destinations are workspace/window navigation, media, calendar,
audio routing, notifications, clipboard history, system health, and power.
A system tray remains a compatibility feature for a later milestone.

## Context arbitration

Widgets do not compete for layout. The implemented root-owned arbiter accepts
presentation candidates with:

- source-scoped stable identity;
- semantic kind and severity;
- creation and update time;
- expiration or persistent lifetime;
- minimum display duration;
- interruptibility and preemption class;
- an optional progress value;
- available typed actions; and
- output affinity when the context belongs to one monitor.

Priority is only one input. Stickiness prevents flicker, expiration removes
stale events, and explicit preemption allows privacy-critical state to interrupt
ordinary media or clock content. Source-scoped identity deduplicates rapid updates in
place without resetting the active minimum-display interval. The reducer uses
normalized millisecond timestamps and a total tie-break order, so equivalent
candidate sets select the same result independent of insertion order. Minimum
display durations are bounded, expired candidates never remain sticky, stale
producers are removed explicitly, and output-affine candidates are invisible on
other outputs. When no stronger content is available, a fallback candidate wins.

Media selection is deterministic: playing outranks paused, paused outranks
recently stopped playback, playback activity wins within a state, and a stable
MPRIS identity resolves remaining ties. Unknown playback states are not
selected, and stopped state expires after 30 seconds. Duration and position are
clamped before progress is projected. Disappearing players and session-bus loss
remove stale media presentation rather than leaving old metadata selected.

## Multi-output behavior

Each output owns a Selvage and displays its local workspace marks. GDK connector
identity binds each surface to its matching Hyprland output, and only the
globally active workspace receives the active mark. Pointer reveal
occurs on the output being touched. Keyboard invocation opens the Panel on the
focused output. Global activity can place a status mark on every Selvage, but
labels appear only on the active output unless configured otherwise.

Output surfaces are created and removed through a surface manager. Initial work
establishes this ownership model; later work hardens hotplug, output renaming,
scale changes, and compositor restarts.

## Motion and accessibility

The layer surface keeps a fixed transparent height while internal content moves.
This avoids compositor geometry churn during reveal. The collapsed pointer
region covers only the top edge; the expanded region follows the visible
Ribbon.

Animation respects the desktop animation preference and an explicit
reduced-motion setting. Reduced motion uses immediate state changes with the
same focus and dismissal semantics. Color is never the only signal for urgent
or privacy-critical state. Navigation, activity, and attention occupy stable
thirds of the Selvage. Empty workspaces use a point/outline signal, occupied and
selected state use different bar widths and fills, warnings use diamonds plus
striping, and critical or privacy state uses a triangle plus striping. Each mark
carries a complete text label in the GTK accessibility tree, selected content
labels the Ribbon, and every Panel action remains keyboard accessible.

## Initial feature priority

1. Local workspace position and active window context.
2. Clock fallback and calendar.
3. Media state and progress through MPRIS.
4. Volume, mute, microphone, and audio-output feedback.
5. Privacy-critical capture, recording, and idle-inhibitor state.
6. OdysseyOS workflow mode.
7. Timers and supervised process progress.
8. Threshold-based system health without permanent percentage labels.
9. Notification and clipboard summaries.
10. Compatibility tray and broader quick settings.
