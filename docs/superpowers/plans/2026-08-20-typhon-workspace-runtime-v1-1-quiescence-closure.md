# Typhon Workspace Runtime v1.1 — Quiescence Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make inactive workspaces quiescent and close workspace transition correctness gaps without replacing Typhon’s canonical mapped-scene authorities.

**Architecture:** Add an event-driven `ActiveSceneView` derived from canonical mapped renderables and popup state. Centralize surface generation publication and workspace membership transitions so hidden content does not affect active scene generation, damage, feedback, or identity, while real transitions reconcile focus/input and incremental XWayland publication.

**Tech Stack:** Rust, Wayland/XWayland compositor state, existing native output scene/damage pipeline, existing `rtk cargo` workflow, no new dependencies.

## Global Constraints

- Preserve one canonical `WindowId`, `DesktopWindow`, `WorkspaceManager`, and `WindowManagementState`.
- Preserve mapped/alive/stateful hidden windows and the existing `renderable_surfaces` registry.
- Do not modify geometry solely for workspace visibility or implement Dwindle, Hybrid Chrome, or Spatial Canvas.
- Do not add workspace polling, timers, threads, locks, allocator hacks, cargo clean, worktrees, staging, or commits.
- Use `apply_patch` for edits and `rtk` for cargo verification.
- Write tests first and run each new focused test red before implementing its production behavior.

---

### Task 1: Add failing domain and active-scene cache tests

**Files:**
- Modify: `src/wm/workspace.rs`
- Modify: `src/compositor/state/desktop_window_tests.rs`
- Modify: `src/native_output/tests/fullscreen_frame_scene.rs` or the repository-consistent native scene test module
- Test: the new tests in those files

**Interfaces:**
- Produces expected `WorkspaceManager::len()`/`workspace_count()` behavior, extensible EWMH identity conversion, and active-scene diagnostics used by later tasks.

- [ ] **Step 1: Write failing tests** for EWMH index 10 conversion, manager rejection of workspace 11, repeated active-scene resolution with no rebuild/allocation, and active popup filtering.
- [ ] **Step 2: Run the focused tests** with `rtk cargo test --locked workspace -- --nocapture` and the selected native scene test filter; confirm failures are missing behavior rather than test errors.
- [ ] **Step 3: Keep the tests as the executable contract** and do not add production implementation until the expected failures are observed.

### Task 2: Implement `ActiveSceneView` and zero per-frame workspace filtering

**Files:**
- Create: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/state/mod.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/fullscreen.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/compositor/server_frames.rs`
- Modify: `src/native_output/runtime/frame.rs`

**Interfaces:**
- Produces `ActiveSceneView` with stable borrowed surfaces and popup IDs, `rebuild_active_scene_view`, `refresh_active_scene_surface`, `active_scene_surfaces`, `active_scene_popup_surface_ids`, and deterministic test counters.

- [ ] **Step 1: Add the derived cache fields and event-driven helpers** without changing mapped authority.
- [ ] **Step 2: Replace `native_frame_renderable_surfaces`’s workspace `.any().filter().cloned().collect()` path with a borrowed active-scene slice; retain fullscreen-specific culling only where required.
- [ ] **Step 3: Return cached active popup IDs and update `ResolvedNativeFrameScene` to borrow them instead of sorting all popups per frame.
- [ ] **Step 4: Run the Task 1 focused tests and the native frame/scene tests; confirm repeated resolution does not increase rebuild counters.

### Task 3: Centralize hidden surface generation and presentation ownership

**Files:**
- Modify: `src/compositor/state/surfaces.rs`
- Modify: `src/compositor/state/surface_commits.rs`
- Modify: `src/compositor/state/subsurfaces.rs`
- Modify: `src/compositor/state/xwayland_windows.rs`
- Modify: `src/compositor/state/frame_callbacks.rs`
- Modify: `src/compositor/state/frames.rs`
- Modify: `src/compositor/state/surface_transactions.rs`
- Modify: `src/compositor/explicit_sync.rs` only if ownership helpers require visibility-aware access

**Interfaces:**
- Produces one surface publication boundary that separates `render_generation` from active `scene_render_generation`, plus visible-only presentation-feedback collection.

- [ ] **Step 1: Write failing hidden commit-storm, latest-buffer reveal, hidden feedback, and hidden frame-callback tests.
- [ ] **Step 2: Run those tests red and record the current scene-generation/feedback failure.
- [ ] **Step 3: Implement `publish_surface_generation` and route normal, damage-only, XWayland, synchronized-subtree, and bufferless publication through it.
- [ ] **Step 4: Make pending presentation feedback visibility-aware without duplicating feedback objects; preserve discard-on-destroy and retry restoration.
- [ ] **Step 5: Audit scheduler-facing predicates so hidden callbacks/feedback do not request active output work while explicit-sync progress remains enabled.
- [ ] **Step 6: Run focused surface, frame, presentation, explicit-sync, native frame, and damage tests green.

### Task 4: Synchronize active-scene invalidation at map, popup, stacking, minimize, and visual-assignment boundaries

**Files:**
- Modify: `src/compositor/state/desktop_windows.rs`
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/surfaces.rs`
- Modify: `src/compositor/state/subsurfaces.rs`
- Modify: `src/compositor/state/window_resize.rs`
- Modify: `src/compositor/layer_shell.rs`
- Modify: `src/compositor/state/fullscreen.rs`
- Modify: `src/compositor/state/hit_testing.rs`

