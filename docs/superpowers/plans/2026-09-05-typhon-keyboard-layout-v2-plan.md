# Typhon Keyboard Layout v2 — Runtime Layout Control Implementation Plan

> **For agentic workers:** Execute this plan task-by-task in the current checkout. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add ephemeral seat-level runtime layout selection through the existing authoritative libxkbcommon state, `astrea.control`, and `astreactl`.

**Architecture:** Keep the compiled Text V1 keymap and the compositor-thread TLS-owned `xkb::State` from Keyboard Layout Core v1. Add a private compatibility bridge for `xkb_state_update_latched_locked()` and expose layout enumeration plus locked-layout mutation through narrow compositor/server wrappers. Control requests execute on the compositor thread, return typed snapshots, and publish only standalone `wl_keyboard.modifiers` when the authoritative serialized state changes.

**Tech Stack:** Rust 2024, existing `xkbcommon = "0.9.0"` safe bindings, libxkbcommon >= 1.10.0, one `pkg-config` build dependency, serde/serde_json, existing Wayland test harness, `rtk` command output filtering.

## Global Constraints

- Preserve exactly one authoritative XKB server state per seat and the existing `KeyboardStateHandle` compositor-thread TLS ownership.
- Require `libxkbcommon >= 1.10.0`; do not add a legacy fallback, synthetic group key, `xkb_state_update_mask()`, keymap recompilation, or a second XKB state.
- Keep the narrow unsafe FFI bridge private to `src/compositor/keyboard.rs`; raw pointers must not escape that module and no unsafe `Send`/`Sync` implementation may be added.
- Use `xkb_state_update_latched_locked()` only to affect locked layout: zero modifier masks, no latched layout, `affect_locked_layout = true`, validated requested index.
- Enumerate layouts from `Keymap::num_layouts()` and `Keymap::layout_get_name(index)`; indices are zero-based stable identifiers and names are display text only.
- Report both `STATE_LAYOUT_EFFECTIVE` and `STATE_LAYOUT_LOCKED`; next/previous wrap from locked state, and zero-layout state is unavailable.
- Preserve depressed/latched/locked modifiers, physically pressed keys, raw client-key ledger, cached keymap bytes, session reset, and existing physical `grp:*` behavior.
- Runtime selection is ephemeral: do not persist configuration, add UI, add a keybinding, add per-device policy, or add an event/subscription protocol.
- Control wire commands remain under `protocol = "astrea.control"`, `version = 1`; use strict `deny_unknown_fields` argument and snapshot types.
- Invalid explicit indices use `invalid_argument`; unavailable keyboard state uses the existing internal/runtime failure model and never revives a failed handle.
- A changed focused state sends one standalone `wl_keyboard.modifiers` event through existing serial/flush machinery; no `wl_keyboard.key` or `wl_keyboard.keymap` is sent.
- Compile, test, and build in `/home/agony/GitHub/Typhon` so Cargo uses the checkout-local `target` directory. Use `rtk` for shell commands and never dispatch sub-agents.
- Do not stage or commit unrelated pre-existing worktree edits.

---

### Task 1: Add the modern XKB compatibility bridge and layout state API

**Files:**
- Create: `build.rs`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `src/compositor/keyboard.rs`
- Test: `src/compositor/keyboard.rs`

**Interfaces:**
- `KeyboardLayoutEntry { index: u32, name: String }` remains an internal display snapshot entry.
- `KeyboardLayoutState { effective_index: u32, locked_index: u32, layouts: Vec<KeyboardLayoutEntry> }` is derived from the compiled keymap and authoritative state.
- `XkbKeyboardState::layout_snapshot(&self) -> Result<KeyboardLayoutState, KeyboardLayoutError>` enumerates the compiled keymap and serializes effective/locked layouts.
- `XkbKeyboardState::set_locked_layout(&mut self, index: u32) -> Result<KeyboardLayoutChange, KeyboardLayoutError>` validates and updates only the locked layout.
- `XkbKeyboardState::next_layout(&mut self) -> Result<KeyboardLayoutChange, KeyboardLayoutError>` and `previous_layout` derive wrapping from `STATE_LAYOUT_LOCKED`.
- `KeyboardLayoutChange` contains the resulting `KeyboardLayoutState` and `changed: bool`; no raw XKB pointer is exposed.

