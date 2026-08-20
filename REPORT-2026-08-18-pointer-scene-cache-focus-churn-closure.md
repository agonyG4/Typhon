# PointerSceneHit Cache, Focus-Churn, and Hot-Path Closure Report

Date: 2026-08-19

Status labels used below: **CONFIRMED**, **STRONG HYPOTHESIS**, **NATIVE-PROVEN**, and **UNPROVEN**.

## 1. Baseline HEAD and dirty state

The implementation baseline was the existing dirty Typhon checkout at `/home/agony/GitHub/Typhon`; the requested Documents path was not a Git checkout. No reset, restore, checkout, clean, or stash operation was used.

Baseline:

```text
HEAD 0ef9f7b99fa38d0fc04bf5ffa8f494db5a6eade6
0ef9f7b docs: design instant resize and pointer ownership closure
2fc5fd1 test(render): cover fullscreen restore buffer ages
23207a1 test(render): reproduce fullscreen frame-scene mismatch
8b7ea5b docs: design fullscreen frame-scene authority closure
abe4a94 docs: record post-closure qualification status
ba19656 test(scene): keep decoration reuse rooted in a live surface
e8aaf71 fix(scene): preserve empty popup cache identity
41b9e1e fix(input): reject zero high-resolution wheel steps
707a18f fix(scene): qualify popup and decoration visual ownership
c0daade docs: record final corrective closure verification
a341914 test: align moved MacTahoe decoration coordinates
beea7cb test: align borderless MacTahoe extents
3d333cf docs: record corrective closure verification
5cd67f1 fix: close WindowVisual input and scroll ownership gaps
b12f219 docs: plan Typhon corrective closure
```

The worktree already contained substantial prior WindowVisual, resize, render-ahead, fullscreen, and protocol changes plus untracked reports/specifications. Those changes were preserved. The baseline full suite was **1654 passed, 40 failed, 2 ignored**.

## 2. Residual stale-cache root cause

**CONFIRMED.** `PointerSceneHitCache` previously keyed reuse by only `(x, y, scene_render_generation)`. `wl_surface.set_input_region` can change pointer ownership without changing rendered pixels or `scene_render_generation`.

The deterministic reproducer is:

1. A is above B and accepts input at stationary point P.
2. `pointer_scene_hit_at(P)` caches `Client(A)`.
3. A commits an empty input region without a render-visible scene generation change.
4. The existing production path calls `refresh_pointer_focus_at_last_position()`.
5. The old cache returned stale `Client(A)` instead of recomputing B.

The failing-first test was `pointer_scene_hit_cache_requires_current_pointer_hit_generation` in `src/compositor/state/window_decoration_tests.rs`. It initially failed to compile because the state and cache had no pointer-hit generation. After implementation it passes and verifies that the same coordinate with a new pointer generation recomputes and stores the new generation.

The end-to-end regression is `overlapping_server_decoration_does_not_focus_window_underneath` in `src/compositor/tests/input_output/window_interaction.rs`. It commits A's input region empty and then inclusive while the pointer remains stationary; it does not clear the cache manually. Both transitions pass.

## 3. Hit-test dependency audit

| Dependency | Classification | Evidence/action |
|---|---|---|
| Surface input region | Input-only; not reliably represented by render generation | Advances `pointer_hit_generation` immediately after applying the region and before focus refresh |
| WindowVisual geometry / SSD extents | Render/geometry generation plus origin invalidation | Existing scene generation remains authoritative; resize regression proves immediate geometry |
| Window stacking and layer ordering | Render generation plus origin/group invalidation | Shared `VisualStackGroup` remains the authority |
| Popup topology and ordering | Render generation plus group invalidation | Popup groups remain independent and above parent SSD |
| Subsurface topology/order | Render generation plus origin/group invalidation | Existing subsurface ordering paths retain invalidation |
| Map/unmap/destruction | Render generation plus origin invalidation | Destruction regression proves a cached identity is not reused after invalidation |
| SSD mode/geometry and resize extents | Render/geometry generation | Existing render generation is sufficient; no second generation is incremented for ordinary geometry |
| XWayland Shape input region | **UNPROVEN / not applicable in current source** | Source audit shows Shape is negotiated diagnostically but not applied to Typhon hit testing; no unsupported special case was added |
| Pointer constraints, grabs, DND, compositor move/resize | Routing precedence, not ordinary scene-cache ownership | Existing precedence remains before ordinary focus reconciliation |

