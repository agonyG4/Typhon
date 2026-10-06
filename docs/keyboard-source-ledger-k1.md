# Keyboard Source Ledger K1

## Problem

Keyboard events from libinput and raw evdev previously lost their physical
device identity before reaching `NativeInputState`. The state then tracked one
global pressed-key list and four independent modifier booleans. Two devices
holding the same key could therefore release the logical key too early, and a
left/right modifier pair could clear its family while the other key remained
held.

K1 restores a one-way boundary: physical keyboard sources are reconciled before
the compositor's existing logical seat keyboard path.

## Runtime device identity

`KeyboardDeviceId` is a nonzero, runtime-only identity. The active input backend
owns its `KeyboardDeviceIdAllocator`. IDs increase monotonically, are not
reused after removal or session suspend, and allocation stops at exhaustion
instead of wrapping. Device paths, sysnames, names, and vendor/product values
are not identities.

The libinput backend maps the ref-counted `input::Device` handle to its ID. It
registers keyboard-capable devices during the initial `DeviceAdded` drain and
on later `DeviceAdded` events. A keyboard event whose device is unexpectedly
missing from the map may register that exact device handle once. `DeviceRemoved`
looks up and removes the exact handle, then emits an ordered source-removal
ingress event for its ID. Additions alone do not count as user activity.

The raw evdev fallback assigns an ID lazily when a device first produces a
non-pointer `EV_KEY` event. That ID remains on the `NativeInputDevice` until its
FD is retired or its source incarnation is reset at a session boundary. Raw
terminal conditions retire the device after its readable events are drained;
retirement emits one source-removal event only when the device had an ID. K1
does not discover newly added raw evdev nodes while running.

## Physical ownership and logical seat state

`NativeInputState` owns a `KeyboardSourceLedger`. Each active source has a fixed
bitset over Linux key codes, and the ledger keeps fixed aggregate ownership
counts for the same key space. Out-of-range key codes are ignored safely.

For each code, the ledger emits one logical press only on aggregate `0 -> 1`
and one logical release only on `1 -> 0`. Duplicate presses, duplicate
releases, unknown-source releases, and non-final owner removal do not create a
logical transition. Removing a source drops only that source's keys and returns
the aggregate keys that became unpressed.

Source-removal releases are ordered by key code with ordinary keys first and
modifier keys last. They pass through the same logical router as a final
physical release, preserving compositor shortcut ownership, deferred modifiers,
shortcut inhibition, and balanced client key delivery. Removal is not new user
activity, though it is classified as an event that may require pointer
constraint reconciliation.

Modifier masks are derived from aggregate ownership. Left and right Shift,
Ctrl, Alt, and Super remain active as families until their final logical key
release. The binding manager's modifier-family release hook runs only when the
family changes from active to inactive.

The compositor's existing `XkbKeyboardState` remains the authoritative logical
seat state. It receives only aggregate transitions through
`NativeInputEffect`; K1 does not create per-device XKB states or keymaps.

## Session suspend and resume

The runtime keeps its current session authority and input-discard behavior.
Suspend discards pending backend input and clears the native source ledger and
logical publication ledgers. Backend allocators remain monotonic. Raw device
source IDs are reset while their open FDs remain available, and libinput's
device map is cleared and rebuilt from lifecycle events after resume. A stale
pre-suspend source therefore cannot own a post-resume key.

VT-switch clearing drains aggregate logical keys once each before clearing the
ledger. It does not emit duplicate releases for keys shared across devices.

## Performance

The ledger uses fixed bitsets and fixed aggregate counts, with a small
preallocated source vector for the normal one-to-four-keyboard case. Normal key
events require no path construction, filesystem access, udev lookup, lock,
thread, channel, or asynchronous runtime. The libinput lookup hashes the
existing `input::Device` handle. The reserved source capacity avoids first-key
allocation for the normal one-to-four-device case; additional source-table
growth and source-removal release lists may allocate on their infrequent paths.

## Repeat compatibility status

K2 resolved the temporary raw-repeat compatibility path. Raw evdev `EV_KEY`
value `2` is discarded during normalization, and compositor shortcut repeat is
scheduled from aggregate logical key state. Libinput and raw evdev therefore
share the same compositor repeat authority. Ordinary Wayland clients continue
to use their `wl_keyboard.repeat_info` settings.

## K1 boundary

K1 adds source identity, aggregate key ownership, lifecycle reconciliation, and
regression coverage. K2 separately adds compositor shortcut repeat. The
keyboard milestones do not add media or system actions, a symbolic binding
engine, LED synchronization, virtual
keyboards, text-input or input-method support, EIS/libei, keyboard settings, or
Shell integration.

The intended later boundaries remain:

- **K2:** deterministic compositor shortcut repeat.
- **K3:** compiled physical-key and keysym binding engine.
- **K4:** typed Astrea session-action and media-key foundation.
- **K5:** physical keyboard LED synchronization.
- **K6:** virtual keyboard, text-input, and input-method support.
