# Typhon Native Output Damage Authority v1 Closure Report

Date: 2026-08-27

## Result

The native output damage authority closure is implemented in the dirty checkout. The change separates surface identity, logical surface damage, output-buffer repair, journal accounting, and physical presentation.

The two task-owned correctness closures are:

- same-geometry content identity changes with authoritative `Empty` no longer create a native output footprint repaint;
- native no-visual-change paths account owned surface-damage lineage through a distinct settlement disposition without fabricating a physical presentation.

No O1, scheduler, KMS-worker policy, triple-buffer, Direct Scanout admission, VRR, tearing, workspace, Dwindle, XWayland, or cursor policy was changed by this task.

## Pre-change source audit

The audit covered the requested native output, EGL, compositor frame-batch, surface-journal, swapchain, transaction, cursor, and test modules.

Before this change, `native_scene_surface_transition_damage()` in `src/native_output/output/damage.rs` compared `content_generation` and `commit_sequence` and repainted the current surface bounds when those identities changed while local damage was `Empty`. That made identity act as an output-footprint authority.

The existing output-repair authority was already separate and was preserved:

- explicit Atomic output uses `AtomicOutputSlot.last_presented_serial`, `AtomicOutputSwapchain.presentation_serial`, and confirmed-presentation scene/planner history;
- compatibility EGL invalidates its repaint history after swap failure;
- fatal post-draw explicit failures quarantine rendered slot ownership;
- confirmed pageflip completion, rather than render or submit, advances explicit presentation state.

The pre-change native no-primary path resolved a scene, dropped it, and called `finish_no_primary_work()`. When frame work was pending, that helper called `server.finish_frame()`, which created `FramePresentation::software_now(...)`. A native KMS frame that had not been presented therefore received software presentation semantics.

The pre-change Atomic composited renderer captured a global surface-damage token before constructing the final `ResolvedNativeFrameScene`. Its render-skip path marked the output transaction `NoVisualChange` but restored the frame batch as if rendering had failed, dropping the distinction between terminal no-visual completion and retryable render failure.

The initial Atomic modeset already had an `initial_resolved_scene` and filtered lineage capture; its confirmed initial presentation settlement was retained.

## RED tests and corrected proofs

Deterministic failing tests were added before the production fix.

- `identity_only_generation_change_with_empty_damage_stays_empty` initially failed because the identity-only transition produced a non-empty footprint.
- `identity_only_commit_sequence_change_with_empty_damage_stays_empty` covered the second identity axis.
- `no_visual_change_batch_settles_owned_surface_damage_without_presentation` initially observed no settlement baseline where `SurfaceCommitCounter(1)` was required.
- `repeated_no_visual_change_settlement_does_not_lose_empty_journal_history` was tightened to assert the settlement baseline on each iteration; the first version queried only `damage_since` and was correctly rejected as vacuous.

The former `rejected_content_only_retry_repaints_same_geometry` proof was invalid: it declared `Empty` damage but changed the simulated logical pixels. It was replaced with valid identity-only Empty tests. The integrated Direct Scanout oracle was corrected to use the same logical image across buffer identity replacement while still asserting a different buffer identity and an Empty composited sample.

## Exact production changes

`src/native_output/output/damage.rs` now treats same-geometry local `Empty` as no logical output damage. Geometry, mapping, removal, stack/visibility, decoration, cursor, and non-empty local damage paths remain unchanged.

`src/compositor/mod.rs` defines `SurfaceDamageSettlement::{Presented, NoVisualChange}`. The existing monotonic `presented_surface_commits` map remains internal for compatibility, with an English comment documenting that it is now a damage-accounting baseline and not proof of physical presentation. Two bounded counters distinguish the settlement dispositions.

`src/compositor/state/surfaces.rs` factors both public/narrow settlement operations through one keyed journal operation. Generation validation, monotonic protection, journal lookup, global RenderableSurface index lookup, client-cursor lookup, and `HistoryLost -> Full` behavior remain intact.

`src/compositor/state/frame_callbacks.rs` now takes the token owned by a frame batch through `complete_no_visual_change_frame_batch()` and settles it as `NoVisualChange` after safe release completion. The existing callback, presentation-feedback, commit-timing, and FIFO behavior remains the no-visual terminal behavior.

`src/native_output/runtime/presentation_cycle.rs` captures no-primary lineage from the final `ResolvedNativeFrameScene.surface_ids()`, adds only the current software client cursor when software composition actually owns it, and passes the token into the no-visual frame-batch terminal path. It no longer routes this native path through `server.finish_frame()`.

