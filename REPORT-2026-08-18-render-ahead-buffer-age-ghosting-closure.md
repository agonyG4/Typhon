# Render-Ahead / Buffer-Age Ghosting Closure Report

Date: 2026-08-18

## 1. Baseline and dirty worktree

Baseline HEAD recorded before this task:

```text
2fc5fd1528f614eb5bae8a6491d0aee80f2975de
```

The worktree already contained substantial tracked compositor, renderer, native-output, test, and dependency changes from the preceding closures, plus untracked reports, plans, `.codex/`, `scene_history.rs`, and retry tests. The pre-change diff stat covered 63 tracked files with approximately 3,730 insertions and 248 deletions. No destructive cleanup was used. Those changes remain preserved and task changes were not swept into a broad commit.

## 2. User runtime and qualification boundary

The requested normal native command is:

```bash
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
./bin/start-oblivion-one-tty
```

The launcher keeps Direct Scanout off unless explicitly overridden. The target configuration is therefore native composition with partial repaint, buffer age, and adaptive triple buffering.

Native DRM device discovery and libseat acquisition succeeded. The first retry used the old `target/release/oblivion-one` binary and exposed a missing scene-history enqueue as `composited pageflip has no matching scene transition`. After rebuilding the release binary, the exact command reached Atomic EGL/GLES, KMS, Xwayland, and the shell session and remained alive for the 20-second qualification window. No native move/resize stress was performed in that short run.

## 3. Findings

### CONFIRMED

`PartialRepaintPlanner` used one `current_damage` value both as render-time repair input and as the damage inserted into presentation history. Under render-ahead, the render predecessor and the confirmed pageflip predecessor can differ.

Explicit Atomic slot age is indexed by confirmed presentation serial. Its journal therefore must contain confirmed presentation transitions, not render-time repair damage.

### STRONG HYPOTHESIS

The defect can permanently preserve B-only move/resize pixels in a slot and expose them again when that slot rotates back through a valid age. This matches the reported persistent and reappearing titlebar/button ghosts.

### NATIVE-PROVEN

Native startup is proven through the first compositor frame and Xwayland/shell readiness after rebuilding the release binary. Native ghost-free move/resize stress and triple-off/full-repaint causal comparison remain unproven.

### UNPROVEN

Whether the separate report of complete old-frame rollback during ordinary video playback has an independent KMS/submission-order cause remains open.

## 4. Failing regression and fix

The pre-fix regression constructed:

```text
A presented
render B
render C ahead while A is still presented
present B
present C
```

It expected the newest journal transition to be B→C, but the old implementation stored A→C. The failure was deterministic: the journal contained the render-time A→C rectangle instead of the presentation-time B→C rectangle.

The fixed regression is:

```text
presentation_journal_uses_actual_predecessor_after_render_ahead
```

and passes.

## 5. Chosen architecture

Render and presentation damage are now separate:

```text
RepaintPlan::render_damage
RepaintPlan::repair_damage
        ↓
render the target slot

NativeSceneHistory submitted snapshot + confirmed predecessor
        ↓ pageflip-time preparation
PresentedTransitionDamage
        ↓
presentation-domain journal
```

`PartialRepaintPlanner::commit_presented_transition()` accepts an explicit transition damage value. `RepaintPlan` no longer has authority over presentation history.

For explicit Atomic output, pageflip settlement now:

1. finds the submitted scene snapshot by pageflip token;
2. computes the transition from the actually presented snapshot to that submitted snapshot;
3. records a bounded `PresentedTransition` trace event;
4. completes the Atomic swapchain using that transition damage;
5. validates completed frame ID and transaction ID;
6. commits the explicit journal transition;
7. promotes `NativeSceneHistory` for the matching token.

If no composited predecessor is valid, the transition is full-output damage. This is a genuine history boundary, not a generic render-ahead workaround.

The transition preparation is non-mutating and uses exact submitted snapshots. Mutable current compositor state is not consulted at pageflip.

