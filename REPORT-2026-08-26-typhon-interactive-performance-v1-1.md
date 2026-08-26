# Typhon Interactive Performance v1.1 — Implementation Report

Date: 2026-08-26

Baseline: current dirty worktree at `4a18af4` (`perf: close interactive move and resize hot paths`). The worktree was treated as authoritative; unrelated existing changes were preserved.

## A. Executive result

v1.1 closes the remaining interactive hot-path multipliers identified after v1:

- ordinary pointer locality now consumes ActiveScene spatial authority without refreshing the broad origin cache first;
- grabbed interaction targets use the ActiveScene surface-ID index and aligned origins, with the old global path retained as a fallback;
- interaction pointer dispatch iterates live resources directly and uses lazy diagnostics;
- EGL scene replay now uses a repair-aware reverse cumulative visibility planner with bounded region fragmentation and conservative overdraw fallback;
- floating move and resize geometry is queued as latest desired state and applied by `prepare_frame()`, while terminal release force-flushes the exact latest target.

The changes target cursor lag and compositor CPU amplification from content-only commits, raw mouse polling, repeated pointer allocations, and command-level occlusion work. They do not claim a host-level CPU/GPU improvement because native KMS/NVIDIA qualification was unavailable in this environment.

## B. Revalidated root causes

| Finding | Worktree status and evidence | Decision |
|---|---|---|
| F1: global origin recomputation before locality | Present in the reviewed v1 path. `pointer_scene_hit_at()` previously refreshed `surface_origin_cache` before exact-cache/locality checks. ActiveScene already owns indexed origins and refreshes them only when spatial facts change. | Removed the pre-locality global refresh. Added recompute and ActiveScene-index counters and a content-only regression test. The global cache remains for legitimate fallback callers. |
| F2: grabbed target global lookup | Present in `pointer_target_for_grabbed_surface_at_output()`: global cache refresh plus linear `renderable_surfaces.position()`. | ActiveScene index/origin is now the ordinary path. A conservative global cache/linear fallback remains for targets absent from ActiveScene. Pending root-placement deltas preserve exact pointer coordinates before a deferred visual resize flush. |
| F3: per-sample pointer allocation/eager formatting | Present in interaction pointer dispatch and locked/relative diagnostics. | Removed the temporary cloned pointer vector, retained dead resources once, iterated directly, counted dispatches, and converted high-rate diagnostics to lazy closures. Existing disabled-lazy-formatter test remains green. |
| F4: suffix-scan occlusion | Present in the v1 EGL draw loop as a per-command scan of commands above the current command. | Replaced with an explicit reverse planner per repair/scissor region; execution still iterates selected commands in original forward composition order. |
| F5: whole-command occlusion decision | Present in the v1 coverage check against the entire lower command bounds. | Planner intersects commands with the remaining repaired region and subtracts only proven opaque coverage clipped through that region. |
| F6: raw floating resize preview/configure work | Present: floating resize updated visual preview and could flush configure work in the raw update path. | Raw move/resize samples now replace pending desired state. Visual/configure work is applied from frame preparation; the ordinary raw path has no preview or configure flush. |
| F7: existing frame-preparation authority | Already present: `prepare_frame()` flushed explicit sync, color, tiled resize, resize configure, and clients. | Extended the existing authority with floating interaction geometry between tiled resize and resize configure. `has_pending_frame_prepare_work()` advertises pending floating state. |

## C. Architecture implemented

### Pointer and spatial authority

`ActiveSceneView.surface_indices`, `surface_origins`, and `surfaces` are used for exact same-owner locality and grabbed-target coordinate resolution. Content-only ActiveScene surface replacement does not recompute origins. Spatial changes still invalidate/rebuild the relevant ActiveScene authority, and the broad origin cache remains available for non-ActiveScene fallback paths.

When a resize target changes the root placement but has not yet reached the frame-preparation boundary, pointer delivery applies that pending root-placement delta to the current indexed origin. This keeps client-local pointer coordinates equivalent to the former immediate-preview behavior without mutating visual geometry at raw input frequency.

### Pointer dispatch

`send_window_interaction_pointer_motion()` retains live pointer resources and sends motion/frame events through a direct filtered iteration. No per-sample cloned resource vector is built. Interaction, locked absolute, locked relative-route, and implicit-grab motion diagnostics use `pointer_debug_log_lazy`.

### EGL visibility planner

Each scene repair pass starts with a bounded fixed-array visible region. Commands are visited in reverse order, marked drawable when they intersect remaining repair, and proven opaque rectangles are subtracted cumulatively. The selected decisions are then consumed in the original forward order. The region is capped at 32 pieces; if subtraction would overflow, occlusion is disabled for that repair and the region is restored to the full repair rectangle, producing conservative extra drawing rather than missing pixels.

### Frame-prepared floating interaction geometry

Move samples replace the latest pending pointer target. Floating resize samples calculate and clamp the latest desired `WindowGeometry` and replace the pending target bound to `WindowInteractionId` and root surface. `prepare_frame()` flushes the latest move/resize state, then sends queued resize configure work and flushes clients. User-final release force-flushes pending geometry before resize-end/finalization and clears all pending interaction state on cancellation/end.

The existing `ResizeConfigureFlow` remains responsible for duplicate suppression, latest queued state, in-flight capacity, ACK tracking, captured commits, and final configure behavior. The in-flight depth policy was not changed.

