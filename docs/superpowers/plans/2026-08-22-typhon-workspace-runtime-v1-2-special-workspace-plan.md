# Typhon Workspace Runtime v1.2 + Special Workspace v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining workspace-runtime quiescence debt and add a typed Special Workspace overlay on the same event-driven ActiveScene authority, while preserving canonical WM membership, existing interaction lifecycles, and the repository's O(1) steady-state rendering assumptions.

**Architecture:** `WindowManagementState` owns canonical `WorkspaceLocation` plus orthogonal layout. `WorkspaceManager` owns the regular workspace set and the visible-special toggle. `ActiveSceneSelection` is derived presentation state containing the active regular workspace and optional visible special workspace. `SceneWorkIndex` is a typed, event-maintained index (`Global` or `Location(WorkspaceLocation)`) for pending callbacks, feedback, and scheduler work. Global/output-owned surfaces remain independent of workspace selection; auxiliary surfaces in a managed application tree inherit the canonical managed root's scene location.

**Tech Stack:** Rust, Smithay/Wayland compositor state, XWayland EWMH, native-output input routing, existing compositor test harnesses, Cargo locked builds.

## Global Constraints

- The existing dirty working tree is authoritative. Preserve all unrelated modified, deleted, and untracked files; never reset, restore, stash, clean, or overwrite them.
- Do not create a worktree, commit, branch, pull request, or other repository integration artifact. Leave task changes uncommitted for review.
- Use `/home/agony/GitHub/Typhon` as the working directory and use `rtk` for repository search, reads, diffs, and Cargo/test commands where available.
- Execute the work as one integrated runtime closure: finish and verify quiescence before enabling Special Workspace behavior, then reuse the finalized scene/index APIs.
- Special Workspace is not `WorkspaceId(11)`, is not in the regular workspace list, does not increment `_NET_NUMBER_OF_DESKTOPS`, and has no ext-workspace-v1 handle.
- `WorkspaceLocation` is canonical WM membership. `ActiveSceneSelection` is derived presentation/input selection. Neither may substitute for the other.
- Truly global/output-owned surfaces (including layer-shell and cursor surfaces) are `SceneWorkOwner::Global`. Managed auxiliary surfaces have no independent location and inherit scene visibility from their canonical managed root.
- Any canonical relationship change (XDG parentage, X11 `WM_TRANSIENT_FOR`, or equivalent) must migrate derived scene ownership atomically with the existing workspace membership transition.
- Remove the ambiguous numeric `workspace()` API. Use `location()`, `regular_workspace()`, and `special_workspace()` explicitly; never reinterpret Special as the active regular workspace.
- Workspace location changes must not issue geometry configures, unmap/remap, minimize, recreate, rescale, dim, blur, or animation side effects.
- Do not implement Dwindle or a tiled layout engine; retain the existing layout membership and geometry behavior.
- Preserve canonical subsurface/window ordering, input/grab lifecycles, buffer ownership, fullscreen mode, and existing KMS/O(1) steady-state constraints.
- Every production change follows TDD: add a focused failing test, run it and record the expected failure, add the smallest implementation, rerun, then refactor only after green.
- Before relying on a structural claim about an existing source file, query codebase-memory and run `check_index_coverage` for every operated-on existing file. If coverage reports a gap, inspect that source range directly before proceeding.

## Phase 0: Baseline and execution ledger