`src/native_output/runtime/presentation_metrics.rs` creates a frame batch only when pending frame work needs ownership, attaches the exact token, completes `NoVisualChange`, and logs `native.no_visual_change` rather than a presentation-like finish event.

`src/native_output/scanout/atomic_egl_gbm.rs` captures exact lineage after final scene resolution and scene-signature validation. An EGL `NoLogicalDamage` result now cancels the unused render slot before GPU ownership, attaches the exact token to the existing batch, and completes the batch as `NoVisualChange`; it does not restore/requeue the batch as a render failure. Actual preparation, GPU, and KMS failure paths retain their prior retry/fatal/quarantine behavior.

## Authority and ownership model

The resulting model is:

```text
surface identity / scene identity
    -> version, stale-scene, resource, Direct Scanout, and transaction identity

surface logical damage
    -> changed logical pixels for the surface

presented output history + target buffer age + slot ownership
    -> repair pixels required by the target output buffer

NoVisualChange
    -> account frozen logical lineage and complete protocol work without physical presentation

confirmed pageflip / backend presentation authority
    -> advance physical output serial, presented scene, and Presented surface lineage
```

Identity is no longer used as a substitute for logical damage. Output repair remains in the renderer/swapchain domain.

## NoVisualChange protocol behavior

The explicit no-visual terminal path now:

- completes eligible frame callbacks through the existing no-visual callback disposition;
- discards `wp_presentation` feedback rather than assigning a software or KMS timestamp;
- discards commit-timing claims according to the existing no-visual contract;
- retains the existing FIFO behavior and does not clear a FIFO barrier merely because no visual latch occurred;
- completes safe frame-batch buffer releases;
- advances only the owned surface journal accounting baseline;
- does not create `FramePresentation::software_now(...)`;
- does not advance explicit output presentation serials or presented scene history;
- does not call EGL presented-transition authority or confirmed pageflip settlement.

The ordinary compositor `finish_frame()` API remains available for genuine software/non-native paths and test support. A repository search found no normal native no-primary call to it after this change.

## Exact frame and cursor lineage

For composited native frames, the token is captured only after final scene resolution and scene-signature validation. Primary samples are exactly the surfaces in `ResolvedNativeFrameScene`, so inactive-workspace and fullscreen-culled surfaces are absent.

Software client-cursor lineage uses the frozen render-time `client_cursor_render_state()` identity only when software composition is active. Hidden and theme cursors add no client surface sample.

Hardware cursor lineage remains owned by the existing frozen `NativeCursorImageKey`/`NativeCursorSourceKey::Client` path. Bundled hardware cursor content is sampled by its frozen source key. Independent cursor-only `PlaneDelta` transactions retain their own exact cursor token and settle it on their matching pageflip. Theme cursor sources carry no client surface token.

Direct Scanout remains surface-local: its candidate surface and any exact frozen client cursor source are sampled without global capture. Its resource/buffer identity remains separate from composited logical damage.

## Output-slot repair and rejection proof

No output repair implementation was moved into compositor surface damage. Existing deterministic tests continue to cover:

- buffer age 1/2/3+ and invalid-history full repair;
- confirmed-presentation-only planner/history advancement;
- failed swap invalidation in compatibility EGL;
- explicit rendered candidates not advancing history before matching presentation;
- discarded/fatal rendered candidates retaining the existing quarantine or invalidation ownership policy;
- rejected and dropped output attempts not consuming surface presentation lineage.

The corrected integrated oracle keeps rendered, submitted, and presented states distinct and compares final physical output against a full-reference logical image.

## Deterministic tests and evidence

Focused post-change results:

- native output damage: 43 passed;
- output retry/repair: 3 passed;
- integrated swapchain oracle: 7 passed;
- presentation transactions: 57 passed;
- explicit scanout: 66 passed;
- cursor/plane paths: 59 passed;
- pageflip paths: 67 passed;
- EGL buffer-age/damage filter: 33 passed;
- compositor frame consumption: 41 passed;
- compositor surface publication/journal tests: 15 passed;
- compositor surface-frame protocol tests: 46 passed;
- additional buffer-age filter: 2 passed;
- no-visual-change filter: 5 passed;
- surface-damage filter: 13 passed.

The key deterministic evidence is:

- identity-only generation and commit-sequence changes with same geometry and `Empty` produce empty native output damage;
- 128 Empty journal commits, exceeding the journal capacity of 64, settle their baseline on every iteration without `HistoryLost`;
- a subsequent 3x5 Partial entry remains `DamageSince::Known` rather than becoming Full;
- a no-visual frame batch settles the exact sampled commit and records a `NoVisualChange` settlement counter, with no Presented settlement counter;
- the integrated oracle proves Empty logical content accounting leaves the physical presentation serial unchanged while advancing surface accounting;
- the corrected Direct Scanout proof preserves the logical image while changing the buffer identity.

