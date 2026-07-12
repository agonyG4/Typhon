# Task 3 Report: compositor-only cursor synchronization regression

## Scope

Implemented only the Task 3 regression and its test-only capture plumbing.

Files changed:

- `src/compositor/tests/input_output/output_keyboard_cursor.rs`
  - Added `compositor_only_interaction_motion_prevents_post_grab_cursor_teleport`.
- `src/compositor/tests/support/input_client.rs`
  - Added a persistent-client helper that captures the initial state, sends the existing compositor-only pointer update command, then sends a normal same-coordinate pointer motion sample.
- `src/compositor/tests/support/registry_state.rs`
  - Added test-only snapshots for cursor position, render/scene generation, render cause, pointer-motion count/event log, relative-motion count, and pointer focus.

No production compositor behavior or cursor ownership code changed. `server_runtime.rs` was not modified because the existing command channel already exposed both required motion paths.

## TDD evidence

### Red

The regression was added first, before the new support helper existed.

Command:

```text
cargo test compositor_only_interaction_motion_prevents_post_grab_cursor_teleport --lib -- --exact
```

Observed expected failure:

```text
error[E0425]: cannot find function
`create_client_cursor_then_synchronize_compositor_only_motion_and_send_normal_sample`
in this scope
 --> src/compositor/tests/input_output/output_keyboard_cursor.rs:559:21
error: could not compile `oblivion-one` (lib test) due to 1 previous error
```

### Green

After adding the minimal test plumbing, the fully qualified focused test passed:

```text
cargo test compositor::tests::input_output::output_keyboard_cursor::compositor_only_interaction_motion_prevents_post_grab_cursor_teleport --lib -- --exact

running 1 test
test compositor::tests::input_output::output_keyboard_cursor::compositor_only_interaction_motion_prevents_post_grab_cursor_teleport ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 591 filtered out
```

The focused cursor test module also passed after `cargo fmt --check`:

```text
cargo test compositor::tests::input_output::output_keyboard_cursor --lib

running 41 tests
... all 41 passed ...

test result: ok. 41 passed; 0 failed; 0 ignored; 0 measured; 551 filtered out
```

## Assertions covered

- A client cursor uses the nonzero `(3, 4)` hotspot.
- Initial cursor position/generations and dispatch/focus state are captured.
- The compositor-only update moves the render position to final pointer coordinates minus hotspot.
- It advances the render generation with `CursorMotion`, without advancing the scene generation.
- It preserves pointer event log, pointer-motion count, relative-motion count, and pointer focus.
- A subsequent normal sample at the same coordinates leaves cursor render position and render generation unchanged. The test intentionally permits normal client dispatch in this final step.

## Commit

`4d160e9 test: cover compositor-only cursor synchronization`

## Concerns

- None. The normal same-coordinate sample is allowed to produce ordinary client pointer dispatch; the no-dispatch assertions apply only to the compositor-only update, as required.

## Reviewer findings resolution (follow-up `9c4d5b3`)

### Relative-pointer dispatch coverage

The persistent-client helper now enables the existing `relative_pointer` input capability, binds `zwp_relative_pointer_manager_v1`, and creates a relative pointer for the same `wl_pointer`. The `_relative_pointer` binding remains in scope through the initial, compositor-only, interaction-update, and normal-sample captures. The regression compares `relative_motion_count` before and after the compositor-only update, so that comparison now observes a live relative-pointer recipient rather than an inert counter.

### Interaction route coverage and corrected behavioral red

The helper now starts a real `xdg_toplevel.resize` interaction using the cursor-enter serial. Its passing sequence is the fixed route: send `UpdatePointerPositionWithoutClientDispatch`, capture the cursor and no-dispatch state, then send `UpdateInteraction` with the same final coordinates before the subsequent normal sample.

For the behavioral red replay, the synchronization command was temporarily replaced with the pre-`2bd8291` interaction-only route (`UpdateInteraction { x, y }`). The exact regression failed because the visible cursor remained at its initial render position instead of the final hotspot-adjusted position:

```text
cargo test compositor::tests::input_output::output_keyboard_cursor::compositor_only_interaction_motion_prevents_post_grab_cursor_teleport --lib -- --exact

assertion `left != right` failed
  left: ClientCursorSnapshot { surface_id: 2, logical_x: 89, logical_y: 82, width: 24, height: 24 }
 right: ClientCursorSnapshot { surface_id: 2, logical_x: 89, logical_y: 82, width: 24, height: 24 }
```

This is the intended stale-compositor-cursor failure: the expected final pointer is `(92, 86)` after applying the `(3, 4)` hotspot, but the interaction-only route leaves the cursor at `(89, 82)`.

After restoring `UpdatePointerPositionWithoutClientDispatch`, the exact regression passed:

```text
running 1 test
test compositor::tests::input_output::output_keyboard_cursor::compositor_only_interaction_motion_prevents_post_grab_cursor_teleport ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 591 filtered out
```

The focused module and formatting check also passed:

```text
cargo fmt --check
cargo test compositor::tests::input_output::output_keyboard_cursor --lib

test result: ok. 41 passed; 0 failed; 0 ignored; 0 measured; 551 filtered out
```

No production hooks or cursor-ownership behavior changed.

## Reviewer finding fix: valid resize press serial

The resize request now follows the established pointer-button pattern. After the
initial pointer motion and cursor setup, the helper sends
`ServerCommand::PointerButton { button: 0x110, pressed: true }`, waits for the
server command barrier, roundtrips, requires `state.pointer_button_serial`, and
uses that serial for `xdg_toplevel.resize(... BottomRight)`. The live relative
pointer resource, cursor hotspot and generation assertions, compositor-only
no-dispatch assertions, and same-coordinate no-teleport assertion are
unchanged.

The helper asks the server for the `UpdateInteraction` result and advances the
pending resize through `PrepareFrame`; the regression asserts both that the
interaction update applied and that the toplevel has an active resize visual.
This proves the interaction route is active rather than a no-op before checking
the subsequent normal pointer sample.

### Corrected behavioral red

With the valid resize active, the synchronized compositor-only command was
temporarily replaced with the pre-`2bd8291` interaction-only route. The exact
regression failed because the cursor stayed at its initial render position:

```text
cargo test compositor::tests::input_output::output_keyboard_cursor::compositor_only_interaction_motion_prevents_post_grab_cursor_teleport --lib -- --exact

assertion `left != right` failed
  left: ClientCursorSnapshot { surface_id: 2, logical_x: 89, logical_y: 82, width: 24, height: 24 }
 right: ClientCursorSnapshot { surface_id: 2, logical_x: 89, logical_y: 82, width: 24, height: 24 }
```

### Corrected green

After restoring `UpdatePointerPositionWithoutClientDispatch`, the same exact
regression passed with the valid resize serial and active interaction route:

```text
running 1 test
test compositor::tests::input_output::output_keyboard_cursor::compositor_only_interaction_motion_prevents_post_grab_cursor_teleport ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 591 filtered out
```

Final verification also passed:

```text
cargo fmt --check
cargo test compositor::tests::input_output::output_keyboard_cursor --lib

test result: ok. 41 passed; 0 failed; 0 ignored; 0 measured; 551 filtered out
```
