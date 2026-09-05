# Typhon Keyboard Layout Core v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Typhon's hand-written XKB approximation with one cached,
libxkbcommon-backed seat keyboard core while preserving physical compositor
bindings and Wayland input protocol behavior. The v1.4 closure makes one
physical XKB state authoritative, keeps raw forwarding bookkeeping separate,
and excludes raw evdev repeat notifications from authoritative XKB updates.

**Architecture:** Add a focused `compositor::keyboard` module that compiles
RMLVO into one immutable keymap, owns one physical XKB state over that keymap,
serializes the explicit Wayland projection, and caches Text V1 bytes.
`physical_state` sees every real press/release and owns global group, latch,
lock, and LED semantics. A separate raw forwarded-key ledger tracks only
client-visible keys for enter state, deferred replay, release reconciliation,
and session reset. `CompositorState` owns a sendable
`KeyboardStateHandle`; the handle owns only a unique identity while the actual
`XkbKeyboardState` lives in compositor-thread TLS. This keeps
`CompositorState::default()` non-panicking and makes cross-thread use fail
closed. Normal startup uses requested configuration, baseline `br(abnt2)`,
then `us` fallback, preserving the requested repeat rate and delay across
fallback. Keyboard resource publication and forwarded key events consume that
same TLS-owned instance. Every real physical value 1/0 updates the physical XKB
state exactly once; value 2 never updates it. Physical-only actions can publish
global group/lock changes even when raw forwarding is suppressed. Client-only
replay forwards a raw key without updating XKB or fabricating modifiers.
Session suspend clears transient physical keys and the raw ledger without
resetting layout or lock state.

**Tech Stack:** Rust 2024, `xkbcommon` 0.9 safe bindings, libxkbcommon,
Wayland server/client protocol bindings, existing cargo test harness, `rtk`
for command output filtering.

## Global Constraints

- Preserve physical Linux keycodes for `AstreaBindingManager` and
  `wl_keyboard::key`.
- Update `physical_state` exactly once for every real physical press/release
  with `evdev keycode + 8`, including transitions that are consumed or
  deferred. Raw evdev `value=2` is a repeat notification and creates no XKB
  state action.
- Keep XKB state transitions separate from client-visible key forwarding so a
  deferred raw replay never updates the physical state twice. Publish a
  changed projected modifier snapshot after the corresponding visible key, or
  standalone for a physical-only global group/lock change.
- Serialize depressed, latched, locked, and effective group fields directly
  from `physical_state`. Use no fixed modifier bit positions and no hardcoded
  group zero; do not filter physical modifiers because their raw keys were
  suppressed.
- Publish cached XKB Text V1 bytes as a NUL-terminated `XkbV1` keymap.
- Preserve v1 repeat-info gating, configured repeat values, client-side
  Wayland repeat, native shortcut repeat policy, focus isolation, shortcut
  inhibition, and deferred Alt/Super routing.
- Pass RMLVO values to libxkbcommon without sanitizing or hand-building XKB
  include syntax.
- Preserve explicit empty-vs-absent layout/variant values and let native XKB
  options such as `grp:alt_shift_toggle` produce group changes.
- Advertise `wl_seat` Keyboard only after compositor-thread XKB initialization;
  failed initialization advertises Pointer only and rejects unexpected
  `get_keyboard` requests with `MissingCapability`.
- Do not modify unrelated pre-existing worktree changes.

---

### Task 1: Add the XKB dependency and focused keyboard state module

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Create: `src/compositor/keyboard.rs`
- Modify: `src/compositor/mod.rs`
- Test: `src/compositor/keyboard.rs` unit tests

**Interfaces:**
- Produces `KeyboardConfig`, `KeyboardSerializedState`, and
  `XkbKeyboardState` for compositor state and protocol code.
- `XkbKeyboardState::from_config(&KeyboardConfig) -> Result<Self, String>`
  compiles RMLVO with `xkbcommon::xkb::Keymap::new_from_names`.
