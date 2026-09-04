# Typhon Keyboard Layout Core v1

## Goal

Replace Typhon's hand-written XKB keymap and modifier approximation with one
seat-level libxkbcommon state. Preserve physical Linux-keycode compositor
bindings, shortcut inhibition, deferred Alt/Super handling, raw Wayland key
codes, client isolation, and client-side repeat behavior.

## Architecture

Add `src/compositor/keyboard.rs` containing `KeyboardConfig`,
`KeyboardSerializedState`, and `XkbKeyboardState`. The state owns the compiled
`xkbcommon::xkb::Keymap`, its mutable `xkb::State`, the validated RMLVO config,
cached NUL-terminated XKB Text V1 serialization, and repeat configuration.
`Default` constructs the baseline `br`/`abnt2` map without panicking; requested
environment configuration is attempted first and falls back to the baseline,
then to `us` if necessary, with diagnostics containing the rejected values.

`CompositorState` owns a sendable `KeyboardStateHandle`, not an XKB object.
The handle lazily creates a unique identity and stores the actual
`XkbKeyboardState` in compositor-thread TLS. A handle used after crossing to a
different thread fails closed, and a failed handle never retries. The native
input path remains responsible for physical binding decisions, but every
physical key transition produces exactly one XKB state action. Client
forwarding is a separate action: deferred or consumed Alt/Super transitions
still update XKB, while only the client-visible action publishes
`wl_keyboard.key`. For a visible transition, the compositor updates XKB first,
publishes the raw evdev key, and publishes a complete
`wl_keyboard.modifiers` event afterward when serialized state changed. This
keeps key-before-modifiers ordering without double-updating XKB. Focus enter
uses raw client-visible pressed keys and the current XKB serialization; it
never rebuilds modifier state from the pressed-key set.

`wl_seat.get_keyboard` sends the shared cached Text V1 keymap and configured
repeat info (only for protocol versions that support it), then registers the
resource. Seat binding first ensures keyboard initialization: a ready handle
advertises Pointer and Keyboard, while a failed handle advertises Pointer
only. An unexpected `get_keyboard` after failure is rejected with
`MissingCapability` and is not registered. Each resource receives a fresh
anonymous file descriptor backed by the cached bytes. No resource request
recompiles the keymap.

## Configuration and fallback

The configuration contains optional rules, model, variant, and options plus a
required layout and repeat rate/delay. Defaults remain `layout=br`,
`variant=abnt2`, no options, rate 25, delay 600 ms. Existing environment
variables remain supported; rules, model, repeat rate, and repeat delay gain
the corresponding `OBLIVION_ONE_XKB_*` variables. Empty optional values are
treated as absent. Negative repeat values fall back to the defaults. RMLVO
strings are passed to libxkbcommon unchanged; no sanitizing or hand-built XKB
include syntax remains.

## Protocol semantics

Modifier masks come only from `xkb_state_serialize_mods` for depressed,
latched, and locked components, and the group comes from
`xkb_state_serialize_layout(STATE_LAYOUT_EFFECTIVE)`. This preserves dynamic
modifier indices, Caps/Num/Scroll lock actions, AltGr, latching, and multiple
layout groups. The published keymap is serialized explicitly with
`KEYMAP_FORMAT_TEXT_V1`, remains NUL-terminated, and is sent as
`wl_keyboard::KeymapFormat::XkbV1`.

The default layout is `br` with variant `abnt2`. Explicit environment values
are preserved, including an explicitly empty variant. A configuration such as
`layout=br,us`, `variant=abnt2,`, and
`options=grp:alt_shift_toggle` is passed to libxkbcommon as native RMLVO; the
group switch emerges from the compiled option and is not special-cased in
Typhon.

## Testing

Unit tests cover deterministic configuration parsing, fallback, keycode
conversion, XKB modifier/group/LED serialization, multi-layout compilation,
and repeat validation. Wayland integration tests extend the registry state to
capture all modifier fields, group, keymap bytes/size, repeat values, and
event ordering. They verify Text V1 parsing, raw evdev key delivery, real
Shift/Control/Caps/AltGr behavior, multi-layout group changes through the
native input pipeline, focus-enter state, v1 repeat gating, repeat
suppression, client isolation, physical bindings, and shortcut inhibition.
They also cover seat capability failure, repeat preservation through fallback,
TLS ownership, permanent failure, and unique state identities. Existing
XWayland behavior remains covered by the current harness; no second XKB stack
is introduced.

## Non-goals

No layout-switching UI/API, Eclipse settings integration, per-window or
per-device policy, keysym-based compositor bindings, compositor-side repeat,
physical LED backend ownership changes, IME redesign, or unrelated input,
rendering, window-management, or XWayland refactoring.
