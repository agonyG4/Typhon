# Typhon Hybrid Floating/Tiled Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close workspace scene-transition quiescence and add a no-Dwindle Floating/Tiled runtime foundation with exact `Super+V` switching, floating geometry preservation, and layout-derived chrome policy.

**Architecture:** Keep `WorkspaceLocation` and `LayoutMembership` as orthogonal canonical state. Defer pointer refresh across workspace/special membership transitions until the final `ActiveSceneView` exists. Store the last floating `WindowGeometry` on each managed desktop window and switch the sole geometry authority by layout mode without recreating clients.

**Tech Stack:** Rust, Smithay/Wayland compositor state, XWayland, native input binding/action pipeline, Cargo locked tests.

## Global Constraints

- Preserve `WorkspaceLocation { Regular(WorkspaceId), Special(SpecialWorkspaceId) }`.
- Preserve `LayoutMembership { Floating, Tiled }` as an independent axis.
- Do not add Dwindle nodes, split trees, ratios, gaps, timers, threads, locks, polling, or per-frame layout scans.
- Keep `WorkspaceLocation` canonical and `ActiveSceneSelection` derived.
- Do not modify the resize protocol pipeline, `ToplevelVisualGeometry` design, or `MAX_IN_FLIGHT_RESIZE_CONFIGURES`.
- Do not recreate clients during layout transitions.
- Preserve XDG/X11 parentage, EWMH desktop, ext-workspace state, focus, pointer focus, popup ownership, and constraints.
- Preserve all pre-existing dirty working-tree changes; do not reset, restore, stash, clean, or commit.
- Use `rtk` for shell/test commands and `apply_patch` for edits.

---

### Task 1: Close scene-transition pointer ordering