- `XkbKeyboardState::from_environment() -> Result<Self, String>` attempts
  requested, baseline, and `us` configurations with diagnostics. The fallback
  configurations inherit only the requested repeat rate/delay; their RMLVO is
  still the documented baseline or minimal `us` configuration.
- `XkbKeyboardState::update_physical_key(evdev_key: u32, pressed: bool) -> bool`
  performs checked `+8` conversion and reports direct projected-state changes.
- `XkbKeyboardState::wayland_serialized_state() -> KeyboardSerializedState`
  serializes all modifier and group fields from the physical state, while
  `keymap_file() -> io::Result<(File, u32)>` exposes the cached keymap.

- [ ] **Step 1: Add the safe dependency and module declaration.**

  Add `xkbcommon = "0.9.0"` to `Cargo.toml` and `mod keyboard;` to
  `src/compositor/mod.rs`. Do not add raw FFI or unrelated dependencies.

- [ ] **Step 2: Write failing unit tests for deterministic defaults and RMLVO.**

```rust
#[test]
fn default_keyboard_config_preserves_native_defaults() {
    let config = KeyboardConfig::default();
    assert_eq!(config.layout, "br");
    assert_eq!(config.variant.as_deref(), Some("abnt2"));
    assert_eq!(config.options, Some(String::new()));
    assert_eq!(config.repeat_rate, 25);
    assert_eq!(config.repeat_delay, 600);
}

#[test]
fn rmlvo_compiles_multiple_layouts_without_manual_include_syntax() {
    let config = KeyboardConfig {
        layout: "br,us".into(),
        variant: Some("abnt2,".into()),
        options: Some("grp:alt_shift_toggle".into()),
        ..KeyboardConfig::default()
    };
    let state = XkbKeyboardState::from_config(&config).unwrap();
    assert!(state.keymap_text_v1().starts_with("xkb_keymap"));
}

#[test]
fn update_physical_key_uses_xkb_offset_but_keeps_evdev_api() {
    let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
    assert!(!state.update_physical_key(30, true));
    assert!(!state.update_physical_key(42, true));
    assert_eq!(state.keymap_keycode_for_evdev(30), Some(38));
}
```

- [ ] **Step 3: Run the new tests and verify the expected RED failure.**

  Run `rtk cargo test keyboard::tests -- --nocapture`.

  Expected: compile failure because the types and module implementation do
  not exist yet. Fix only test/module naming errors before continuing if the
  failure is unrelated.

- [ ] **Step 4: Implement `KeyboardConfig` and environment parsing.**

  Store optional rules/model/variant/options, required layout, and repeat
  rate/delay. Use existing `OBLIVION_ONE_XKB_LAYOUT`, `..._VARIANT`, and
  `..._OPTIONS` plus `..._RULES`, `..._MODEL`, `..._REPEAT_RATE`, and
  `..._REPEAT_DELAY`. Preserve non-empty RMLVO values exactly; parse repeat
  integers and use defaults for parse failures or negative values.

- [ ] **Step 5: Implement RMLVO compilation and Text V1 caching.**

  Create a context with `CONTEXT_NO_ENVIRONMENT_NAMES`, pass empty strings for
  absent rules/model/variant and `None` for absent options, call
  `Keymap::new_from_names`, serialize with
  `KEYMAP_FORMAT_TEXT_V1`, append exactly one NUL byte, and initialize one
  physical `xkb::State` from the immutable keymap. Return a descriptive error including
  every requested RMLVO value when compilation fails.

- [ ] **Step 6: Implement state serialization, checked key updates, LEDs, and FD creation.**

  `update_physical_key` must use `checked_add(8)` and reject values outside
  the legal XKB range. Compare the explicit Wayland projection before and
  after each state update. Serialize all modifier fields and the effective
  group directly from `physical_state`.
  Expose logical Caps/Num/Scroll LED queries.
  `keymap_file` writes the cached bytes to an unlinked unique runtime file and
  returns its byte size including the NUL.

