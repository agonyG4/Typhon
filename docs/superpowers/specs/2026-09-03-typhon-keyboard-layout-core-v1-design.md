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

`CompositorState` owns one `XkbKeyboardState` on the compositor thread. The
native input path remains responsible for physical binding decisions. Only
`CompositorState::send_keyboard_key` updates XKB, and only after the event has
reached the Wayland forwarding path. That method updates its raw pressed-key
bookkeeping, updates XKB with `evdev + 8`, ensures focus, emits
`wl_keyboard.key` with the unchanged evdev code, and emits a complete
`wl_keyboard.modifiers` event afterward when XKB state changed. Focus enter
uses the raw pressed keys and the current XKB serialization; it never rebuilds
modifier state from the pressed-key set.

`wl_seat.get_keyboard` sends the shared cached Text V1 keymap and configured
repeat info (only for protocol versions that support it), then registers the
resource. Each resource receives a fresh anonymous file descriptor backed by
the cached bytes. No resource request recompiles the keymap.

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

## Testing

Unit tests cover deterministic configuration parsing, fallback, keycode
conversion, XKB modifier/group/LED serialization, multi-layout compilation,
and repeat validation. Wayland integration tests extend the registry state to
capture all modifier fields, group, keymap bytes/size, repeat values, and
event ordering. They verify Text V1 parsing, raw evdev key delivery, real
Shift/Control/Caps/AltGr behavior, multi-layout group changes, focus-enter
state, v1 repeat gating, repeat suppression, client isolation, physical
bindings, and shortcut inhibition. Existing XWayland behavior remains covered
by the current harness; no second XKB stack is introduced.

## Non-goals

No layout-switching UI/API, Eclipse settings integration, per-window or
per-device policy, keysym-based compositor bindings, compositor-side repeat,
physical LED backend ownership changes, IME redesign, or unrelated input,
rendering, window-management, or XWayland refactoring.