## 4. Chosen invalidation architecture

**CONFIRMED.** A dedicated `pointer_hit_generation: u64` now participates in cache validity:

```text
(x, y, scene_render_generation, pointer_hit_generation)
```

`pointer_hit_generation` advances for input/topology mutations capable of changing ordinary ownership. Input-region commits advance it before `refresh_pointer_focus_at_last_position()`. Surface-origin invalidation advances it for geometry, ordering, map/unmap, and related topology changes. The cache is updated only after immutable hit computation completes.

The generation is not advanced for title text, content damage, frame callbacks, presentation feedback, or other changes that do not alter pointer ownership. Input-region-only commits preserve the existing scene render generation while still invalidating pointer ownership.

The cache stores stable `WindowId`, surface identity, root surface identity, and `DecorationHit` values; it does not store unmanaged references.

## 5. Client-to-SSD focus stress

**CONFIRMED by deterministic event routing.** The real server test performs 1,000 cycles of:

```text
A client -> A titlebar/SSD -> A button -> A client
```

This sends 4,000 production pointer-motion commands through scene hit resolution, pointer enter/leave routing, desktop focus, keyboard focus, and constraint reconciliation. It does not call `pointer_scene_hit_at()` as a substitute for event routing.

Results:

- Focused window remained A throughout.
- Keyboard focus remained A.
- Focus generation was unchanged across the stress interval.
- B received zero pointer enters and zero pointer leaves during the stress interval.
- A's crossing sequence began `enter, leave, enter`.
- A received exactly 1,001 enters and 1,000 leaves, matching the initial enter plus 1,000 client/SSD crossings.
- Repeated motion within A's SSD was a same-window focus no-op; the deterministic instrumentation asserted that same-window no-ops occurred.
- Active interaction, popup-grab, locked-pointer, confined-pointer, DND, and layer precedence checks remain before the ordinary same-window fast path.

The test also covers the combined input-region/SSD case: after A's client region is removed, the stationary client point resolves to B, while an A titlebar point still resolves to A's decoration; restoring A's input region returns the stationary client point to A.

## 6. Hot-path findings and optimization

**CONFIRMED structurally.** Before this closure, each uncached `pointer_scene_hit_uncached()` call cloned the complete `surface_origin_cache` and each visual group rediscovered its root with a linear `.position()` scan through `renderable_surfaces`.

`VisualStackGroup` now retains `root_surface_index` alongside `root_surface_id`. Hit testing validates the indexed root identity and uses the index directly. The hot path borrows `surface_origin_cache` immutably; no complete-map/vector clone is performed.

Instrumentation is disabled by default and can be enabled by `TYPHON_POINTER_DEBUG` or deterministic test control. It tracks calls, cache hits/misses, groups, surfaces, CPU nanoseconds, focus invocations/no-ops, keyboard reconciliation, constraint reconciliation, origin-cache clones, and root linear searches.

The deterministic 10,000-motion unit workload (`pointer_scene_hit_metrics_cover_repeated_positions_without_hot_path_clones`) produced/asserted:

| Metric | Result |
|---|---:|
| Pointer scene-hit calls | 10,003 |
| Cache hits | at least 1; repeated-position behavior passes |
| Cache misses | at least 7,500 |
| Origin-cache clones | 0 |
| Root linear searches in `pointer_scene_hit_uncached` | 0 |
| Groups/surfaces inspected | non-zero on misses |
| CPU duration | recorded and non-zero |
| Cache entries | one current-position entry |

The real 4,000-motion event stress also asserted zero origin-cache clones and zero root linear searches. No spatial index was introduced.