- [ ] **Step 7: Implement fallback initialization and run GREEN tests.**

  Try the requested config, then default `br(abnt2)`, then `us` with no
  variant/options. Emit one precise diagnostic per failed requested/fallback
  compile and return an error only if all three fail. Run
  `rtk cargo test keyboard::tests -- --nocapture`; expected: PASS.

- [ ] **Step 8: Commit the self-contained module.**

```bash
git add Cargo.toml Cargo.lock src/compositor/keyboard.rs src/compositor/mod.rs
git commit -m "feat: add libxkbcommon keyboard state core"
```

### Task 2: Replace manual compositor keyboard state with the seat XKB state

**Files:**
- Modify: `src/compositor/input.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/protocols/input.rs`
- Test: `src/compositor/keyboard.rs` and existing input tests

**Interfaces:**
- `CompositorState` owns `keyboard_state: KeyboardStateHandle`.
- `CompositorState::ensure_keyboard_state()` lazily initializes the TLS-owned
  state without panicking during `Default` construction; a missing owner or
  failed initialization permanently disables keyboard state for that handle.
- `CompositorState::keyboard_serialized_state()` returns the current direct
  physical XKB snapshot or all-zero state when initialization is unavailable.
- `CompositorState::send_keyboard_initial_state(&wl_keyboard::WlKeyboard)`
  sends the cached keymap and supported repeat info from the owned state.

- [ ] **Step 1: Add a failing unit assertion for real modifier serialization.**

```rust
#[test]
fn caps_lock_changes_xkb_locked_mask_without_manual_modifier_state() {
    let mut state = XkbKeyboardState::from_config(&KeyboardConfig::default()).unwrap();
    assert_eq!(state.wayland_serialized_state().locked, 0);
    state.update_physical_key(58, true);
    state.update_physical_key(58, false);
    assert_ne!(state.wayland_serialized_state().locked, 0);
}
```

- [ ] **Step 2: Run the focused test and verify RED against the old implementation.**

  Run `rtk cargo test keyboard::tests::caps_lock_changes_xkb_locked_mask -- --nocapture`.
  Expected: compile or assertion failure because the real state API is not
  wired into the compositor module yet.

- [ ] **Step 3: Remove obsolete manual keyboard helpers.**

  Delete `KeyboardModifierState`, its fixed masks, `KeyboardLayoutConfig`,
  `xkb_symbols_include`, `keymap_contents`, sanitizers, stateless
  `send_keyboard_initial_state`, and stateless `keymap_file` from
  `input.rs`. Remove imports used only by those helpers and retain pointer and
  serial helpers in `input.rs`.

- [ ] **Step 4: Add the seat-owned state and lazy initialization.**

  Replace `keyboard_modifiers` with `keyboard_state: KeyboardStateHandle` in
  `CompositorState`. `ensure_keyboard_state` calls
  `XkbKeyboardState::from_environment`, logs a clean unavailable-state
  diagnostic, and never panics. Keep the handle in compositor state and keep
  the actual XKB object in the owning compositor thread's TLS; a failed or
  cross-thread handle does not retry.

- [ ] **Step 5: Rewire `get_keyboard` initialization.**

  In `wl_seat::GetKeyboard`, call the state method that publishes the cached
  keymap and configured repeat info, then register the resource. Do not compile
  configuration in the protocol dispatch and do not publish repeat info for
  protocol version 1.

- [ ] **Step 6: Rewire forwarded key events and focus events.**

  Keep raw `pressed_keys` bookkeeping only for client-visible keys. Apply the
  explicit `PhysicalOnly`, `PhysicalAndClient`, and `ClientOnly` action stream:
  physical actions update `physical_state`, visible actions publish the raw
  key, and deferred Alt/Super replay is client-only with no XKB update. Publish
  `wl_keyboard::key { key }` with the raw evdev value, then publish the
  projected modifiers/group only if it changed; a physical-only global change
  may publish a standalone modifiers event. Update focus enter and modifier
  publication to use the same projection and never reconstruct masks from
  `pressed_keys`.

