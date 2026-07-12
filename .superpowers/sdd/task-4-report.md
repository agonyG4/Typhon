# Task 4 Report: native active-interaction input regression

## Scope

Added one deterministic regression in `src/native_output/tests/input.rs`:

`native_input_active_resize_updates_real_compositor_without_client_motion`

The regression starts a real `OwnCompositorServer`, maps a real Wayland xdg
toplevel through a test-local client, starts a compositor-owned resize, and
then calls `apply_native_input_effect` with absolute `(132, 112)` motion while
`server.window_interaction_active()` is true. It does not modify production
routing, cursor ownership, or `NativeInputState`.

The small Wayland setup in the same test module is necessary because the
existing client harness is compiled under `src/compositor/tests` for the
library test target. `src/native_output/tests/input.rs` is compiled for the
binary test target; the binary links the normal library artifact, which does
not contain `#[cfg(test)] compositor::tests`. Promoting that harness would
require a public production-facing test hook or cross-target test-support
feature. Neither is appropriate for this task.

## Assertions covered

- A mapped toplevel exists before resize starts.
- `begin_window_resize_at` starts a compositor-owned resize, and the
  interaction is active immediately before and after `apply_native_input_effect`.
- The active interaction consumes the absolute coordinates: raw resize update
  metrics advance by one, then `prepare_frame` applies the pending resize.
- Software/client cursor rendering is active. Its `(3, 4)` hotspot-adjusted
  position reconstructs the compositor last pointer position as `(132, 112)`.
- The input application requests redraw.
- The live client records no additional `wl_pointer.motion` after the active
  grab update. The baseline setup motion is retained only to acquire the
  cursor-enter serial.
- The `relative_motion` field is supplied to the input effect but the local
  client deliberately does not create a relative-pointer resource. The existing
  Task 3 integration regression retains a live relative-pointer recipient and
  verifies unchanged relative-pointer count for the compositor-only update.
- Existing normal-motion and `NativeInputState` ownership coverage remains in
  the native-input suite, including
  `native_input_window_interaction_motion_routes_through_compositor_owner`.

## TDD evidence

### Harness red

The first attempted reuse of `crate::compositor::tests::support` failed before
any production change: the native-output test is in the binary crate and has
no `crate::compositor` module. The real Wayland harness is private to the
library test target. This established the cross-target limitation above.

### Behavioral red

With the regression in place, the Task 2 synchronized route was temporarily
replaced in the working tree by the pre-Task-2 interaction-only call:

```text
apply_native_window_action(action, context.server, context.perf, context.resize_perf)
```

The exact regression failed as expected:

```text
assertion `left == right` failed
  left: (92, 86)
 right: (132, 112)
```

`(92, 86)` is the cursor-derived stale compositor pointer before the active
update. The expected `(132, 112)` is the absolute native input passed to
`apply_native_input_effect`. The Task 2 route was restored immediately after
the replay and no production file remains modified.

### Green and verification

```text
cargo test --bin oblivion-one native_output::tests::input::native_input_active_resize_updates_real_compositor_without_client_motion -- --exact
1 passed

cargo test --bin oblivion-one native_output::tests::input::
61 passed

cargo test --lib compositor::tests::input_output::output_keyboard_cursor::compositor_only_interaction_motion_prevents_post_grab_cursor_teleport -- --exact
1 passed

cargo fmt --check
git diff --check
```

## Concerns

- The test-local Wayland client is intentionally scoped to this binary-only
  regression. It is the smallest path that can keep a real client cursor alive
  while `apply_native_input_effect` borrows the real server directly.
- Relative-pointer event counts remain covered by the already-approved Task 3
  compositor integration regression; adding the relative-pointer protocol to
  this local harness would duplicate that coverage without improving the native
  routing assertion.
