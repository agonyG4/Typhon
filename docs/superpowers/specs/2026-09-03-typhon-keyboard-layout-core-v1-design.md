# Typhon Keyboard Layout Core v1 (v1.2 closure)

## Goal

Replace Typhon's hand-written XKB keymap and modifier approximation with one
seat-level libxkbcommon state. Preserve physical Linux-keycode compositor
bindings, shortcut inhibition, deferred Alt/Super handling, raw Wayland key
codes, client isolation, and client-side repeat behavior.

## Architecture

Add `src/compositor/keyboard.rs` containing `KeyboardConfig`,
`KeyboardSerializedState`, and `XkbKeyboardState`. The state owns one compiled
`xkbcommon::xkb::Keymap`, two mutable states over that same keymap, the
validated RMLVO config, cached NUL-terminated XKB Text V1 serialization, and
repeat configuration. `physical_state` receives every real hardware
transition and owns global XKB group, latch, lock, and LED semantics.
`client_state` receives only transitions in the logical `wl_keyboard.key`
stream; these are not independent keyboard configurations.
`Default` constructs the baseline `br`/`abnt2` map without panicking; requested
environment configuration is attempted first and falls back to the baseline,
then to `us` if necessary, with diagnostics containing the rejected values.

`CompositorState` owns a sendable `KeyboardStateHandle`, not an XKB object.
The handle lazily creates a unique identity and stores the actual
`XkbKeyboardState` in compositor-thread TLS. A handle used after crossing to a
different thread fails closed, and a failed handle never retries. The native
input path remains responsible for physical binding decisions, but every
physical key transition produces exactly one physical XKB state action. Client
forwarding is a separate action: deferred or consumed Alt/Super transitions
still update `physical_state`, while only the client-visible action updates
`client_state` and publishes `wl_keyboard.key`. Actions are explicitly
`PhysicalOnly`, `PhysicalAndClient`, or `ClientOnly`; replayed modifiers are
client-only and never update the physical state again. The Wayland projection
is `client_state` depressed modifiers plus `physical_state` latched, locked,
and effective group state. A visible transition updates the relevant states,
publishes the raw evdev key, and publishes a complete projected
`wl_keyboard.modifiers` event afterward when that projection changed. A
physical-only transition may publish a standalone projected modifiers event
when global group/lock state changed. This keeps key-before-modifiers ordering
without double-updating either state. Focus enter uses raw client-visible
pressed keys and the same projected serialization; it never rebuilds modifier
state from the physical pressed-key set.

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

`mods_depressed` comes from `client_state` via `xkb_state_serialize_mods`.
`mods_latched`, `mods_locked`, and the effective group come from
`physical_state` via the corresponding libxkbcommon serialization calls. This
preserves dynamic modifier indices, Caps/Num/Scroll lock actions, AltGr,
latching, and multiple layout groups while preventing hidden compositor-owned
modifiers from leaking into the client projection. The published keymap is serialized explicitly with
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
TLS ownership, permanent failure, unique state identities, deferred Alt
projection, consumed Super+Space group publication, client-only replay, and
raw evdev `value=2` repeat suppression including Caps Lock. Session suspend
clears transient physical and client key state while retaining global lock and
layout state. Existing XWayland behavior remains covered by the current
harness; no second keymap or independently configured XKB stack is
introduced.

## Non-goals

No layout-switching UI/API, Eclipse settings integration, per-window or
per-device policy, keysym-based compositor bindings, compositor-side repeat,
physical LED backend ownership changes, IME redesign, or unrelated input,
rendering, window-management, or XWayland refactoring.
