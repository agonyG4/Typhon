# Typhon Keyboard Layout v2 — Runtime Layout Control

## Goal

Add seat-level runtime layout selection on top of the accepted Keyboard Layout
Core v1. The feature queries compiled layouts, reports the current effective and
locked groups, supports strict index selection and deliberate next/previous
wrapping, publishes changed Wayland modifier/group state, and exposes the
capability through `astrea.control` and `astreactl`.

Settings UI, persistence, per-device policy, new keybindings, and event
subscriptions remain out of scope.

## Design decisions

### One authoritative XKB state

The existing `KeyboardStateHandle` continues to own one
`xkbcommon::xkb::State` in compositor-thread TLS. Runtime selection mutates
that state in place; it never creates another state, changes the raw key
ledger, releases/represses physical keys, rebuilds state, or recompiles the
keymap. The cached Text V1 keymap bytes remain unchanged.

Layout enumeration comes from the compiled `xkb::Keymap` through
`num_layouts()` and `layout_get_name(index)`. The zero-based index is the only
stable identifier. Empty names remain representable and are formatted with a
bounded fallback at the CLI output boundary.

### Effective and locked groups

The runtime selection is a persistent XKB locked-layout update. Snapshots always
report both `STATE_LAYOUT_LOCKED` and `STATE_LAYOUT_EFFECTIVE`; a temporary
depressed or latched group action may make them differ. Next and previous use
the locked index as their base and wrap modulo the compiled layout count. A
single-layout keymap makes set 0, next, and previous successful no-ops. A
zero-layout keymap is unavailable and never enters wrapping arithmetic.

`set_locked_layout(index)` validates `index < keymap.num_layouts()` before
calling the out-of-band API. It requests no latched or locked modifier changes,
does not alter latched layout, and affects only the locked layout:

```text
affect_latched_mods = 0
latched_mods        = 0
affect_latched_layout = false
latched_layout      = 0
affect_locked_mods  = 0
locked_mods         = 0
affect_locked_layout = true
locked_layout       = index
```

The method compares the complete serialized Wayland state before and after the
call and returns the resulting snapshot plus whether it changed. Setting the
already-locked index therefore succeeds without redundant publication, even
when a temporary effective group differs.

## libxkbcommon compatibility bridge

The current `xkbcommon = "0.9.0"` Rust wrapper is retained. A small build-time
`pkg-config` check requires `xkbcommon >= 1.10.0`; an older or unavailable host
library is a build dependency failure, with no legacy fallback.

The keyboard implementation contains the only new unsafe boundary: a private
`extern "C"` declaration matching the installed `xkbcommon/xkbcommon.h`
signature for `xkb_state_update_latched_locked`. The wrapper accepts only the
validated layout index and does not expose raw pointers. Immediately beside
the call, its safety invariant states that the pointer comes from the live
`xkb::State`, remains owned by the compositor thread, is not retained by C,
outlives the call, is not concurrently accessed, and uses the exact header ABI.

The bridge returns the C state-component mask as a change indicator. It is used
only for the out-of-band locked-layout operation; authoritative physical key
events continue using the safe `xkb::State::update_key` path.

## Runtime and server API

`XkbKeyboardState` gains a focused layout snapshot and mutation API. The
`KeyboardStateHandle` forwards these operations through the existing TLS map,
with the same permanent failure and cross-thread failure-closed behavior as
physical key updates. `CompositorState` and `OwnCompositorServer` expose narrow
wrappers that return a complete typed snapshot or a typed layout error.

The server mutation flow is:

```text
control request
    -> NativeRuntime::dispatch_control_command
    -> OwnCompositorServer / CompositorState
    -> KeyboardStateHandle in compositor-thread TLS
    -> xkb_state_update_latched_locked
    -> complete authoritative snapshot
```

