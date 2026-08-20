# Typhon Fullscreen / Restore Ghosting — Frame-Scene Authority Closure Report

Date: 2026-08-18

## 1. Baseline and dirty-worktree boundary

The requested snapshot hash in the task description did not match the repository checkout. The recorded baseline was:

```text
HEAD before this closure: abe4a9421ae372d423f359b8994a85aa85076a94
```

The initial `git log --oneline -15`, `git status --short`, `git diff --stat`, and `git diff --name-only` were recorded before editing. The worktree was already dirty across Cargo metadata, compositor protocol/state/tests, EGL/native output, launch environment, and earlier closure documentation, with approximately 49 pre-existing dirty entries and a pre-existing diff of about `+2911/-151`, plus untracked closure documents and artifacts.

No reset, restore, checkout, clean, or stash command was used. The pre-existing dirty work was preserved. The current production changes are still mixed with that baseline and were not staged with unrelated files.

Focused commits created during this closure:

```text
8b7ea5b docs: design fullscreen frame-scene authority closure
23207a1 test(render): reproduce fullscreen frame-scene mismatch
2fc5fd1 test(render): cover fullscreen restore buffer ages
```

## 2. Reproduction and evidence classification

The user-supplied native reproduction is:

```text
normal desktop → fullscreen → leave fullscreen
```

with stale titlebars, decorations, borders, background regions, popping artifacts, and possible old complete frames resurfacing. The native reproduction was not rerun in this environment because no usable TTY/DRM qualification session was available.

The findings are classified as follows:

- **CONFIRMED:** before the fix, solitary fullscreen rendering used the filtered native surface list while frame history used the raw logical surface list.
- **CONFIRMED:** the raw snapshot also reconstructed decorations from culled surfaces, so hidden SSD identities could enter presented history.
- **CONFIRMED:** DirectPrimary presentation was able to enter composited scene-history machinery even though no compositor framebuffer was rendered.
- **CONFIRMED:** bootstrap history was sourced from mutable logical state rather than an exact resolved rendered plan.
- **CONFIRMED:** Direct → composited history was incomplete because the compositor scene history was not invalidated alongside output damage history.
- **STRONG HYPOTHESIS:** delayed fullscreen/restore configure convergence could produce mixed placement and committed-size geometry.
- **UNPROVEN:** every whole-screen rollback during ordinary video playback is caused by this scene mismatch.

## 3. Failing test proving the original mismatch

Commit `23207a1` captured the red regression before production changes. Its pre-fix observation was:

```text
renderer surface IDs: [102]
snapshot surface IDs: [101, 102]
```

The test fixture contains two decorated windows, makes the front owner solitary fullscreen, and compares the renderer’s filtered identities with the old snapshot construction. The fixed test now asserts that the resolved plan and snapshot have identical ordered surface IDs and that the solitary fullscreen snapshot has no SSD for either the fullscreen owner or the culled rear window.

The restore-damage test additionally proves that returning rear-window content and its SSD are included in the transition damage. The age-matrix test runs a deterministic normal → F1…F20 → restore sequence and compares reused framebuffer pixels with a fresh reference for buffer ages 1, 2, and 3.

## 4. Final resolved-frame architecture

`ResolvedNativeFrameScene<'a>` in `src/native_output/runtime/frame.rs` is now the authority for a composited native frame. It contains:

```text
exact ordered surfaces
decorations derived from that exact surface slice
popup surface IDs
external overlay surface IDs
resolved render generation
fullscreen visibility metrics
compact NativeSceneSnapshot
```

The normal path borrows surface storage. The filtered fullscreen path owns only the filtered `RenderableSurface` list already produced by the compositor; it does not clone complete client pixel buffers into presentation history.

CPU rendering, GLES request construction, current-scene damage, and composited frame snapshots consume this authority. `NativeFrameSceneSnapshot::from_server` is no longer used for rendered frames; rendered snapshots are created with `from_resolved_frame_scene`.

The plan’s compact identity includes ordered surface IDs, resolved bounds, content generation, commit sequence, decoration identity/bounds/visual signature, popup/external classification, and visibility signature. Debug assertions compare renderer and snapshot surface/decor identities.

