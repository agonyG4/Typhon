# Typhon Special Workspace v1.1 Final Closure Report

## 1. Baseline HEAD

`9d3fb34b45f6ce4ffc4582c3231e220b3643e959`

## 2. Initial dirty-tree summary

The checkout was intentionally dirty before this closure. The initial status
contained two deleted historical reports, broad existing modifications across
WM, compositor, input, XWayland, native output, and O1-adjacent code, plus the
existing Special Workspace/quiescence report, O1 study, design/plan documents,
`scene_work.rs`, and `special_workspace.rs` as untracked artifacts. All of that
work was preserved; no reset, restore, stash, clean, cargo clean, worktree, or
commit was performed.

## 3. Current architecture discovered

The runtime uses canonical `WindowId`, `WorkspaceId`, `SpecialWorkspaceId`, and
`WorkspaceLocation::{Regular, Special}` membership. `WindowManagementState`
keeps WM location orthogonal to `LayoutMembership::{Floating, Tiled}`.
`WorkspaceManager` owns active regular selection and optional visible Special
selection. `ActiveSceneView` is derived presentation/input state. The typed
`SceneWorkIndex` is derived readiness bookkeeping. Layer-shell and cursor/output
surfaces remain global; managed auxiliary application surfaces inherit from the
canonical managed root through parent/transient ownership.

## 4. Confirmed bugs before implementation

- Composited fullscreen treated a regular fullscreen owner as solitary while a
  visible Special application tree was active, so native frame filtering could
  remove Special content.
- Special close changed selection and rebuilt presentation without running the
  canonical terminal interaction cleanup for disappearing Special owners.
- Auxiliary X11 roots with no independent management state could receive the
  regular scene band despite inheriting Special visibility.
- Callback-only classification used global explicit-sync and surface-tree
  collection emptiness.
- Empty Special selection changes always advanced scene render generation.
- Special presentation ordering had pushed `subsurfaces.rs` over its source
  layout limit, and the new Special native input test pushed `input.rs` over
  its limit.

## 5. Fullscreen solitary-tree root cause

`fullscreen_render_plan_metrics()` only checked fullscreen coverage, minimized
state, and popups. It did not ask whether application content ordered above the
fullscreen owner survived in the active scene. `native_frame_renderable_surfaces()`
then filtered to the fullscreen owner tree and layer overlay roots.

## 6. Final fullscreen composition rule

The focused `has_visible_application_content_outside_fullscreen_owner()`
predicate examines the cached ActiveScene ordering. It finds the fullscreen
owner and asks whether any non-layer application tree appears above it. This
naturally includes visible Special application trees without making a
Special-specific culling branch or treating Special as layer-shell. Metrics
and native frame filtering use the same `solitary_tree_active` result.

`culled_surface_count` and `wallpaper_culled` are now false/zero when the
solitary filter is not actually applied.

## 7. Direct Scanout relationship

Direct Scanout remains conservative. Its Special-content blocker now resolves
application ownership through the canonical scene owner helper, so auxiliary
Special roots cannot bypass the existing rejection. Composited fullscreen and
Direct Scanout therefore both preserve visible Special application content,
while fullscreen protocol state remains unchanged.

## 8. Special scene-selection transition design

Special toggle captures the old `ActiveSceneSelection`, changes only the
`WorkspaceManager` selection, captures the new selection, identifies departing
owners, performs terminal cleanup, rebuilds ActiveScene once, refreshes idle /
pointer / layer focus, and advances scene generation only if the derived visual
scene changed. WM `WorkspaceLocation` membership is not substituted with
selection state.

## 9. Departing-window ownership calculation

For every desktop window, the transition walks the existing canonical
parent/transient owner relationship and compares its owner location against the
old and new selections. Only `visible-before && hidden-after` IDs are passed to
hide-time cleanup. Auxiliary IDs are included so their root surfaces, grabs,
and pointer state are covered; global/output-owned surfaces have no managed
owner and are not included.