If the serialized snapshot changed and a keyboard focus exists, the server
reuses `send_keyboard_modifiers_without_key()` and the existing serial/flush
machinery. The result is exactly one standalone `wl_keyboard.modifiers` event;
no `wl_keyboard.key` or `wl_keyboard.keymap` event is sent. With no keyboard
focus, the state still changes and the next normal keyboard enter reports it.
No-op operations return the current snapshot and publish nothing.

Physical key processing retains v1 ordering: a physical key that changes XKB
state publishes `wl_keyboard.key` before its follow-up modifiers event. XKB
`grp:*` actions and control-driven changes operate on the same state, so a
physical group action after a control switch is reflected by the next query.

## Control protocol

The existing `astrea.control` version 1 protocol gains these commands:

```text
keyboard.layout.get       {}
keyboard.layout.set       {"index": 1}
keyboard.layout.next      {}
keyboard.layout.previous  {}
```

Empty commands use strict `deny_unknown_fields` argument types. `set` requires
an integer, non-negative `index` field and rejects missing, negative, fractional,
string, unknown, or out-of-range values. Every success returns a complete
`KeyboardLayoutSnapshot`:

```json
{
  "effectiveIndex": 1,
  "lockedIndex": 1,
  "layoutCount": 2,
  "layouts": [
    {"index": 0, "name": "Portuguese (Brazil)"},
    {"index": 1, "name": "English (US)"}
  ]
}
```

The typed snapshot is added to `AstreactlResult` and uses the existing bounded
response codec. Invalid indices use `invalid_argument`; unavailable keyboard
state uses the existing internal/runtime failure representation. A failed
`KeyboardStateHandle` is never revived.

## astreactl

The CLI adds:

```text
astreactl keyboard layout
astreactl keyboard next
astreactl keyboard previous
astreactl keyboard set 1
```

The wire commands remain exactly those specified above. All forms support the
existing `--json` output. Human output is deterministic, sanitizes every XKB
layout name with the existing terminal sanitizer, marks the effective layout,
and displays the numeric stable index:

```text
Effective: 1
Locked: 1

  0  Portuguese (Brazil)
* 1  English (US)
```

Unnamed layouts receive an explicit bounded display fallback and are never
used as identifiers.

## Session and lifecycle behavior

Runtime locked layout is part of authoritative XKB locked state. Existing
session/VT reset behavior remains unchanged: transient depressed physical keys
and the raw client ledger are cleared, locked modifiers and locked layout are
preserved, and resume enter publishes the current snapshot. Runtime switching
does not write configuration or persist across compositor restart.

Keyboard initialization fallback/failure, TLS ownership, client isolation,
deferred Alt/Super routing, inhibition, repeat filtering, Caps/Num Lock, and
XWayland’s shared keyboard path remain v1 behavior.

## Testing strategy

Tests are layered around observable behavior:

- XKB unit tests cover enumeration, names, effective/locked distinction,
  valid/invalid set, next/previous wrapping, single-layout no-ops, preservation
  of depressed/latched/locked modifiers and physical pressed keys, unchanged
  keymap bytes, and no recompilation.
- Wayland tests cover standalone modifier publication, no key/keymap events,
  no-op event counts, wrap behavior, no-focus switching followed by enter,
  held Shift/Control/RightAlt, physical `grp:*` interoperability, and locked
  layout persistence through session reset.
- Control tests cover command parsing, strict arguments, typed success and
  error envelopes, invalid indices, unavailable state, and response bounds.
- `astreactl` tests cover every command, `--json`, typed decoding, malformed
  and unknown fields, server errors, and sanitized human output.
- Existing v1 keyboard, native input, session, XWayland, repeat, and authority
  regression suites remain required.

## Non-goals

No Settings UI, layout indicator, persistence, per-window/application/
workspace/device policy, runtime RMLVO replacement, keymap recompilation,
default keybinding, new Wayland or DBus protocol, event subscription service,
IME change, virtual keyboard redesign, remapping, macros, or physical LED
backend change is included.
