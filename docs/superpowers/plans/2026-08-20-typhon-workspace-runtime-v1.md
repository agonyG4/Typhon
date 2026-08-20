# Typhon Workspace Runtime v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add runtime workspace switching and focused-family movement while preserving mapped protocol state, geometry, WindowVisual behavior, and existing compositor ownership boundaries.

**Architecture:** Keep `WorkspaceManager` and workspace mutation pure in `src/wm`; keep live windows in the existing compositor `DesktopWindow` registry. Add one active-workspace scene predicate consumed by existing render, hit-test, fullscreen, idle, frame, focus, and commit paths. Route native bindings and XWayland EWMH through their existing typed action/command/event boundaries.

**Tech Stack:** Rust, Wayland/XDG, XWayland/x11rb, libinput evdev bindings, existing compositor render-generation and WindowVisual systems, Cargo with the repository `rtk` wrapper.

## Global Constraints

- Preserve the current dirty worktree; do not reset, restore, checkout, stash, clean, discard, stage unrelated work, or commit.
- Reuse the existing Cargo target directory; never run `cargo clean` or create a fresh build tree/worktree.
- Workspace visibility is not minimization: never use `minimize_desktop_window`, `WindowState::minimized`, X11 unmap, or minimized protocol state for workspace hiding.
- Do not drain or duplicate `renderable_surfaces`; keep one `DesktopWindow` registry and one scene/hit-test authority.
- Do not mutate geometry, `ToplevelVisualGeometry`, `SurfacePlacement`, resize state, configure queues, or layout membership during workspace changes.
- Do not add Dwindle, tiled geometry/chrome, workspace animation, Spatial Canvas, layout transactions, or output-bound workspace identity.
- Use `rtk` for Cargo, formatter, checker, test, clippy, and repository validation commands.
- Every production behavior change follows a failing-first test, then minimal implementation, then focused green verification.

---

### Task 1: Pure workspace runtime API

**Files:**
- Modify: `src/wm/workspace.rs`
- Modify: `src/wm/window.rs`
- Modify: `src/lib.rs` tests
- Test: `src/lib.rs` tests and focused WM tests

**Interfaces:**
- Produce `WorkspaceSwitchOutcome::{Changed, NoChange, UnknownWorkspace}`.
- Produce `WorkspaceManager::activate(WorkspaceId) -> WorkspaceSwitchOutcome`.
- Produce `WindowManagementState::with_workspace(WorkspaceId) -> Self` preserving layout membership.

- [ ] Write failing tests for valid switch, same-workspace no-op, unknown workspace rejection, and workspace mutation preserving Floating and Tiled layout.
- [ ] Run `rtk cargo test --locked workspace -- --nocapture` and confirm the missing API or expected assertion failures.
- [ ] Implement the pure APIs without adding window collections, locks, timers, threads, renderer, or output dependencies.
- [ ] Run the focused WM tests and confirm green.

### Task 2: Central active-workspace visibility policy

**Files:**
- Create: `src/compositor/state/workspaces.rs`
- Modify: `src/compositor/state/mod.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/desktop_window.rs`
- Test: compositor state tests

**Interfaces:**
- Produce one compositor policy for managed-root, root-surface, and arbitrary-surface active-workspace visibility.
- Preserve auxiliary-parent traversal without assigning auxiliary management state.
- Produce one explicit `RenderGenerationCause` for workspace switching and moving.

- [ ] Write failing tests for two overlapping windows on different workspaces, mapped/geometry/mode/layout preservation while hidden, auxiliary inheritance, and layer-shell visibility.
- [ ] Run the focused visibility tests with `rtk cargo test --locked workspace_visibility -- --nocapture` and verify red.
- [ ] Implement O(1)-style visibility helpers over the existing registry and surface/root relationships; do not create a second scene collection.
- [ ] Add active-workspace state accessors and explicit render-generation causes without changing geometry.
- [ ] Run visibility tests and existing desktop-window lifecycle tests.

