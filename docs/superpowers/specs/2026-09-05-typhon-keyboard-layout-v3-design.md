# Typhon Keyboard Layout v3 — Transactional Runtime RMLVO Reconfiguration and Persistence

## Goal

Add complete runtime keyboard configuration to the accepted Keyboard Layout
Core v1/v2 implementation. Typhon can validate a new RMLVO configuration,
compile it with libxkbcommon, persist it safely, wait for all authoritative
physical and client-tracked keys to be released, and then atomically replace
the active XKB keymap and state. The change is exposed through `astrea.control`
and the typed `astreactl` client and is restored on the next compositor start.

This is a compositor/runtime capability, not a Settings UI or a new input
policy system.

## Preserved authority and invariants

The existing v1/v2 input path remains authoritative:

```text
raw evdev transition -> one compositor-thread XKB state -> wl_keyboard
```

`KeyboardStateHandle` continues to hold only an identity in `CompositorState`;
the `xkb::Keymap` and `xkb::State` stay in compositor-thread TLS. No XKB object
is sent to the persistence worker and no unsafe `Send`/`Sync` implementation is
introduced. Physical transitions continue to use `xkb_state_update_key()` and
the v2 locked-layout operation continues to use the private
`xkb_state_update_latched_locked()` bridge. The client key ledger, raw evdev
keycodes, session reset behavior, inhibition behavior, cached Text V1 bytes,
and XWayland shared keyboard path remain unchanged.

Runtime configuration never uses `xkb_state_update_mask()`, synthetic key
events, held-key replay, a second XKB state, a client-side XKB reconstruction,
or manually generated keymap text.

## Configuration model and startup

The compositor-owned value is:

```rust
pub struct KeyboardConfig {
    pub rules: Option<String>,
    pub model: Option<String>,
    pub layout: String,
    pub variant: Option<String>,
    pub options: Option<String>,
    pub repeat_rate: i32,
    pub repeat_delay: i32,
    pub default_layout_index: u32,
}
```

`Some("")` remains distinct from `None` for nullable RMLVO fields. Values are
not normalized or silently rewritten; embedded NUL bytes and bounded structural
violations are rejected before libxkbcommon is called. Defaults remain `br`
with `abnt2`, repeat rate 25, repeat delay 600, and default layout index 0.

Startup resolves values in this order:

```text
defaults -> valid keyboard.json -> OBLIVION_ONE_XKB_* overrides -> XKB compile
```

The existing `OBLIVION_ONE_XKB_RULES`, `MODEL`, `LAYOUT`, `VARIANT`, `OPTIONS`,
`REPEAT_RATE`, and `REPEAT_DELAY` semantics are preserved. A layout override
without a variant override deliberately yields `None`; an explicit empty
variant yields `Some("")`. A default-layout environment override is honored
only when valid for the resulting compiled layout list. Persisted invalid or
insecure data is not deleted or treated as an implicit runtime mutation;
startup logs the failure and falls back to the normal startup candidates.

## Persistence

The dedicated store uses:

```text
$XDG_CONFIG_HOME/AstreaOS/input/keyboard.json
$HOME/.config/AstreaOS/input/keyboard.json  (when XDG_CONFIG_HOME is absent)
```

The document is bounded, version 1, camelCase on the wire, and rejects unknown
fields. It contains only the desired configuration:

```json
{
  "version": 1,
  "rules": null,
  "model": null,
  "layout": "br,us",
  "variant": "abnt2,",
  "options": "grp:alt_shift_toggle",
  "repeatRate": 25,
  "repeatDelay": 600,
  "defaultLayoutIndex": 0
}
```

The store follows the existing secure cursor-store boundary: bounded reads,
owner and mode checks, 0700 parent directories, a 0600 file, no symlink
traversal, descriptor-relative validation, temporary-file publication, file
and directory synchronization, and stale temporary cleanup. A write failure
leaves the previous JSON document intact. Durability is performed only by the
dedicated worker, never in the compositor event/render loop.

The worker receives `KeyboardConfig` and returns a bounded completion. It does
not receive `xkb::Context`, `xkb::Keymap`, `xkb::State`, Wayland resources, or
`CompositorState`.

## Transaction state machine

There is at most one pending mutation:

```text
Idle
  -> PrepareCandidate
  -> Persisting
  -> WaitingForQuiescence
  -> Commit
  -> Idle
```