## 7. Cache identity invalidation and destruction

**CONFIRMED.** `pointer_scene_hit_cache_does_not_survive_destroyed_window` seeds a cached stable decoration identity, removes the owning desktop window and renderable surface, runs production origin invalidation, and verifies the next stationary lookup returns `None` rather than the destroyed identity.

The cache contract test also verifies that a repeated coordinate is a hit when both generations are unchanged and is recomputed when only `pointer_hit_generation` changes.

## 8. Resize-pacing classification

The baseline had four closure-relevant resize failures. They were stale expectations of the earlier serialized configure behavior, not regressions in the approved immediate/bounded design.

| Failure group | Count | Baseline | After closure | Classification/reason |
|---|---:|---|---|---|
| Resize configure/pending-frame assertions | 2 | Failed | Passed | Obsolete: immediate interaction dispatch flushes configure work before the pending-frame query |
| Resize coalescing / no-progress assertions | 2 | Failed | Passed | Obsolete assertions updated to latest-geometry behavior, outstanding `<= 3`, and bounded retention |
| Other resize regressions | 0 | Green | Green | No lost final geometry, stale rollback, ACK ownership, or incorrect final resizing state found |

The updated tests assert:

- latest target geometry is preserved;
- responsive clients are not serialized to one configure/commit at a time;
- slow/no-progress clients remain bounded by `max_in_flight_configures <= 3`;
- retained configure state remains bounded (`<= 4` including the latest retained slot);
- final resize state is cleared after interaction end;
- immediate visual geometry remains current before client commit.

## 9. Render-ahead, damage, and buffer-age regressions

**CONFIRMED by tests.** No presentation-domain damage journal, buffer-age handling, or `ResolvedNativeFrameScene` logic was changed by this closure.

Passed focused regressions include:

- fullscreen restore reference for buffer ages 1, 2, and 3;
- oversized SSD retry reference for buffer ages 1, 2, and 3;
- render-ahead oversized SSD repair reference;
- fullscreen restore damage repair for the culled window and SSD.

The full suite contained no new damage/render-ahead failures. Persistent native move/resize ghosting remains **UNPROVEN natively** because native qualification was blocked before an interactive session could be run.

## 10. Remaining full-suite failures

Final full suite:

```text
1661 passed; 36 failed; 2 ignored
```

Classification:

| Failure group | Count | Baseline before task | After task | Related to this closure? | Reason |
|---|---:|---:|---:|---|---|
| Unix socket path/SUN_LEN setup | 2 | 2 | 2 | No | Test runtime path is longer than the platform `sockaddr_un` limit |
| XWayland startup/display-lease cluster | 33 | 33 | 33 | No | Same SUN_LEN setup failure, followed by poisoned shared display-test lock |
| Direct scanout identity XRGB eligibility | 1 | 1 | 1 | No | Existing `eligibility.eligible` failure in direct-scanout setup |
| Resize-pacing/input closure failures | 4 | 4 | 0 | Yes | Resolved by correcting obsolete pacing assertions |

The user-provided earlier report cited 43 failures; the current reproducible baseline in this checkout was 40, and the final count is recorded from the commands run here rather than copied from that earlier report.

## 11. Native qualification

**UNPROVEN / BLOCKED.** The requested native launcher was attempted with the supplied Typhon environment. The machine exposed `/dev/dri/card1` and the compositor reached direct DRM/KMS initialization, but native startup stopped at the pre-render atomic `TEST_ONLY` commit:

```text
pre-render atomic TEST_ONLY commit failed: Permission denied (os error 13)
```

The launcher also reported that user `agony` is not in the `input` group. Therefore no native titlebar, focus-flicker, pointer-hitch, or resize/ghosting claim is made. Native titlebar and native resize results are both **UNPROVEN**.

## 12. Validation commands