### Task 3: Scene, hit-test, fullscreen, and decoration integration

**Files:**
- Modify: `src/compositor/render.rs`
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/compositor/state/window_decoration.rs`
- Modify: `src/compositor/state/fullscreen.rs`
- Modify: `src/compositor/state/subsurfaces.rs`
- Modify: relevant compositor tests

**Interfaces:**
- Make existing scene traversal skip inactive application trees while preserving global layer-shell.
- Make `pointer_scene_hit_at` and decoration hit resolution use the same visibility policy.
- Make fullscreen eligibility and native renderable-surface selection require active-workspace ownership.

- [ ] Add failing tests proving hidden client/SSD surfaces are absent from render and pointer hits, and inactive fullscreen does not cull or scan out over the active workspace.
- [ ] Run focused scene/fullscreen/hit-test tests and confirm red.
- [ ] Integrate the predicate at existing scene walk/filter points; do not add a per-frame whole-window scan or second hit-test algorithm.
- [ ] Verify layer-shell remains visible and decoration style is unchanged.
- [ ] Run affected focused tests with `rtk cargo test --locked` filters.

### Task 4: Focus restoration, activation, and input/grab reconciliation

**Files:**
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/surface_focus.rs`
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/pointer_constraints.rs`
- Modify: `src/compositor/state/data_device.rs`
- Modify: `src/compositor/state/shortcut_inhibition.rs`
- Modify: `src/compositor/state/input_dispatch.rs`
- Test: focus, interaction, pointer-constraint, popup-grab, and layer-shell tests

**Interfaces:**
- Produce `CompositorState::switch_workspace(WorkspaceId)` as the single runtime transition.
- Produce `CompositorState::move_focused_window_to_workspace(WorkspaceId)`.
- Keep `focused_window_id` active-workspace-valid at every direct focus path.

- [ ] Add failing tests for focus MRU restoration, empty workspace clearing, minimized-candidate skipping, hidden focus rejection, layer-shell focus preservation, and stale interaction/grab cancellation.
- [ ] Add failing tests for one render generation per switch/family move and zero for no-op operations.
- [ ] Implement one transition transaction: validate, cancel incompatible input state, mutate active workspace, reconcile focus/pointer/fullscreen/idle/publication, advance one generation, request one redraw.
- [ ] Implement family-root resolution and workspace movement preserving geometry, mode, minimized state, and layout membership.
- [ ] Route authorized inactive activation through workspace navigation before focus without weakening security checks.
- [ ] Run focused focus/input/fullscreen/idle tests.

### Task 5: XDG and X11 transient workspace inheritance

**Files:**
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/desktop_windows.rs`
- Modify: `src/compositor/protocols/xdg.rs`
- Modify: `src/xwayland/xwm/event_types.rs`
- Test: `src/compositor/state/desktop_window_tests.rs`, XDG lifecycle tests, X11 transient tests

**Interfaces:**
- Reconcile child management workspace after valid XDG parent changes.
- Reconcile managed X11 dialog/toplevel descendants after transient relationship rebuilds.
- Preserve child layout membership independently from parent layout.

- [ ] Add failing tests for XDG parent inheritance before map, dynamic SetParent, parent removal preservation, X11 dynamic transient inheritance, family movement, and cycle rejection.
- [ ] Implement inheritance at relationship mutation/rebuild boundaries, preserving auxiliary windows without independent management.
- [ ] Run focused XDG and X11 transient tests.

### Task 6: Native number-row workspace bindings

**Files:**
- Modify: `src/native_output/input/bindings.rs`
- Modify: `src/native_output/input/state.rs`
- Modify: `src/native_output/input/routing.rs`
- Modify: native input tests and binding fixtures

**Interfaces:**
- Add `KEY_1` through `KEY_0` constants using existing evdev semantics.
- Add typed `BindingAction` and `NativeWindowAction` variants carrying `WorkspaceId`.
- Route switch and family-move actions to compositor methods.