- [ ] **Step 1: Write failing unit tests for layout enumeration and out-of-band semantics.** Add tests in the existing `#[cfg(test)]` module using a deterministic `br,us` configuration and the existing `xkb::State` reference style:

```rust
#[test]
fn runtime_layout_snapshot_comes_from_compiled_keymap() {
    let config = KeyboardConfig {
        layout: "br,us".into(),
        variant: Some("abnt2,".into()),
        ..KeyboardConfig::default()
    };
    let state = XkbKeyboardState::from_config(&config).unwrap();
    let snapshot = state.layout_snapshot().unwrap();
    assert_eq!(snapshot.effective_index, 0);
    assert_eq!(snapshot.locked_index, 0);
    assert_eq!(snapshot.layouts.len(), 2);
    assert_eq!(snapshot.layouts[0].index, 0);
    assert_eq!(snapshot.layouts[1].index, 1);
}

#[test]
fn runtime_layout_set_preserves_physical_state_and_keymap_bytes() {
    let config = KeyboardConfig {
        layout: "br,us".into(),
        variant: Some("abnt2,".into()),
        ..KeyboardConfig::default()
    };
    let mut state = XkbKeyboardState::from_config(&config).unwrap();
    state.update_physical_key(42, true);
    let keymap_before = state.keymap_text_v1().to_string();
    let before = state.wayland_serialized_state();

    let change = state.set_locked_layout(1).unwrap();

    assert!(change.changed);
    assert_eq!(change.snapshot.locked_index, 1);
    assert_eq!(change.snapshot.effective_index, 1);
    assert_eq!(state.wayland_serialized_state().depressed, before.depressed);
    assert_eq!(state.keymap_text_v1(), keymap_before);
    assert!(state.physical_pressed_keys.contains(&42));
}

#[test]
fn runtime_layout_navigation_wraps_from_locked_layout() {
    let config = KeyboardConfig {
        layout: "br,us".into(),
        variant: Some("abnt2,".into()),
        ..KeyboardConfig::default()
    };
    let mut state = XkbKeyboardState::from_config(&config).unwrap();
    assert_eq!(state.next_layout().unwrap().snapshot.locked_index, 1);
    assert_eq!(state.next_layout().unwrap().snapshot.locked_index, 0);
    assert_eq!(state.previous_layout().unwrap().snapshot.locked_index, 1);
}

#[test]
fn runtime_layout_rejects_out_of_range_index_without_mutation() {
    let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
    let before = state.layout_snapshot().unwrap();
    assert!(matches!(
        state.set_locked_layout(1),
        Err(KeyboardLayoutError::InvalidIndex { index: 1, count: 1 })
    ));
    assert_eq!(state.layout_snapshot().unwrap(), before);
}
```

- [ ] **Step 2: Run the new unit tests and verify the expected RED failure.**

  Run:

```bash
rtk cargo test --lib compositor::keyboard::tests::runtime_layout -- --nocapture
```

  Expected: compile failure because the layout types and methods do not exist. Fix only naming/import errors if the test harness reports one before the intended missing-API failure.

- [ ] **Step 3: Add the build-time libxkbcommon floor.** Add `pkg-config = "0.3"` under `[build-dependencies]` in `Cargo.toml` and create `build.rs` with the smallest possible qualification:

```rust
fn main() {
    pkg_config::Config::new()
        .atleast_version("1.10.0")
        .probe("xkbcommon")
        .unwrap_or_else(|error| {
            panic!("Typhon requires libxkbcommon >= 1.10.0: {error}");
        });
}
```

  Run `pkg-config --modversion xkbcommon` and record the detected version in the implementation report. Run `rtk cargo check` to confirm the build script and lockfile resolve in the local checkout.