## 6. Token and frame identity

The scene history is keyed by submitted pageflip token and exact frame snapshot. Atomic swapchain completion additionally validates its pool generation and completed frame identity. The pageflip path rejects a scene-history/frame mismatch and rejects a transaction mismatch instead of inserting guessed damage.

The trace now records, for composited frames, frame ID, render generation, scene/snapshot signatures, render and repair damage signatures, slot, age, and the scene frame that was presented when rendering occurred. A separate `PresentedTransition` event records the actual predecessor/current frame IDs and transition signature. Direct Scanout continues to use a Direct identity event and is not given a fabricated composited signature.

## 7. Pixel regressions

The deterministic movement oracle warms three modeled slots, renders B and C ahead, presents A→B→C, then reuses the physical B slot. A small C→D update must repair B-only pixels. The test:

```text
presentation_domain_journal_clears_b_only_pixels_from_reused_slot
```

passes and compares the partial result with a fresh reference.

The resize oracle performs the same sequence with an intermediate B-only right edge/titlebar region:

```text
presentation_domain_journal_clears_b_only_resize_edge_from_reused_slot
```

passes.

Existing geometry, decoration, cursor, content-only, oversized SSD, fullscreen, and retry tests continue to pass. Relevant tests include `native_output_damage_for_window_move_covers_old_new_surface_bounds`, `native_decoration_damage_covers_old_new_state_change_and_disappearance`, `rejected_decoration_only_retry_keeps_decoration_damage`, and `rejected_content_only_retry_repaints_same_geometry`.

## 8. Buffer-age results

`triple_buffer_swapchain_oracle_matches_full_reference` now models physical slot contents and presentation serials rather than supplying arbitrary age values. It exercised ages 1, 2, and 3 and passed pixel-for-pixel reference comparison.

The explicit Atomic age model remains presentation-serial based. The journal now uses the same domain:

```text
P0 → P1 → P2 → P3
```

produces newest journal entries equivalent to P2→P3, P1→P2, and P0→P1.

## 9. Rejection, retry, and delayed presentation

Scene-history tests cover:

```text
A presented, B rejected, C presented  => A→C
A presented, B presented, C presented => A→B, B→C
B presented, C delayed, D ready       => B→C when C pageflips
B presented, C discarded, D presented => B→D
```

The submitted C snapshot remains authoritative even if logical compositor state advances to D before C pageflips. Existing output retry and out-of-order ownership tests pass.

## 10. KWin and Hyprland comparison

The relevant KWin lesson is that the damage journal and swapchain age must share one sequence domain; its explicit sequence-based journal indexing reinforces that rule. The relevant Hyprland lesson is that buffer-age accumulation is tied to the output damage ring and rendering/swapchain sequence, rather than to an unrelated raw logical scene.

Typhon retains its stronger explicit KMS presentation-token model. It applies the shared invariant by keeping render repair in the render domain and confirmed Atomic journal transitions in the presentation domain.

## 11. Compatibility EGL

The compatibility EGL/GBM path was not mechanically changed to use KMS pageflip predecessor semantics. Its successful `eglSwapBuffers` settlement remains in the EGL surface's swap/render sequence domain and passes the frame render damage as the matching transition value for that backend.

This backend distinction is documented in the design specification. Explicit Atomic output uses confirmed presentation serials; compatibility EGL must continue to use the domain represented by its EGL buffer age.

## 12. Previous fullscreen closure and bootstrap

The previous resolved-frame-scene work remains intact. Fullscreen snapshot identity, buffer-age fullscreen reference tests, retry tests, and Direct/Composited separation pass. No Direct Scanout expansion was made here.

Bootstrap was audited. The initial native scene snapshot is created only after the initial scene was resolved and rendered, and the explicit backend promotes the initial rendered frame before `NativeSceneHistory::new` receives it. No additional fake frame-zero bootstrap was introduced.

## 13. Native diagnostic matrix

The first pre-rebuild attempt used the stale release executable and failed at the new invariant:

