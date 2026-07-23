# X11 Visible Placement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent managed X11 windows from being allocated entirely outside the usable output when collision-free placement is impossible.

**Architecture:** Clamp cascade candidates to the usable output before overlap testing. Preserve collision avoidance when possible and fall back to the first visible candidate when every candidate overlaps.

**Tech Stack:** Rust, Cargo tests, Typhon compositor state.

## Global Constraints

- Preserve client-positioned X11 roles.
- Preserve unrelated working-tree changes.
- Do not commit without explicit user authorization.

---

### Task 1: Bound managed X11 placement

**Files:**
- Modify: `src/compositor/state/desktop_window_tests.rs`
- Modify: `src/compositor/state/desktop_windows.rs`

**Interfaces:**
- Consumes: `CompositorState::usable_output_geometry`, `cascaded_root_position`, and existing desktop-window frame geometry.
- Produces: `allocate_managed_frame` placements whose origins remain inside the usable output.

- [x] **Step 1: Write the failing regression test**

Add a test that sets a `1920x1080` output, occupies the initial cascade area with an XDG window, inserts a `1898x1013` managed X11 window, and asserts its frame remains within the usable output. Enter fullscreen and restore floating mode, then assert the visible placement is preserved.

- [x] **Step 2: Verify the regression fails**

Run: `cargo test compositor::state::desktop_window_tests::managed_x11_placement_stays_visible_when_overlap_is_unavoidable -- --exact --nocapture`

Expected: FAIL because the unchecked fallback has a `y` coordinate beyond the output bottom.

- [x] **Step 3: Implement the minimal placement correction**

Clamp each candidate origin against the usable output and retain the first clamped candidate as the overlap fallback. Return that fallback if the search finds no collision-free candidate.

- [x] **Step 4: Verify focused and neighboring behavior**

Run the focused regression, `cargo test compositor::state::desktop_window_tests`, and `cargo test compositor::tests::xwayland`.

- [x] **Step 5: Verify and build**

Run `cargo fmt --check`, `git diff --check`, `cargo test`, and `cargo build --release`.