- [ ] **Step 4: Add the private exact-ABI FFI declaration.** In `src/compositor/keyboard.rs`, use the public opaque `xkb::ffi::xkb_state` type returned by `xkb::State::get_raw_ptr()` and the installed header’s exact types:

```rust
mod xkb_compat {
    use std::os::raw::c_int;
    use xkbcommon::xkb;

    #[link(name = "xkbcommon")]
    unsafe extern "C" {
        fn xkb_state_update_latched_locked(
            state: *mut xkb::ffi::xkb_state,
            affect_latched_mods: u32,
            latched_mods: u32,
            affect_latched_layout: bool,
            latched_layout: i32,
            affect_locked_mods: u32,
            locked_mods: u32,
            affect_locked_layout: bool,
            locked_layout: i32,
        ) -> c_int;
    }

    pub(super) fn set_locked_layout(state: &xkb::State, index: i32) -> bool {
        // SAFETY: `state.get_raw_ptr()` points to this live compositor-thread
        // XKB state; libxkbcommon does not retain it, the state outlives this
        // call, no concurrent access exists, the index was validated before
        // entry, and this declaration matches xkbcommon.h on >= 1.10.0.
        unsafe {
            xkb_state_update_latched_locked(
                state.get_raw_ptr(),
                0,
                0,
                false,
                0,
                0,
                0,
                true,
                index,
            ) != 0
        }
    }
}
```

  Keep the unsafe call private and do not add an FFI module for unrelated XKB functions. Confirm the declaration against `/usr/include/xkbcommon/xkbcommon.h` before compiling.

- [ ] **Step 5: Implement layout snapshots and strict mutation.** Add `KeyboardLayoutError` variants for unavailable/zero-layout state, invalid index with `{ index, count }`, and bounded FFI failure only if the C result indicates failure. Use `keymap.num_layouts()` as the count, convert every index to `u32`, preserve empty `layout_get_name(index)` values as `String::new()`, and read effective/locked state with `serialize_layout(STATE_LAYOUT_EFFECTIVE)` and `serialize_layout(STATE_LAYOUT_LOCKED)`.

  In `set_locked_layout`, reject `index >= count` and any index that cannot be represented as `i32` before the FFI call; pass the checked `i32` to the private bridge; short-circuit an already-locked index as `changed: false`; capture the complete `wayland_serialized_state()` before and after; invoke only the eight arguments shown above; never touch `physical_pressed_keys`, the raw client ledger, modifiers, latched layout, or keymap bytes. Implement next/previous using locked state and `(locked + 1) % count` / `(locked + count - 1) % count`, with single-layout operations returning successful no-ops.

- [ ] **Step 6: Run the focused unit tests GREEN and verify no reconstruction paths returned.**

```bash
rtk cargo test --lib compositor::keyboard::tests::runtime_layout -- --nocapture
rtk rg -n "xkb_state_update_mask|xkb_state_update_latched_locked|Keymap::new_from|State::new|synthetic|rebuild" src/compositor/keyboard.rs
```

  The only runtime mutation bridge must be `xkb_state_update_latched_locked`; `State::new` remains only initialization/reference-test code, and no runtime method may recompile a keymap or construct another authoritative state.

- [ ] **Step 7: Commit the core API and dependency gate.**

```bash
rtk git add build.rs Cargo.toml Cargo.lock src/compositor/keyboard.rs
rtk git diff --cached --check
rtk git commit -m "feat: add runtime xkb layout state control"
```

### Task 2: Thread the layout API through the compositor and Wayland publication

**Files:**
- Modify: `src/compositor/keyboard.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/server_toplevel.rs`
- Test: `src/compositor/tests/input_output/output_keyboard_cursor.rs`
- Test: `src/compositor/keyboard.rs`

