# XWayland Retained Buffer Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an XWayland window ready when its surface buffer was committed before the XWayland serial association.

**Architecture:** Preserve the existing readiness ownership boundary: the compositor reports real retained content, while the XWM continues to join serials and enforce map, property, association, and buffer gates. On serial commit, the compositor detects retained current content and queues its existing generation-bound buffer-ready event through an idempotent insertion helper.

**Tech Stack:** Rust, Wayland server protocol state, XWayland shell v1, SHM protocol fixtures, Cargo tests.

## Global Constraints

- Do not infer buffer readiness from association alone.
- Do not publish unassigned surfaces.
- Do not change non-XWayland role behavior.
- Do not change Steam configuration or address `SLSTEAM` and `CLOUDREDIRECT` messages.
- Preserve the current Typhon session until a verified release binary is ready.

---

### Task 1: Cover and fix retained pre-association buffer readiness

**Files:**
- Modify: `src/compositor/tests/xwayland.rs`
- Modify: `src/compositor/state/roles.rs`

**Interfaces:**
- Consumes: `CompositorState::commit_xwayland_surface_serial(surface_id) -> Result<XwaylandSurfaceCommit, AssociationError>`, `current_surface_buffers`, and `xwayland.buffer_ready_events`.
- Produces: idempotent `CompositorState::note_xwayland_buffer_ready(surface_id)` behavior that also runs after an XWayland serial commits over retained content.

- [ ] **Step 1: Write the failing protocol regression**

Add a fixture in `src/compositor/tests/xwayland.rs` that binds the private XWayland client, creates a `wl_surface`, commits a real SHM buffer while the surface is unassigned, then creates `xwayland_surface_v1`, sets its serial, and commits without another attachment. Assert:

```rust
assert_eq!(fixture.server.take_xwayland_association_events().len(), 1);
assert_eq!(fixture.server.take_xwayland_buffer_ready_events().len(), 1);
assert!(fixture.server.renderable_surfaces().is_empty());

admit_first_buffer(&mut fixture, 37, 42);
assert_eq!(fixture.server.renderable_surfaces().len(), 1);
assert_eq!(
    fixture.server.renderable_surfaces()[0].buffer_id().get(),
    fixture.initial_buffer_id,
);
```

- [ ] **Step 2: Run the regression to verify RED**

Run:

```bash
cargo test --locked xwayland_buffer_committed_before_serial_becomes_ready -- --nocapture
```

Expected: FAIL because `take_xwayland_buffer_ready_events()` returns zero events after the serial-only commit.

- [ ] **Step 3: Implement minimal retained-content readiness**

In `commit_xwayland_surface_serial`, after the association commit succeeds and `committed_serial` is stored, check `current_surface_buffers.contains_key(&surface_id)` and call `note_xwayland_buffer_ready(surface_id)` when true. Adjust `note_xwayland_buffer_ready` to avoid inserting an existing `(generation, surface_id)` pair:

```rust
let event = (generation, surface_id);
if !self.xwayland.buffer_ready_events.contains(&event) {
    self.xwayland.buffer_ready_events.push(event);
}
```

Keep association state borrows scoped so the current-buffer lookup and event insertion occur after the mutable `surface_states` borrow ends.

- [ ] **Step 4: Run focused regressions to verify GREEN**

Run:

```bash
cargo test --locked xwayland_buffer_committed_before_serial_becomes_ready -- --nocapture
cargo test --locked compositor::tests::xwayland -- --nocapture
WAYLAND_DISPLAY="$WAYLAND_DISPLAY" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
  cargo test --locked x11_window_reaches_window_ready_without_direct_fd_polling -- --nocapture
```

Expected: all focused tests pass; the installed-Xwayland test reaches `WindowReady` through `NativeEventLoop`.

- [ ] **Step 5: Run repository verification and build release**

Run:

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
./bin/check-source-layout
git diff --check
cargo build --locked --release
```

Expected: every command exits zero and `target/release/oblivion-one` is ready before restarting Typhon.

- [ ] **Step 6: Commit the fix**

```bash
git add src/compositor/state/roles.rs src/compositor/tests/xwayland.rs
git commit -m "fix(xwayland): report retained buffers after association"
```

- [ ] **Step 7: Verify Steam natively**

After restarting into the new release binary with managed XWayland enabled, launch Steam while ignoring `SLSTEAM` and `CLOUDREDIRECT` log messages. Confirm its normal main window is `IsViewable`, its XID appears in `_NET_CLIENT_LIST`, and Typhon logs `xwayland_window_admitted` for its surface.
