# Typhon client frame-clock admission implementation plan

## 1. RED callback ownership tests

* Add state/runtime tests proving render-ahead READY retains callbacks.
* Add tests proving immediate and exact READY admission send one callback
  before pageflip.
* Add failed-admission retry coverage.
* Add tests for fallback, invalidation, render failure, no-visual, direct
  scanout, shutdown, and resource death.
* Add a deterministic 6,060,606 ns virtual Chromium-like O1 oracle.
* Run focused tests and verify the new tests fail against the current raw
  render-completion behavior.

## 2. Batch state and settlement model

* Add an explicit callback pacing state to `CompositorFrameBatch`.
* Replace `completed_after_render` settlement terminology with admission and
  presentation-fallback terminals while retaining render timing telemetry.
* Make terminal helpers drain/send live callbacks exactly once and preserve
  transfer/cancel reconciliation on restore and abandonment.

## 3. Move protocol completion to exact admission

* Rename renderer completion to a mark-only operation.
* Carry the exact protocol batch identity through explicit rendered output.
* Complete callbacks from presentation runtime only after successful immediate
  or READY lane admission and transaction bookkeeping.
* Keep direct scanout's existing KMS-owned callback path separate.
* Make physical presentation send deferred callbacks as an actual fallback.

## 4. Metrics and audits

* Add bounded callback render/admission/fallback/failed-admission counters and
  timing fields.
* Verify no-visual, cursor, XWayland, SHM, DMA-BUF, transitions, destruction,
  disconnect, and shutdown paths.
* Confirm no released SHM backing is reread and no READY/PENDING output slot
  owns a materialized SHM lease.

## 5. Verification and commits

* Run focused tests after each ownership change.
* Run `rtk cargo fmt --check`, `rtk cargo build --release`, `rtk cargo check`,
  strict clippy, `rtk cargo test`, `git diff --check`, and `git status --short`.
* Review the complete diff adversarially and commit the implementation.
* Native DRM/KMS qualification is reported only if executed on the actual TTY;
  this environment does not authorize or provide that qualification context.