**Interfaces:**
- `KeyboardStateHandle::layout_snapshot`, `set_locked_layout`, `next_layout`, and `previous_layout` run only through the existing TLS map and permanently fail closed if the state is unavailable or used off-thread.
- `CompositorState::keyboard_layout_snapshot`, `set_keyboard_layout`, `next_keyboard_layout`, and `previous_keyboard_layout` return a complete public `KeyboardLayoutSnapshot` or a narrow `KeyboardLayoutControlError`.
- `OwnCompositorServer` exposes the same four operations as public server wrappers, matching its existing keyboard publication wrappers.

- [ ] **Step 1: Write a failing compositor publication test.** Extend the existing Wayland keyboard fixture in `src/compositor/tests/input_output/output_keyboard_cursor.rs` to configure `br,us`, invoke the server’s runtime set wrapper, and assert that the client event log contains exactly one `keyboard_modifiers` event with group 1 and no keyboard key or keymap event. Invoke set 1 again and assert the modifier-event count does not change.

- [ ] **Step 2: Run the publication test RED.**

```bash
rtk cargo test --lib compositor::tests::input_output::output_keyboard_cursor::runtime_layout -- --nocapture
```

  Expected: compile failure for the missing server/state methods.

- [ ] **Step 3: Add TLS handle forwarding.** Mirror `KeyboardStateHandle::update_physical_key`: match `Ready(id)`, borrow the state in `KEYBOARD_STATES`, call the layout method, and mark the handle `Failed` when the TLS entry is missing. For `Uninitialized`, return the existing unavailable error after the caller has attempted `ensure_keyboard_state`; for `Failed`, never initialize or retry.

- [ ] **Step 4: Add `CompositorState` and `OwnCompositorServer` wrappers.** Ensure initialization before every operation. Convert the internal layout state into `control_snapshots::KeyboardLayoutSnapshot`, copy entries without changing names, and map internal invalid-index/unavailable errors without panicking. Keep the mutation and snapshot read in the same TLS call so the returned snapshot is authoritative.

- [ ] **Step 5: Publish changed state through the existing standalone path.** After each mutation wrapper receives `KeyboardLayoutChange { changed, snapshot }`, call `send_keyboard_modifiers_without_key()` only when `changed` is true and a focused keyboard exists. Do not call a key path, alter `pressed_keys`, resend `send_keyboard_initial_state`, create a new serial counter, or flush a redundant event. With no focus, leave the XKB state changed and let `ensure_keyboard_focus()` publish it on the next enter.

- [ ] **Step 6: Verify held modifiers and session retention.** Add compositor tests for Shift and Control held across `set_keyboard_layout(1)`, followed by their real releases; assert no synthetic key events, no duplicate modifier transitions, and a final depressed mask of zero. Add a session-reset test that sets layout 1, clears/restores keyboard focus through the accepted v1 path, and asserts locked/effective layout 1 after enter.

- [ ] **Step 7: Run compositor keyboard tests GREEN and commit.**

```bash
rtk cargo test --lib compositor::keyboard::tests -- --nocapture
rtk cargo test --lib compositor::tests::input_output::output_keyboard_cursor -- --nocapture
rtk git add src/compositor/keyboard.rs src/compositor/state/input_resources.rs src/compositor/server_toplevel.rs src/compositor/tests/input_output/output_keyboard_cursor.rs
rtk git diff --cached --check
rtk git commit -m "feat: expose runtime layout switching from compositor"
```

### Task 3: Add typed control commands and compositor-thread dispatch