- [ ] Confirm the graph project and generation remain `home-agony-GitHub-Typhon`, indexed/ready, generation 2026-08-22 or newer.
- [ ] Capture the current dirty status with `rtk git status --short` and do not compare task results against a clean-tree assumption.
- [ ] Read the approved design at `docs/superpowers/specs/2026-08-22-typhon-workspace-runtime-v1-2-special-workspace-design.md` and this plan before editing source.
- [ ] Run the baseline checks without changing files: `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, and the narrow workspace/frame test filters that compile in the current tree. Record pre-existing failures separately from task failures.
- [ ] Maintain a short execution ledger in commentary, identifying the current phase, the focused failing test, the green command, and any pre-existing failure. Do not add a repository report unless the user asks for one.

## Phase 1: Typed WM domain and regular/special manager state

### Task 1.1: Add typed special identity and location

Files:

- Add `src/wm/special_workspace.rs`.
- Modify `src/wm/window.rs` and `src/wm/mod.rs`.
- Migrate domain assertions in `src/lib.rs` and workspace-specific test modules.

Interfaces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SpecialWorkspaceId(NonZeroU32);

impl SpecialWorkspaceId {
    pub const DEFAULT: Self = Self(NonZeroU32::new(1).expect("non-zero"));
    pub const fn new(raw: u32) -> Option<Self>;
    pub const fn get(self) -> u32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WorkspaceLocation {
    Regular(WorkspaceId),
    Special(SpecialWorkspaceId),
}

impl WindowManagementState {
    pub const fn location(self) -> WorkspaceLocation;
    pub const fn regular_workspace(self) -> Option<WorkspaceId>;
    pub const fn special_workspace(self) -> Option<SpecialWorkspaceId>;
    pub const fn with_location(self, location: WorkspaceLocation) -> Self;
}
```

TDD sequence:

- [ ] Add tests proving `SpecialWorkspaceId::new(0)` is rejected, `DEFAULT` is stable, IDs and locations have the required copy/equality/order/hash behavior, and regular/special plus floating/tiled combinations remain representable.
- [ ] Run `rtk cargo test --locked special_workspace` and confirm compilation/test failure because the type and accessors do not exist.
- [ ] Implement the types and replace the stored `workspace: WorkspaceId` with `location: WorkspaceLocation`.
- [ ] Run the focused tests and then migrate every `workspace()`/`with_workspace()` call to an explicit accessor or `with_location` operation. A regular-only caller must fail loudly or use `Option`, never fall back to active regular.
- [ ] Run `rtk rg "\.workspace\(\)|with_workspace|WorkspaceId::new\(11" src` and remove all production uses of the ambiguous API and numeric-special convention.

### Task 1.2: Extend `WorkspaceManager` without polluting regular workspaces

Files:

- Modify `src/wm/workspace.rs` and `src/wm/mod.rs`.
- Add manager tests beside the existing workspace tests or in `src/lib.rs` if that is the established module boundary.

Interfaces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialWorkspaceToggleOutcome {
    Opened { id: SpecialWorkspaceId },
    Closed { id: SpecialWorkspaceId },
}

impl WorkspaceManager {
    pub const fn visible_special_workspace(&self) -> Option<SpecialWorkspaceId>;
    pub fn toggle_special_workspace(&mut self, id: SpecialWorkspaceId)
        -> SpecialWorkspaceToggleOutcome;
    pub fn set_visible_special_workspace(&mut self, id: Option<SpecialWorkspaceId>);
}
```

- [ ] Add failing tests that a manager with ten regular workspaces still reports ten, the active regular workspace is unchanged when Special opens/closes, a regular switch while Special is visible leaves Special visible, and toggling the same typed ID is open/close rather than a regular activation.
- [ ] Run the manager-focused test filter and observe the expected missing API/failing assertions.
- [ ] Implement the optional visible-special field and typed outcome. Keep `workspaces()` and workspace publication regular-only.
- [ ] Run all domain workspace tests and check that no numeric special value is accepted by EWMH conversion or regular membership validation.

## Phase 2: ActiveScene selection and explicit scene ownership

### Task 2.1: Replace workspace-derived presentation with `ActiveSceneSelection`

Files:

- Modify `src/compositor/state/active_scene.rs`, `src/compositor/state/workspaces.rs`, `src/compositor/state/mod.rs`, and `src/compositor/state/surfaces.rs`.
- Add focused tests in `src/compositor/state/active_scene_tests.rs` and register them in the state test module.

Interfaces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveSceneSelection {
    pub regular: WorkspaceId,
    pub special: Option<SpecialWorkspaceId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneWorkOwner {
    Global,
    Location(WorkspaceLocation),
}

impl CompositorState {
    pub fn active_scene_selection(&self) -> ActiveSceneSelection;
    pub fn scene_work_owner_for_surface(&self, surface: WlSurface) -> SceneWorkOwner;
    pub fn scene_work_owner_for_window(&self, window_id: WindowId) -> SceneWorkOwner;
}
```