## D. Deterministic before/after evidence

- Content-only pointer test: an explicit baseline origin-cache refresh records one global recompute; changing only broad render generation and moving to a nearby decoration point does not increase that count. The same-owner locality test records one `active_scene_index_hit` and one locality hit.
- Renderer planner: `visibility_planner_visits_each_command_once` visits 100 synthetic commands exactly once. The former suffix architecture would inspect approximately 4,950 command pairs for the same single repair in the comparable worst-case shape. Cumulative partial coverage, repair-aware large-command coverage, transparent content, and fragmentation overflow are tested.
- Floating resize scheduling: `floating_resize_coalesces_geometry_until_frame_flush` submits 1,000 raw resize samples, records 999 pending replacements, performs zero visual applications before flush, and applies the latest target once at frame preparation. A subsequent raw sample is force-applied once at terminal release.
- Integration resize scheduling: three raw pointer targets coalesce to one frame-prepared visual application; the latest geometry is retained and configure bounds remain within existing policy.
- Pointer resource dispatch: the implementation has no temporary pointer-resource collection; `interaction_pointer_temporary_vectors` remains zero as the explicit observability counter. Dispatch iterations are counted separately.

These are source/operation-count guarantees, not host benchmark results.

## E. Tests and verification

Focused checks passed during implementation:

```text
rtk cargo fmt --all -- --check
rtk cargo check --locked                         # 0 errors, 6 existing dead-code warnings
rtk cargo test visibility_planner --locked -- --test-threads=1
  # 5 passed
rtk cargo test pointer_scene_hit --locked -- --test-threads=1
  # 6 passed
rtk cargo test floating_resize_ --locked -- --test-threads=1
  # 2 passed
rtk cargo test window_interaction --locked -- --test-threads=1
  # 75 passed
rtk cargo test --lib compositor::state::window_interaction_tests --locked -- --test-threads=1
  # 64 passed
rtk git diff --check
```

Final full verification used a short temporary path because the repository's socket-backed tests otherwise exceeded Linux `SUN_LEN` in this environment:

```text
TMPDIR=/tmp rtk cargo test --locked -- --test-threads=1
  # 2,952 passed, 5 ignored, 40 filtered; 30 suites; 183.12s
```

The final run was green. Formatting and `git diff --check` were also clean.

## F. Real-host qualification

No real KMS/NVIDIA session was available, so no CPU/GPU percentage or Typhon-vs-Hyprland claim is made. The following S1–S12 gates remain pending on the target host:

1. idle;
2. empty-workspace pointer motion;
3. ChatGPT/Electron hover in a populated workspace;
4. Chromium/Chrome hover;
5. moving over heavy content;
6. floating browser/Electron resize;
7. forced software cursor;
8. locked relative-pointer client;
9. XWayland application;
10. multi-output crossing;
11. popups, subsurfaces, transparency, and opaque holes;
12. release between frame opportunities for terminal exactness.

For qualification, correlate `pidstat`, `perf stat/record`, `nvidia-smi dmon` where available, and Typhon aggregate counters. Avoid enabling per-event diagnostics during normal measurements.

## G. Review pass 1 findings

The correctness/ownership review found and fixed:

- a legacy floating-resize branch still contained the old immediate preview/configure behavior; it is now unreachable outside the frame-prepared flush path;
- existing direct tests assumed raw move/resize mutated visual geometry immediately; they now issue the explicit preparation boundary where they assert post-flush geometry;
- deferred resize preview initially caused stale local pointer coordinates for the same raw sample; the grabbed-target resolver now incorporates the pending root-placement delta;
- cancellation and interaction teardown clear pending move/resize state, and terminal release flushes before resize-end configure/finalization.

The review also confirmed direct pointer resource iteration preserves event/frame delivery and that bounded planner overflow is fail-open.

## H. Review pass 2 findings

The performance/architecture challenge found and fixed:

- the old suffix-coverage helper was unused after planner integration and was removed, leaving no suffix scan in the renderer hot path;
- a locked relative-route snapshot built its diagnostic vector before checking whether pointer debug was enabled; the complete construction now lives inside a lazy formatter closure;
- residual searches confirmed no elapsed-time move gate, no raw floating `preview_resize_root_window_to()` call, and no raw floating `flush_pending_resize_configure()` call;
- the first full run exposed test assumptions about raw-event geometry and a long temporary socket path. The assumptions were updated to the frame contract, and the final run used `TMPDIR=/tmp` to remove the unrelated socket-name environment failure.

No new timer, sleep, fixed refresh constant, renderer allocation proportional to command count, client-specific policy, or unrelated KMS/Dwindle rewrite was introduced.

## I. Remaining risks

- Host-level CPU/GPU and cursor-latency improvement is unqualified until S1–S12 are measured on the real native session.
- The bounded planner deliberately trades fragmented-region precision for conservative overdraw after 32 pieces; this is observable via `region_fragmentation_overflow_fallbacks` and `peak_region_piece_count`.
- Non-ActiveScene grabbed targets still use the conservative global origin-cache fallback; those paths should be measured if they dominate a real workload.
- Existing dead-code warnings and the need for a short `TMPDIR` in socket-backed tests are environmental/repository conditions, not regressions attributed to this patch.