- [ ] **Step 7: Close repeat and session transient-state semantics.**

  Treat raw evdev values as release (`0`), press (`1`), and repeat (`2`). A
  repeat may reach binding repeat policy but creates no physical XKB update and
  no normal Wayland key. During session suspend/VT switching, reconcile tracked
  transient physical keys and raw forwarded keys before discarded input can
  strand depressed state; retain physical locked/layout state.

- [ ] **Step 8: Run focused existing tests and new unit tests.**

  Run `rtk cargo test compositor::tests::input_output::output_keyboard_cursor`
  and `rtk cargo test keyboard::tests -- --nocapture`.

  Expected: all existing keyboard/focus/shortcut-inhibition tests pass and
  the new XKB tests pass.

- [ ] **Step 9: Commit the compositor integration.**

```bash
git add src/compositor/input.rs src/compositor/mod.rs \
  src/compositor/state/input_resources.rs src/compositor/protocols/input.rs
git commit -m "feat: route compositor keyboard events through xkb state"
```

### Task 3: Strengthen Wayland registry capture and protocol integration tests

**Files:**
- Modify: `src/compositor/tests/support/registry_state.rs`
- Modify: `src/compositor/tests/input_output/output_keyboard_cursor.rs`
- Modify: `src/compositor/tests/support/output_bindings.rs` only if a helper is
  needed for deterministic keyboard setup

**Interfaces:**
- Registry state captures `mods_depressed`, `mods_latched`, `mods_locked`,
  `group`, `repeat_rate`, and `repeat_delay` for every relevant event.
- Existing tests retain their current helper APIs and remain deterministic
  without using desktop environment configuration.

- [ ] **Step 1: Add failing assertions for keymap parseability and repeat values.**

```rust
#[test]
fn xkb_v1_keymap_is_parseable_and_reports_its_event_size() {
    let state = request_keyboard_from_seat(&socket_path).unwrap();
    assert_eq!(state.keyboard_keymap_bytes.len(), state.keyboard_keymap_size as usize);
    assert_eq!(state.keyboard_keymap_bytes.last(), Some(&0));
    let text = std::ffi::CStr::from_bytes_with_nul(&state.keyboard_keymap_bytes).unwrap();
    let context = xkbcommon::xkb::Context::new(xkbcommon::xkb::CONTEXT_NO_FLAGS);
    let keymap = xkbcommon::xkb::Keymap::new_from_string(
        &context,
        text.to_str().unwrap(),
        xkbcommon::xkb::KEYMAP_FORMAT_TEXT_V1,
        xkbcommon::xkb::KEYMAP_COMPILE_NO_FLAGS,
    );
    assert!(keymap.is_some());
}
```

  Adapt only the helper setup names to the repository’s existing test support.

- [ ] **Step 2: Run the focused integration test and verify RED.**

  Run `rtk cargo test compositor::tests::input_output::output_keyboard_cursor::xkb_v1_keymap_is_parseable_and_reports_its_event_size`.
  Expected: failure or missing capture fields before the registry/test changes
  are complete.

- [ ] **Step 3: Capture all keyboard protocol fields.**

  Extend `RegistryTestState` with modifier vectors for depressed/latched/
  locked, group values, repeat rate/delay, and an event log entry for keymap,
  key, and modifiers. Record all fields in the client keyboard dispatcher.

- [ ] **Step 4: Add semantic XKB integration coverage.**

  Verify raw evdev keys remain `29`, `30`, and `42` in client events. Parse the
  received Text V1 map with libxkbcommon and assert Shift/Control/Caps state
  by modifier name, not fixed masks. Add deterministic `br`, `us`, and
  `br,us` unit/integration construction where the current harness permits it.