- [ ] Add a failing regression test with regular workspace 1 active, a regular window on 1, a regular window on 2, and a special window hidden. Assert the cached active scene contains only the regular-1 tree and no test-only fallback exposes all renderables when the scene is empty.
- [ ] Add failing tests that a visible special scene includes special roots in addition to active regular roots, while a hidden special remains mapped/alive/buffered and is excluded from presentation/input.
- [ ] Run the focused ActiveScene tests and observe the old fallback/regular-only implementation fail.
- [ ] Store the selection in `ActiveSceneView`, rebuild only on selection/order/membership events, and remove the `cfg(test)` fallback that returns all renderables for an empty active scene.
- [ ] Make visibility resolution explicit: managed regular compares `WorkspaceLocation::Regular` to `selection.regular`; managed special compares `WorkspaceLocation::Special` to `selection.special`; a hidden special never matches regular. Do not make auxiliary surfaces independently managed.
- [ ] Add explicit owner classification for global layer-shell/cursor/output surfaces and managed auxiliary surfaces. Resolve XDG parent/transient roots through the canonical relationship index before deciding ownership.
- [ ] Keep `ActiveSceneView` as the sole presentation/input cache; callers such as hit-testing, fullscreen, layer-shell focus, and frame batching consume cached IDs/origins instead of partitioning `renderable_surfaces` each frame.

### Task 2.2: Atomically migrate ownership when canonical relationships change

Files:

- Modify `src/compositor/state/workspaces.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/state/windows.rs`, `src/compositor/desktop_window.rs`, and any relationship/index module identified by graph tracing.
- Update parentage/transient tests in `src/compositor/state/desktop_window_tests.rs`.

- [ ] Add failing tests for: child insertion inheriting a regular location, child insertion inheriting Special, an XDG parent move from regular to special, an X11 transient-owner move, and removing a parent without leaving a stale owner/index entry.
- [ ] Run the focused relationship tests and confirm the old workspace-only or stale derived-state behavior fails.
- [ ] Refactor the existing `WorkspaceMembershipTransition` so each canonical location mutation produces one atomic transition containing old/new membership and the corresponding scene-owner migration. Apply membership, relationship indexes, SceneWorkIndex entries, Astrea dirty state, and active-scene invalidation as one state transition.
- [ ] Preserve layout membership, minimized state, geometry, buffers, and constraints across all location moves. Add assertions that no geometry/configure command is queued by a location-only change.
- [ ] Re-run all desktop-window inheritance tests, including existing auxiliary X11 tests, and verify global/output surfaces never receive `WorkspaceLocation`.

## Phase 3: Quiescence-first indexed callback, feedback, and scheduler work

### Task 3.1: Add event-maintained `SceneWorkIndex`

Files:

- Add `src/compositor/state/scene_work.rs`.
- Modify `src/compositor/state/mod.rs`, `src/compositor/mod.rs`, `src/compositor/state/frame_callbacks.rs`, and `src/compositor/state/frames.rs`.
- Update frame fixture setup in `src/compositor/state/frame_tests.rs` and related test modules.

Data model:

```rust
#[derive(Default)]
pub(crate) struct SceneWorkIndex {
    callbacks_by_owner: HashMap<SceneWorkOwner, HashSet<ObjectId>>,
    feedback_by_owner: HashMap<SceneWorkOwner, HashSet<ObjectId>>,
    callback_owner: HashMap<ObjectId, SceneWorkOwner>,
    feedback_owner: HashMap<ObjectId, SceneWorkOwner>,
    pending_prepare_by_owner: HashMap<SceneWorkOwner, usize>,
}
```

Required operations are `insert_callback`, `remove_callback`, `insert_feedback`, `remove_feedback`, `set_prepare_work`, `migrate_owner`, `has_visible_callback`, `has_visible_feedback`, `has_visible_prepare_work`, and exact visible extraction. The index is an optimization/cache and must be rebuilt or asserted during relationship transitions; it is never a second source of canonical membership.