- [ ] Add failing tests for all ten Super mappings and all ten Super+Shift mappings.
- [ ] Add failing tests for press-only/non-repeat behavior, shortcut inhibition, modifier forwarding, and non-regression of Ctrl+Shift+Alt session commands.
- [ ] Implement default bindings with exact modifiers, `RepeatPolicy::Disabled`, `InhibitionPolicy::Respect`, and non-reserved policy.
- [ ] Implement routing and redraw behavior without leaking consumed Super/Shift events.
- [ ] Run native binding/input focused tests.

### Task 7: XWayland EWMH desktop model and command/event routing

**Files:**
- Modify: `src/xwayland/xwm/atoms.rs`
- Modify: `src/xwayland/xwm/mod.rs`
- Modify: `src/xwayland/xwm/commands.rs`
- Modify: `src/xwayland/xwm/events.rs`
- Modify: `src/xwayland/xwm/event_types.rs`
- Modify: `src/xwayland/xwm/startup.rs`
- Modify: compositor XWayland server/event integration
- Test: XWM atom/startup/event/command tests and compositor XWayland tests

**Interfaces:**
- Add tested WorkspaceId↔EWMH conversion helpers with explicit rejection of invalid and `0xFFFFFFFF` values.
- Add typed root desktop publication and per-window desktop publication commands.
- Normalize valid `_NET_CURRENT_DESKTOP` and `_NET_WM_DESKTOP` client requests into typed events.

- [ ] Add failing pure conversion and publication tests for ten desktops, current desktop, viewport/workarea cardinality, and invalid indices.
- [ ] Add failing command/event tests ensuring compositor policy, not raw X11 parsing, mutates workspace state.
- [ ] Implement atom advertisement only for completed `_NET_WM_DESKTOP` support, preserve client lists, and never unmap workspace-hidden windows.
- [ ] Connect switch/move publication through existing `WindowBackendCommand`/XWM boundaries.
- [ ] Run focused XWayland suites.

### Task 8: Idle, frame callbacks, commit damage, and control snapshots

**Files:**
- Modify: `src/compositor/state/input_dispatch.rs`
- Modify: `src/compositor/state/frame_callbacks.rs`
- Modify: `src/compositor/state/surface_commits.rs`
- Modify: `src/compositor/state/surfaces.rs`
- Modify: `src/compositor/server_control.rs`
- Test: lifecycle, frame, damage, idle, and control snapshot tests

**Interfaces:**
- Hidden application trees are ineffective for idle inhibition and visible output callback completion.
- Hidden commits retain state but avoid unnecessary visible output damage where safe.
- Snapshots remain `mapped: true` and publish the moved workspace number.

- [ ] Add failing tests for hidden/active idle inhibitor effectiveness, mapped snapshot preservation, and hidden commit/damage behavior.
- [ ] Implement visibility checks at existing policy points without faking presentation feedback or breaking client commits.
- [ ] Run focused idle/frame/damage/control tests.

### Task 9: Integration validation, two reviews, and final report

**Files:**
- Create: `REPORT-2026-08-20-typhon-workspace-runtime-v1.md`

- [ ] Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, and `rtk git diff --check`.
- [ ] Run focused WM, visibility, focus, XDG/X11 transient, bindings, pointer, fullscreen, idle, frame, control, and XWayland suites.
- [ ] Attempt `rtk cargo test --locked`; classify environment/pre-existing failures without weakening tests.
- [ ] Run source-layout and clippy checks; classify known pre-existing violations.
- [ ] Perform Review Pass 1 for minimization misuse, geometry/configure changes, focus/visibility invariants, inheritance, fullscreen, hit tests, layer-shell, and EWMH conversions.
- [ ] Perform Review Pass 2 for frame scans, allocations, generations, locks/threads, Dwindle compatibility, extensible workspaces, multi-output neutrality, and hidden-client cadence.
- [ ] Rerun affected tests after review fixes.
- [ ] Write the required English report with baseline, architecture, behavior, evidence, blockers, both reviews, final status, and explicit out-of-scope statements.
