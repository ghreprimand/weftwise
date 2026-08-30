# Interface Model

Weftwise treats the top edge as an ambient surface whose information density
changes with intent and urgency.

## Presentation levels

### Selvage

The resting surface is a 2-3 pixel line anchored to the top edge with no
exclusive zone. It is always easy to acquire with a pointer but does not reserve
application space.

Its three stable regions are:

- **Navigation:** local workspace marks. Active, occupied, empty, and urgent
  states remain visually distinct without requiring labels.
- **Activity:** progress for the single currently selected media session, timer,
  build, download, render, or other timed activity.
- **Attention:** privacy and exceptional system state such as recording, active
  microphone or camera use, disconnection, mute, critical temperature, or a
  blocked operation.

The Selvage is a cue, not a dense encoding system. It communicates presence,
location, progress, and urgency. Exact names and values appear after reveal.

### Ribbon

Touching the top edge gives immediate visual confirmation. Remaining at the
edge for a short dwell reveals a 26-30 pixel Ribbon. Moving into the Ribbon
keeps it open; leaving dismisses it after a brief grace period.

The Ribbon presents the selected context with a label and direct actions. A
volume change can temporarily select audio, media playback can show title and
transport controls, and an urgent privacy event can preempt ordinary content.
When nothing contextual is active, it shows time, date, current workspace, and
the active application.

Right-click-only and hover-tooltip-only essential actions are avoided. Scroll
actions become active only after reveal so accidental top-edge scrolling cannot
change system state.

### Panel

Clicking the Ribbon or invoking a Hyprland binding opens the Panel on the
focused output. It provides keyboard navigation, search, richer controls, and
history. Escape and outside click dismiss it reliably and restore prior focus.

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
- progress when meaningful;
- available typed actions; and
- output affinity when the context belongs to one monitor.

Priority is only one input. Stickiness prevents flicker, expiration removes
stale events, and explicit preemption allows privacy-critical state to interrupt
ordinary media or clock content.

## Multi-output behavior

Each output owns a Selvage so workspace location remains visible where it
matters. Pointer reveal occurs on the output being touched. Keyboard invocation
opens the Panel on the focused output. Global activity can appear on every
Selvage as a quiet cue, but descriptive content appears on only the active
output unless configuration requests otherwise.

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