- [ ] Add unit tests for global work, regular work, special work, owner migration, exact removal, double removal, and hidden-only work not satisfying active-scene readiness.
- [ ] Run `rtk cargo test --locked scene_work` and observe failure because the index does not exist.
- [ ] Implement typed owner buckets and exact ID maps. Keep global work visible regardless of active workspace/special selection; evaluate location owners through the current `ActiveSceneSelection` only at the event boundary.
- [ ] Change callback and feedback queue/discard paths to update the index at insertion, terminal completion, discard, restore, and owner migration. Preserve user modifications in `frame_callbacks.rs`.
- [ ] Add debug/test invariant checks that every queued callback/feedback has exactly one owner entry and every owner entry points to an existing queued resource or documented frame batch.

### Task 3.2: Eliminate full-vector take/repartition scans

Files:

- Modify `src/compositor/state/frame_callbacks.rs`, `src/compositor/state/frames.rs`, and frame-batch restore/discard paths.
- Update tests that directly populate pending vectors so they use queue helpers or refresh the index explicitly.

- [ ] Add failing instrumentation tests proving `take_visible_pending_frame_callbacks()` and `take_visible_pending_presentation_feedbacks()` do not inspect/repartition hidden resources, and preserve hidden resources exactly.
- [ ] Run the instrumentation tests and observe the old `mem::take` plus full scan behavior fail.
- [ ] Implement exact-owner extraction: remove only visible/global IDs from indexed buckets, preserve hidden IDs in place, and return visible resources without allocating proportional to the total hidden backlog. Keep callback/feedback counts exact.
- [ ] Fix discard accounting so discarding hidden callbacks/feedback does not decrement visible counters; discard only subtracts resources that were counted visible at admission. Cover mixed visible/hidden batches and repeated discard.
- [ ] Ensure frame-batch restore, retry, terminal completion, and surface destruction all update vector/map/index state consistently.

### Task 3.3: Make frame-prepare readiness indexed/event-driven

Files:

- Modify `src/compositor/state/frames.rs`, `src/compositor/state/surface_pacing.rs`, `src/compositor/state/surface_tree_readiness.rs`, `src/compositor/state/commit_timing_runtime.rs`, and the queue/mutation modules found by graph tracing (`surface_commits.rs`, `subsurfaces.rs`, `xdg_lifecycle.rs`, `shutdown.rs` as applicable).
- Add/extend focused scheduler tests in `src/compositor/state/frame_tests.rs`.

- [ ] Add failing tests where hidden active-FIFO barriers, hidden explicit-sync work, and hidden surface-tree transactions coexist with no active-scene work. Assert `has_pending_frame_prepare_work()` is false and does not resolve visibility repeatedly.
- [ ] Add failing tests where a hidden item becomes visible through a workspace/special/relationship transition and readiness becomes true immediately after the transition, without a polling scan.
- [ ] Run the focused scheduler tests and record the old repeated scans as the expected failure/performance regression.
- [ ] Add owner metadata/index entries when active FIFO, explicit sync, and surface-tree work is admitted; remove/migrate them on consume, cancel, relationship change, and visibility transition. Keep readiness predicates for resource-specific state but perform them only for indexed candidates.
- [ ] Make `has_pending_frame_prepare_work`, `has_pending_explicit_sync_work`, `has_unowned_frame_work`, and callback/feedback readiness read indexed counts/sets. No per-frame partition/filter/sort/parent walk/all-vector visibility scan remains on the steady-state path.
- [ ] Add one atomic `reconcile_scene_work_for_transition` call after canonical membership/relationship changes, then assert index consistency in debug/test builds.

## Phase 4: Quiescence scene-order and generation closure

### Task 4.1: Separate visible scene order from hidden stacking changes

Files:

- Modify `src/compositor/state/subsurfaces.rs`, `src/compositor/state/active_scene.rs`, `src/compositor/state/surfaces.rs`, and `src/compositor/state/layer_shell.rs` if cached global IDs are needed.
- Extend `src/compositor/state/task_05_8_tests.rs` or add a narrowly scoped scene-order test module.

- [ ] Add failing tests for a hidden-only committed-stack reorder: active-scene generation, pointer-hit generation, rebuild count, and repaint scheduling must remain unchanged.
- [ ] Add failing tests for an active visible reorder: exactly one active-scene order update and one scene generation advance occur; repeated no-op reorder does nothing.
- [ ] Run those tests and observe the current `any(surface_is_visible...)` scene-effect bug fail.
- [ ] Make `refresh_active_scene_surface_order()` return a typed outcome (`Unchanged`, `HiddenOnly`, `VisibleOrderChanged`) based on cached active IDs/order, not on the mere presence of any visible surface in the global list.
- [ ] Advance scene generation only for `VisibleOrderChanged`; preserve global stack/subsurface semantics and ensure layer-shell/cursor order changes are classified as global/output effects where required.
- [ ] Prove repeated native frame scene resolution uses cached IDs, performs no per-frame allocation/parent walk, and does not rebuild when neither selection nor visible order changed.

### Task 4.2: Quiescence gate and review

- [ ] Run all focused quiescence tests: callbacks, feedback, scheduler readiness, discard accounting, hidden reorder, empty-scene strictness, and scene-index migration.
- [ ] Inspect `rtk git diff -- src/compositor/state/frame_callbacks.rs ...` for accidental changes to unrelated dirty code.
- [ ] Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, and the focused test suite. Do not begin Special Workspace implementation until these pass or any remaining failures are proven pre-existing and documented.
- [ ] Review the call graph for `surface_is_visible_in_active_workspace`, `refresh_active_scene_surface_order`, and `take_visible_pending_*`; every remaining caller must use cached/typed APIs and no old numeric workspace fallback may remain.

## Phase 5: Special Workspace overlay runtime

### Task 5.1: Add special toggle, family moves, admission, and publication dirtiness

Files:

- Modify `src/compositor/state/workspaces.rs`, `src/compositor/server.rs`, `src/compositor/state/desktop_windows.rs`, `src/compositor/state/windows.rs`, `src/compositor/desktop_window.rs`, and `src/compositor/state/surface_focus.rs`.
- Add `src/compositor/state/special_workspace_tests.rs` and register it in `src/compositor/state/mod.rs`; update existing desktop-window tests.

- [ ] Add failing tests for `toggle_default_special_workspace`, regular-to-special family move, special-to-current-regular family move, child/transient family canonicalization, and no-op move of an already-correct family.
- [ ] Assert every move preserves `LayoutMembership`, minimized state, committed geometry, buffer identity, constraints, and interaction lifecycle; assert no configure/unmap/remap command is generated.
- [ ] Add failing tests that opening/closing Special changes derived selection and scene caches, while regular manager membership/list/count and active regular identity remain unchanged.
- [ ] Run focused tests and record failure against the regular-only manager and membership implementation.
- [ ] Implement typed toggle/move outcomes and route all mutations through the atomic membership/SceneWorkIndex transition from Phase 2. Mark Astrea publication dirty for membership moves without changing protocol shape.
- [ ] Make new managed windows default to active regular; new children inherit the canonical root location; global/output surfaces remain global.

### Task 5.2: Implement overlay scene order without per-frame sorting

Files:

- Modify `src/compositor/state/active_scene.rs`, `src/compositor/state/subsurfaces.rs`, `src/compositor/state/workspaces.rs`, `src/compositor/layer_shell.rs`, and any renderable-surface cache module.
- Extend special-scene tests.

- [ ] Add failing order tests for regular app surfaces below special app surfaces, special app surfaces below layer-shell Top/Overlay, canonical subsurface ordering within each family, and global cursor/layer behavior independent of workspace.
- [ ] Add failing tests that toggling Special does not create independent WorkspaceLocation for popups/auxiliary app surfaces; they inherit the canonical root's owner.
- [ ] Run focused order tests and observe old global-stack/regular-only behavior fail.
- [ ] Build the cached `ActiveSceneView` order from typed owner/layer buckets at selection/order events. Preserve existing stack position and subsurface sequence inside each bucket. Do not sort or allocate in native frame traversal.
- [ ] Keep Special mapped and stateful while hidden; opening/closing only changes derived selection and scene visibility, not application lifecycle.