- [ ] **Step 5: Add ordering and repeat assertions.**

  Assert a modifier-changing key emits `keyboard_key` before
  `keyboard_modifiers`, focus enter publishes all four state fields, v1 has no
  repeat info, supported versions receive 25/600 by default, and native value
  2 repeat input is still not forwarded as another normal key event.

- [ ] **Step 6: Run all focused compositor input tests.**

  Run `rtk cargo test compositor::tests::input_output::output_keyboard_cursor`
  and `rtk cargo test compositor::tests::support::registry_state`.
  Expected: PASS with no changes to binding or shortcut-inhibition behavior.

- [ ] **Step 7: Commit test coverage.**

```bash
git add src/compositor/tests/support/registry_state.rs \
  src/compositor/tests/input_output/output_keyboard_cursor.rs \
  src/compositor/tests/support/output_bindings.rs
git commit -m "test: cover xkb keyboard protocol semantics"
```

### Task 4: Final qualification, diff audit, and repository verification

**Files:**
- Modify: only files required by failing verification or formatting

- [ ] **Step 1: Run the focused native input and XWayland regression tests.**

  Run `rtk cargo test native_output::tests::input` and
  `rtk cargo test native_output::tests::input_shortcut_inhibition`.
  Confirm physical bindings, deferred modifiers, repeat suppression, and
  shortcut inhibition remain green. Run the existing XWayland input target if
  its test filter is available.

- [ ] **Step 2: Audit obsolete behavior and required invariants.**

  Run `rtk rg -n 'KeyboardModifierState|KeyboardLayoutConfig|xkb_symbols_include|keymap_contents|sanitize_xkb|mods_latched: 0|group: 0|XKB_.*MASK|key \+ 8|key\+8' src`. Inspect every match and remove or qualify only obsolete
  keyboard remnants. Confirm only `xkb update = evdev + 8` remains and Wayland
  key publication uses raw evdev values.

- [ ] **Step 3: Format and run the full verification suite.**

  Run `rtk cargo fmt --check`, `rtk cargo test`, and
  `rtk cargo clippy --all-targets -- -D warnings`. Record exact pass/failure
  summaries and separate environment dependency failures from code failures.

- [ ] **Step 4: Review the final diff and worktree scope.**

  Run `rtk git diff --check`, `rtk git diff 1ecd864..HEAD --stat` for the new commits,
  and `rtk git status --short`. Confirm pre-existing XWayland/pacing changes
  are not staged or committed by this task and no pointer/rendering/KMS files
  changed.

- [ ] **Step 5: Commit any verification-only fixes.**

```bash
git add Cargo.toml Cargo.lock src/compositor/keyboard.rs src/compositor/input.rs \
  src/compositor/mod.rs src/compositor/state/input_resources.rs \
  src/compositor/protocols/input.rs src/compositor/tests/support/registry_state.rs \
  src/compositor/tests/input_output/output_keyboard_cursor.rs
git commit -m "fix: close keyboard layout core verification gaps"
```

- [ ] **Step 6: Report exact results and remaining follow-up.**

  Report the prior root problem, new architecture, files, dependency/API,
  keycode and modifier semantics, keymap/repeat/fallback behavior, tests,
  exact verification commands/results, and remaining physical LED/layout-v2
  follow-up work.

## v1.4 correction plan

**Goal:** Close the single-XKB-authority gap while preserving the accepted
session leave/enter reset semantics and the physical/client routing contract.

**Architecture:** Keep one authoritative `physical_state` per seat. Every real
physical press/release updates it exactly once, while repeat value 2 does not
update XKB. Keep the raw forwarded-key ledger only for Wayland enter state,
deferred raw replay, release reconciliation, and session reset. Serialize all
Wayland modifier and group fields directly from `physical_state`; no replay
state reseeding, fixed modifier masks, or filtering is allowed.

