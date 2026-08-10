# M7 Native Geometry Regression Fix Implementation Plan

> For agentic workers: REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** Make normal XDG window geometry persistent and independent of activation, focus, stacking, close, and restore order.

**Architecture:** Keep CompositorState::surface_placements as the authoritative placement store. Allocate each initial XDG cascade position with the bounded, occupancy-aware initial-frame allocator, store it as SurfacePlacement::absolute_root_at, and never derive an already-managed XDG root origin from renderable_surfaces or window_stacking. Preserve stacking mutations and subsurface parent-relative placement as separate state dimensions; managed X11 continues using the same generic allocator without routing XDG through X11 protocol semantics.

**Tech Stack:** Rust, Smithay/Wayland compositor state, existing render-plan helpers, deterministic Rust tests, Markdown qualification ledger.

## Global Constraints

- Work directly on main at C:/Users/vitor_crispim/Downloads/Typhon/Typhon.
- Starting HEAD is 211dfe835d1d6d6faf449e7a0239d6f099945e6e; preserve the twelve pre-existing modified bin/ files.
- Do not create branches or worktrees; do not reset, clean, amend, squash, detach HEAD, rewrite history, or modify Eclipse.
- Do not change protocol XML, M7-B action protocol, capability authentication, focus policy, pointer capture, resize ownership, or raise behavior.
- All Markdown documentation is English.
- Do not derive persistent geometry from WindowId ordering or current Z-order.
- Native desktop qualification remains pending until the user reruns the real interaction.

## Architecture Note (current tree before edits)

1. Initial XDG toplevel placement is currently left as SurfacePlacement::root() in state/windows.rs::register_toplevel_surface; no absolute XDG position is stored when the desktop window is inserted.
2. Current normal-root origins are calculated by render::surface_origins() -> root_surface_ordinals() -> root_surface_origin_for_ordinal() in render.rs. RootPlacementMode::CascadedWindow maps the root's current renderable-surface ordinal through cascaded_root_position().
3. Stacking is stored in CompositorState::window_stacking; render order is stored in renderable_surfaces and rebuilt by reorder_renderable_surfaces_by_committed_stack() / reorder_renderable_surfaces_by_window_stack().
4. Move updates persistent placement through set_surface_placement_with_cause() in state/subsurfaces.rs; XDG move/resize paths ultimately store placement in surface_placements.
5. Resize updates root placement and visual geometry through the existing window_resize.rs and toplevel_visual_geometries paths; this plan does not alter those paths.
6. Minimize removes renderable trees into WindowState and restore extends the stored surfaces back; the existing surface_placements entry remains the geometry authority.
7. Managed X11 already allocates and persists SurfacePlacement::absolute_root_at() in desktop_windows.rs and keeps frame geometry in DesktopWindow::x11_geometry.
8. The defect is: XDG CascadedWindow placement consumes mutable render/stack order, while raise_root_window() and raise_window_id() mutate that order.

## File Map

- Modify src/compositor/state/desktop_windows.rs: allocate and persist one bounded initial absolute XDG placement using current occupancy and usable output geometry.
- Modify src/compositor/state/windows.rs: stop resetting a newly inserted XDG window to dynamic SurfacePlacement::root().
- Modify src/compositor/state/desktop_window_tests.rs: add focused stacking/geometry, 100-cycle raise, close-without-reflow, and new-window placement tests.
- Modify docs/M7_QUALIFICATION_STATUS.md: record the observed native click/raise failure, deterministic fix result, and pending native retest.
- Create this plan file.

### Task 1: Add the failing production-path regression

**Files:**
- Modify: src/compositor/state/desktop_window_tests.rs

**Interfaces:**
- Consume existing CompositorState::insert_desktop_window, surface_placement, raise_window_id, remove_desktop_window, and desktop_window_frame seams.
- Produce normal_window_geometry_is_independent_of_stacking_order plus close/new-window coverage.

- [ ] Step 1: Write the failing test.

Create three DesktopWindow::new_xdg windows with CompositorState::allocate_window_id(), insert them, capture each desktop_window_frame, assert the initial stack is A/B/C, raise A then B, and assert stack transitions B/C/A and C/A/B while all frames remain identical. Add a 100-iteration alternating raise_window_id(A/B) loop with zero-drift assertions. Remove B and assert A/C frames are unchanged, then insert D and assert A/C remain unchanged while D receives a distinct initial placement.

~~~rust
#[test]
fn normal_window_geometry_is_independent_of_stacking_order() {
    let mut state = CompositorState::new(None);
    let a = state.allocate_window_id().expect("A window id");
    let b = state.allocate_window_id().expect("B window id");
    let c = state.allocate_window_id().expect("C window id");
    for (window_id, surface_id) in [(a, 401), (b, 402), (c, 403)] {
        state.insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
            .expect("insert XDG window");
    }

    let initial = [a, b, c].map(|id| state.desktop_window_frame(id).expect("frame"));
    assert!(state.raise_window_id(a));
    assert_eq!(state.window_stacking, vec![b, c, a]);
    assert_eq!(initial, [a, b, c].map(|id| state.desktop_window_frame(id).expect("frame")));
    assert!(state.raise_window_id(b));
    assert_eq!(state.window_stacking, vec![c, a, b]);
    assert_eq!(initial, [a, b, c].map(|id| state.desktop_window_frame(id).expect("frame")));

    for index in 0..100 {
        assert!(state.raise_window_id(if index % 2 == 0 { a } else { b }));
        assert_eq!(initial, [a, b, c].map(|id| state.desktop_window_frame(id).expect("frame")));
    }
}
~~~

