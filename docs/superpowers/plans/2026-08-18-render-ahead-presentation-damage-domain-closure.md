# Render-Ahead Presentation-Damage Domain Closure Plan

Date: 2026-08-18

The working-tree baseline is intentional and includes earlier fullscreen
scene-authority, retry/rejection, buffer-age, and visual-geometry work. Do not
reset, restore, clean, stash, or stage unrelated dirty files.

## Task 1 — Red A/B/C journal regression

* Exact files: `src/egl_renderer/damage_tests.rs`.
* Interfaces consumed: `PartialRepaintPlanner::plan`, `RepaintPlan`, planner
  history.
* Interface produced: a regression representing A presented, B rendered, C
  rendered ahead, B pageflip, C pageflip.
* Failing test: `presentation_journal_uses_actual_predecessor_after_render_ahead`.
* Expected pre-fix failure: the newest journal entry is C's render-time A→C
  damage instead of the actual B→C transition.
* Minimal implementation: none before the red run.
* Verification: run the exact filtered test and record the assertion output.
* Commit boundary: test-only commit if task-owned staging is separable.

## Task 2 — Separate render and presentation damage

* Exact files: `src/egl_renderer/damage.rs`,
  `src/egl_renderer.rs`, `src/egl_renderer/damage_tests.rs`,
  `src/native_output/scanout/egl_gbm.rs`.
* Interfaces consumed: `RepaintPlan`, `EglSceneFrameCommit`, EGL swap
  completion.
* Interface produced: `RepaintPlan::render_damage` and
  `PartialRepaintPlanner::commit_presented_transition(OutputDamage)`.
* Failing test: the Task 1 regression.
* Expected pre-fix failure: production planner history is populated from the
  render plan.
* Minimal implementation: remove the production `commit_presented(plan)`
  path, rename the render-domain field, and pass compatibility EGL's own
  swap-domain transition explicitly.
* Verification: focused damage tests and `cargo check --locked --all-targets`.
* Commit boundary: refactor/render damage-domain commit.

## Task 3 — Prepare exact scene transitions

* Exact files: `src/native_output/runtime/scene_history.rs`,
  `src/native_output/output/damage.rs`.
* Interfaces consumed: token-keyed submitted snapshots,
  `native_output_damage_for_scene_snapshots`.
* Interface produced: `PreparedNativePresentationTransition` and
  `NativeSceneHistory::prepare_pageflip_transition`.
* Failing test: `pageflip_transition_uses_the_actual_presented_predecessor`.
* Expected pre-fix failure: no API can calculate B→C after C was rendered
  while A was still presented.
* Minimal implementation: compare the exact presented and token-matched
  submitted snapshots, including current cursor states; return full damage for
  no predecessor.
* Verification: scene-history tests for B→C, rejection A→C, and logical state
  advancing beyond a still-pending C.
* Commit boundary: scene-history transition preparation.

## Task 4 — Wire explicit Atomic pageflip settlement

* Exact files: `src/native_output/runtime/cycle/pageflip.rs`,
  `src/native_output/scanout/atomic_egl_gbm.rs`.
* Interfaces consumed: Atomic swapchain pageflip completion,
  `PreparedNativePresentationTransition`.
* Interface produced: Atomic renderer settlement with explicit
  `PresentedTransitionDamage`.
* Failing test: Atomic/pageflip ownership tests plus scene-history transition
  tests.
* Expected pre-fix failure: Atomic completion has no presentation-domain
  transition argument and promotes render-time damage.
* Minimal implementation: prepare before mutation, complete the matching
  swapchain token, pass transition damage to the renderer, validate frame and
  transaction IDs, then promote scene history.
* Verification: focused native output, pageflip, and retry suites.
* Commit boundary: explicit Atomic presentation settlement.

## Task 5 — Cross-ledger identity validation

* Exact files: `src/native_output/runtime/cycle/pageflip.rs`,
  `src/native_output/scanout/output_swapchain.rs`,
  `src/native_output/runtime/scene_history.rs`.
* Interfaces consumed: pageflip token, completed frame ID, transaction ID,
  output pool generation.
* Interface produced: fatal mismatch diagnostics and safe history refusal.
* Failing test: a mismatched scene/frame pageflip fixture.
* Expected pre-fix failure: scene history and output completion could be
  promoted independently.
* Minimal implementation: reject a transition when token, frame, transaction,
  or pool-generation ownership does not match.
* Verification: existing stale-token and output-swapchain identity tests,
  plus the new explicit mismatch assertion.
* Commit boundary: presentation ownership validation.

## Task 6 — Movement pixel oracle

* Exact files: `src/egl_renderer/damage_tests.rs`,
  `src/native_output/tests/output.rs`.