### Task 5: Remove the secondary XKB projection

**Files:**
- Modify: `src/compositor/keyboard.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/server_toplevel.rs`
- Modify: `src/native_output/input/routing.rs`
- Test: `src/compositor/keyboard.rs`
- Test: `src/native_output/tests/input.rs`

- [ ] **Step 1: Keep one physical state and a separate raw ledger.** Remove
  the secondary XKB state and its rebuild path. Retain raw pressed-key
  bookkeeping only where Wayland forwarding and session reconciliation need it.

- [ ] **Step 2: Enforce action routing.** PhysicalOnly and
  PhysicalAndClient update the physical state once for real value 1/0;
  ClientOnly forwards a raw key without an XKB update. Preserve key-before-
  modifiers ordering and standalone publication for physical-only changes.

- [ ] **Step 3: Serialize the authoritative snapshot.** Publish depressed,
  latched, locked, and effective group values directly from the physical state,
  including physical modifiers whose raw keys are suppressed.

- [ ] **Step 4: Add independent group-sensitive references.** Parse the
  published Text V1 keymap into independent XKB states and compare complete
  snapshots for held RightAlt across `us,br` with `,abnt2` and
  `grp:alt_shift_toggle`, plus Ctrl+Shift with `grp:ctrl_shift_toggle`. The
  reference oracle must not use Typhon's projected state.

- [ ] **Step 5: Cover RightAlt inhibition and retain the existing matrix.**
  Verify deferred RightAlt replay while shortcut inhibition is effective,
  including promoted release, then keep deferred Alt/Super, consumed
  Super+Space, focus-enter, repeat, and event-ordering tests green.

### Task 6: Preserve Wayland keyboard focus across session suspension

**Files:**
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/server_toplevel.rs`
- Modify: `src/native_output/runtime/session_io.rs`
- Test: `src/native_output/tests/input.rs`

- [ ] **Step 1: Remember the pre-suspend keyboard focus.** Add a compositor
  field for the keyboard surface to restore. The suspend helper must send a
  protocol `leave`, clear the keyboard focus, clear transient state, and clear
  the raw forwarded-key ledger.

- [ ] **Step 2: Restore focus after input recovery.** Add a server method
  called by `NativeRuntime::resume_input`. If the remembered surface is still
  the focused live surface, call normal focus reconciliation so the client
  receives `enter` with an empty key list and the current direct
  modifiers/group; otherwise discard the target.

- [ ] **Step 3: Test forwarded Ctrl and an ordinary Z.** With a real Wayland
  keyboard client, forward Ctrl and press Z in separate cases, invoke the
  suspend/reset/resume boundary, then capture enter keys and modifiers. Assert
  no Ctrl or Z remains logically pressed, using modifier indices obtained from
  the parsed keymap.

- [ ] **Step 4: Test persistent Caps Lock.** Toggle Caps Lock before the
  transient reset, then assert the resumed modifier snapshot has no depressed
  transient keys but retains the XKB locked Caps mask.

### Task 7: Final v1.4 verification

- [ ] **Step 1:** Run focused keyboard, native-input, session, and compositor
  keyboard tests in the repository checkout so Cargo uses the local target
  directory.
- [ ] **Step 2:** Run `rtk cargo fmt --check`, `rtk cargo test`, and
  `rtk cargo clippy --all-targets -- -D warnings`; report unrelated baseline
  failures separately and do not stage unrelated worktree changes.
- [ ] **Step 3:** Audit for secondary XKB state/replay assumptions, unpaired
  session focus reset, fixed modifier masks, `value=2` XKB updates, duplicate
  physical transitions, and non-English Markdown.
- [ ] **Step 4:** Run `rtk git diff --check`, inspect the staged file list, and
  commit the v1.4 closure separately from unrelated pacing changes.
