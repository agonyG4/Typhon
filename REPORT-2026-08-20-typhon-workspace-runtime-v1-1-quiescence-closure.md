# Typhon Workspace Runtime v1.1 — Quiescence Closure

Date: 2026-08-20  
Repository: `/home/agony/GitHub/Typhon`

## 1. Baseline HEAD

`0ef9f7b99fa38d0fc04bf5ffa8f494db5a6eade6`.

## 2. Initial dirty working-tree summary

The supplied checkout was already heavily modified and authoritative. The initial tracked diff was 111 files, 7,065 insertions, and 1,042 deletions. Existing modified and untracked work was preserved; no reset, restore, stash, clean, new worktree, cargo clean, staging, or commit was used.

## 3. Bugs confirmed

The current source confirmed per-frame workspace filtering, hidden commits advancing active-scene generation, hidden feedback/callback work participating in scheduling, non-atomic inherited workspace changes, side effects on same-workspace moves, low-level resize cancellation, workspace-unsafe layer focus restoration, synthetic EWMH membership for auxiliary X11 windows, ten-workspace-coupled EWMH conversion, global workspace publication, and hidden popup IDs contaminating native scene identity.

## 4–5. Mapped scene and active scene

`renderable_surfaces` remains the mapped/live lifecycle authority. A new derived `ActiveSceneView` owns an ordered, workspace-visible `RenderableSurface` snapshot, cached origins, active popup IDs, workspace identity, and test counters. This keeps protocol state intact while allowing native presentation, fullscreen planning, and input to borrow a stable view without per-frame filtering or clone-all allocation.

The view also keeps an ID-to-index map. Ordinary visible commits update one cached surface; tree updates batch replacements and recompute origins at most once when placement/origin inputs changed. Full rebuilds are limited to relevant topology, visibility, workspace, and role events.

## 6. Active-scene invalidation events

The update boundary covers initial state, map/unmap/destroy, minimize/restore, workspace activation and membership moves, XDG/X11 relationship changes, popup map/unmap/relink, stacking/reorder, role visibility changes, and relevant placement changes. Content-only hidden commits update canonical protocol/buffer state without rebuilding the active view.

## 7–9. Generation and hidden publication behavior

`set_render_generation_with_scene_effect` and `publish_surface_generation` separate global compositor generation from active-scene generation. Hidden `SurfaceCommit`, `SurfaceDamage`, XWayland publication, synchronized-subtree publication, and hidden placement/stack changes may advance global state but do not advance `scene_render_generation`, active pointer generation, direct-scanout eligibility, or active scene identity. Visible changes retain existing surface-damage semantics; the damage planner was not globally weakened.

## 10. Popup and native-scene identity

Active popup IDs are cached, sorted once on relevant popup events, and borrowed by native frame resolution. Popup ownership is resolved through `PopupNode.owner_root_id`, so an XDG popup follows its owning workspace while layer-shell-owned popups retain global behavior. Hidden popup map/unmap/content churn does not alter the active native scene snapshot.

## 11–13. Workspace membership transaction

Workspace moves and dynamic inheritance use a plan/apply transition. Desired memberships are resolved with memoized parent-chain traversal, changes are applied together, affected interaction ownership is cleaned up, focus/idle/pointer state is reconciled, the active scene is rebuilt once when required, and the scene generation advances once for the visibility transition.

XDG `xdg_toplevel.set_parent` updates the relationship and runs the same transition machinery. X11 `WM_TRANSIENT_FOR` updates rebuild the transient graph and use the same inherited membership transaction. Neither path fabricates geometry configure traffic solely for workspace membership.

## 14. True no-op family moves

`move_window_family_to_workspace` plans first and returns before cleanup, commands, focus changes, interaction cancellation, or generation changes when every family member is already on the requested workspace.

## 15. Interaction and resize terminal lifecycle

Affected active interactions terminate through `end_window_interaction_by_id_with_reason`, preserving the existing final visual reconciliation and `send_resize_end_configure` lifecycle. Membership is applied before affected resize cleanup so a departing window cannot add a second active-scene transition. Unrelated inactive-window moves do not cancel another workspace’s interaction.

## 16. Layer-shell focus restoration

Layer focus restoration now requires an alive, non-layer, valid toplevel that is workspace-managed, non-minimized, and visible in the newly active workspace. Restoration uses canonical desktop focus state, preserving `focused_window_id`, serial, and publication updates.

## 17–19. XWayland/EWMH behavior

