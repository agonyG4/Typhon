# Agent Kernel: Resize and Window State Follow-up

Date: 2026-06-11

Scope: read-only investigation of resize correctness around pending resize
commits, xdg configure ACK serials, resize preview lifetime, minimized hidden
commits, subsurfaces/popups during resize, and focus/pointer retargeting.

Non-goals:

- Do not edit production source code in this task.
- Do not change protocol, CLI, JSON, service, or shell contracts.
- Do not redesign native presentation; this report only calls out interactions
  that can affect resize correctness.

## Summary

The resize pipeline is better than the earlier Gecko resize notes imply. The
current compositor has explicit ACK serial tracking, coalesced resize configures,
and a render-side `ResizePreview` path that prevents stale committed buffers from
being upscaled while the client has not committed the requested size yet.

The main remaining risks are in cross-state behavior:

- a pending resize can survive into hidden/minimized state and then be consumed
  by a later hidden commit;
- `resize_preview` is cleared on any buffer commit for that surface, but there is
  no focused test that proves stale preview state is cleared at the right time
  when the window is minimized/restored, has subsurfaces, or receives a popup
  reconfigure;
- root resize preview changes geometry and invalidates origin cache, but pointer
  focus is only refreshed on popup commit paths, not after every preview geometry
  change;
- reactive popup constraints are refreshed when `set_window_geometry` changes,
  but resize preview itself does not reconfigure child popups.

This is not yet proof of a native freeze bug. It is a map of the correctness
edges most likely to explain "resize/window state feels stuck" when combined
with SDDM/native output and clients that keep committing while hidden.

## Evidence Map

Resize state fields live in `CompositorState`:

- `pending_resize_configure`, `sent_resize_commits`, and
  `pending_resize_commits`: `src/compositor/mod.rs:197-200`.
- frame callback, presentation feedback, and buffer release queues:
  `src/compositor/mod.rs:206-211`.

Core resize lifecycle:

- interactive resize begins from hit testing or client xdg request:
  `src/compositor/mod.rs:1763-1830`.
- resize interaction stores the root, start geometry, and pointer start:
  `src/compositor/mod.rs:1832-1875`.
- pointer updates coalesce through `queue_resize_root_window_to()`:
  `src/compositor/mod.rs:1906-1940`.
- resize end flushes a pending configure and then sends a final non-resizing
  configure if the drag committed: `src/compositor/mod.rs:1944-1953`.
- queued resize stores one pending configure and immediately updates the visual
  preview: `src/compositor/mod.rs:2284-2311`.
- `preview_resize_root_window_to()` updates renderable size/placement and stores
  `ResizePreview`: `src/compositor/mod.rs:2314-2365`.
- `present_frame()` is where pending resize configures are flushed in normal
  frame flow: `src/compositor/server.rs:226-233`.
- `has_pending_frame_work()` returns true for pending resize configure, frame
  callbacks, or presentation feedback: `src/compositor/mod.rs:2826-2830`.

ACK and commit matching:

- ACK promotes the newest sent resize commit with serial <= ACK serial and
  retains newer serials: `src/compositor/mod.rs:2497-2511`.
- commit placement is only consumed when the committed logical/window geometry
  matches the pending resize size: `src/compositor/mod.rs:2470-2494`.
- the predicate is exact width/height equality:
  `src/compositor/mod.rs:125-127`.

Preview and rendering:

- `RenderableSurface` carries `resize_preview`:
  `src/compositor/surface.rs:5-17`.
- `ResizePreview` stores committed size plus right/bottom anchor flags:
  `src/compositor/surface.rs:41-47`.
- buffer commit clears `resize_preview`: `src/compositor/mod.rs:2948-2964`.
- render composition uses `resize_preview_content_target()` before blitting:
  `src/compositor/render.rs:1011-1023`.
- the target shrink/anchor logic is in
  `src/compositor/render.rs:1104-1127`.
- undersized committed buffers are not upscaled by preview:
  `src/compositor/render.rs:1129-1137`.

Minimized commits:

- minimize removes the root surface tree from `renderable_surfaces` and stores
  snapshots in `WindowState`: `src/compositor/mod.rs:2025-2068`.
- restore extends those snapshots back into `renderable_surfaces`:
  `src/compositor/mod.rs:2070-2085`.
- current code now detects commits whose root is minimized:
  `src/compositor/mod.rs:903-924`.
- minimized commits update the hidden snapshot or push a new hidden renderable:
  `src/compositor/mod.rs:987-1025`.