**Files:**
- Modify: `src/control.rs`
- Modify: `src/control_snapshots.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Test: `src/control_tests.rs`
- Test: `src/native/control_tests.rs`
- Test: `src/native_output/runtime/cycle_dispatch.rs` or its existing runtime test module

**Interfaces:**
- `ControlCommand` gains `KeyboardLayoutGet`, `KeyboardLayoutSet`, `KeyboardLayoutNext`, and `KeyboardLayoutPrevious`, mapped exactly to `keyboard.layout.get`, `.set`, `.next`, and `.previous`.
- `KeyboardLayoutSnapshot` and `KeyboardLayoutEntrySnapshot` are public serde types in `src/control_snapshots.rs`, with camelCase fields and `deny_unknown_fields`.
- `AstreactlResult::KeyboardLayout(KeyboardLayoutSnapshot)` carries every successful query/mutation response.
- `KeyboardLayoutGetArgs {}` and `KeyboardLayoutSetArgs { index: u32 }` are private strict dispatch argument types.

- [ ] **Step 1: Write failing codec and snapshot tests.** Add command round-trip assertions:

```rust
assert_eq!(ControlCommand::parse("keyboard.layout.get"), Some(ControlCommand::KeyboardLayoutGet));
assert_eq!(ControlCommand::KeyboardLayoutSet.as_str(), "keyboard.layout.set");
```

  Add serde tests that accept the complete camelCase snapshot and reject a missing field, wrong field type, or `unknownField`. Add request fixtures for `{}` and `{"index": 1}`.

- [ ] **Step 2: Run the codec tests RED.**

```bash
rtk cargo test --lib control -- --nocapture
rtk cargo test --lib control_snapshots -- --nocapture
```

  Expected: missing enum variants/types or failed fixture decoding.

- [ ] **Step 3: Add the command variants and strict typed snapshots.** Implement the four command `as_str`/`parse` arms. Add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyboardLayoutSnapshot {
    pub effective_index: u32,
    pub locked_index: u32,
    pub layout_count: u32,
    pub layouts: Vec<KeyboardLayoutEntrySnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyboardLayoutEntrySnapshot {
    pub index: u32,
    pub name: String,
}
```

  Add the result variant and keep the existing untagged result envelope.

- [ ] **Step 4: Add dispatch argument validation.** Define private `EmptyKeyboardLayoutArgs {}` and `KeyboardLayoutSetArgs { index: u32 }` with `#[serde(deny_unknown_fields)]`. Deserialization must reject negative, fractional, string, missing, and unknown fields before calling the server. Do not parse an explicit index through a signed type and cast it.

- [ ] **Step 5: Dispatch all four commands on the compositor thread.** Add match arms in `NativeRuntime::dispatch_control_command`: validate args, call the corresponding `OwnCompositorServer` wrapper, serialize its returned snapshot, and map errors to `ControlErrorCode::InvalidArgument` for invalid index or `ControlErrorCode::Internal` with bounded `keyboard_state_unavailable` detail for unavailable state. Preserve the existing success/error response envelope and response-size validation.

- [ ] **Step 6: Add dispatch behavior tests.** Exercise valid get/set/next/previous, out-of-range set, all strict argument failures, typed success payloads, unavailable keyboard state, and no-op mutation response. Assert the server wrapper is the only mutation route and no new control-side active-layout variable exists.

- [ ] **Step 7: Run control/native tests and commit.**

```bash
rtk cargo test --lib control -- --nocapture
rtk cargo test --lib control_snapshots -- --nocapture
rtk cargo test --lib native::control -- --nocapture
rtk git add src/control.rs src/control_snapshots.rs src/native_output/runtime/cycle_dispatch.rs src/control_tests.rs src/native/control_tests.rs
rtk git diff --cached --check
rtk git commit -m "feat: add keyboard layout control commands"
```

### Task 4: Add typed astreactl commands and deterministic output

**Files:**
- Modify: `src/bin/astreactl.rs`
- Modify: `src/astreactl/client.rs`
- Modify: `src/astreactl/output.rs`
- Modify: `tests/astreactl.rs`
- Modify: `docs/astreactl.md`
- Test: `src/astreactl/client.rs`
- Test: `src/astreactl/output.rs`

