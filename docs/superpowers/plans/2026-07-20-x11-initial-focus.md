# X11 Initial Focus Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make newly mapped focusable X11 toplevels such as Steam Settings and Friends become active and stack above the current main window.

**Architecture:** Reuse `CompositorState::focus_desktop_window` immediately after successful X11 admission and retained-content publication. Existing role classification remains the authority: normal/dialog windows focus and raise; auxiliary roles reject focus.

**Tech Stack:** Rust, Wayland/Xwayland compositor state, XWM command tests, Cargo.

## Global Constraints

- Do not add Steam-specific window matching.
- Do not focus auxiliary popup, notification, override-redirect, or support roles.
- Preserve existing activation-request validation.
- Route activation through the existing backend command queue.

---

### Task 1: Focus newly admitted normal X11 windows

**Files:**
- Modify: `src/compositor/server.rs`
- Test: `src/compositor/tests/xwayland.rs`

**Interfaces:**
- Consumes: the `WindowId` returned by `CompositorState::insert_x11_window`.
- Produces: a focused/raised normal X11 window and queued `XwmCommand::Focus { window: Some(handle), timestamp: 0 }`.

- [ ] **Step 1: Add failing normal and auxiliary admission tests**

Using `first_buffer_fixture`, admit a normal `fake_snapshot` and assert `take_xwayland_backend_commands(0)` contains `Focus { window: Some(snapshot.handle), .. }`. In a second fixture, set `window_type` to `PopupMenu`, admit it, and assert no focus command targets its handle.

- [ ] **Step 2: Verify RED**

```bash
cargo test --locked --lib x11_window_ready_initial_focus
```

Expected: the normal-window assertion fails because admission does not focus the new window.

- [ ] **Step 3: Implement minimal admission focus**

Bind the successful `insert_x11_window` result as `window_id`. After `adopt_current_xwayland_surface_content`, call `focus_desktop_window(window_id)`. Keep client-list/family command construction unchanged.

- [ ] **Step 4: Run focused verification**

```bash
cargo test --locked --lib x11_window_ready_initial_focus
cargo test --locked --lib compositor::tests::xwayland
cargo test --locked --lib compositor::state::desktop_window_tests
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
git diff --check
```

Expected: all commands exit zero without warnings.

- [ ] **Step 5: Commit**

```bash
git add src/compositor/server.rs src/compositor/tests/xwayland.rs
git commit -m "fix(xwayland): focus newly mapped toplevels"
```

### Task 2: Full gate and live stacking validation

**Files:**
- Verify: full repository
- Build: `target/release/oblivion-one`

- [ ] **Step 1: Run full verification**

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -- --test-threads=1
./bin/check-source-layout
git diff --check
```

- [ ] **Step 2: Clean-build release and compare hashes**

```bash
cargo clean --release -p oblivion-one
cargo build --locked --release
sha256sum target/release/oblivion-one
```

- [ ] **Step 3: Validate Steam dialogs after restart**

Open Settings and Friends. Confirm both are `IsViewable`, each newly opened dialog becomes `_NET_ACTIVE_WINDOW`, the dialog is last in `_NET_CLIENT_LIST_STACKING`, and the Xwayland PID remains stable.
