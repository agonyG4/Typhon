# Typhon Presented-Scene Retry and Buffer-Age Correctness Closure Report

Date: 2026-08-17

## Baseline and scope

Baseline HEAD:

```text
a08a480fb552e9d26f907390964930d2fdebd698
```

The worktree was already substantially dirty before this closure. It contained unrelated tracked changes across compositor, protocol, launch, XWayland, EGL, and native-output files; a deleted prior SDD report; previous plans/specifications; `.codex/`; `error.txt`; `error.txt.save`; and the earlier uncommitted `NativeSceneHistory` implementation. No reset, restore, clean, stash, staging, branch, or commit operation was performed.

This closure added the presented-scene retry regression, snapshot-relative native damage, explicit buffer-age 1/2/3 coverage, the dedicated retry test module, this plan, and this report.

## Residual defect and hypothesis result

The residual defect was confirmed. The old damage path used `scene_changed`, which was derived from `scene_render_generation != last_rendered_scene_generation`. After rendering B, rejecting B, and retrying B at the same logical generation, that comparison could be false even though the target buffer still represented presented A.

The deterministic regression is `native_output::tests::output_retry::rejected_same_generation_retry_repairs_from_presented_scene`:

```text
present A
render B at generation 7
queue and discard B
assert presented remains A
retry B at generation 7
assert presentation-relative damage is non-empty
```

The test was observed failing before the production change because the old `scene_changed = false` branch returned empty damage for a CSD surface with no decoration delta. It passes after the change.

## Fix and ownership semantics

`NativeSceneSnapshot` now carries the existing render-derived surface identity plus content generation and surface commit sequence. `native_output_damage_for_scene_snapshots` no longer accepts the logical `scene_changed` flag. It compares the presented snapshot with the exact current frame snapshot and repairs:

- old and current visual bounds for geometry changes;
- current surface damage for unchanged visual identity;
- the current surface bounds when content identity changed without usable current damage;
- added and removed surface bounds;
- decoration bounds and decoration visual signatures;
- old and new cursor regions.

Logical bounds remain unclipped until the final output clipping step. A retry is therefore still bounded to the old/new visual regions; there is no rejection-triggered full-output workaround.

`last_rendered_scene_generation` remains in the scheduler timing path for logical render coalescing and repaint-cause decisions. A repository search confirms it is no longer passed to the presented-scene native damage function and cannot suppress presentation-relative repair.

`NativeSceneHistory` remains the ownership authority:

- render completion creates a ready snapshot;
- queue/admission creates a token-keyed submitted snapshot;
- rejection/discard removes the submitted snapshot and leaves presented unchanged;
- only exact confirmed pageflip promotion advances presented history;
- the explicitly synchronous compatibility path promotes immediately because its backend semantics are immediate.

`NativeFrameSceneSnapshot::from_server` is called by `replace_ready_scene` after the frame has been rendered and before rendered-frame callbacks and submission bookkeeping can advance mutable compositor state. Pageflip promotion uses the queued frame snapshot and token; it does not reconstruct state from the current server scene.

## Regression coverage

The pixel-level retry tests compare a partial framebuffer against a clean full-reference framebuffer:

- oversized MacTahoe SSD: A width 2200 → B width 1400, including explicit stale traffic-light and visible titlebar-edge samples;
- oversized CSD geometry without decorations;
- client/content-only generation change with unchanged geometry;
- decoration-only visual-signature change;
- the same-generation rejected-frame retry sequence.

The existing 31-state 2200 → 800 SSD shrink reference remains green. The dedicated edge matrix covers exact widths 2200, 2050, 1900, 1700, 1400, 1000, and 800, shrinking and expanding across both-offscreen, left-offscreen, right-offscreen, and inside placements.

The explicit buffer-age test exercises age 1, 2, and 3 with the oversized SSD shape. It uses `PartialRepaintPlanner` and `NativeSceneHistory`, models intervening confirmed presentations for ages 2 and 3, rejects B, retries B at the same render generation, requires `RepaintMode::Partial`, and compares the repaired framebuffer pixel-for-pixel with clean B. The dedicated planner suite also passes 29 tests.

Scene-history tests cover exact pageflip identity, replacement/discard, stale-token non-regression, and multiple rejected B/C frames followed by a presented D.

## Renderer and client-mode qualification

The CPU/native deterministic path and the shared presented-snapshot damage path are green. The EGL partial repaint planner is green for its full suite, including age and rejection-history behavior. No renderer branch was weakened, and no buffer-age, render-ahead, triple-buffering, KMS-worker, or Direct Scanout feature was disabled.

The full XWayland test family was included in the full test run, but this environment cannot create its filesystem/abstract test sockets because the current workspace path exceeds the platform `SUN_LEN` limit. No live managed XWayland resize qualification is claimed. Fullscreen and Direct Scanout rules were not changed by this closure.

## Validation results

Fresh focused and binary evidence:

```text
native_output::tests::output_retry::*                 3 passed
native_output::tests::output::*                      38 passed
native_output::tests::*                              352 passed
egl_renderer partial repaint tests                  29 passed
oblivion-one binary test suite                      908 passed
```

Fresh required checks:

```text
cargo fmt --check                                  passed
cargo check --locked --all-targets                  passed
git diff --check                                   passed
bash bin/check-source-layout                       failed only at pre-existing limits:
  src/compositor/state/windows.rs: 1517 > 1500
  src/compositor/mod.rs: 816 > 800
  src/compositor/server.rs: 1520 > 1500
cargo clippy --locked --all-targets -- -D warnings failed only at the known pre-existing
  large-variant warning/error for src/xwayland/xwm/event_types.rs:XwmEvent
```

The fresh full `cargo test --locked` run reported 1,647 passed, 37 failed, and 2 ignored. The failures were environment/test-isolation failures: 35 XWayland/Astrea socket cases reported `path must be shorter than SUN_LEN` or follow-on poisoned locks, plus two existing compositor test filesystem races (`AlreadyExists`/`NotFound`). No retry, native damage, scene-history, planner, decoration, or binary-native test failed.

## Native qualification

Native visual qualification was not available. `/dev/dri/renderD128` exists, but `/dev/dri/card0` was not available at the final probe and `target/debug/astreactl status` reported that no Typhon instances were running. Therefore no real DRM/KMS oversized resize, XWayland, CPU/GLES live session, or rejection trace is claimed.

## Corrective commits and remaining uncertainty

No corrective commit was created because the repository already contained unrelated user-owned dirty work. HEAD remains unchanged. The task-owned source and documentation changes are intentionally uncommitted.

Remaining uncertainty is limited to live native qualification and the environment-blocked XWayland/socket cases. The deterministic invariant is covered:

```text
Damage correctness is relative to the scene represented by the presented/buffer history,
not merely to the most recently rendered logical generation.
```

## Final worktree status

The final status was captured with `rtk git status --short`. Existing unrelated changes remain alongside the task-owned files; no cleanup was performed. The report itself is included below as a new untracked file.

```text
 D .superpowers/sdd/2026-08-07-m7-a-desktop-interaction-plan/task-2-report.md
 M Cargo.lock
 M Cargo.toml
 M docs/superpowers/specs/2026-08-11-wayland-selection-idle-inhibit-design.md
 M src/compositor/desktop_window.rs
 M src/compositor/mod.rs
 M src/compositor/protocols/core.rs
 M src/compositor/protocols/data_control.rs
 M src/compositor/protocols/primary_selection.rs
 M src/compositor/state/desktop_window_tests.rs
 M src/compositor/state/fullscreen.rs
 M src/compositor/state/input_resources.rs
 M src/compositor/state/selection_runtime.rs
 M src/compositor/state/surface_focus.rs
 M src/compositor/state/surfaces.rs
 M src/compositor/state/window_decoration.rs
 M src/compositor/state/window_decoration_tests.rs
 M src/compositor/state/windows.rs
 M src/compositor/state/xwayland_mode.rs
 M src/compositor/tests/data_control.rs
 M src/compositor/tests/input_output/pointer_cursor.rs
 M src/compositor/tests/input_output/pointer_cursor_lifecycle.rs
 M src/compositor/tests/input_output/relative_and_constraints.rs
 M src/compositor/tests/primary_selection.rs
 M src/compositor/tests/protocol_buffers.rs
 M src/compositor/tests/protocol_error.rs
 M src/compositor/tests/support/clipboard_dmabuf.rs
 M src/compositor/tests/support/locked_relative.rs
 M src/compositor/tests/support/server_runtime.rs
 M src/compositor/tests/support/subsurface_client.rs
 M src/compositor/tests/support/window_ops.rs
 M src/compositor/tests/xdg.rs
 M src/egl_renderer.rs
 M src/egl_renderer/geometry.rs
 M src/launch_env.rs
 M src/native_output/launch.rs
 M src/native_output/mod.rs
 M src/native_output/output/damage.rs
 M src/native_output/runtime/bootstrap.rs
 M src/native_output/runtime/cycle.rs
 M src/native_output/runtime/cycle/pageflip.rs
 M src/native_output/runtime/cycle_dispatch.rs
 M src/native_output/runtime/kms_worker/rejection.rs
 M src/native_output/runtime/mod.rs
 M src/native_output/runtime/presentation.rs
 M src/native_output/runtime/presentation_ready.rs
 M src/native_output/runtime/presentation_worker.rs
 M src/native_output/runtime/session_io.rs
 M src/native_output/tests/mod.rs
 M src/native_output/tests/output.rs
?? .codex/
?? docs/superpowers/plans/2026-08-14-keyboard-focus-selection.md
?? docs/superpowers/plans/2026-08-15-kms-worker-timing-throughput-closure.md
?? docs/superpowers/plans/2026-08-16-ssd-damage-mactahoe-closure.md
?? docs/superpowers/plans/2026-08-17-oversized-resize-presentation-ghosting.md
?? docs/superpowers/plans/2026-08-17-presented-scene-retry-buffer-age-closure.md
?? docs/superpowers/plans/2026-08-17-windowvisual-post-closure-qualification.md
?? docs/superpowers/specs/2026-08-16-ssd-damage-mactahoe-closure-design.md
?? docs/superpowers/specs/2026-08-17-oversized-resize-presentation-ghosting-design.md
?? docs/superpowers/specs/REPORT-2026-08-17-oversized-resize-presentation-ghosting.md
?? docs/superpowers/specs/REPORT-2026-08-17-presented-scene-retry-buffer-age-closure.md
?? error.txt.save
?? src/native_output/runtime/scene_history.rs
?? src/native_output/tests/output_retry.rs
```