**Interfaces:**
- CLI forms are `astreactl keyboard layout`, `keyboard next`, `keyboard previous`, and `keyboard set INDEX`.
- Wire commands remain `keyboard.layout.get`, `keyboard.layout.next`, `keyboard.layout.previous`, and `keyboard.layout.set`.
- All forms use the existing global `--json`, socket discovery, timeout, and typed `AstreactlResult` decoder.

- [ ] **Step 1: Write failing parser/decoder/output tests.** Add CLI tests for the four command forms and invalid arity/negative/non-integer index. Add typed decoder fixtures for the complete snapshot, missing fields, wrong types, unknown fields, and server invalid_argument/internal responses. Add human-output assertions that mark the effective index, show locked index, preserve numeric ids, and sanitize layout names containing newline/tab/ESC.

- [ ] **Step 2: Run astreactl tests RED.**

```bash
rtk cargo test --bin astreactl -- --nocapture
rtk cargo test --test astreactl -- --nocapture
```

  Expected: parser rejects `keyboard` as unknown or typed decoder returns the existing unknown-command error.

- [ ] **Step 3: Add the parser branch and help text.** In `run`, route `command == "keyboard"` to a focused `parse_keyboard_command(&positionals[1..])`. Map `layout` to an empty-object get request, `next`/`previous` to empty-object mutations, and `set INDEX` to `{"index": parsed_u32}`. Reject extra args and values beginning with `-` using the established usage error style. Add the four forms to `--help`.

- [ ] **Step 4: Add typed result decoding.** Import `KeyboardLayoutSnapshot`, add the result variant, and map all four wire commands to `serde_json::from_value::<KeyboardLayoutSnapshot>`. Do not decode arbitrary `Value` or reuse another snapshot type.

- [ ] **Step 5: Add sanitized human formatting.** Add `AstreactlResult::KeyboardLayout` to `output::human` and implement `format_keyboard_layout`. Print:

```text
Effective: 1
Locked: 1

  0  Portuguese (Brazil)
* 1  English (US)
```

  Use `sanitize_terminal_text` for every layout name. Display unnamed entries with the fixed text `Unnamed` and never use names for selection or identity. Do not add colors.

- [ ] **Step 6: Update CLI documentation and test all forms.** Add the commands, wire names, snapshot schema, JSON behavior, strict index rules, no persistence, no UI, and sanitized human output to `docs/astreactl.md`. Extend `valid_result` and request fixtures in `tests/astreactl.rs` for all four commands; verify `--json` returns exactly the typed JSON object.

- [ ] **Step 7: Run astreactl tests and commit.**

```bash
rtk cargo test --bin astreactl -- --nocapture
rtk cargo test --test astreactl -- --nocapture
rtk git add src/bin/astreactl.rs src/astreactl/client.rs src/astreactl/output.rs tests/astreactl.rs docs/astreactl.md
rtk git diff --cached --check
rtk git commit -m "feat: expose keyboard layouts through astreactl"
```

### Task 5: Add end-to-end layout, group-action, focus, and session qualification

**Files:**
- Modify: `src/compositor/tests/input_output/output_keyboard_cursor.rs`
- Modify: `src/native_output/tests/input.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs` only if a focused runtime test helper is required
- Test: existing control/client helpers in the affected test files

**Interfaces:**
- Tests use the public server wrappers and existing real Wayland keyboard client/event capture helpers; they must not access an XKB pointer or create a second Typhon state.
- Reference assertions use an independently created `xkb::State` driven only by the original physical `update_key` sequence where XKB semantics are compared.

- [ ] **Step 1: Add real Wayland no-key publication coverage.** Start `br,us` at group 0, invoke runtime set 1, and capture the client event log. Assert the sequence contains no `wl_keyboard.key` or `wl_keyboard.keymap`, exactly one standalone `wl_keyboard.modifiers` with group 1, and a complete depressed/latched/locked snapshot. Set 1 again and assert the modifier count is unchanged.

- [ ] **Step 2: Add wrapping and no-focus coverage.** Assert next from the final layout wraps to 0 and previous from 0 wraps to the final layout. Temporarily remove keyboard focus, perform a runtime switch, assert no invalid event is emitted, restore focus, and assert enter publishes the current effective/locked group.