| Command | Result |
|---|---|
| `rtk cargo fmt --check` | Pass |
| `rtk cargo check --locked --all-targets` | Pass |
| `rtk cargo test --locked` | 1661 passed, 36 failed, 2 ignored; failures classified above |
| `rtk cargo clippy --locked --all-targets -- -D warnings` | Blocked by existing `clippy::large_enum_variant` in `src/xwayland/xwm/event_types.rs` (`XwmEvent::WindowReady`) |
| `rtk run "bash bin/check-source-layout"` | Existing size-limit failures in `src/compositor/tests/windows.rs`, `src/compositor/state/windows.rs`, `src/compositor/server.rs`, and `src/compositor/mod.rs` |
| `rtk git diff --check` | Pass |
| Native launcher | Blocked by input permissions and DRM `TEST_ONLY` EPERM |

## 13. Commits and boundaries

No new commits were created. The current HEAD remains `0ef9f7b99fa38d0fc04bf5ffa8f494db5a6eade6`, and all closure changes remain uncommitted so they can be reviewed alongside the existing dirty closure work. The implementation was kept in the natural boundaries of cache correctness, event-routing tests, hot-path optimization/instrumentation, resize-test contract alignment, and documentation rather than creating artificial commits.

## 14. Remaining blockers

1. Existing XWayland/astreactl test runtime paths exceed `SUN_LEN` and poison the shared XWayland test lock.
2. Existing direct-scanout identity eligibility test remains red.
3. Existing clippy large-enum-variant failure remains in XWayland event types.
4. Existing source-layout size limits remain exceeded.
5. Native interactive qualification remains blocked by DRM `TEST_ONLY` permission failure and missing input-group access.

## 15. Final `git status --short`

The final status was captured after implementation and validation. It contains the pre-existing dirty closure files, the new cache/focus changes, and the required documentation artifacts; no unrelated dirty file was removed or reverted.

