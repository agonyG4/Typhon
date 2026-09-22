# XWayland Resize Context Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Distinguish strict resize-owned backend commands from non-resizing configure commands that only captured enqueue-time resize context.

**Architecture:** Keep epoch capture in `WindowBackendCommand`. Refactor only the `Configure` dequeue branch in `OwnCompositorServer::take_xwayland_backend_commands`: strict handling for `resizing: true`, explicit state matching for `resizing: false`, and unchanged strict handling for `FinalizeResize`. Extend the existing visual regression module with presentation and timeout drain assertions.

**Tech Stack:** Rust, Cargo tests, XWayland compositor fixtures, conditional tracing.

## Global Constraints

- Never place Cargo `target/`, build artifacts, caches, or temporary compilation output inside `/home/agony/GitHub/Typhon`.
- All compilation, tests, and generated output must use `/mnt/Aether/Desktop/GitHub`.
- Do not remove enqueue-time `resize_epoch` capture.
- Do not globally reorder native XWayland event processing.
- Do not use subagents.
- Preserve unrelated working-tree changes.

---

### Task 1: Add red post-resize move drain regressions

**Files:**
- Modify: `src/compositor/tests/xwayland_resize_visual.rs`
- Modify: `src/compositor/tests/xwayland_resize_visual_ownership.rs`

**Interfaces:**
- Consumes the existing `first_buffer_fixture`, window interaction helpers, `XwmEvent::ResizeSyncPresented`, `XwmEvent::ResizeSyncTimedOut`, and `take_xwayland_backend_commands` APIs.
- Produces failing tests proving that a move configure queued with E1 is currently discarded after E1 retires.

- [ ] **Step 1: Strengthen the presentation regression**

  In `move_after_resize_release_cannot_be_overwritten_by_late_presentation`, assert before event injection that the move queued a `WindowBackendCommand::Configure` with `resizing: false`, the move geometry, and `resize_epoch: Some(resize_epoch)`. Inject `ResizeSyncPresented`, assert the resize epoch is `None`, then drain backend commands and require `XwmCommand::ConfigureFrame` for M with no stale position-only configure.

- [ ] **Step 2: Run the presentation regression and verify the expected red failure**

  Run:

  ```bash
  CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target cargo test move_after_resize_release_cannot_be_overwritten_by_late_presentation -- --exact
  ```

  Expected: the test fails because the queued move is discarded when `current_resize_epoch` is `None`.

- [ ] **Step 3: Add the timeout regression**

  Add `move_after_resize_release_cannot_be_dropped_by_resize_timeout` in the ownership regression module. Reproduce E1 release, queue a move without draining, inject `ResizeSyncTimedOut` for E1, assert canonical and visual geometry are M and the epoch is `None`, then drain and require `XwmCommand::ConfigureFrame(M)`.

- [ ] **Step 4: Run the timeout regression and verify the expected red failure**

  Run:

  ```bash
  CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target cargo test move_after_resize_release_cannot_be_dropped_by_resize_timeout -- --exact
  ```

  Expected: the test fails because the queued move is discarded after timeout retirement.

### Task 2: Implement explicit dequeue ownership policy

**Files:**
- Modify: `src/compositor/server_backend.rs`

**Interfaces:**
- Consumes the existing `WindowBackendCommand::Configure` and `FinalizeResize` variants and `trace_x11_resize_backend_command`.
- Produces the required `XwmCommand` translation policy without changing epoch capture or native event ordering.

- [ ] **Step 1: Split strict resizing handling from non-resizing context handling**

  In `take_xwayland_backend_commands`, first reject `resizing: true` when `resize_epoch` is missing or differs from the current epoch, then translate matching work to `BeginResizeSync`. For `resizing: false`, match `(resize_epoch, current_resize_epoch)`: `None` becomes `ConfigureFrame`, equal `Some` values become position-only `Configure` carrying the stored epoch, stored epoch plus `None` becomes `ConfigureFrame`, and differing `Some` values discard without rebinding.

- [ ] **Step 2: Add conditional trace reasons for each non-resizing decision**

  Emit `position_only_configure` traces with reasons for matching translation, `resize_context_retired` plus `translation=configure_frame` for the fallback, and `newer_resize_epoch_superseded` for discard. Keep strict resize and finalize traces conditional and preserve their existing ownership fields.

- [ ] **Step 3: Run the two regressions and preserved ownership tests**

  Run:

  ```bash
  CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target cargo test move_after_resize_release -- --nocapture
  CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target cargo test xwayland_resize_visual_ownership -- --nocapture
  ```

  Expected: both new move tests and all existing E1→E2 ownership tests pass, including the queued position-only configure discard case.

### Task 3: Verify the focused and full repository state

**Files:**
- No source changes unless formatting or test diagnostics require a focused correction.

**Interfaces:**
- Consumes the implementation and regression tests from Tasks 1–2.
- Produces fresh verification evidence and a focused commit.

- [ ] **Step 1: Verify the effective target directory**

  Run:

  ```bash
  test "$(realpath /mnt/Aether/Desktop/GitHub/Typhon-target)" != "$(realpath /home/agony/GitHub/Typhon/target)"
  ```

- [ ] **Step 2: Run requested focused checks**

  Run the requested `move_after_resize_release`, `xwayland_resize_visual`, `xwayland_resize_visual_ownership`, `resize_sync`, and `window_interaction` Cargo tests with `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target`.

- [ ] **Step 3: Run requested full checks**

  Run `cargo check --all-targets`, `cargo test`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `git diff --check`, and `./bin/check-source-layout`, all with the verified Aether target directory where compilation occurs. Record unrelated baseline failures separately.

- [ ] **Step 4: Review and commit the logical implementation**

  Confirm the diff contains only the dequeue policy, traces, and regressions (plus the pre-existing unrelated modification left untouched), then commit with:

  ```bash
  git add src/compositor/server_backend.rs src/compositor/tests/xwayland_resize_visual.rs src/compositor/tests/xwayland_resize_visual_ownership.rs
  git commit -m "fix(xwayland): preserve moves after resize retirement"
  ```