The explicit atomic path has a Rust borrow boundary because protocol bookkeeping mutates `OwnCompositorServer` while renderable surfaces are borrowed. It therefore resolves the immutable damage plan before bookkeeping and resolves the actual render plan after bookkeeping. A signature is carried across that boundary; if the signatures differ, atomic rendering is rejected before GPU sampling, the transaction is settled as render preparation failure, the frame batch is restored, and the swapchain slot is cancelled. The actual GLES render and its snapshot always consume the same resolved plan. This turns a potential divergence into an explicit failure rather than allowing mismatched pixels/history to proceed.

## 5. Direct Scanout ownership

DirectPrimary no longer calls the composited `replace_ready_scene` path and no longer queues a fake `NativeFrameSceneSnapshot`. Direct diagnostics record direct identity fields instead:

```text
transaction ID
surface ID
client framebuffer ID
content epoch
candidate key
submission/pageflip token
```

On confirmed Direct entry, both histories are invalidated:

```text
EGL/output partial-repaint history
NativeSceneHistory composited predecessor/ready/submitted state
```

On Direct replacement, the direct ownership remains direct. A rejected Direct admission does not mutate composited scene history. A returned composited frame creates a real resolved scene and is repaired from an unknown composited predecessor. The existing `PartialRepaintPlanner` ordering was corrected so invalidated history is checked before an empty-current-damage early `Skip`.

The deterministic scene-history tests cover token ordering, rejected frames, Direct no-promotion, Direct invalidation, returned composition, and out-of-order pageflip protection.

## 6. Bootstrap history

Bootstrap no longer fabricates a frame-zero snapshot from raw mutable server state. It resolves one plan, renders the initial native frame, and derives the initial compact snapshot from that exact plan before the initial KMS ownership is promoted. `NativeSceneHistory` retains an `Option` internally and Direct transitions clear the confirmed composited scene. The initial modeset/pageflip sequence was not qualified on real DRM here, so native confirmation of the initial presentation remains an environmental qualification item.

## 7. Fullscreen visibility and overlays

Fullscreen composited visibility is now decided by the explicit fullscreen render facts: owner, output coverage, minimized state, and popup policy. It is no longer derived from `direct_scanout_scene_candidate().is_ok()`.

Direct Scanout still applies its stricter dmabuf, format/modifier, geometry, sync, cursor, and KMS admission requirements. Existing allowed overlay semantics are preserved. Popup and external overlay IDs are frozen in the resolved plan and carried into the snapshot, so renderer, damage, and history cannot independently classify them.

## 8. Fullscreen/restore visual geometry

Fullscreen, maximize, and restore now install a target `ToplevelVisualGeometry` through the existing visual geometry path before the next frame resolves. The override remains authoritative until the committed client geometry equals the target, then retires through reconciliation.

The same geometry drives root surface assignment, SSD layout/button anchoring, decoration hit testing, and visual damage bounds. The delayed-client test proves that a pending fullscreen configure uses the fullscreen target before commit, while a pending restore configure uses the saved floating target rather than the still-committed fullscreen size. The repeated stress test performs 100 enter/commit/restore/commit cycles and asserts the override never exposes a mixed geometry.

XWayland mode transitions continue to use the existing X11 visual geometry path and were not given an application-specific workaround. Full XWayland client tests could not run because the repository’s socket tests are blocked by the environment permission failure described below.

## 9. Damage, buffer age, and framebuffer references

`NativeSceneSnapshot` is compact and excludes client pixel ownership. Scene-relative damage compares the confirmed presented composited snapshot with the exact current resolved snapshot, including surface bounds/content/commit identity, decorations, popup/external classification, and visibility signature.

The deterministic fullscreen regression:

```text
normal
→ fullscreen F1 … F20
→ restore
```

uses a synthetic framebuffer painter, rotates the conceptual reused-slot state, samples stale-pixel locations for the rear titlebar, both SSD/button regions, fullscreen content, and background, and compares the result with a fresh full-reference restore framebuffer. Ages 1, 2, and 3 all pass.

This is a deterministic scene/framebuffer model, not a real GBM/EGL scanout capture. The existing oversized-SSD retry matrix also passes for ages 1/2/3.

## 10. CPU/GLES parity

