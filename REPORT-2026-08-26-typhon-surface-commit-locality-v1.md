# Typhon Surface Commit Locality v1 — Closure Report

**Date:** 2026-08-27
**Scope:** client buffer-rotation damage fidelity, content/topology quiescence, popup locality, and EGL authoritative-empty handling.
**Working-tree policy:** the existing dirty checkout was authoritative; unrelated changes were preserved.

## A. Baseline findings

The source audit confirmed the seven findings in the implementation prompt:

| Finding | Previous behavior | Evidence |
| --- | --- | --- |
| F1 — buffer identity promoted damage | `commit_surface_buffer()` required both equal buffer size and equal `BufferId` to preserve client damage. Same-size A/B/C rotation became `Full`. | The new partial-rotation test failed before the fix with `Full` instead of the requested 1x1 partial region. |
| F2 — attachment plus empty damage promoted `Full` | `protocols/core.rs` converted every attached buffer with empty damage into `RenderableSurfaceDamage::Full`. | The authoritative-empty rotation test failed before the fix with `Full` instead of `Empty`. |
| F3 — EGL treated explicit empty damage as contradictory | `draw_scene_with_buffer_age()` forced `Full` whenever `scene_changed` and final output damage was empty, without distinguishing `Some(Empty)` from missing authority. | Focused authority tests now cover both explicit-empty preservation and missing-authority fallback. |
| F4 — every buffer commit reordered global topology | Existing-surface publication unconditionally called `reorder_renderable_surfaces_by_committed_stack()`. | The helper performs whole-scene collection, tree, sort, and reconstruction work; the 1,000-commit test now proves only the initial map reorders. |
| F5 — popup content caused topology and pointer maintenance | Every popup buffer commit refreshed popup membership, raised the surface tree, and refreshed pointer focus. | The 1,000-popup-commit test now proves those services occur once at mapping and zero times for content-only updates. |
| F6 — existing repair infrastructure was available | Surface journals, partial scene-damage mapping, EGL scene-cache geometry reuse, and output buffer-age repair already carried the required concepts. | Existing triple-buffer and damage-planner tests remain green; no second damage system was introduced. |
| F7 — geometry request presence was treated as changed work | Geometry-dependent maintenance ran for repeated identical `XdgWindowGeometry`; cached subsurface commits also promoted based only on request presence. | The actual-value comparison is now used in both callers, with repeated-geometry tests and the 1,000-commit proof. |

## B. Final architecture

### Independent authorities

Surface publication now keeps these concerns separate:

- buffer identity continues to select the current resource, SHM/DMABUF import, lifetime/release lineage, explicit-sync lineage, and Direct Scanout identity;
- client logical damage continues to populate `RenderableSurfaceDamage` and `SurfaceDamageJournal`;
- visual mapping changes—first map, extent/viewport/scale/transform/placement changes, or geometry changes—promote damage conservatively and own the required scene/topology work.

A different same-size buffer no longer promotes damage by itself. First mapping still creates a full renderable surface, so an initial attached buffer without explicit damage remains visible.

### Content versus topology

Existing mapped-surface commits update the buffer and damage journal without rebuilding the global render stack. Reorder is retained for initial insertion and actual placement/topology transitions. Popup membership refresh, tree raising, and pointer-focus refresh are likewise restricted to mapping, placement, or visual-topology transitions.

### Geometry idempotence

`apply_committed_window_geometry()` now returns before derived visual, popup, reactive-child, and dependent maintenance when the committed value is unchanged. Bufferless commits compare the actual stored geometry before deciding whether a full damage cause is required; cached subsurface commits use the same comparison.

### EGL damage authority

The renderer now distinguishes an authoritative `Some(OutputDamage::Empty)` from absent scene-damage authority. Explicit empty damage remains empty; the conservative full fallback is retained only when authority is missing and a changed scene would otherwise have no damage.

## C. Deterministic before/after evidence

The RED tests were run before the production fix. Same-size buffer rotation produced `Full` instead of partial damage, and authoritative empty buffer rotation produced `Full` instead of `Empty`.

After the fix, the root fixture performs an initial map, three A/B/C rotation commits, and 1,000 additional independent same-size content commits. Its final counters are:

```text
buffer rotations                         1,002
partial damage preserved                 1,003
authoritative empty damage preserved         0
mapping-caused full promotions                1
global stack reorders                         1
global stack reorder skips                1,003
identical geometry no-ops                  1,003
popup topology updates                         0
popup pointer refreshes                        0
active-root scene refreshes                    1 (initial mapping only)
```

The popup fixture performs one mapping commit followed by 1,000 independent content commits:

```text
popup topology updates                         1
popup pointer refreshes                        1
global stack reorders                          2 (root and popup mapping)
global stack reorder skips                 1,000
identical geometry no-ops                   1,000
```

The existing output oracle `triple_buffer_swapchain_oracle_matches_full_reference` continues to compare repaired output-buffer contents against a full reference, including age greater than one and a failed swap path. Existing Direct Scanout identity/resource tests remain green, so logical empty/partial damage does not replace buffer identity.

## D. Correctness tests

Focused tests passed:

```text
rtk cargo test --locked surface_frames -- --test-threads=1                         44 passed
rtk cargo test --locked xdg -- --test-threads=1                                    76 passed
rtk cargo test --locked windows_geometry -- --test-threads=1                        3 passed
rtk cargo test --locked direct_scanout -- --test-threads=1                         32 passed
rtk cargo test --locked triple_buffer_swapchain_oracle -- --test-threads=1           1 passed
rtk cargo test --locked kitty_like_resize_swapchain -- --test-threads=1               1 passed
rtk cargo test --locked subsurface -- --test-threads=1                              41 passed
rtk cargo test --locked layer_shell -- --test-threads=1                             51 passed
rtk cargo test --locked scene_damage_authority -- --test-threads=1                   2 passed
rtk cargo test --locked repeated_committed_window_geometry_is_a_derived_work_noop -- --test-threads=1  1 passed
```

The new deterministic tests are:

- `wayland_same_size_buffer_rotation_preserves_partial_damage`;
- `wayland_same_size_buffer_rotation_preserves_authoritative_empty_damage`;
- `wayland_first_map_without_explicit_damage_stays_visually_live`;
- `wayland_content_commits_skip_global_stack_reorder`;
- `wayland_popup_content_commits_skip_popup_topology_maintenance`;
- `repeated_committed_window_geometry_is_a_derived_work_noop`;
- `scene_damage_authority_preserves_explicit_empty_damage`;
- `scene_damage_authority_falls_back_when_damage_is_missing`.

## E. Verification

Passed:

```text
rtk cargo fmt --all -- --check
rtk cargo check --locked
rtk git diff --check
```

The serialized full suite executed 1,892 passing tests and 2 ignored tests. One known/flaky external integration test, `one_child_exit_wakes_the_sigchld_signalfd_once`, observed zero instead of one SIGCHLD event in the aggregate run; the exact standalone retry passed:

```text
rtk cargo test --locked --test sigchld one_child_exit_wakes_the_sigchld_signalfd_once -- --test-threads=1
# 1 passed
```

Clippy was run with `-D warnings`. It remains blocked by 22 errors and one warning in unrelated pre-existing dirty/untracked workspace, tiled-layout, fullscreen, protocol, and test files. No remaining diagnostic points at the surface-commit locality changes after the new popup-placement return warning was removed.

## F. Real-host qualification

Not run in this environment. No CPU, GPU, frame-rate, latency, KMS, NVIDIA, or 165 Hz improvement is claimed.

The user should qualify the existing Typhon telemetry on real KMS hardware with stationary idle, 1000 Hz pointer motion over light and Chromium/Electron clients, populated heavy workspaces, floating/tiled move and resize, pointer-lock, software cursor, and XWayland scenarios.

## G. Remaining risks

- The full aggregate test run retains the external/flaky SIGCHLD failure described above.
- Clippy cleanliness is still blocked by unrelated dirty-tree diagnostics.
- Real-host Chromium/Electron contribution and numerical CPU/GPU improvement remain unmeasured.
- The popup test proves content/topology counters, while the existing popup lifecycle and grab suites provide the destruction, focus, and grab correctness coverage.

## H. Review results

### Review pass 1 — correctness and ownership

Verified that same-size buffer identity changes still update current-buffer ownership and are visible to existing resource/scanout paths; first map remains full; actual geometry changes still trigger mapping work; explicit input-region and lifecycle paths remain separate; popup mapping still refreshes topology and pointer focus; and existing direct-scanout, explicit-sync, subsurface, resize, and output repair tests pass.

### Review pass 2 — adversarial performance/regression

Searched for the old buffer-identity damage predicate, attachment-empty promotion, request-presence geometry promotion, unconditional surface-commit reorder, and unconditional popup maintenance. The normal content path now has no global reorder, popup membership refresh, tree raise, pointer refresh, output-membership reconcile, or geometry-derived assignment unless the corresponding visual/topology state changed. The cached subsurface caller uses actual geometry comparison as well.

The dirty checkout was not reset, cleaned, stashed, restored, or replaced. No prior scheduler, scanout, pacing, occlusion, or workspace changes were discarded.