## Phase 6: Focus, input, activation, grabs, and fullscreen

### Task 6.1: Focus policy and pointer/keyboard ownership

Files:

- Modify `src/compositor/state/windows.rs`, `src/compositor/state/surface_focus.rs`, `src/compositor/layer_shell.rs`, `src/compositor/state/workspaces.rs`, and interaction modules identified by graph tracing (`window_interaction.rs`, `window_resize.rs`, `pointer_constraints.rs`, `hit_testing.rs`, `input_dispatch.rs`).
- Add/extend special focus and interaction tests.

- [ ] Add failing tests for best special focus by `last_focus_serial` then topmost order, close-special restoration to best regular, exclusive layer retaining actual keyboard focus, regular focus behind visible Special remaining valid, and exposed regular click not auto-hiding Special.
- [ ] Add failing tests that switching locations cancels only pointer/keyboard/grab ownership whose canonical root leaves the active scene; unaffected global or regular ownership survives.
- [ ] Run focused tests and observe old workspace-only focus/cancellation behavior fail.
- [ ] Implement focus selection against cached `ActiveSceneSelection`/surface IDs. Keep exclusive layer focus authoritative; if no special app is focusable, use regular fallback without changing Special visibility.
- [ ] Route grab cancellation through owner-aware transition deltas, preserving existing terminal resize lifecycle and avoiding broad global cancellation.

### Task 6.2: Authorized activation and fullscreen/scanout safety

Files:

- Modify `src/compositor/protocols/activation.rs`, `src/compositor/state/fullscreen.rs`, `src/native_output` scene selection call sites, and relevant tests.

- [ ] Add failing activation tests for an authorized hidden-special token: open the owning Special, then focus/raise the surface; unauthorized activation behavior remains unchanged.
- [ ] Add failing fullscreen tests that regular fullscreen remains its mode, visible Special prevents regular direct-scanout/solitary culling, and special fullscreen is composed safely.
- [ ] Run focused tests and observe current visibility-only activation/fullscreen eligibility fail.
- [ ] Implement activation as an explicit typed selection transition followed by focus, preserving authorization checks and avoiding geometry/configure side effects.
- [ ] Make direct-scanout eligibility conservatively reject whenever visible Special app content exists, while retaining normal regular fullscreen mode/state and cached scene traversal.

## Phase 7: Native input bindings and action routing

Files:

- Modify `src/native_output/input/events.rs`, `src/native_output/input/bindings.rs`, `src/native_output/input/state.rs`, and `src/native_output/input/routing.rs`.
- Update `src/native_output/tests/input.rs`, `src/native_output/tests/input_shortcut_inhibition.rs`, and `src/native_output/tests/input_interaction_liveness.rs`.

- [ ] Add failing binding tests for exact `Super+S` press-only default-special toggle, `Super+Shift+S` press-only family move, repeat suppression, exact modifiers, shortcut inhibition, and no leaked consumed key events. Use evdev `KEY_S = 31`.
- [ ] Add failing routing tests that typed toggle/move actions reach server state and trigger one redraw/scene update, while regular numeric workspace actions retain existing behavior.
- [ ] Run the focused input tests and observe missing actions/bindings fail.
- [ ] Add `NativeWindowAction::ToggleDefaultSpecialWorkspace` and `MoveFocusedWindowToOrFromSpecialWorkspace` (or equivalent explicit typed variants), `BindingAction` variants, defaults, and routing. Preserve the existing ledger/consumption/repeat/inhibition model.
- [ ] Verify no numeric workspace action is overloaded for Special and no key-release/repeat event leaks to clients.

## Phase 8: XDG, X11, EWMH, control, and ext-workspace integration

### Task 8.1: XDG and X11 location protocol behavior