Both renderer entry points now consume the same `ResolvedNativeFrameScene` surface, decoration, overlay, ordering, and generation inputs by construction. `cargo check --all-targets` and the focused native output suites pass. No real GPU/DRM CPU-versus-GLES framebuffer comparison was possible in this environment; that remains part of native qualification.

## 11. Direct Scanout OFF/ON qualification

Neither native mode was claimed:

- **Direct Scanout OFF:** no TTY/DRM native session was available to run the requested real fullscreen/video/restore sequence.
- **Direct Scanout ON:** no usable hardware/direct-scanout session was available.
- **Direct candidate fallback:** deterministic admission/rejection and pacing model tests pass; no native KMS trace was captured.

The source/test closure is independent of Direct Scanout being enabled, and the pure composited fullscreen identity and restore tests pass with no physical scanout dependency.

## 12. Whole-frame rollback boundary

The separate report of complete old frame rollback during ordinary video playback remains **UNPROVEN and unresolved by this closure**. No claim is made that this scene-authority fix eliminates a KMS submission or pageflip-ordering rollback.

If it persists after native qualification, the bounded trace added here should be collected for:

```text
logical frame ID
resolved scene signature
render path
swapchain slot
GBM framebuffer ID
transaction ID
submission token
pageflip token/sequence
presented framebuffer ID
```

An old framebuffer presented under a newer token would be a separate KMS/P0 issue, not a reason to add more generic damage repainting.

## 13. Validation results

Passing focused suites:

```text
fullscreen_frame_scene: 3 passed
scene_history tests: 5 passed
window_decoration_tests: 13 passed
partial_repaint_tests: 30 passed
presentation pacing tests: 29 passed
native output output tests: 41 passed
direct native-output bin suite: 181 passed
```

Passing structural checks:

```text
cargo fmt --all -- --check
cargo check --locked --all-targets
git diff --check
cargo clippy --locked --all-targets -- -D warnings \
  -A clippy::large_enum_variant \
  -A clippy::too_many_arguments \
  -A unfulfilled_lint_expectations \
  -A clippy::redundant_closure
```

Known baseline/environment failures:

```text
cargo test --locked
  1147 passed, 539 failed, 2 ignored
  failures are primarily socket/client setup with EPERM/Operation not permitted;
  native KMS FD tests also fail with the restricted environment

cargo clippy --locked --all-targets -- -D warnings
  unchanged baseline lint: clippy::large_enum_variant in
  src/xwayland/xwm/event_types.rs:XwmEvent

bash bin/check-source-layout
  unchanged baseline oversized files:
  src/compositor/state/windows.rs:1519 (limit 1500)
  src/compositor/server.rs:1520 (limit 1500)
  src/compositor/mod.rs:817 (limit 800)
```

The focused rendering/scene/geometry tests introduced no failures.

## 14. Performance and safety review

This closure does not globally disable buffer age, triple buffering, render-ahead, KMS worker operation, or Direct Scanout. It does not synchronously wait for pageflip and does not clone complete pixel buffers into history. Full repair is used for first/unknown composited history and the real Direct → composited ownership discontinuity; ordinary scene damage remains snapshot-relative.

## 15. Remaining blockers

1. Run native TTY/DRM qualification with the MacTahoe theme, Direct Scanout disabled, enabled, and candidate-fallback cases.
2. Capture the bounded KMS trace if complete old-frame rollback persists during ordinary video playback.
3. Re-run the full socket/XWayland suite in an environment that permits compositor socket and KMS FD operations.
4. Resolve the pre-existing source-layout and `XwmEvent` clippy baseline issues separately; they were not expanded in this closure.

## 16. Final worktree status

The following is the final `git status --short` snapshot. The report itself is intentionally left uncommitted so its required status section remains truthful. All listed pre-existing dirty paths were preserved.

```text
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
 M src/compositor/state/surface_commits.rs
 M src/compositor/state/surface_focus.rs
 M src/compositor/state/surfaces.rs
 M src/compositor/state/task_05_8_tests.rs
 M src/compositor/state/window_decoration.rs
 M src/compositor/state/window_decoration_tests.rs
 M src/compositor/state/window_interaction_tests.rs
 M src/compositor/state/window_resize.rs
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
 M src/native_output/tests/mod.rs
 M src/native_output/tests/output.rs
?? .codex/
?? REPORT-2026-08-17-fullscreen-frame-scene-authority-closure.md
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
