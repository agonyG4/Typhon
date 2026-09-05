# Typhon Keyboard Layout Core v1.4 — Single XKB Authority Closure

## Goal

Replace Typhon's hand-written XKB keymap and modifier approximation with one
seat-level libxkbcommon state. Preserve physical Linux-keycode compositor
bindings, shortcut inhibition, deferred Alt/Super handling, raw Wayland key
codes, client isolation, and client-side repeat behavior.

## Architecture

Add `src/compositor/keyboard.rs` containing `KeyboardConfig`,
`KeyboardSerializedState`, and `XkbKeyboardState`. The state owns one compiled
`xkbcommon::xkb::Keymap`, one mutable physical state over that keymap, the
validated RMLVO config, cached NUL-terminated XKB Text V1 serialization, and
repeat configuration. `physical_state` receives every real hardware
transition and is the sole authority for global XKB group, latch, lock, and
LED semantics. The compositor keeps a separate raw forwarded-key ledger for
Wayland enter state, deferred replay, release reconciliation, and session
reset; that ledger is not another XKB state.
`Default` constructs the baseline `br`/`abnt2` map without panicking; requested
environment configuration is attempted first and falls back to the baseline,
then to `us` if necessary, with diagnostics containing the rejected values.

`CompositorState` owns a sendable `KeyboardStateHandle`, not an XKB object.
The handle lazily creates a unique identity and stores the actual
`XkbKeyboardState` in compositor-thread TLS. A handle used after crossing to a
different thread fails closed, and a failed handle never retries. The native
input path remains responsible for physical binding decisions, but each real
physical value 1/0 produces exactly one physical XKB state update; repeat value
2 never updates XKB. Client forwarding is a separate action: deferred or
consumed Alt/Super transitions still update `physical_state`, while only the
client-visible action publishes `wl_keyboard.key`. Actions are explicitly
`PhysicalOnly`, `PhysicalAndClient`, or `ClientOnly`; the latter never updates
XKB and is used for deferred raw replay. The Wayland projection serializes all
modifier and group fields directly from `physical_state`. A visible transition
publishes the raw evdev key and then a complete projected
`wl_keyboard.modifiers` event when the physical snapshot changed. A
physical-only transition may publish a standalone projected modifiers event
when global group/lock state changed. This keeps key-before-modifiers ordering
without double-updating XKB. Focus enter uses the raw forwarded-key ledger and
the same direct physical serialization; it never rebuilds modifier state from a
pressed-key replay.

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

`mods_depressed`, `mods_latched`, `mods_locked`, and the effective group all
come directly from `physical_state` via the corresponding libxkbcommon
serialization calls. This preserves dynamic modifier indices, Caps/Num/Scroll
lock actions, AltGr, latching, and multiple layout groups. A physical modifier
is therefore visible even if the corresponding raw key is suppressed by a
compositor binding; raw key forwarding and XKB authority are separate
decisions. The published keymap is serialized explicitly with
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
TLS ownership, permanent failure, unique state identities, deferred Alt/Super
projection including RightAlt, consumed Super+Space group publication,
client-only replay, and raw evdev `value=2` repeat suppression including Caps
Lock. Independent reference XKB states verify held RightAlt across `us,br`
with `br(abnt2)` and `grp:alt_shift_toggle`, plus Ctrl+Shift group switching
with `grp:ctrl_shift_toggle`; those oracles do not use Typhon's projection.
Session suspend clears transient physical key state and the raw ledger while
retaining global lock and layout state. Existing XWayland behavior remains
covered by the current harness; no second keymap or independently configured
XKB stack is introduced.

## v1.4 authority correction

There is exactly one authoritative libxkbcommon state per seat. Every real
physical press/release updates it once, and repeat value 2 is never applied to
it. The compositor's raw forwarded-key ledger remains separate and is used only
for `wl_keyboard.enter.keys`, deferred raw replay, release reconciliation, and
session reset. Deferred replay is a raw client-only action; it does not replay
keys into a second XKB state and does not fabricate modifier snapshots.

All four Wayland modifier/group fields are serialized directly from the
authoritative physical state, with no fixed masks or filtering. This allows
physical modifiers to remain observable when raw key forwarding is suppressed
and keeps group-sensitive keys such as RightAlt coherent across layout changes.
PhysicalOnly, PhysicalAndClient, and ClientOnly retain their routing contract;
the promoted release after deferred replay is PhysicalAndClient, so it closes
the raw ledger while applying the one real physical release.

Session suspension is a protocol boundary. Before transient state is cleared,
the compositor sends `wl_keyboard.leave` for the current keyboard focus and
remembers that surface. It clears the physical pressed-key set and raw ledger
without resetting physical locked modifiers or layout. After successful input
recovery, the remembered surface is restored only if it is still the focused
surface; `wl_keyboard.enter` carries an empty pressed-key list and the current
direct modifier/group snapshot. A changed focus or a destroyed surface
discards the remembered target and follows normal focus reconciliation.

The regression suite parses the published Text V1 keymap and verifies
group-sensitive RightAlt behavior for `us,br` with `br(abnt2)` and
`grp:alt_shift_toggle` in both group directions. It also exercises a real
Wayland client across forwarded Ctrl plus VT/session reset, an ordinary held Z,
and Caps Lock; transient keys must disappear from leave/enter state while the
Caps Lock locked mask remains set. Existing deferred Alt/Super, consumed group
switch, repeat, inhibition including RightAlt, focus, fallback, raw-keycode,
and event-ordering coverage remains required.

## Non-goals

No layout-switching UI/API, Eclipse settings integration, per-window or
per-device policy, keysym-based compositor bindings, compositor-side repeat,
physical LED backend ownership changes, IME redesign, or unrelated input,
rendering, window-management, or XWayland refactoring.