Files:

- Modify `src/compositor/server_xwayland_events.rs`, `src/compositor/window_backend.rs`, `src/compositor/server_backend.rs`, `src/xwayland/xwm/mod.rs`, `src/xwayland/xwm/commands.rs`, `src/xwayland/xwm/event_types.rs`, and `src/xwayland/xwm/events.rs`.
- Update X11/EWMH tests in `src/xwayland/xwm/events.rs` and `src/compositor/tests/xwayland*` modules.

- [ ] Add failing tests for typed `ClearWorkspace` removing `_NET_WM_DESKTOP`, regular-to-special admission clearing the property, special-to-regular setting the EWMH index, and an EWMH move request for a Special window mapping to a valid regular workspace.
- [ ] Add failing tests that Special toggle never publishes a root `_NET_CURRENT_DESKTOP` change and regular workspace count remains unchanged.
- [ ] Run focused X11 tests and observe absent command/event behavior fail.
- [ ] Add `WindowBackendCommand::ClearWorkspace`, `XwmCommand::ClearWorkspace`, command-kind/primary-handle support, and execution that deletes `_NET_WM_DESKTOP`. Use explicit regular/special accessors in admission and request handling.
- [ ] Keep EWMH current-desktop and ext-workspace-v1 publication regular-only; reject/translate invalid special desktop indices without fallback reinterpretation.

### Task 8.2: Control snapshot and publication consistency

Files:

- Modify `src/compositor/server_control.rs`, `src/native/control_tests.rs`, `src/compositor/workspace_protocol.rs`, and any toplevel publication module that exposes workspace labels.

- [ ] Add failing control tests for regular numeric workspace, Special label `"special"`, mapped `true` for hidden Special, and global/auxiliary surfaces using their canonical visibility semantics.
- [ ] Add failing publication tests that membership moves mark Astrea dirty, regular protocol count/current state remain stable, and no ext-workspace handle is made for Special.
- [ ] Run focused control/protocol tests and observe current `management.workspace().to_string()` behavior fail for Special.
- [ ] Implement explicit label/visibility mapping and preserve existing regular publication format; do not redesign protocols.

## Phase 9: Integrated review and performance qualification

- [ ] Run graph verification for all changed existing source files and inspect any incomplete/partial coverage ranges directly.
- [ ] Use `rtk rg` to prove there are no production `workspace()` calls, `WorkspaceId(11)` conventions, independent auxiliary `WorkspaceLocation` assignments, or Special-to-active-regular fallbacks.
- [ ] Use `rtk rg` and targeted source inspection to prove every canonical relationship mutation calls the atomic membership plus SceneWorkIndex migration path.
- [ ] Add or update instrumentation assertions for: no hidden callback/feedback full scan, no repeated visibility parent walks in frame preparation, no hidden-only scene generation advance, stable cached scene rebuild counts, and no steady-state native frame allocations caused by workspace selection.
- [ ] Review global/output-owned surfaces separately from managed auxiliary surfaces: layer-shell/cursor remain global; application popups/transients/subsurfaces inherit root location.
- [ ] Review input, activation, fullscreen, EWMH, control, and ext-workspace behavior against the approved design artifact line by line.
- [ ] Fix task-owned findings before final verification; do not broaden the implementation into Dwindle/tiled layout or protocol redesign.

## Phase 10: Final verification and handoff

- [ ] Run `rtk cargo fmt --check`.
- [ ] Run `rtk cargo check --locked --all-targets`.
- [ ] Run focused tests for WM domain, ActiveScene, SceneWorkIndex, frame quiescence, special workspace, input, X11, control, fullscreen, and interaction behavior.
- [ ] Run the complete suite with `rtk cargo test --locked`.
- [ ] Run `rtk git diff --check` and inspect the complete task diff, explicitly confirming pre-existing dirty changes remain intact.
- [ ] Classify any environment/pre-existing failures with the exact command and output; do not call them passing.
- [ ] Leave all changes uncommitted and report the implementation, verification evidence, known limitations, and preserved dirty files. Link the design and plan using absolute paths.