- [ ] Step 2: Run the focused test and verify the failure is the coupling.

Run:

~~~text
cargo test --locked normal_window_geometry_is_independent_of_stacking_order -- --exact --test-threads=1
~~~

Expected before the production change: failure because an XDG frame falls back to cascaded_root_position() using its current window_stacking ordinal.

- [ ] Step 3: Add close/new-window assertions beside the primary regression.

Use remove_desktop_window(b) after capturing A/C, then insert fresh XDG D. Assert A/C frames are unchanged and D's absolute placement is not equal to either survivor's placement. Do not assert a new smart-placement algorithm.

- [ ] Step 4: Run the focused test again and retain the red result.

~~~text
cargo test --locked normal_window_geometry_is_independent_of_stacking_order -- --exact --test-threads=1
~~~

Record the expected failing assertion before changing production code.

### Task 2: Persist bounded initial XDG placement and remove the dynamic reset

**Files:**
- Modify: src/compositor/state/desktop_windows.rs
- Modify: src/compositor/state/windows.rs

**Interfaces:**
- Consume existing render::cascaded_root_position() and surface_placements.
- Produce SurfacePlacement::absolute_root_at() for each newly inserted XDG desktop window, while keeping X11 placement unchanged.

- [ ] Step 1: Remove the unbounded XDG placement ordinal. Initial XDG placement must be derived from current occupancy, not from lifetime create count or Z-order.

- [ ] Step 2: Generalize the existing managed-frame allocator as allocate_initial_frame(). Use the current usable output geometry, persistent desktop-window frames, bounded cascade candidates, first visible non-overlapping candidate, and a visible distinct fallback. Use the same geometry allocator for compositor-managed X11 frames without introducing X11 protocol behavior into XDG insertion.

- [ ] Step 3: The existing insertion path must store the returned XDG placement through set_surface_placement_with_cause().

- [ ] Step 4: Remove the unconditional set_surface_placement(surface_id, SurfacePlacement::root()) from register_toplevel_surface(). Insertion already installs the persistent placement before the XDG role maps.

- [ ] Step 5: Run the focused regression green.

~~~text
cargo test --locked normal_window_geometry_is_independent_of_stacking_order -- --exact --test-threads=1
~~~

Expected: PASS, with activation/raise changing stack order only.

### Task 3: Verify neighboring XDG, X11, resize, mode, popup, and activation behavior

**Files:**
- Modify: src/compositor/state/desktop_window_tests.rs only if a narrowly scoped assertion is needed.

- [ ] Step 1: Run focused existing filters.

~~~text
cargo test --locked desktop_window_tests -- --test-threads=1
cargo test --locked windows:: -- --test-threads=1
cargo test --locked xwayland_focus -- --test-threads=1
cargo test --locked xwayland_root_stack -- --test-threads=1
~~~

- [ ] Step 2: Confirm coverage for initial placement, raise/focus, move, resize, minimize/restore, fullscreen/maximized, popup/subsurface root-relative geometry, shell activation, and managed X11 absolute frames. Add only missing assertions beside their current fixtures; do not add a new binary.

- [ ] Step 3: Inspect the diff for forbidden coupling. Verify render::surface_origins() still resolves relative subsurfaces from the root, RootPlacementMode::Absolute is consumed directly, and no raise/stack function writes surface_placements.

### Task 4: Update the native qualification ledger

**Files:**
- Modify: docs/M7_QUALIFICATION_STATUS.md

- [ ] Step 1: Add an M7 Integrated Native Qualification section stating click focus + raise initially failed because normal XDG geometry was coupled to stack ordinal. Record the deterministic fix and regression result after testing, and explicitly state Native retest: PENDING.

- [ ] Step 2: Keep the milestone table honest. Do not change any native DEFERRED or pending status to PASS; deterministic repository results are not a real desktop retest.

### Task 5: Run repository gates and report evidence

**Files:** No additional source files.

- [ ] Step 1: Run the locked repository gates.

~~~text
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -- --test-threads=1
./bin/check-source-layout
git diff --check
~~~

If the Windows host cannot execute the Linux Typhon toolchain, report each unavailable command as NOT RUN — HOST ENVIRONMENT; never convert missing output into a pass.

- [ ] Step 2: Run committed-range whitespace validation.

~~~text
git log --check 211dfe835d1d6d6faf449e7a0239d6f099945e6e..HEAD
~~~

- [ ] Step 3: Recheck branch, HEAD, status, and diff stat, preserving the pre-existing modified bin/ files.

~~~text
git branch --show-current
git rev-parse HEAD
git status --short --branch
git diff --stat
~~~

The final report must list starting HEAD, final HEAD, branch, added commits if any, and unrelated pre-existing edits.

## Self-Review Checklist

- [ ] Root cause names renderable_surfaces / stack order -> root ordinal -> CascadedWindow origin.
- [ ] Initial XDG placement uses a bounded occupancy-aware allocator and reuses released space.
- [ ] Persistent geometry remains in surface_placements; no second geometry database exists.
- [ ] Raise, focus, hover focus, minimize/restore, close, and new-window insertion do not reflow survivors.
- [ ] Move and resize retain existing authoritative placement paths.
- [ ] Managed X11 remains absolute and protocol-specific.
- [ ] Popups/subsurfaces remain root-relative.
- [ ] The primary regression is named normal_window_geometry_is_independent_of_stacking_order.
- [ ] Native retest is explicitly pending.
