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