Preparation runs on the compositor thread and validates the complete candidate
by compiling a real keymap, producing bounded NUL-terminated Text V1 bytes,
checking that at least one layout exists, and checking the requested default
index. A runtime compile error is returned as `invalid_argument`; it never
falls back to another layout and never mutates the active state.

The candidate is retained in compositor TLS as a
`PreparedKeyboardConfiguration { config, keymap, serialized_keymap_v1 }`.
Persistence must succeed before the candidate can be committed. While the
worker is writing, input continues through the old active state. After a
successful completion, commit waits until both the XKB physical pressed set
and `CompositorState::pressed_keys` are empty. Release input can therefore
finish normally and the next compositor cycle performs the commit. A second
mutation returns a bounded busy error. A disconnected control client does not
cancel a persisted candidate or prevent its commit; its response is simply
dropped.

Commit inspects the old authoritative state, enumerates all locked modifier
names, and migrates only same-named locked modifiers into a newly constructed
candidate state. Absent modifiers are dropped. Depressed and latched state is
cleared, and the requested default locked layout is applied through the v2
`xkb_state_update_latched_locked()` path. The keymap, state, config, and cached
bytes are then swapped together in one compositor-thread operation. Generation
increments only for an actual active configuration change.

If preparation or persistence fails, the candidate is discarded and both the
active runtime and previous JSON remain unchanged. If persistence succeeds but
shutdown occurs before quiescence, the candidate is discarded from memory and
the durable JSON is restored on the next startup.

Identical active/persisted configurations are successful no-ops: no compilation,
write, client event, or generation increment occurs. Runtime v2 layout commands
remain ephemeral and do not write this file or increment the configuration
generation.

## Publication semantics

Keyboard initialization is factored into keymap and repeat publication helpers.
On a full RMLVO replacement, every live `wl_keyboard` resource receives the
new Text V1 keymap. Repeat information is sent only to protocol version 4+
resources and modifiers are sent only to the focused client. No key, enter,
leave, or synthetic event is generated.

A repeat-only change persists and updates active repeat values, then publishes
only version-gated repeat information. A default-layout-only change persists,
updates the authoritative locked layout without recompiling, and publishes
modifiers only when the Wayland-visible group changes. The focused client
receives the new state through the existing serial/flush path; a no-focus
change is reflected by the next normal enter.

The configuration snapshot reports generation, source (`default`, `persisted`,
or `environment`), persistence status (`missing`, `persisted`, `invalid`,
`insecure`, or `unavailable`), whether an environment override is active, an
optional bounded pending marker, the complete typed configuration, and the
existing `KeyboardLayoutSnapshot`.

## Control protocol and CLI

`astrea.control` version 1 adds:

```text
keyboard.config.get  {}
keyboard.config.set  { full typed desired configuration }
```

The set command is replacement-shaped rather than patch-shaped. Arguments and
responses use strict typed serde structures with camelCase fields and no
unknown fields. Validation failures use `invalid_argument`; persistence,
worker, cross-thread, and runtime failures use the existing bounded internal
error envelope. `keyboard.config.get` returns the active snapshot and may
include `pending: true` while a candidate is waiting.

`astreactl keyboard config` queries the snapshot. `astreactl keyboard configure`
accepts typed flags for RMLVO, repeat values, and default layout index, plus
nullable-field clear flags. The merge form fetches the current snapshot before
sending a full replacement; omitted flags preserve existing values and never
silently clear them. Existing v2 layout commands and every existing CLI option
remain compatible. JSON output is typed and human output is bounded and
sanitized with no colors.

## Non-goals

No Settings/UI work, per-window or per-device policy, virtual keyboard, IME,
LED backend, new keybindings, XWayland-specific keyboard configuration, DBus
API, event subscription service, manual keymap text generation, or rewrite of
the accepted v1/v2 history is included.

## Verification strategy

Focused tests cover candidate compilation, NUL/Text V1 bounds, default-index
validation, rollback, locked-modifier migration and clearing, quiescence,
publication routing, no-op behavior, persistence security/atomicity, startup
precedence/recovery, strict control envelopes, worker ordering/busy behavior,
and typed CLI output. Existing v1/v2, session, native input, client, and
XWayland suites remain required. Final source guards explicitly check that no
secondary XKB authority, runtime fallback, unsafe cross-thread XKB ownership,
mask-copy migration, hot-path fsync, keymap-only-focused publication,
repeat/default recompilation, runtime-layout JSON write, auto-persisted
environment override, or unbounded configuration path was introduced.
