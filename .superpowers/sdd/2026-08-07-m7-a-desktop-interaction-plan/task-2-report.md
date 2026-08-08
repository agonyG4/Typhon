# Typhon M7-A Task 2 report

## Scope

Implemented desktop hover focus and exact click activation on `main` at Task 1 commit `3105a3c`. The slice remains limited to managed desktop focus/activation, pointer-target capture, family-aware raising, minimized restore, serial/backend deduplication, and the related XDG/X11/input regressions. Task 3 extents, Task 4 borders, M7-B, and Eclipse were not touched.

## TDD evidence

The preserved red-test diff was run before production edits:

```text
cargo test --locked --quiet desktop_window_tests -- --test-threads=1
```

Initial outcome: RED at compilation because the preserved API changes referenced `WindowFocusReason` without imports in `server.rs` and `server_xwayland.rs` (`E0433`, two errors).

After the import-only unblock, the focused suite reached the intended behavioral RED:

```text
cargo test --locked --quiet desktop_window_tests -- --test-threads=1
```

Outcome: 47 passed, 3 failed: focused/topmost activation returned `Unavailable`, minimized activation returned `Unavailable`, and the transient-family stack regression rejected the existing family raise.

The hover expectation was then made explicit in the existing overlapping-window interaction regression and run before the final behavior was complete:

```text
cargo test --locked --quiet window_interaction_absolute_motion_targets_only_original_surface -- --test-threads=1
```

Outcome: RED; the test observed the old focus behavior and later exposed the exact activation/stack hit-test interaction.

## Implemented behavior

- Added reason-aware managed-window focus and activation policy.
- Hover focus changes keyboard focus by managed `WindowId` without raising.
- Focus serial/backend activation work is gated on managed-window transitions.
- Pointer motion suppresses desktop hover focus during move/resize, grabs, popup grabs, constraints, DND/drag, lock, and exclusive layer interaction paths.
- Pointer press captures one target/root/window identity, activates that exact window, restores minimized targets, and delivers the button to the captured surface without a post-activation re-hit-test.
- Activation uses the existing family-aware root raise path and avoids duplicate work for a focused/topmost target.
- Existing popup, layer, lock, constraint, override-redirect, notification, support, compositor-owned, XDG, and X11 transient ordering behavior remains covered by the focused tests.
- Pointer interaction validation now also checks the captured managed `WindowId`.

## GREEN and verification

```text
cargo test --locked --quiet desktop_window_tests -- --test-threads=1
```

50 passed, 0 failed.

```text
cargo test --locked --quiet window_interaction -- --test-threads=1
```

51 compositor interaction tests and 5 support tests passed, 0 failed.

```text
git diff --check
cargo fmt --all -- --check
cargo check --locked --quiet
cargo clippy --locked --all-targets --all-features --quiet -- -D warnings
```

All passed.

No native TTY/DRM, XWayland hardware-session, or game qualification was run in this task; this report claims automated validation only.

## Fix round 1/5 review evidence

### RED

Added focused regressions before the production fixes and observed:

```text
cargo test --locked --quiet pointer_press_activation_restores_minimized_window -- --test-threads=1
```

RED: the no-surface minimized target returned `Accepted` instead of `Unavailable`.

```text
cargo test --locked --quiet already_topmost_transient_family_does_not_queue_duplicate_restack -- --test-threads=1
```

RED: an already-topmost transient family queued a duplicate `RestackExact`.

```text
cargo test --locked --quiet xwayland_attachment_replacement_preserves_frame_and_keyboard_focus -- --test-threads=1
```

RED: same-window XWayland surface replacement advanced `focus_generation`.

### GREEN

The fixes reject unavailable root resources before logical focus or minimized restore, separate restore contents from the one activation focus, gate `focus_generation` on managed `WindowId` transitions, suppress duplicate restack for effective topmost transient families, and narrow the unused reason allowance to the specific `KeyboardNavigation` variant.

```text
cargo test --locked --quiet desktop_window_tests -- --test-threads=1
cargo test --locked --quiet window_interaction -- --test-threads=1
cargo test --locked --quiet xwayland_attachment_replacement_preserves_frame_and_keyboard_focus -- --test-threads=1
cargo test --locked --quiet xwayland_root_stack -- --test-threads=1
```

GREEN: 51 desktop-window tests passed; 51 interaction tests plus 5 support tests passed; the targeted replacement test passed; and 14 XWayland root-stack tests passed. The overlapping interaction regression now asserts the captured A target is topmost after activation and that the button is delivered to A, not B.

```text
git diff --check
cargo fmt --all -- --check
cargo check --locked --quiet
cargo clippy --locked --all-targets --all-features --quiet -- -D warnings
```

All static gates passed.

The broad `cargo test --locked --quiet xwayland -- --test-threads=1` filter was not used as a qualification gate: 34 tests failed during test-environment setup with `path must be shorter than SUN_LEN` and poisoned display-test locks, and one existing compositor geometry-ordering test failed at `update_window_interaction`. No native qualification claim is made.