`WorkspaceId::from_ewmh` is a checked identity conversion independent of the default workspace count; `WorkspaceManager` decides whether an ID exists. Runtime root publication uses the live manager count and current desktop. Workspace switches publish only root desktop state; managed X11 moves publish only changed windows.

Auxiliary/non-workspace-managed X11 windows receive no synthetic `_NET_WM_DESKTOP`. Dynamic role changes compare active-scene visibility and refresh the derived view when a managed window becomes auxiliary or vice versa.

## 20. Configure-count evidence

Workspace, XDG relationship, and X11 relationship focused suites pass without membership-only geometry configure failures. The only allowed transition configure is the canonical terminal configure for an already-running interactive resize.

## 21. Scene-generation evidence

The v1.1 hidden-surface test publishes three hidden generations: global render generation advances while active-scene generation remains stable. Activating the hidden workspace advances the scene once and exposes the latest hidden surface generation. Workspace no-op tests preserve the scene generation.

## 22. Active-scene allocation/rebuild evidence

The repeated native-frame test resolves 1,000 frames with `Cow::Borrowed`, a stable active-scene rebuild count, and zero incremental updates. Fullscreen-only culling may still allocate its pre-existing filtered vector; inactive workspace presence alone does not.

## 23. Hidden-commit stress evidence

Focused hidden-pattern tests pass. Hidden commits retain latest buffers, commit sequencing, damage journals, explicit-sync state, frame ownership, and callback/feedback state while avoiding active-scene generation and active-output invalidation. Hidden feedback remains pending until the surface is presentable; it is not falsely marked presented.

## 24. Focused tests

Fresh focused results include:

- workspace: 15 passed;
- pointer scene: 5 passed;
- active scene: 2 passed;
- v1.1 task tests: 20 passed;
- frame: 226 passed;
- presentation feedback: 9 passed;
- fullscreen: 53 passed;
- direct scanout: 32 passed;
- damage: 94 passed;
- XDG: 74 passed;
- layer surface: 16 passed;
- scene history: 10 passed;
- interaction: 100 passed;
- auxiliary X11 membership: 1 passed;
- EWMH workspace identity/publication checks: 1, 1, and 1 passed;
- isolated native resize test: still fails in the pre-existing dirty resize path because the test drags inward from the configured 160×120 minimum and correctly skips an unchanged target; the full suite run did not report it as a failure.

## 25. Full-suite result

Fresh `rtk cargo test --locked`: **1,688 passed, 35 failed, 2 ignored**.

## 26. Pre-existing/environment failures

All 35 full-suite failures originate from the environment’s Unix-domain socket path limit: `InvalidInput: path must be shorter than SUN_LEN`. The dependent XWayland `PoisonError` failures are lock cascades from that setup failure. No task-owned assertion failure appeared in the full-suite output.

## 27. Review Pass 1 — correctness/ownership

The independent review identified frame claims over all mapped surfaces, teardown callback ownership, and workspace resize termination. These were fixed by limiting FIFO/commit-timing claims to active-scene surfaces, scrubbing callbacks on explicit surface teardown while preserving existing client-disconnect behavior, and applying membership before canonical terminal interaction cleanup.

## 28. Review Pass 2 — performance/latency

The independent audit identified popup ownership resolution, stale pointer-hit cache ordering, resize transition coalescing, repeated active-tree origin recomputation, hidden pending-work scans, and O(N²) inheritance reconciliation. The fixes are the owner-root popup lookup, pointer-generation invalidation during active-scene rebuilds, membership-before-cleanup ordering, batched origin updates with an ID index, visibility counters refreshed at event boundaries, and memoized parent-chain resolution. No runtime benchmark was run.

## 29. Final git status

The checkout remains intentionally dirty and uncommitted. Final status contains 151 modified/untracked entries, including the pre-existing user work plus this report, the v1.1 design/plan documents, and the active-scene implementation. No files were staged or committed. Final `git diff --check` is clean; `rtk cargo fmt -- --check` and `rtk cargo check --locked --all-targets` pass.

The codebase-memory coverage check for all operated-on v1.1 source paths reported `metadata_match`, `no_recorded_issue`, full index mode, and generation match. This remains a best-effort coverage signal rather than proof of source completeness.

## 30. Explicit scope confirmation

This closure did not implement Dwindle, Hybrid Chrome/Floating-Tiled behavior, or Spatial Canvas. The resulting architecture leaves workspace runtime state separate from those future feature tasks.
