# Interface Model

Weftwise uses three UI states at the top edge. Pointer input and current system
state determine which one is visible.

## Presentation levels

### Selvage

The resting surface is a 2-3 pixel line anchored to the top edge. It accepts
pointer input at the screen boundary and must not reserve application space.
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

The line brightens when the pointer reaches the top edge. Holding there for the
configured dwell time reveals a 26-30 pixel Ribbon. Moving into the Ribbon keeps
it open; leaving starts the dismissal timer.

The Ribbon labels the selected context and exposes its actions. The implemented
Hyprland projection shows the active workspace and bounded window title on the
focused output, and uses a boundary-aligned local clock when compositor context
is absent. A volume change
selects audio temporarily. Media playback can show title and transport controls.
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

Widgets do not compete for layout. Producers publish presentation candidates
with enough metadata for deterministic selection:

- stable identity and source;
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
ordinary media or clock content.

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
or privacy-critical state, and every Panel action remains keyboard accessible.

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