## 10. Resize/move terminal cleanup

Special close uses `end_window_interaction_by_id_with_reason()` rather than
clearing interaction state. An active resize therefore applies the existing
visual placement and queues its canonical terminal resize configure exactly
once. Move interactions use the same terminal lifecycle and release path.
The existing `MAX_IN_FLIGHT_RESIZE_CONFIGURES` bound is unchanged.

## 11. Pointer/grab cleanup

Departing roots clear applicable pointer constraints, implicit grabs, popup
grabs, held-button ownership, and last pointer press through the existing
transition cleanup. Pointer focus is refreshed afterward. A regular interaction
survives Special close when its canonical owner is not departing. Exclusive
layer-shell focus is preserved and recomputed instead of being cleared with a
stale Special application focus.

## 12. Auxiliary canonical-owner scene-band fix

`renderable_root_stack_key()` now derives the application scene band from
`scene_work_owner_for_window()`. A Special root and its auxiliary X11 tree both
receive the Special application band; a regular auxiliary tree remains regular.
Layer-shell Background/Bottom stay below applications and Top/Overlay stay
above them. No auxiliary receives an independent `WorkspaceLocation`.

## 13. Callback-only visibility fix

`has_only_pending_surface_frame_callbacks()` now requires visible callbacks and
uses `has_pending_frame_prepare_work()` for the prepare-work decision. That
helper queries the typed visibility-aware SceneWorkIndex, while visible resize,
color, and presentation-feedback guards remain in place. Hidden explicit-sync
and surface-tree work no longer changes active callback-only classification.

## 14. SceneWorkIndex role

SceneWorkIndex remains bookkeeping only. It is rebuilt at existing event
boundaries from canonical pending callbacks, feedback, explicit-sync, surface
tree, and pacing structures. It does not own protocol objects, callbacks,
transactions, fences, or surface lifetimes. Canonical relationship and
membership transitions rebuild the index atomically with the existing derived
scene refresh.

## 15. Empty-Special generation fix

An empty Special open or close changes selection bookkeeping but leaves ordered
active surface IDs, popups, and origins unchanged, so scene generation delta is
zero. A populated Special open or close changes active surface membership and
advances scene generation exactly once. Hidden-only reorder remains zero.

## 16. ActiveScene update/result semantics

`rebuild_active_scene_view()` now returns `ActiveSceneUpdate` with independent
`selection_changed` and `visual_scene_changed` fields. The latter compares
ordered active surface IDs, popup IDs, and cached origins at the derived
presentation boundary. Callers do not reconstruct scene identity themselves.

## 17. Source-layout regressions fixed

- Presentation root/tree ordering was extracted into
  `src/compositor/state/scene_order.rs`; `subsurfaces.rs` is now 1326 lines.
- The Special native binding regression moved to
  `src/native_output/tests/special_workspace.rs`; `input.rs` is now 1987
  lines.

The source-layout checker still reports seven unrelated historical violations:
`src/compositor/tests/windows.rs`, `src/compositor/state/desktop_windows.rs`,
`src/compositor/state/windows.rs`, `src/compositor/mod.rs`,
`src/compositor/server.rs`, `src/native_output/runtime/bootstrap.rs`, and
`src/xwayland/xwm/events.rs`.

## 18. Focused tests added

- `visible_special_application_blocks_solitary_fullscreen_culling`.
- `empty_special_selection_change_does_not_advance_scene_generation`.
- `populated_special_selection_changes_scene_generation_once_each_direction`.
- `active_scene_update_separates_selection_from_visual_change`.
- `auxiliary_x11_scene_band_inherits_canonical_special_or_regular_owner`.
- `callback_only_ignores_hidden_prepare_work_but_rejects_visible_prepare_work`.
- `closing_special_ends_departing_resize_through_terminal_lifecycle`.
- `closing_special_does_not_cancel_unrelated_regular_interaction`.
- The existing Special binding assertions are preserved in the new focused
  native test module.