## Full verification

Passed:

```text
rtk cargo fmt --all -- --check
rtk cargo check --locked
TMPDIR=/tmp rtk cargo test --locked
rtk git diff --check
```

The final full locked suite result was 3,033 passed and 5 ignored, including the final no-visual oracle regression.

`rtk cargo clippy --locked --all-targets --all-features -- -D warnings` did not pass. It reported 22 errors and 1 warning, all in existing dirty-checkout areas outside this task’s new logic: workspace protocol/style, `SurfaceOpaqueRegion` derive, tiled layout/resize, fullscreen style, adaptive buffering, layout constraints/solve, window-interaction tests, and related pre-existing source. No diagnostic referenced the task’s changed damage, frame-batch, presentation-cycle, Atomic render-skip, or integrated-oracle logic. No unrelated code was changed to manufacture a green result.

`rtk run "bin/check-source-layout"` did not pass. It reported existing file-size violations in compositor test/support files, compositor state/window files, `src/compositor/state/surfaces.rs`, `src/compositor/mod.rs`, `src/compositor/server.rs`, native bootstrap/presentation-cycle/input routing, and XWayland files. These files were already part of the broad dirty checkout and the task did not split unrelated modules to evade the limit.

## Source-scan locality review

The relevant production call sites were re-searched after implementation.

- The global `capture_surface_damage_presentation()` API remains only for the legacy `mark_render_damage_presented()` compatibility/test wrapper. No normal native frame calls it.
- Normal composited native capture uses filtered surface IDs from the final scene and exact cursor commit/source-key additions.
- Initial modeset capture uses `initial_resolved_scene` and remains exact.
- Direct Scanout capture is candidate-local.
- Cursor sidecar capture is source-key-local.
- Pageflip settlement consumes each token entry through keyed generation/journal/index lookups; it does not filter the global renderable vector or all cursor surfaces for each sample.
- Remaining vector retain/drain/reorder operations are topology/cold-path operations, not ordinary content or presentation settlement.

## Adversarial review pass 1 — damage authority and ownership

Reviewed: new BufferId/commit identity/generation with Empty, Partial and Full damage, move/resize/map/unmap, workspace and fullscreen scene changes, software cursor, hardware cursor-only PlaneDelta, Direct Scanout A-to-B with identical logical contents, Atomic no-logical-damage, compatibility skip, rejected render, KMS failure, pageflip absence, surface destruction, stale generation, newer commit after render, XWayland and popup/subsurface paths.

Findings:

- identity-only Empty needed the deleted footprint fallback; fixed by removing the identity branch;
- no-visual frame batches owned tokens but did not settle them; fixed with explicit `NoVisualChange` settlement;
- Atomic renderer skip restored batches as render failures; fixed with terminal no-visual completion;
- the cursor-plane branch is evaluated before primary no-work handling, so deferred or required hardware cursor work is not silently consumed by the no-primary path;
- no further task-owned correctness issue remained after the focused and full tests.

## Adversarial review pass 2 — locality and accidental scans

Reviewed every relevant occurrence of global renderable vector searches, retains, drains, global presentation capture, settlement calls, `finish_frame`, `FramePresentation::software_now`, scene-history transitions, output serial advancement, cursor sidecar ownership, and source layout.

Findings:

- no identity-to-footprint fallback remains in native scene transition damage;
- no normal native no-primary path calls `finish_frame()` or creates a software presentation;
- normal native composited capture is bounded by exact sampled surfaces;
- settlement is bounded by token entries and keyed state access;
- physical output serial/history advancement remains behind actual output presentation completion;
- the remaining global capture wrapper is explicitly legacy/test compatibility and is not a normal native-frame fallback;
- no further task-owned locality issue remained.

## Remaining uncertainty and known repair boundary

Authoritative client Empty is now preserved as Empty for same-geometry content transitions. A target output buffer with invalid or insufficient age/history can still require conservative full or repaired repaint; that is intentional output-buffer repair authority and is not derived from content identity. Existing rejected-output repair/quarantine tests remain green.

The internal `presented_surface_commits` name is retained to avoid a broad public/internal rename; its field comment and explicit settlement enum define its post-change accounting meaning.

The deterministic tests prove the ownership and accounting transitions, but no real TTY/DRM/KMS/165 Hz qualification was executed. No claim is made about a measured hardware frame-rate improvement or the user’s observed approximately 30 FPS symptom.

## Hardware qualification

Real TTY/DRM/KMS/165 Hz testing: **not executed**.
