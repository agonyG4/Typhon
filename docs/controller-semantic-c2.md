# Typhon Controller Semantic Foundation C2

C1 observes physical controllers and builds synchronized, bounded
`ControllerFrame` values. C2 adds a small semantic translation layer from each
physical frame to a per-device `ControllerSemanticFrame`. It does not add a
consumer, desktop navigation feature, or public controller API.

## Physical buttons and semantic actions

Standard buttons use location-based names such as `South`, `East`,
`LeftShoulder`, and `DpadUp`. These names do not depend on labels printed on a
particular controller. C2 exposes eight typed actions: `NavigateUp`,
`NavigateDown`, `NavigateLeft`, `NavigateRight`, `Activate`, `Cancel`, `Menu`,
and `System`.

The default mapping is:

| Physical state | Semantic action |
| --- | --- |
| South | Activate |
| East | Cancel |
| Start | Menu |
| Guide | System |
| D-pad or left stick up/down/left/right | matching Navigate action |

The internal mapping value can be constructed with alternate physical buttons,
including swapping Activate and Cancel. C2 does not add settings, persistence,
or runtime mapping mutation. North/West, shoulders, trigger buttons, Select,
stick clicks, the right stick, analog triggers, and other controls remain
physical state and have no default semantic action.

`ControllerActionMask` is a fixed-width value. A semantic frame carries the
full held mask and the press/release differences from the prior per-device
state. Multiple physical sources are aggregated before those differences are
calculated. Linux `BTN_DPAD_*` keys and `ABS_HAT0X/Y` therefore describe one
D-pad direction, and a D-pad direction can overlap a left-stick direction
without duplicate presses or premature releases. Diagonal left-stick input
can hold two directions simultaneously.

Left-stick navigation has its own component-wise hysteresis: a directional
component enters at approximately 0.55 normalized value and remains held until
the component falls to 0.40. This is separate from C1's lower threshold for user-activity
detection. Stick state is normalized using the device's evdev axis ranges.

## State and delivery

Each `ControllerDevice` owns its mapper. On open, the mapper is seeded from a
non-transitioning snapshot of the C1 frame builder, which already contains the
current evdev key and ABS state. A button or direction held before open or
session resume is reported as held without a phantom press. Controllers do not
share semantic state.

The manager accepts a generic, infallible callback for semantic transitions.
It forwards each transition frame with its `ControllerDeviceId` after C1 has
built the logical frame. The native runtime currently supplies a no-op sink;
semantic telemetry is counted independently. This callback is an internal
attachment point for a future consumer, not a queue or a consumer API.

A real consumer must not interrupt exhaustion of an evdev synchronized
iterator. Consumer integration must preserve C1.1's full-batch invariant. The
no-op production sink currently leaves synchronized batch draining
uninterrupted.

Controller removal, session suspension, or runtime-generation replacement
invalidates all semantic held state for that `ControllerDeviceId`. Future
consumers must treat device lifetime as an ownership boundary; a disappearing
device does not require synthesized `Released` events.

C2 has no repeat engine. A stationary held control produces no new evdev
events, so correct repeat behavior would require time-based scheduling and a
real consumer requirement. The held mask preserves state for that future work.
There are no action sets, UI contexts, focus rules, or timers.

## Boundaries and performance

Games continue to read evdev or hidraw directly through SDL, Steam Input, Wine,
Proton, or native APIs. Typhon remains an independent observer. Controller
events do not enter `NativeHardwareInputEvent`, which belongs to the normal
keyboard/pointer seat path. C2 does not emit Astrea shortcuts: that path
publishes and flushes Wayland state and is not an observer-side semantic
consumer. No Shell, Eclipse, QML, Wayland client, or other UI integration is
included.

Semantic mapping runs only while an already-admitted controller frame is
processed. It is fixed-state bitmask and hysteresis work, with no allocation,
lock, string lookup, second evdev pass, polling, timer, or extra wakeup. It
does not touch scene, rendering, cursor, Wayland dispatch, or shell state.

C2 does not implement synthetic keyboard or pointer input, ownership tracking,
controller grouping, leases, uinput, a controller protocol, or hidraw
correlation. Those ownership and grouping directions remain possible C3 work.
A Shell or Eclipse adapter remains separate future work and should wait until
its consumer architecture is stable.
