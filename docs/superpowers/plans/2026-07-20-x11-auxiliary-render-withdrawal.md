# X11 Auxiliary Render Withdrawal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove late-classified Steam helper surfaces from rendering and input without destroying their retained XWayland buffer or association.

**Architecture:** Add a reversible compositor-state publication transition next to the existing XWayland adoption path. Resolve the X11 root surface before desktop removal, withdraw its renderable tree with `SurfaceUnmap` damage semantics, and leave final protocol/buffer teardown to existing destroy and disconnect paths.

**Tech Stack:** Rust 2024, Wayland server state, XWayland/XWM, Cargo unit tests.

## Global Constraints

- Do not add Steam-specific class or title matching.
- Do not delay normal X11 window admission with a property-settling timer.
- Preserve `current_surface_buffers`, XWayland association state, roles, and placements.
- Keep override-redirect popup publication unchanged.
- Work directly on `main` under the user's standing approval and commit this fix atomically.

---

### Task 1: Reversible XWayland render withdrawal

**Files:**
- Modify: `src/compositor/state/xwayland_windows.rs`
- Modify: `src/compositor/server.rs`
- Test: `src/compositor/state/xwayland_windows.rs`

**Interfaces:**
- Consumes: `CompositorState::root_surface_id_for_surface(u32) -> u32`, `RenderableSurface`, `RenderGenerationCause::SurfaceUnmap`, and the existing `WindowReady` adoption path.
- Produces: `CompositorState::withdraw_xwayland_surface_content(surface_id: u32) -> bool`.

- [ ] **Step 1: Write the failing render-tree withdrawal test**

Add a `#[cfg(test)]` module to `src/compositor/state/xwayland_windows.rs`. Build one root `RenderableSurface` and one child using SHM snapshots, store `absolute_root_at(10, 10)` for the root and `subsurface(root_id, 1, 1)` for the child, and assert the wished-for API removes both renderables while retaining both placements:

```rust
#[test]
fn xwayland_withdrawal_unpublishes_render_tree_without_forgetting_placement() {
    let mut state = CompositorState::default();
    let root_id = 42;
    let child_id = 43;
    state.renderable_surfaces.push(test_surface(root_id, 10, 10));
    state.renderable_surfaces.push(test_surface(child_id, 1, 1));
    assert!(state.set_surface_placement(
        root_id,
        SurfacePlacement::absolute_root_at(10, 10),
    ));
    assert!(state.set_surface_placement(
        child_id,
        SurfacePlacement::subsurface(root_id, 1, 1),
    ));
    let generation = state.render_generation();

    assert!(state.withdraw_xwayland_surface_content(root_id));

    assert!(state.renderable_surfaces.is_empty());
    assert_eq!(
        state.surface_placement(root_id),
        SurfacePlacement::absolute_root_at(10, 10),
    );
    assert_eq!(
        state.surface_placement(child_id),
        SurfacePlacement::subsurface(root_id, 1, 1),
    );
    assert!(state.render_generation() > generation);
    assert!(!state.withdraw_xwayland_surface_content(root_id));
}
```

The local `test_surface` helper must allocate a `BufferIdentity` and use `CommittedSurfaceBuffer::shm_snapshot`, matching the established helpers in `window_interaction_tests.rs` and `task_05_8_tests.rs`.