**Interfaces:**
- Produces event-driven cache updates for map/unmap, popup lifecycle, render order, minimize/restore, placement/visual assignment, and active input/render consumers.

- [ ] **Step 1: Write failing hidden popup identity and active-scene/hit-test agreement tests.
- [ ] **Step 2: Run them red.
- [ ] **Step 3: Add cache rebuild/update calls at the actual lifecycle boundaries and update visible surface clones incrementally for content/visual mutations.
- [ ] **Step 4: Ensure fullscreen, decoration, pointer, and hit-test paths consume the same cached visible membership.
- [ ] **Step 5: Run focused popup, hit-test, fullscreen, direct-scanout, resize-visual, and scene-history tests.

### Task 5: Refactor workspace membership into an atomic planned transition

**Files:**
- Modify: `src/compositor/state/workspaces.rs`
- Modify: `src/compositor/state/desktop_windows.rs`
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/pointer_constraints.rs`
- Modify: `src/compositor/state/input_dispatch.rs`
- Modify: `src/compositor/state/data_device.rs`
- Modify: `src/compositor/state/surface_focus.rs`

**Interfaces:**
- Produces `WorkspaceMembershipTransition` planning/application helpers used by inheritance and family moves, plus ownership-aware transition cancellation.

- [ ] **Step 1: Write failing dynamic XDG/X11 inheritance, inactive-to-inactive move, true no-op family move, unrelated-interaction preservation, and latest-generation tests.
- [ ] **Step 2: Run them red.
- [ ] **Step 3: Implement plan-first membership changes with empty-change early return and changed-window X11 publication only.
- [ ] **Step 4: Route dynamic parent/transient reconciliation through the same transaction and preserve workspace on parent removal.
- [ ] **Step 5: Cancel only affected ownership; route affected interactive resize/move termination through `end_window_interaction_by_id_with_reason`.
- [ ] **Step 6: Reconcile active focus, pointer focus/constraints, idle inhibition, active scene, toplevel dirtiness, and one scene generation only when required.
- [ ] **Step 7: Run the workspace, interaction, resize, focus, pointer-constraint, and XDG relationship suites.

### Task 6: Make layer-shell focus restoration workspace-safe

**Files:**
- Modify: `src/compositor/layer_shell.rs`
- Modify: `src/compositor/state/surface_focus.rs`
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/desktop_window_tests.rs`

**Interfaces:**
- Produces canonical layer/application focus acquisition and restoration through `set_desktop_focus`, with current-workspace eligibility and no hidden `focused_window_id`.

- [ ] **Step 1: Write failing exclusive-layer workspace-switch tests for a populated and empty target workspace.
- [ ] **Step 2: Run them red.
- [ ] **Step 3: Store/clear remembered application focus by active workspace and route layer/application restoration through canonical focus state.
- [ ] **Step 4: Run layer-shell, focus, and workspace tests.

### Task 7: Decouple EWMH identity, manager count, and incremental publication

**Files:**
- Modify: `src/wm/workspace.rs`
- Modify: `src/wm/mod.rs`
- Modify: `src/compositor/state/workspaces.rs`
- Modify: `src/compositor/server_xwayland_events.rs`
- Modify: `src/compositor/server_backend.rs`
- Modify: `src/compositor/window_backend.rs`
- Modify: `src/xwayland/xwm/mod.rs`
- Modify: `src/xwayland/xwm/commands.rs`
- Modify: `src/xwayland/xwm/atoms.rs` only if a clear property command is needed
- Modify: `src/xwayland/xwm/events.rs`
- Modify: `src/xwayland/xwm/startup.rs`

**Interfaces:**
- Produces manager-derived desktop count, pure checked EWMH conversion, root-only switch publication, changed-window membership publication, and no auxiliary fake desktop membership.

- [ ] **Step 1: Write failing EWMH extensibility, incremental switch, changed-window move, and auxiliary admission tests.
- [ ] **Step 2: Run them red.
- [ ] **Step 3: Add `WorkspaceManager::len()`/`workspace_count()` and pure checked conversion.
- [ ] **Step 4: Remove per-window publication from active workspace switching and publish only actual membership changes.
- [ ] **Step 5: Stop auxiliary admission from queuing `SetWorkspace`; add typed clear behavior for managed-to-auxiliary reclassification if required.
- [ ] **Step 6: Run XWayland event/command/startup and compositor backend tests.

### Task 8: Run audits, two reviews, verification, performance evidence, and final report

**Files:**
- Create: `REPORT-2026-08-20-typhon-workspace-runtime-v1-1-quiescence-closure.md`
- Modify: `docs/superpowers/plans/2026-08-20-typhon-workspace-runtime-v1-1-quiescence-closure.md` only for execution checkboxes if useful

- [ ] **Step 1: Run the source-layout checker and classify pre-existing violations.
- [ ] **Step 2: Run `rtk cargo fmt -- --check`, `rtk cargo check --locked --all-targets`, `git diff --check`, all focused suites, and `rtk cargo test --locked`.
- [ ] **Step 3: Perform independent correctness/ownership and performance/latency reviews; fix every task-owned issue and rerun affected tests.
- [ ] **Step 4: Refresh graph coverage for every operated feature path and disclose any parse-partial limitation.
- [ ] **Step 5: Record baseline, dirty status, performance counters, generation deltas, command/configure evidence, test totals, review findings, and explicit out-of-scope confirmation in the required report.