## 19. Configure-count evidence

The departing Special resize regression observed exactly one
`FinalizeResize` backend command and one post-terminal pointer refresh. Empty
Special toggles produced no backend geometry command. No XDG geometry,
`ConfigureWindow`, `ToplevelVisualGeometry`, or `SurfacePlacement` mutation was
added for ordinary Special selection or fullscreen composition.

## 20. Scene-generation evidence

- Empty Special open: delta `0`.
- Empty Special close: delta `0`.
- Populated Special open: delta `1`.
- Populated Special close: delta `1`.
- Hidden-only stack reorder: delta `0`.
- Repeated native frame resolution continues to borrow the stable ActiveScene
  cache without rebuild churn.

## 21. Scheduler/quiescence evidence

The callback-only regression passes with one visible callback and hidden typed
prepare work, and fails callback-only when prepare work is assigned to the
active regular owner. The active predicate contains no global explicit-sync or
surface-tree emptiness check. Hidden commit queues remain parked in their
existing canonical structures and continue through the existing event-driven
protocol paths.

## 22. Full validation results

Passed:

- `rtk cargo fmt --check`.
- `rtk cargo check --locked --all-targets`.
- `rtk git diff --check`.
- Focused fullscreen: 54 passed.
- Focused Direct Scanout: 32 passed.
- Focused Special: 19 passed.
- Focused SceneWork: 2 passed.
- Focused window interaction: 66 passed.
- Focused frame: 229 passed.
- Focused layer-shell: 51 passed.
- Focused workspace: 23 passed.
- Focused native Special binding: 1 passed.
- Full locked test attempt: 1761 passed, 36 failed, 2 ignored.

## 23. Pre-existing/environment failures

The full suite failures are environmental/pre-existing and were not hidden or
weakened. Thirty-five failures are the repository's SUN_LEN/path-length
environment failures in Astrea discovery and XWayland bootstrap/lease tests;
the resulting poisoned display-test lock accounts for their cascading failures.
One cursor-persistence lock replacement assertion expected `Err(Insecure)` but
observed `Err(Busy)` in the full parallel run. The cursor test was rerun alone
and passed. No native DRM/KMS qualification is claimed.

## 24. Review Pass 1 — correctness / ownership

The review checked fullscreen and scanout agreement, actual native surface
selection, terminal resize/move lifecycle, stale pointer/grab state, unrelated
regular interaction, focus fallback, layer-shell authority, auxiliary owner
resolution, scene bands, geometry/configure invariants, and Special protocol
identity. It found the exclusive layer-shell focus edge described in section
11; the fix preserves the layer surface while clearing stale application focus.
Affected tests were rerun successfully.

## 25. Review Pass 2 — hot path / future Tiled

The review checked hidden-work scans, duplicate ActiveScene rebuilds and scene
generations, allocations, source layout, per-frame fullscreen behavior, SceneWorkIndex
authority, locks/threads/timers, geometry coupling, and future per-location
Tiled/Dwindle compatibility. The scene-band parent walk is confined to
event-driven ActiveScene/order rebuilds; native frame filtering uses cached
ActiveScene surfaces. No O1/KMS code, layout tree, workspace thread, timer, or
lock was introduced. No task-owned issue remained after the review.

## 26. Final git status

The working tree remains intentionally dirty and includes all pre-existing
tracked modifications, deletions, and untracked artifacts, plus the v1.1
design, plan, report, scene-order module, focused native test module, and
closure implementation/test changes. Nothing was staged or committed.

## 27. Scope confirmation

Hybrid Floating/Tiled runtime policy remains the current foundation. Hybrid
Tiled/Dwindle v1, split nodes, ratios, tiled resize/movement, gaps, chrome
policy, and all other Dwindle/layout features remain explicitly unimplemented.
