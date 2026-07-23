# X11 Client Configure Resize Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep client-driven left/top X11 resize requests atomic so Steam does not enter a geometry feedback loop.

**Architecture:** Preserve the complete constrained geometry produced by `x11_configure_request_geometry` for floating managed X11 windows outside compositor-driven interactive resize. Validate both the emitted XWM command and persisted authoritative geometry with a left-edge anchor regression test.

**Tech Stack:** Rust, Typhon compositor state, X11/Xwayland `ConfigureRequest`, Cargo tests

## Global Constraints

- Preserve active compositor-driven resize authority.
- Preserve unrequested X11 geometry fields.
- Do not change stacking or X11 resize-sync behavior.
- Do not commit unrelated dirty-worktree changes.

---

### Task 1: Restore atomic client-requested geometry

**Files:**
- Modify: `src/compositor/tests/xwayland.rs`
- Modify: `src/compositor/server.rs`

**Interfaces:**
- Consumes: `OwnCompositorServer::x11_configure_request_geometry`, `XwmEvent::ConfigureRequested`, `X11ConfigureFlags`
- Produces: an `XwmCommand::Configure` and authoritative X11 geometry that preserve requested x/width as one constrained box

- [x] **Step 1: Write the failing left-edge regression test**

Add a test that admits an X11 window at `(100, 120, 640, 480)`, sends a request with x `120`, width `620`, and fields `x=true`, `width=true`, then checks that the command and persisted geometry are `(120, managed_y, 620, 480)` and that the original right edge remains `740`.

- [x] **Step 2: Run the regression test and verify it fails**

Run: `cargo test x11_client_configure_left_resize_preserves_right_edge -- --exact`

Expected: FAIL because the current uncommitted policy replaces requested x `120` with the authoritative managed x.

- [x] **Step 3: Apply the minimal policy correction**

Remove the non-interactive `ConfigureRequested` override that forcibly copies authoritative x and y into the already constrained requested geometry. Keep the active-resize branch unchanged.

- [x] **Step 4: Restore the partial-field expectation**

In `x11_partial_moveresize_preserves_unrequested_geometry`, expect requested x `200` while retaining the compositor-managed y and the old width and height.

- [x] **Step 5: Run focused and surrounding tests**

Run: `cargo test x11_client_configure_left_resize_preserves_right_edge -- --exact`

Expected: PASS.

Run: `cargo test x11_partial_moveresize_preserves_unrequested_geometry -- --exact`

Expected: PASS.

Run: `cargo test compositor::tests::xwayland`

Expected: all Xwayland compositor tests PASS.

- [x] **Step 6: Run formatting, build, and lint verification**

Run: `cargo fmt --check`, `cargo check`, and the repository's applicable Clippy command.

Expected: all commands exit successfully without new warnings.

- [x] **Step 7: Build the release binary for live validation**

Run: `cargo build --release --bin oblivion-one`

Expected: exit status 0 and an updated `target/release/oblivion-one`.

No commit is included because the worktree contains pre-existing user and agent changes and the user did not request a commit.