- the earlier minimize report documents why this split matters:
  `docs/research/agent-kernel-minimize-window-state.md`.

Subsurfaces/popups/input:

- subsurface placement is parent-relative in core protocol handling:
  `src/compositor/protocols/core.rs:218-245`.
- descendant stacking exists for parent commits and popup commits:
  `src/compositor/mod.rs:958-969` and `src/compositor/mod.rs:1343-1433`.
- popup registration/configure stores parent-relative placement:
  `src/compositor/mod.rs:1458-1477` and `src/compositor/mod.rs:1617-1688`.
- reactive child popups are reconfigured when xdg window geometry changes:
  `src/compositor/protocols/xdg.rs:145-177`.
- pointer hit testing uses cached surface origins:
  `src/compositor/mod.rs:1330-1340` and `src/compositor/mod.rs:2655-2689`.
- pointer focus refresh exists, but is called after popup commits, not from the
  resize preview path: `src/compositor/mod.rs:970-976` and
  `src/compositor/mod.rs:2707-2715`.

Existing tests with useful coverage:

- resize drag sends root configure:
  `src/compositor/tests/windows.rs:187-200`.
- coalesced pointer updates until present:
  `src/compositor/tests/windows.rs:202-219`.
- pending resize configure reports frame work:
  `src/compositor/tests/windows.rs:221-237`.
- preview renderable target before client commit:
  `src/compositor/tests/windows.rs:239-270`.
- ACK serial promotion and newer-serial retention:
  `src/compositor/tests/windows.rs:272-346`.
- pending resize commit waits for matching size:
  `src/compositor/tests/windows.rs:348-372`.
- resize end clears `Resizing` state:
  `src/compositor/tests/windows.rs:374-389`.
- top-left resize placement/anchor:
  `src/compositor/tests/windows.rs:391-413`.
- CSD window geometry with buffer margin:
  `src/compositor/tests/windows.rs:415-433`.
- configure-only resize advances render generation:
  `src/compositor/tests/windows.rs:435-451`.
- threshold and duplicate visual update suppression:
  `src/compositor/tests/windows.rs:453-506`.
- left-edge shrink preview anchors right edge:
  `src/compositor/tests/windows.rs:525-555`.
- scaled buffer resize tests:
  `src/compositor/tests/windows.rs:557-611`.
- xdg toplevel resize request uses requested edge:
  `src/compositor/tests/windows.rs:654-669`.
- popup configure/reposition/grab tests:
  `src/compositor/tests/xdg.rs:66-244`.
- subsurface parent-relative and stacking tests:
  `src/compositor/tests/subsurface.rs:16-66`.

## Current Lifecycle

1. A frame-edge hit or client `xdg_toplevel.resize` request starts a
   `WindowInteraction`.
2. Motion beyond threshold computes root geometry with the requested resize
   edges.
3. `queue_resize_root_window_to()` stores exactly one pending resize configure
   and immediately updates the renderable root size/placement as a preview.
4. Native/nested frame flow calls `present_frame()`, which flushes the pending
   resize configure to the client.
5. The client ACKs a configure serial; `ack_xdg_surface_configure()` moves the
   newest ACKed resize into `pending_resize_commits`.
6. On the next matching surface commit, `take_pending_resize_commit_placement()`
   consumes that pending resize and computes final placement for the committed
   logical size.
7. `update_renderable_surface_buffer()` installs the new buffer and clears
   `resize_preview`.
8. On drag release, `end_window_interaction()` flushes any remaining resize
   configure and sends a final configure without the `Resizing` state.

## Findings

### 1. ACK serial handling is sound, but only under exact-size commits

Confirmed by code and tests.

The ACK path does the important Wayland thing: it does not blindly use the ACKed
serial as an exact map key. It picks the newest sent resize whose serial is not
newer than the ACK and keeps later serials. This matches the usual wlroots/Hypr
shape where multiple configures can be in flight and clients ACK the latest one
they accepted.

The risk is the strict commit-size match. A client that ACKs a resize but
commits an intermediate, constrained, or CSD-window-geometry-adjusted size will
leave `pending_resize_commits` in place until a later exact match. That is good
for avoiding wrong placement, but bad if the pending commit then leaks across
minimize/restore or a later unrelated commit.

Needed tests:

- `pending_resize_commit_survives_mismatch_and_is_replaced_by_newer_ack`
- `stale_pending_resize_commit_does_not_apply_after_new_non_resize_configure`
- `csd_resize_ack_with_buffer_margin_consumes_window_geometry_size`

