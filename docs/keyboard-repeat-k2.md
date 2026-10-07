# Typhon Keyboard K2 — Deterministic Compositor Shortcut Repeat

K2 gives compositor bindings one repeat authority across libinput and raw
evdev. Raw kernel `EV_KEY` value `2` is a transport repeat notification, so the
raw backend now drops it before it becomes a native keyboard event. Physical
input entering Typhon contains only logical key press and release transitions.

## Repeat target and source ownership

`NativeInputState` owns a fixed-size `KeyboardRepeatState` with at most one
active target. A target records the `BindingId` selected by its initial press,
the logical key code, the exact aggregate modifier mask, its inhibition policy,
its initial or repeating phase, and one absolute deadline. It stores no
`KeyboardDeviceId`.

K1's `KeyboardSourceLedger` remains the source of truth. A second keyboard
pressing a key already held does not create a new logical transition or change
the repeat target. Releasing one owner also leaves the target active while
another source still holds the key. The final logical release cancels it.

Only a consumed `BindingTrigger::Press` binding with
`RepeatPolicy::Enabled` can arm repeat. A new logical non-modifier press takes
repeat ownership: it replaces the previous target when its binding is
repeat-enabled, or cancels it otherwise. An old target does not resume when the
new key is released.

## Configuration and deadlines

The compositor's selected `KeyboardConfig` is the repeat configuration
authority. Startup initializes compositor repeat from the selected rate and
delay. A committed runtime keyboard configuration mutation updates compositor
repeat from the same snapshot used to publish `wl_keyboard.repeat_info`.

A zero rate disables compositor binding repeat. A zero delay sets the first
deadline to the press processing time; service happens on a later native turn.
The steady interval is one second divided by the configured rate, with a
minimum interval of one nanosecond. Configuration changes rebase the deadline
from the current monotonic time. Rate zero cancels the active target.

`KeyboardRepeatState` computes the next absolute deadline. The existing
`NativeWakePlan` arbitrates it with presentation, control, explicit-sync,
cursor, and the other native deadlines. `NativeEventLoop` continues to arm the
single shared timerfd. K2 adds no event-loop timer or timer source.

A repeat deadline is input-domain work. A repeat-only turn enters the existing
native input batch for effect application and Wayland flushing, but does not
dispatch libinput, drain raw evdev, or probe hardware readiness. The deadline
alone does not request rendering, scene, cursor, or Wayland read-side work. A
binding action may request visual work after it runs.

## Ordering and validity

When hardware input and repeat are due together, Typhon materializes and
processes the hardware batch first. Final key releases and aggregate modifier
changes therefore invalidate stale repeat targets before service. If that
batch leaves input backlog, repeat service waits until the backlog clears.

Each native cycle emits at most one repeat action. A late service does not
catch up missed intervals; the next deadline is scheduled from the actual
service time. Before each action Typhon validates the exact compiled `BindingId`
selected by the initial press, that its aggregate trigger key is still held,
its modifier mask and repeat policy still match, and shortcut inhibition still
permits it. Repeat does not rematch the binding table or switch to another
candidate.
Respect-policy targets cancel when shortcut inhibition becomes effective;
Bypass-policy targets may continue.

For a K3B `KeySym` target, the retained modifier snapshot remains the aggregate
physical `ModifierMask`; it is not replaced by XKB's consumed symbolic
modifiers. When a symbolic repeat is due, the runtime obtains a fresh read-only
translation from the authoritative compositor XKB state and validates the exact
stored `BindingId` against its raw or translated identity. A layout or lock-state
change that makes that binding stop matching cancels the repeat. Physical
repeat targets do not request an XKB translation.

Repeat only reapplies the consumed compositor binding action with
`AstreaShortcutPhase::Repeated`. It is not physical user activity and does not
change keyboard source ownership, compositor or client XKB state, or client key
forwarding. Ordinary Wayland clients continue to receive their existing
`wl_keyboard.repeat_info`; K2 does not synthesize `wl_keyboard.key` events.

Session suspend, VT keyboard clearing, and shutdown discard the active target
and deadline. A fresh physical press is required after resume.

## Performance and scope

Repeat tracking uses a fixed-size target and one deadline. The normal repeat
path adds no scheduler heap allocation, lock, thread, channel, async task, or
filesystem work. The compiled action catalog resolves the repeat's stored
binding ID without re-matching or cloning its payload.

K2 adds deterministic compositor shortcut repeat only. It does not add media
or system actions, a symbolic or compiled binding engine, LED synchronization,
keyboard grouping, virtual keyboards, text-input, input-method support, EIS,
or Shell integration.