- [ ] **Step 3: Add held-modifier coverage.** Hold Shift and separately Control across `set 1`; assert each remains physically depressed, no key up/down is synthesized, and the real release clears the final depressed state. Add RightAlt/AltGr coverage against the independent physical XKB reference.

- [ ] **Step 4: Add `grp:*` interoperability coverage.** Configure `br,us` with `grp:alt_shift_toggle`, perform control set 1, drive physical Alt+Shift, query, then previous. Assert every result comes from the same authoritative state and reflects both effective and locked groups without a control-side counter.

- [ ] **Step 5: Add session persistence coverage.** Set locked layout 1, execute the accepted leave/clear/restore session path, and assert locked layout remains 1 after enter alongside retained Caps Lock behavior and an empty transient raw-key list.

- [ ] **Step 6: Run the complete focused regression matrix.**

```bash
rtk cargo test --lib compositor::keyboard::tests -- --nocapture
rtk cargo test --lib compositor::tests::input_output::output_keyboard_cursor -- --nocapture
rtk cargo test --bin oblivion-one native_output::tests::input -- --nocapture
rtk cargo test --bin oblivion-one native_output::tests::input_shortcut_inhibition -- --nocapture
```

  Confirm all Keyboard Layout Core v1 tests remain green, especially physical authority, raw repeat value 2, deferred Alt/Super, inhibition, group actions, TLS failure, focus, and session reset.

- [ ] **Step 7: Commit end-to-end qualification.**

```bash
rtk git add src/compositor/tests/input_output/output_keyboard_cursor.rs src/native_output/tests/input.rs
rtk git diff --cached --check
rtk git commit -m "test: qualify runtime keyboard layout control"
```

### Task 6: Final audit and repository verification

**Files:**
- Modify: only files required by failing verification; never stage unrelated shared-checkout changes

- [ ] **Step 1: Run the required dependency and formatting checks.**

```bash
pkg-config --modversion xkbcommon
rtk cargo fmt -- --check
rtk git diff --check
```

  Confirm the installed version is at least 1.10.0 and record the exact version.

- [ ] **Step 2: Audit v1 preservation and v2 guards.** Search the complete source tree for:

```bash
rtk rg -n "xkb_state_update_mask|fake.*(group|modifier)|synthetic.*key|rebuild.*XKB|State::new|new_from_names|keymap.*recompil|keyboard\.layout|active.*layout|layout.*name.*id|Send.*Xkb|Sync.*Xkb" src build.rs docs tests
```

  Inspect each match. `State::new` and `new_from_names` may remain only in initial construction/reference tests; the runtime switch must use the single private out-of-band bridge. There must be no separate active-layout variable, no keymap resend, no pointer escape, no fixed masks, and no persistence write.

- [ ] **Step 3: Run the full verification suite in the checkout.**

```bash
rtk cargo test
rtk cargo clippy --all-targets -- -D warnings
```

  Report exact pass/ignored/failure output. If an unrelated baseline failure occurs, preserve it, identify the exact test and file, and do not hide it or broaden the patch.

- [ ] **Step 4: Review staged scope after every implementation commit.**

```bash
rtk git status --short
rtk git diff HEAD~1 --stat
rtk git diff HEAD~1 --check
```

  Ensure only v2 implementation/docs/tests are in each commit. The pre-existing KMS/presentation edits in the shared checkout must remain unstaged and uncommitted.

- [ ] **Step 5: Final report.** Include the detected libxkbcommon version, FFI signature and safety invariant, effective/locked semantics, keymap-derived enumeration, four control commands, four astreactl forms, Wayland event behavior, wrap/no-op behavior, physical `grp:*` interoperability, session persistence, exact tests, and any remaining follow-up. State explicitly that no Settings UI, persistence, per-device policy, new keybinding, or event protocol was added.