* Interfaces consumed: explicit planner history, physical slot model,
  `AtomicOutputSlot::buffer_age` semantics.
* Interface produced: B-only titlebar/button pixel regression.
* Failing test: `presentation_domain_journal_clears_b_only_pixels_from_reused_slot`.
* Expected pre-fix failure: the B-only pixel survives when the B slot returns.
* Minimal implementation: use B→C in presentation history while retaining
  render-time A→C repair.
* Verification: fresh full-reference equality and probe-pixel assertions.
* Commit boundary: movement presentation-damage regression.

## Task 7 — Resize and visual-transition coverage

* Exact files: `src/native_output/tests/output.rs`,
  `src/native_output/runtime/scene_history.rs`, existing `WindowVisual`/SSD
  tests.
* Interfaces consumed: scene snapshot geometry, decoration identities,
  cursor state, presented transition damage.
* Interface produced: width-changing, left-edge, content-only,
  decoration-only, and cursor-content regressions.
* Failing test: warmed-slot framebuffer references for widths 1200, 1750,
  and 1300, plus transition-specific assertions.
* Expected pre-fix failure: intermediate B-only right edge, traffic light, or
  client bounds survive slot reuse.
* Minimal implementation: no generic full repaint; preserve compact snapshot
  transition comparison.
* Verification: age 1/2/3 matrix where physically representable and the
  existing fullscreen/SSD geometry suites.
* Commit boundary: resize and visual transition tests.

## Task 8 — Age and slot-sequence matrix

* Exact files: `src/native_output/scanout/output_slot.rs`,
  `src/native_output/scanout/output_swapchain.rs`,
  `src/egl_renderer/damage_tests.rs`, `src/native_output/tests/output.rs`.
* Interfaces consumed: presentation serial and slot last-presented serial.
* Interface produced: a test model that tracks slot ID, physical pixels,
  logical scene ID, and confirmed serial.
* Failing test: age 1, age 2, and age 3 full-reference equality.
* Expected pre-fix failure: an age-2 B-slot reuse exposes B-only pixels.
* Minimal implementation: keep age calculation presentation-based and feed
  the journal actual consecutive transitions.
* Verification: planner, AtomicOutputSlot, and full-reference suites.
* Commit boundary: buffer-age matrix tests.

## Task 9 — Rejection, retry, and out-of-order ownership

* Exact files: `src/native_output/runtime/scene_history.rs`,
  `src/native_output/runtime/cycle/pageflip.rs`, existing retry and worker
  ownership tests.
* Interfaces consumed: `discard_submission`, token promotion, retry frame
  identity.
* Interface produced: A→C after B rejection; A→B/B→D after C rejection;
  exact C promotion after logical state D; no stale pageflip regression.
* Failing test: deterministic scene-history and swapchain ownership sequences.
* Expected pre-fix failure: a rejected or stale candidate enters the journal or
  mutable current server state is used for C.
* Minimal implementation: pageflip preparation reads only presented and
  token-matched submitted snapshots.
* Verification: existing rejection/retry/out-of-order tests plus new
  transition assertions.
* Commit boundary: render-ahead ownership tests.

## Task 10 — Compatibility EGL domain audit

* Exact files: `src/native_output/scanout/egl_gbm.rs`,
  `src/egl_renderer.rs`, compatibility tests and trace documentation.
* Interfaces consumed: EGL buffer-age query, EGL swap-with-damage, GBM front
  buffer settlement.
* Interface produced: explicit code comments and tests documenting the EGL
  swap/render sequence domain separately from Atomic KMS presentation serials.
* Failing test: a backend-domain assertion if compatibility history and EGL
  age diverge.
* Expected pre-fix failure: an implicit assumption that EGL age is KMS
  presentation age.
* Minimal implementation: retain the compatibility transition source that
  matches EGL swap age; do not apply Atomic pageflip APIs blindly.
* Verification: compatibility focused tests and source inspection.
* Commit boundary: backend-domain audit.

## Task 11 — Qualification and report

* Exact files: the design document, this plan, and
  `REPORT-2026-08-18-render-ahead-buffer-age-ghosting-closure.md`.
* Interfaces consumed: bounded presentation trace, native launcher, all
  focused and global validation commands.
* Interface produced: evidence-backed qualification status and final dirty
  tree record.
* Failing test: native normal-settings move/resize stress where DRM access is
  available.
* Expected pre-fix failure: persistent stale regions after interaction under
  triple-buffer auto.
* Minimal implementation: execute diagnostics and final normal-settings run;
  do not substitute triple-off or forced-full for acceptance.
* Verification: format, check, test, clippy, source layout, diff-check,
  focused suites, and native matrix.
* Commit boundary: focused task-owned commits only; preserve inseparable dirty
  baseline work and report it.