```text
native bootstrap: composited pageflip has no matching scene transition
```

The fix queues the exact ready scene after Atomic accepts its pageflip token. After `cargo build --locked --release`, the normal command reached native startup, Xwayland readiness, and the shell session and was stopped only by the diagnostic timeout (`exit 124`). `OBLIVION_ONE_TRIPLE_BUFFERING=off` and `OBLIVION_ONE_FORCE_FULL_REPAINT=1` were not rerun after this final rebuild. Direct Scanout ON was not qualified.

Persistent native-buffer acceptance after interaction was not observable in the 20-second startup run. No claim is made that native ghosting or independent complete-frame rollback is eliminated.

## 14. Validation

Passed:

```text
cargo fmt --all -- --check
cargo check --locked --all-targets
git diff --check
```

Focused passing suites included:

```text
PartialRepaintPlanner: 33 passed
NativeSceneHistory: 10 passed
native_output::tests::output: 41 passed
fullscreen_frame_scene: 3 passed
output_retry: 3 passed
```

The full test baseline and post-change result remained:

```text
1147 passed; 539 failed; 2 ignored
```

The failures are dominated by the existing environment's compositor socket `PermissionDenied/Operation not permitted` setup failures and two KMS FD duplication tests. No new failure category was observed.

`cargo clippy --locked --all-targets -- -D warnings` remains blocked by the pre-existing `XwmEvent` large-enum lint in `src/xwayland/xwm/event_types.rs`. `bash bin/check-source-layout` remains blocked by the pre-existing limits in `src/compositor/state/windows.rs`, `src/compositor/server.rs`, and `src/compositor/mod.rs`. No new source-layout violation was introduced.

## 15. Commits and remaining blockers

No new commit was created. The task-owned renderer/native-output changes are interleaved with the preceding dirty closure implementation, so staging a focused commit without sweeping unrelated work was not safe. The new design and plan documents are present.

Remaining qualification blocker:

```text
the current user is not in the `input` group, so interactive keyboard/mouse stress requires the repository's input-permission installer and a new login session.
```

The separate whole-frame rollback investigation also remains open unless a future native trace proves it is the same defect.

## 16. Final worktree status

Captured with:

```bash
git status --short
```

The final output is recorded below after the report was created and the final verification pass completed.

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
 M src/native_output/tests/fullscreen_frame_scene.rs
 M src/native_output/tests/mod.rs
 M src/native_output/tests/output.rs
?? .codex/
?? REPORT-2026-08-17-fullscreen-frame-scene-authority-closure.md
?? REPORT-2026-08-18-render-ahead-buffer-age-ghosting-closure.md
?? docs/superpowers/plans/2026-08-14-keyboard-focus-selection.md
?? docs/superpowers/plans/2026-08-15-kms-worker-timing-throughput-closure.md
?? docs/superpowers/plans/2026-08-16-ssd-damage-mactahoe-closure.md
?? docs/superpowers/plans/2026-08-17-oversized-resize-presentation-ghosting.md
?? docs/superpowers/plans/2026-08-17-presented-scene-retry-buffer-age-closure.md
?? docs/superpowers/plans/2026-08-17-windowvisual-post-closure-qualification.md
?? docs/superpowers/plans/2026-08-18-render-ahead-presentation-damage-domain-closure.md
?? docs/superpowers/specs/2026-08-16-ssd-damage-mactahoe-closure-design.md
?? docs/superpowers/specs/2026-08-17-oversized-resize-presentation-ghosting-design.md
?? docs/superpowers/specs/2026-08-18-render-ahead-presentation-damage-domain-design.md
?? docs/superpowers/specs/REPORT-2026-08-17-oversized-resize-presentation-ghosting.md
?? docs/superpowers/specs/REPORT-2026-08-17-presented-scene-retry-buffer-age-closure.md
?? error.txt.save
?? src/native_output/runtime/scene_history.rs
?? src/native_output/tests/output_retry.rs
```