### 2. Preview state is useful, but reset boundaries need cross-state tests

Confirmed by code; risk is untested combinations.

The renderer no longer stretches the old client buffer to the requested size.
Instead it shrinks/anchors the content target when the committed buffer is
smaller than the preview target. That is the right direction for Gecko/browser
resizes.

`resize_preview` is cleared on buffer commit. That is simple, but there are no
tests proving it clears only after a matching ACKed resize commit, or that it
does not remain trapped in a minimized snapshot. If a hidden/minimized commit
updates the snapshot through `commit_minimized_surface_buffer()`, it uses the
same `update_renderable_surface_buffer()` helper and should clear preview, but
that cross-state behavior is not directly asserted.

Needed tests:

- `resize_preview_clears_after_matching_client_commit`
- `resize_preview_survives_mismatch_until_matching_commit`
- `minimized_resize_commit_clears_hidden_preview_before_restore`
- `restore_after_resize_preview_uses_latest_hidden_committed_size`

### 3. Resize configure is flushed on presentation work, not immediately on motion

Confirmed by code and tests.

Motion coalescing is intentional: tests assert that pointer updates are
coalesced until `PresentFrame`, and `present_frame()` flushes pending resize
configure before completing callbacks/feedback. This is reasonable only if the
frame loop is driven by real repaint/pageflip cadence. If native output is using
timer wakeups or completing presentation work before pageflip, resize configures
can feel late or uneven even when the compositor state machine is correct.

This connects to `docs/research/native-frame-lifecycle-pageflip.md`: validate
resize under the same pageflip/presentation logging used there.

Needed tests/log validation:

- add perf log fields for `pending_resize_configure`, `sent_resize_commits`,
  and `pending_resize_commits` counts during native resize;
- log configure serial, ACK serial, committed logical size, preview size, and
  pageflip sequence together;
- verify there is no repeated repaint with the same pending resize configure
  while a previous native pageflip is still outstanding.

### 4. Minimized hidden commits are no longer a simple resurrection bug, but resize state can leak across hide/show

Confirmed by current code.

The current code checks minimized roots in `commit_surface_buffer()` and diverts
those commits into `commit_minimized_surface_buffer()`. That means the older
"commit recreates visible renderable" bug is not present in this snapshot.

The remaining resize-specific risk is subtler. `pending_resize_commits` is
global per root surface, and hidden commits can consume it while the toplevel is
minimized. That can be correct, but only if restore uses the updated hidden
snapshot and if preview state has been cleared. There is no test that starts a
resize, minimizes before/after ACK, commits the requested size while hidden, and
then restores.

Needed tests:

- `minimize_during_pending_resize_keeps_window_hidden`
- `hidden_commit_matching_pending_resize_updates_minimized_snapshot`
- `restore_after_hidden_resize_commit_uses_updated_geometry`
- `restore_after_hidden_mismatched_commit_does_not_apply_stale_resize`

### 5. Subsurfaces follow parent placement, but resize preview only targets the root

Confirmed by code; resize-specific gap.

Subsurfaces are parent-relative and render origins are computed from the
surface tree, so root placement changes should move children with the root. The
current tests cover move-with-child and basic subsurface stacking. They do not
cover resize preview with child surfaces.

Potential failure modes:

- root preview width/height changes but a child overlapping the preview edge is
  still hit-tested/rendered according to old assumptions;
- child surfaces stay in correct visual order after root preview and after the
  final matching commit;
- child commits during a root resize do not clear the root `resize_preview`.

Needed tests:

- `resize_preview_moves_subsurface_tree_with_root`
- `left_edge_resize_preview_keeps_subsurface_world_origin_consistent`
- `subsurface_commit_during_parent_resize_does_not_clear_root_preview`
- `parent_matching_resize_commit_preserves_subsurface_stacking`

### 6. Reactive popups reconfigure on geometry changes, not on preview-only resize

Confirmed by code; likely policy question.

Reactive popups are reconfigured when the parent xdg window geometry changes.
During preview-only resize, the root renderable size/placement changes before
the client has committed a new window geometry. No popup reconfigure is sent from
`preview_resize_root_window_to()`.

That may be acceptable: clients only promised new geometry after configure/ACK
and commit. But visually, native resize can place a root preview under an old
popup constraint target for one or more frames. If users see "menus frozen while
resizing" or pointer grabs feel wrong, this is the area to test first.

Needed tests:

- `reactive_popup_reconfigures_after_parent_resize_commit_geometry_change`
- `reactive_popup_is_not_reconfigured_by_preview_only_resize`
- `popup_grab_release_stays_targeted_after_parent_resize_preview`
- `outside_click_dismisses_popup_after_parent_resize_preview_moves_root`

### 7. Pointer/focus retarget after preview is under-specified

Confirmed by code; missing tests.

Pointer target selection uses cached origins and current renderable dimensions.
Preview resize invalidates the origin cache through `store_surface_placement()`
and advances render generation, so later pointer hit tests should see the new
geometry.

However, the compositor does not refresh pointer focus immediately after a
resize preview. It refreshes pointer focus on explicit pointer motion, button
handling, and popup commit. During a drag, implicit grabs may make this fine,
but after releasing a resize or when a popup/subsurface appears under the old
cursor position, stale pointer focus is possible unless the next pointer event
retargets it.

Needed tests:

- `pointer_focus_updates_after_resize_release_without_extra_motion`
- `resize_preview_changes_frame_hit_testing_to_new_root_bounds`
- `pointer_release_after_client_requested_resize_uses_original_press_surface`
- `subsurface_under_cursor_after_resize_preview_receives_next_motion_enter`

## Safe Fix Plan

1. Add observability first.
   Log resize serial lifecycle with surface id, requested size, committed
   logical size, preview target size, placement, pending/sent counts, and native
   pageflip/presentation sequence.

2. Add narrow tests before changing behavior.
   Start with pure compositor tests for minimized hidden resize commits and
   preview reset. Then add subsurface and popup tests. Only after those pass
   should native presentation timing be tuned.

3. Harden pending resize lifetime.
   If tests show stale `pending_resize_commits`, clear or supersede pending
   resize state when a newer non-resize configure, unmap, minimize policy
   boundary, or explicit restore boundary makes the old resize impossible to
   consume safely.

4. Make preview reset explicit.
   Prefer a helper that clears preview only for the root surface whose committed
   content resolves the resize. Avoid child/subsurface commits clearing root
   preview accidentally, and assert hidden/minimized updates go through the same
   helper.

5. Define popup policy during preview.
   Either document that preview-only resize does not reconfigure reactive popups,
   or reconfigure reactive children only after the parent commits new xdg window
   geometry. Avoid sending popup configures from raw pointer motion unless tests
   prove a client needs it.

6. Retarget pointer/focus after final resize.
   On resize end or matching commit, consider a guarded
   `refresh_pointer_focus_at_last_position()` if there is no active implicit
   grab. Validate with popup grab and subsurface input-region tests.

7. Validate in native output with pageflip-aware traces.
   Confirm that resize configure, client ACK, matching commit, frame callback,
   presentation feedback, buffer release, repaint, and KMS pageflip complete in
   the expected order.

## Test Backlog

Highest priority:

- `minimize_during_pending_resize_keeps_window_hidden`
- `hidden_commit_matching_pending_resize_updates_minimized_snapshot`
- `restore_after_hidden_resize_commit_uses_updated_geometry`
- `resize_preview_clears_after_matching_client_commit`
- `stale_pending_resize_commit_does_not_apply_after_new_non_resize_configure`

Subsurface/popup priority:

- `resize_preview_moves_subsurface_tree_with_root`
- `subsurface_commit_during_parent_resize_does_not_clear_root_preview`
- `reactive_popup_reconfigures_after_parent_resize_commit_geometry_change`
- `popup_grab_release_stays_targeted_after_parent_resize_preview`

Pointer/focus priority:

- `pointer_focus_updates_after_resize_release_without_extra_motion`
- `resize_preview_changes_frame_hit_testing_to_new_root_bounds`
- `subsurface_under_cursor_after_resize_preview_receives_next_motion_enter`

Native validation:

- perf trace with `OBLIVION_NATIVE_PERF=1` or the current native perf flag;
- log configure/ACK/commit serials next to `render_generation`,
  `render_cause`, `pending_frame_work`, pageflip requested/completed, and
  presentation feedback completion;
- run with a browser/GTK app that continues committing while minimized and
  resize/minimize/restore it repeatedly under native SDDM.

## Risk Ranking

High:

- stale pending resize consumed after minimize/restore;
- preview not cleared or cleared at the wrong boundary;
- native frame loop flushing resize configures without real pageflip cadence.

Medium:

- popup constraints not refreshed until parent geometry commit;
- pointer focus stale after preview/final resize;
- child/subsurface commit during parent resize not covered by tests.

Low:

- ACK serial promotion itself; current code and tests are aligned with the
  expected in-flight configure model.

