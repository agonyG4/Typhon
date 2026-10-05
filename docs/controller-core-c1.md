# Typhon Controller Core C1

C1 adds an optional observer for physical game controllers. With
`OBLIVION_ONE_CONTROLLER=observe`, Typhon discovers credible Linux evdev game
controllers, reads synchronized evdev events through its existing native
reactor, builds bounded per-device frames, and reports meaningful activity to
the compositor's shared idle authority. The default policy is `off`.

Set the policy to one of these exact values:

```text
OBLIVION_ONE_CONTROLLER=off
OBLIVION_ONE_CONTROLLER=observe
```

An unset variable selects `off`. Any other value is a configuration error.
In `off` mode Typhon creates no controller monitor, opens no controller event
device, registers no controller source, and performs no controller work.

When observation is enabled, Typhon watches `/dev/input` with inotify and
validates candidate event devices by their evdev capabilities, rejecting
ordinary keyboard, mouse, and touchpad devices. Device IDs are process-local
monotonic identities; event-node paths are transport details and are not
logical controller identities. Device reads are non-blocking and use evdev's
synchronized event API. A fixed global budget bounds raw event processing per
reactor cycle. Exhausted work continues through a controller-specific
continuation, without promoting the ordinary input or render domains. An idle
connected controller costs no processing until its FD becomes readable; there
is no controller thread, timer, or polling loop.

## Games and input separation

Games keep opening and reading Linux controller interfaces directly through
SDL, Steam Input, Wine, Proton, or native APIs. Typhon is an independent
observer and does not grab devices, revoke permissions, create uinput devices,
or open controller hidraw nodes.

Raw controller events do not enter `NativeHardwareInputEvent`. That enum is
part of the compositor seat path for keyboard and pointer input. C1 emits
bounded logical controller frames and semantic activity transitions in a
separate work domain. It does not send controller state to Wayland or emulate
keyboard or pointer events.

Button and D-pad transitions count as activity. Stick and trigger activity
uses normalized device ranges with enter/exit hysteresis, so drift does not
repeatedly reset idle time while an axis remains held. Native keyboard and
pointer activity and controller activity both call the compositor's shared
user-activity entry point. Hotplug by itself is not activity and does not
damage the scene or request a frame.

## Session behavior and ownership

The native runtime owns the controller manager and each controller's reactor
token. On session suspend it disables controller processing, clears pending
frames and activity hysteresis, unregisters the monitor and device tokens, and
drops the open device handles. Resume reopens and rescans devices, assigns new
runtime IDs, seeds current device state, and registers new sources. Events from
pre-suspend handles are not replayed.

C1 does not maintain a source-aware aggregate keyboard or pointer ownership
ledger because it injects no synthetic seat state. This avoids release
collisions between physical devices and future controller-generated input.
Controller policy, connection counts, topology counters, drain counters,
activity transitions, and failures are available in the control status
snapshot. The evdev synchronized path repairs `SYN_DROPPED`, but the crate does
not expose a recovery counter, so that telemetry field is reported as
unavailable.

## Future boundaries

C2 may add Astrea Shell semantic controller navigation through the existing
authenticated shortcut/control path, including external-use suppression.
C3 may design source-aware aggregate keyboard/pointer ownership, compound
controller grouping, richer external ownership evidence, optional hidraw
correlation, leases, and a standardized protocol or portal. None of those
behaviors is implemented by C1.