- [ ] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test --locked xwayland_withdrawal_unpublishes_render_tree_without_forgetting_placement
```

Expected: compilation fails because `withdraw_xwayland_surface_content` does not exist. This is the intended RED result.

- [ ] **Step 3: Implement minimal render withdrawal**

Add this internal method beside `adopt_current_xwayland_surface_content`:

```rust
pub(in crate::compositor) fn withdraw_xwayland_surface_content(
    &mut self,
    root_surface_id: u32,
) -> bool {
    let withdrawn_ids = self
        .renderable_surfaces
        .iter()
        .filter_map(|surface| {
            (self.root_surface_id_for_surface(surface.surface_id) == root_surface_id)
                .then_some(surface.surface_id)
        })
        .collect::<std::collections::HashSet<_>>();
    if withdrawn_ids.is_empty() {
        return false;
    }

    self.renderable_surfaces
        .retain(|surface| !withdrawn_ids.contains(&surface.surface_id));
    self.invalidate_surface_origin_cache();
    self.reconcile_all_surface_output_memberships();
    self.advance_render_generation(RenderGenerationCause::SurfaceUnmap);
    true
}
```

Do not remove entries from `current_surface_buffers`, `surface_placements`, XWayland association maps, or role maps.

- [ ] **Step 4: Connect withdrawal to XWM desktop removal**

In `OwnCompositorServer::remove_x11_desktop_window`, resolve `root_surface_id` from the desktop window before removing it, then withdraw publication before the existing `remove_desktop_window` call:

```rust
let root_surface_id = self
    .state
    .window(window_id)
    .map(|window| window.root_surface_id);
if let Some(root_surface_id) = root_surface_id {
    let _ = self
        .state
        .withdraw_xwayland_surface_content(root_surface_id);
}
let removed = self.state.remove_desktop_window(window_id).is_some();
```

This shared helper is used by both `WindowWithdrawn` and `WindowDestroyed`; immediate unpublication is correct in both cases, while later surface teardown remains idempotent.

- [ ] **Step 5: Run focused and neighboring tests and verify GREEN**

Run:

```bash
cargo test --locked xwayland_withdrawal_unpublishes_render_tree_without_forgetting_placement
cargo test --locked compositor::state::desktop_window_tests
cargo test --locked compositor::state::window_interaction_tests
cargo test --locked xwayland::xwm
cargo test --locked compositor::render::tests
cargo test --locked native_output::tests::output
```

Expected: every command exits zero; the new regression passes and existing popup, desktop, resize, renderer, and damage behavior remains green.

- [ ] **Step 6: Format, inspect, and commit the atomic fix**

Run:

```bash
cargo fmt --check
git diff --check
git diff -- src/compositor/state/xwayland_windows.rs src/compositor/server.rs
git status --short
```

Then commit only the two implementation files:

```bash
git add src/compositor/state/xwayland_windows.rs src/compositor/server.rs
git commit -m "fix(xwayland): unpublish withdrawn helper surfaces"
```

### Task 2: Full verification and live Steam evidence

**Files:**
- No source changes expected.

**Interfaces:**
- Consumes: the Task 1 release binary and Typhon's isolated `DISPLAY`/`XAUTHORITY` lease.
- Produces: full gate output plus live X11 client-list/map-state evidence.

- [ ] **Step 1: Run the full repository gate**

Run:

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -- --test-threads=1
./bin/check-source-layout
git diff --check
```

Expected: all commands exit zero with no failed tests or denied warnings.

- [ ] **Step 2: Produce a provably fresh release**

Run:

```bash
cargo clean --release -p oblivion-one
cargo build --locked --release
stat -c '%i' target/release/oblivion-one
sha256sum target/release/oblivion-one
```

Expected: release build exits zero and its inode/hash differs from the still-running compositor until restart.

- [ ] **Step 3: Verify the native symptom after restart**

Launch Steam against Typhon's current isolated XWayland lease. Inspect the root client lists and the mapped 10x10 support windows:

```bash
xprop -root _NET_CLIENT_LIST _NET_CLIENT_LIST_STACKING
for support_xid in $(xwininfo -root -tree | awk '/10x10\+10\+10/ {print $1}'); do
    xwininfo -id "$support_xid"
done
```

Expected: the support XID may remain X11-mapped for Steam's internal use, but it is absent from both EWMH client lists and produces no visible or hittable render surface. The main Steam window remains the sole normal desktop client. Opening a Steam menu maps one override-redirect `_NET_WM_WINDOW_TYPE_POPUP_MENU`, which renders normally and disappears on unmap.

Resize is then captured as a separate transaction with `TYPHON_RESIZE_DEBUG=1`; no resize behavior is changed unless that evidence identifies a distinct root cause.