```text
 M Cargo.lock
 M Cargo.toml
 M docs/superpowers/specs/2026-08-11-wayland-selection-idle-inhibit-design.md
 M src/compositor/desktop_window.rs
 M src/compositor/interaction.rs
 M src/compositor/mod.rs
 M src/compositor/protocols/core.rs
 M src/compositor/protocols/data_control.rs
 M src/compositor/protocols/primary_selection.rs
 M src/compositor/render.rs
 M src/compositor/server.rs
 M src/compositor/state/desktop_window_tests.rs
 M src/compositor/state/frame_callbacks.rs
 M src/compositor/state/frames.rs
 M src/compositor/state/fullscreen.rs
 M src/compositor/state/hit_testing.rs
 M src/compositor/state/input_dispatch.rs
 M src/compositor/state/input_resources.rs
 M src/compositor/state/pointer_constraints.rs
 M src/compositor/state/resize.rs
 M src/compositor/state/selection_runtime.rs
 M src/compositor/state/subsurfaces.rs
 M src/compositor/state/support_types.rs
 M src/compositor/state/surface_commits.rs
 M src/compositor/state/surface_focus.rs
 M src/compositor/state/surface_transactions.rs
 M src/compositor/state/surfaces.rs
 M src/compositor/state/task_05_8_tests.rs
 M src/compositor/state/window_decoration.rs
 M src/compositor/state/window_decoration_tests.rs
 M src/compositor/state/window_interaction.rs
 M src/compositor/state/window_interaction_tests.rs
 M src/compositor/state/window_resize.rs
 M src/compositor/state/windows.rs
 M src/compositor/state/xwayland_mode.rs
 M src/compositor/state/xwayland_windows.rs
 M src/compositor/tests/data_control.rs
 M src/compositor/tests/input_output/pointer_cursor.rs
 M src/compositor/tests/input_output/pointer_cursor_lifecycle.rs
 M src/compositor/tests/input_output/relative_and_constraints.rs
 M src/compositor/tests/input_output/window_interaction.rs
 M src/compositor/tests/primary_selection.rs
 M src/compositor/tests/protocol_buffers.rs
 M src/compositor/tests/protocol_error.rs
 M src/compositor/tests/support/clipboard_dmabuf.rs
 M src/compositor/tests/support/locked_relative.rs
 M src/compositor/tests/support/registry_state.rs
 M src/compositor/tests/support/server_runtime.rs
 M src/compositor/tests/support/subsurface_client.rs
 M src/compositor/tests/support/window_ops.rs
 M src/compositor/tests/windows.rs
 M src/compositor/tests/xdg.rs
 M src/compositor/tests/xwayland_resize_visual.rs
 M src/egl_renderer.rs
 M src/egl_renderer/damage.rs
 M src/egl_renderer/damage_tests.rs
 M src/egl_renderer/geometry.rs
 M src/launch_env.rs
 M src/native_output/launch.rs
 M src/native_output/mod.rs
 M src/native_output/output/damage.rs
 M src/native_output/presentation/trace.rs
 M src/native_output/runtime/bootstrap.rs
 M src/native_output/runtime/cycle.rs
 M src/native_output/runtime/cycle/pageflip.rs
 M src/native_output/runtime/cycle_direct.rs
 M src/native_output/runtime/cycle_dispatch.rs
 M src/native_output/runtime/frame.rs
 M src/native_output/runtime/kms_worker/rejection.rs
 M src/native_output/runtime/mod.rs
 M src/native_output/runtime/presentation.rs
 M src/native_output/runtime/presentation_ready.rs
 M src/native_output/runtime/presentation_worker.rs
 M src/native_output/runtime/session_io.rs
 M src/native_output/scanout/atomic_egl_gbm.rs
 M src/native_output/scanout/dumb.rs
 M src/native_output/scanout/egl_gbm.rs
 M src/native_output/scanout/gbm_cpu.rs
 M src/native_output/scanout/mod.rs
 M src/native_output/tests/fullscreen_frame_scene.rs
 M src/native_output/tests/mod.rs
 M src/native_output/tests/output.rs
?? .codex/
?? REPORT-2026-08-17-fullscreen-frame-scene-authority-closure.md
?? REPORT-2026-08-18-instant-resize-windowvisual-input-closure.md
?? REPORT-2026-08-18-pointer-scene-cache-focus-churn-closure.md
?? REPORT-2026-08-18-render-ahead-buffer-age-ghosting-closure.md
?? REPORT-2026-08-18-windowvisual-input-authority-closure.md
?? docs/superpowers/plans/2026-08-14-keyboard-focus-selection.md
?? docs/superpowers/plans/2026-08-15-kms-worker-timing-throughput-closure.md
?? docs/superpowers/plans/2026-08-16-ssd-damage-mactahoe-closure.md
?? docs/superpowers/plans/2026-08-17-oversized-resize-presentation-ghosting.md
?? docs/superpowers/plans/2026-08-17-presented-scene-retry-buffer-age-closure.md
?? docs/superpowers/plans/2026-08-17-windowvisual-post-closure-qualification.md
?? docs/superpowers/plans/2026-08-18-pointer-scene-cache-focus-churn-closure.md
?? docs/superpowers/plans/2026-08-18-render-ahead-presentation-damage-domain-closure.md
?? docs/superpowers/plans/2026-08-18-windowvisual-input-authority-closure.md
?? docs/superpowers/specs/2026-08-16-ssd-damage-mactahoe-closure-design.md
?? docs/superpowers/specs/2026-08-17-oversized-resize-presentation-ghosting-design.md
?? docs/superpowers/specs/2026-08-18-pointer-scene-cache-focus-churn-design.md
?? docs/superpowers/specs/2026-08-18-render-ahead-presentation-damage-domain-design.md
?? docs/superpowers/specs/2026-08-18-windowvisual-input-authority-design.md
?? docs/superpowers/specs/REPORT-2026-08-17-oversized-resize-presentation-ghosting.md
?? docs/superpowers/specs/REPORT-2026-08-17-presented-scene-retry-buffer-age-closure.md
?? error.txt.save
?? src/native_output/runtime/scene_history.rs
?? src/native_output/tests/output_retry.rs
```
