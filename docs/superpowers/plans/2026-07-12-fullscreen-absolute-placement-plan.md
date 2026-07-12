# Fullscreen and Maximized Absolute Placement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make fullscreen and maximized root windows use absolute output-space placement while preserving cascaded placement for floating roots.

**Architecture:** Keep `render::surface_origins` unchanged. Make `CompositorState::fullscreen_window_geometry` and `maximized_window_geometry` produce `RootPlacementMode::Absolute` placements, and make fullscreen exact-cover eligibility inspect that placement contract. Exercise the shared mode transition through server-runtime tests and direct state/render tests where protocol setup would obscure a narrow invariant.

**Tech Stack:** Rust, Cargo, Smithay/Wayland test clients, in-process compositor snapshots, `cargo test`.

## Global Constraints

- Fullscreen roots use `absolute_root_at(0, 0)` and output dimensions.
- Maximized roots use `absolute_root_at(usable.x, usable.y)` and usable-output dimensions.
- Floating roots remain cascaded and the normal renderer cascade formula remains unchanged.
- Do not change CSD or resize production code unless a deterministic regression fails after the placement fix.
- Do not add clipping, direct scanout, opacity, Wayland compliance, multi-output, or renderer redesign work.
- Work directly on the current `main` tree and preserve unrelated work.

---

### Task 1: Add deterministic failing placement regressions

**Files:**
- Modify: `src/compositor/tests/windows.rs`
- Modify: `src/compositor/tests/support/window_ops.rs`
- Modify: `src/compositor/tests/support/registry_state.rs` only if the client request helper lacks state/configure capture needed by the regression.

**Interfaces:**
- Consume existing `OwnCompositorServer`, `ServerCommand`, `LiveTestClient`, `RenderableSurfaceSnapshot`, and registry configure state.
- Produce named regressions that capture root output origins and configured dimensions after the shared mode path.

- [ ] **Step 1: Add later-root fullscreen and maximize tests.** Map at least three roots, transition a nonzero-ordinal root, capture the root snapshot, and assert the desired absolute origin, dimensions, and xdg state. Include focus/raise changes before fullscreen.
- [ ] **Step 2: Add shortcut and client-request fullscreen coverage.** Use the existing native `ToggleFullscreenFocused` command for the shortcut route and extend a live client helper to issue `xdg_toplevel.set_fullscreen`, then assert the same absolute origin and configure state.
- [ ] **Step 3: Add restore, output resize, layer reservation, CSD, and subsurface regressions using existing helpers.** Assert placement mode/local coordinates where snapshots expose them; distinguish xdg window geometry from backing-buffer origin for CSD.
- [ ] **Step 4: Add exact-cover unit coverage for absolute `(0,0)`, cascaded compensating coordinates, absolute nonzero origin, and wrong size.
- [ ] **Step 5: Run the focused test filters before production changes.**

Run:

```bash
cargo test fullscreen -- --nocapture
cargo test maximized -- --nocapture
cargo test window_unmaximize -- --nocapture
```

Expected: newly added origin/eligibility regressions fail because the current stateful placements are cascaded and exact-cover still checks the negative first-surface offset. Existing unrelated tests should continue to pass.

### Task 2: Implement absolute fullscreen and maximized geometry

**Files:**
- Modify: `src/compositor/state/fullscreen.rs`

**Interfaces:**
- Preserve the existing `window_geometry_for_mode` and `set_root_window_mode` interfaces.
- Consume `usable_output_geometry()` as the authoritative maximized rectangle.

- [ ] **Step 1: Change fullscreen geometry to `SurfacePlacement::absolute_root_at(0, 0)` with output dimensions.**
- [ ] **Step 2: Change maximized geometry to `SurfacePlacement::absolute_root_at(usable.x as i32, usable.y as i32)` with usable dimensions.**
- [ ] **Step 3: Change exact-cover eligibility to require output dimensions, `RootPlacementMode::Absolute`, and local `(0,0)`, without changing opacity behavior.
- [ ] **Step 4: Run the focused placement tests and the directly affected compositor tests.**

### Task 3: Verify shared entry paths and state preservation

**Files:**
- Modify production files only if a failing regression demonstrates an active broken path; otherwise no production changes beyond Task 2.
- Modify: `src/compositor/tests/windows.rs` and support helpers as needed for deterministic protocol/shortcut assertions.

- [ ] **Step 1: Confirm `xdg_toplevel.set_fullscreen` delegates to `set_root_window_mode` and passes without a protocol-specific placement branch.**
- [ ] **Step 2: Confirm shortcut fullscreen uses the same final geometry and presentation owner path.**
- [ ] **Step 3:** Confirm unfullscreen and unmaximize restore the exact prior floating size and cascaded local coordinates.
- [ ] **Step 4:** Confirm output resize and exclusive-zone changes keep fullscreen at `(0,0)` while maximized follows usable geometry.

### Task 4: Audit unchanged renderer, resize, and CSD behavior

**Files:**
- No production modifications expected.

- [ ] **Step 1:** Inspect `src/compositor/render.rs` and verify both root-mode branches remain unchanged.
- [ ] **Step 2:** Inspect `src/compositor/state/window_resize.rs` and verify no hardcoded cascaded render placement is used by a proven stateful path; change only if a regression fails.
- [ ] **Step 3:** Inspect CSD geometry snapshots and leave `surface_window_geometries`, `current_visual_root_window_geometry`, and render placement untouched if the CSD regression passes.
- [ ] **Step 4:** Run the required source-layout and `rg` audits.

### Task 5: Full validation and handoff

**Files:**
- No additional source files unless validation exposes a scoped regression.

- [ ] **Step 1:** Run `cargo fmt --check`.
- [ ] **Step 2:** Run `cargo check --all-targets`.
- [ ] **Step 3:** Run `cargo clippy --all-targets -- -D warnings`.
- [ ] **Step 4:** Run `cargo test`.
- [ ] **Step 5:** Run `./bin/check-source-layout` and `git diff --check`.
- [ ] **Step 6:** Check the final diff and report manual native validation honestly; do not claim it if no native session was available.

## Checkpoints

- After Task 1: the new tests fail for the confirmed cascade-offset reason.
- After Task 2: focused fullscreen/maximized tests pass and no CSD/resize production changes are needed.
- After Task 5: all requested validation commands have fresh exit-code evidence.

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Stateful geometry is configured before the client commits its buffer | Snapshot may still show old visual state | Reuse existing configure/roundtrip barriers and capture after server command completion. |
| CSD geometry can obscure root origin | False-positive buffer-origin failure | Assert visible window geometry separately from backing-buffer origin, as required by the task. |
| Existing test helpers do not expose raise/focus or client fullscreen directly | Incomplete entry-path coverage | Extend only the narrow helper/state capture needed for deterministic commands and configure state. |

## Open Questions

None; the user approved the absolute-placement design and required scope.