**Files:**
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/compositor/state/workspaces.rs`
- Modify: `src/compositor/state/window_interaction.rs` only if terminal cleanup requires a narrowly scoped hook
- Test: `src/compositor/state/window_interaction_tests.rs`
- Test: `src/compositor/state/task_05_8_tests.rs` or the existing Special transition test module

**Interfaces:**
- Add a compositor-state transition flag or equivalent private guard used by `defer_pointer_focus_refresh`.
- All workspace/special transition entry points must follow: compute selection/membership delta, defer, cleanup, rebuild, repair focus, release, refresh once.

- [ ] Add a test-only observable or existing metric assertion that can distinguish intermediate pointer refresh from the final refresh.
- [ ] Add regressions for Special close during resize, move, pointer constraint, and popup grab; assert terminal cleanup, no stale pointer ownership, and one final pointer target decision.
- [ ] Add the Regular-active-interaction/Special-close regression and assert the Regular interaction remains active.
- [ ] Implement transition-scoped deferral in `CompositorState`; make interaction terminal refreshes and explicit workspace cleanup refreshes no-op while the guard is active.
- [ ] Remove any pre-rebuild `clear_pointer_focus` from workspace switching when the final scene refresh can perform the leave/enter transition.
- [ ] Apply the guard to Special toggle, regular workspace activation, and active-scene workspace-membership transitions.
- [ ] Run `rtk cargo test --locked` with focused filters for `window_interaction`, workspace/Special transition tests, pointer constraints, and popup grabs.

### Task 2: Strengthen fullscreen native-frame planning

**Files:**
- Modify: `src/native_output/tests/special_workspace.rs` or the existing native fullscreen test module
- Inspect only as needed: `src/compositor/state/fullscreen.rs`, `src/native_output/runtime/frame.rs`, `src/native_output/scanout/direct.rs`

**Interfaces:**
- Reuse the existing native frame planning and fullscreen metrics APIs; do not add a predicate-only test in place of frame planning.

- [ ] Build a real Regular fullscreen owner and managed Special application with the current test harness.
- [ ] Assert hidden Special permits solitary fullscreen planning.
- [ ] Open Special and assert fullscreen remains active, Special content is in native frame renderables, `solitary_tree_active == false`, Direct Scanout is rejected, and configure count does not change.
- [ ] Run the focused native Special/fullscreen/direct-scanout tests.

### Task 3: Add layout and chrome policy state

**Files:**
- Modify: `src/wm/window.rs`
- Modify: `src/wm/mod.rs` only if re-export is required by existing conventions
- Modify: `src/compositor/desktop_window.rs`
- Modify: `src/compositor/state/window_decoration.rs` only to expose/use the policy boundary without visual redesign
- Test: `src/lib.rs` or a focused WM test module

**Interfaces:**
- Add `WindowChromePolicy::{Full, Minimal}` (or equivalent) with a total mapping from `LayoutMembership`.
- Add `WindowManagementState::chrome_policy()` derived only from `layout()`.
- Add `DesktopWindow::floating_geometry: Option<WindowGeometry>` initialized to `None`.

- [ ] Write mapping tests proving Floating→Full and Tiled→Minimal for both Regular and Special locations.
- [ ] Implement the policy projection without consulting backend, protocol, workspace, or Special state.
- [ ] Store the floating geometry snapshot on `DesktopWindow`; do not put compositor geometry types into the WM crate.
- [ ] Keep current decoration rendering unchanged except for a clean policy seam ready for later visual policy adoption.
- [ ] Run WM and decoration-focused tests.

### Task 4: Implement runtime Floating↔Tiled transition

**Files:**
- Modify: `src/compositor/state/window_actions.rs` or a focused new state module
- Modify: `src/compositor/state/windows.rs`/`src/compositor/state/xwayland_mode.rs` only where existing geometry helpers must be reused
- Modify: `src/compositor/server.rs`
- Test: `src/compositor/state/desktop_window_tests.rs` and/or `src/compositor/state/task_05_8_tests.rs`

**Interfaces:**
- Add `CompositorState::toggle_focused_window_layout() -> bool`.
- Add `OwnCompositorServer::toggle_focused_window_layout() -> bool`.
- The method returns `false` for no focused normal managed window and `true` only for a layout transition.

- [ ] Write RED tests for Regular Floating→Tiled: location unchanged, client/window ID unchanged, focus unchanged, floating geometry captured.
- [ ] Write RED tests for Special Floating→Tiled and Regular Tiled states without changing workspace manager state.
- [ ] Write RED tests for Tiled→Floating restoring placement and size, preserving focus, and not emitting close/recreate behavior.
- [ ] Implement Floating→Tiled by snapshotting `current_visual_root_window_geometry` (falling back to current root geometry), setting only `LayoutMembership::Tiled`, and leaving placement untouched.
- [ ] Implement Tiled→Floating by setting only `LayoutMembership::Floating`, taking the saved geometry, and applying it through the existing XDG or X11 geometry authority path.
- [ ] For X11, reuse `set_x11_frame_geometry`, visual assignment, and normal backend configure/state queuing; for XDG, reuse existing placement/visual assignment without changing resize-flow limits.
- [ ] Preserve focus, pointer state, popup grab, and constraint state; do not invoke workspace transition cleanup.
- [ ] Mark the toplevel/publication and render generations through existing causes only when state or geometry changes.
- [ ] Run focused layout and geometry tests.

### Task 5: Add exact Super+V input routing

**Files:**
- Modify: `src/native_output/input/events.rs`
- Modify: `src/native_output/input/bindings.rs`
- Modify: `src/native_output/input/state.rs`
- Modify: `src/native_output/input/routing.rs`
- Modify: `src/native_output/tests/input.rs` or add a focused test module

**Interfaces:**
- Add `BindingAction::ToggleFocusedWindowLayout` and `NativeWindowAction::ToggleFocusedWindowLayout`.
- Route the action to `OwnCompositorServer::toggle_focused_window_layout()`.

- [ ] Add `KEY_V` and a default `Super+V` press-only, repeat-disabled, inhibition-respecting binding.
- [ ] Test exact modifiers, press-only behavior, repeat suppression, shortcut inhibition pass-through, and consumed key-event behavior.
- [ ] Map the binding action into the native window action without adding timers or stateful sequences.
- [ ] Apply the server action and propagate visual redraw only through the existing action path.
- [ ] Run focused native input tests and the shortcut-inhibition suite.

### Task 6: Review future-Dwindle compatibility and performance contract

**Files:**
- Inspect and adjust only the focused files above if review finds an assumption that all windows are Floating
- Test: focused WM/compositor/native input tests

- [ ] Verify no code combines `WorkspaceLocation` and `LayoutMembership` into a new enum.
- [ ] Verify no layout toggle mutates EWMH/ext-workspace/parent relationships or calls workspace transition cleanup.
- [ ] Verify no client recreation, timer, thread, lock, polling, or per-frame tiled scan was introduced.
- [ ] Verify future Dwindle can consume Regular and Special locations independently while keeping canonical window identity.
- [ ] Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, and focused tests before final integration.

### Task 7: Final verification and report

**Files:**
- Create: `REPORT-2026-08-23-hybrid-floating-tiled-foundation.md`
- Modify only if required by verification: files from Tasks 1–6

- [ ] Run `rtk git diff --check` and `bin/check-source-layout`.
- [ ] Run focused transition, fullscreen, layout, chrome, input, popup, pointer-constraint, and direct-scanout tests.
- [ ] Run `rtk cargo test --locked`; classify pre-existing environment failures separately and do not weaken tests.
- [ ] Write the report with architecture, preflight fixes, layout/chrome ownership, geometry authority, transition flow, tests, validation, and remaining Dwindle/Dwindle-resize/Dwindle-animation work.
- [ ] Re-check the final diff and dirty-tree preservation; do not commit.
