# Atomic Recoverable Fallback Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an Atomic EGL/GBM lifecycle renderer fallback abandon its unsubmitted render safely and remain retryable without fatal output quarantine.

**Architecture:** Add one explicit nonfatal `AtomicOutputSwapchain` transition for a GPU-complete, never-submitted rendering slot. In the existing `LifecycleFallback` branch, call the established rare-path `glFinish`, restore the frame batch through the existing retry API, settle the exact output transaction at `RenderExecution`, and clear only rendering ownership. Leave the normal rendered, skipped, compositor fallback, physical-ledger, and KMS submission paths unchanged.

**Tech Stack:** Rust, EGL/GLES, Atomic KMS/GBM swapchain model, compositor frame-batch ownership, existing unit/integration test harnesses, Cargo.

## Global Constraints

- Do not modify Eclipse.
- Reuse the existing build directory and artifacts; do not run `cargo clean`.
- Use the established `unsafe { self.gl.finish() }` safety principle only on the exceptional lifecycle-fallback path.
- Never submit the incomplete lifecycle-fallback framebuffer to KMS.
- Never call `quarantine_rendering(..., OutputQuarantineReason::PostDrawRenderFailure)` for `LifecycleFallback`.
- Restore retryable frame-batch ownership after GPU completion; do not retire it as a render failure.
- Preserve the exact current `LifecycleTransitionId` fallback behavior and existing physical lifecycle ledger semantics.
- Preserve successful Atomic rendering, LegacyScene partial repaint ordering, scissor restoration, renderer fallback contracts, mesh admission, policy reconciliation, endpoint snapping, physical ACK, and Eclipse Dock behavior.
- Do not use subagents.
- Commit each coherent deliverable because this is a git repository.

---

## File Map

- Modify `src/native_output/scanout/output_swapchain.rs`: add the explicit nonfatal rendering-slot retirement operation and document its ownership invariants.
- Modify `src/native_output/scanout/atomic_egl_gbm.rs`: prove GPU completion and restore/settle ownership in the existing lifecycle-fallback arm.
- Modify `src/native_output/tests/scanout.rs`: deterministic swapchain ownership regression coverage.
- Modify `src/native_output/tests/presentation_transactions.rs`: failed-render terminal coverage independent of KMS submission.
- Modify `src/compositor/state/frame_tests.rs`: frame-batch retry ownership coverage, including release storage and non-retirement.
- Modify `src/compositor/tests/surface_frames.rs`: callback retry and replacement-admission coverage remains explicit.
- Modify `src/compositor/state/lifecycle_animation.rs`: physical Lamp retention, canonical replacement, exact fallback identity, Direct Scanout blocking, and reversal coverage.

## Task 1: RED swapchain ownership regression

**Files:**
- Modify: `src/native_output/tests/scanout.rs`

**Interfaces:**
- Consumes: `AtomicOutputSwapchain::from_presented_slots`, `acquire_render_slot`, `current`, `presentation_serial`, `current_framebuffer_id`, `pending_slot`, `ready_slot`, `worker_queued_slot`, `rendering_slot`, `quarantine_slot_id`, `free_slot_count`, `validate_invariants`.
- Produces: a failing regression test specifying `complete_unpresented_render(slot)`.

- [ ] **Step 1: Add the failing test before production code.**

Add `complete_unpresented_render_returns_slot_to_free_without_changing_output_ownership` beside the existing swapchain transition tests:

```rust
#[test]
fn complete_unpresented_render_returns_slot_to_free_without_changing_output_ownership() {
    let mut swapchain = AtomicOutputSwapchain::from_presented_slots(
        explicit_slot_set(),
        OutputSlotId::new(0).unwrap(),
        7,
    )
    .unwrap();
    let current = swapchain.current();
    let presentation_serial = swapchain.presentation_serial();
    let current_framebuffer = FramebufferId::new(41).unwrap();
    swapchain.set_current_framebuffer_id(current_framebuffer);
    let slot = swapchain.acquire_render_slot().unwrap();
    let free_before = swapchain.free_slot_count();

    swapchain.complete_unpresented_render(slot).unwrap();

    assert_eq!(swapchain.rendering_slot(), None);
    assert_eq!(swapchain.quarantine_slot_id(), None);
    assert!(!swapchain.is_poisoned());
    assert_eq!(swapchain.current(), current);
    assert_eq!(swapchain.presentation_serial(), presentation_serial);
    assert_eq!(swapchain.current_framebuffer_id(), Some(current_framebuffer));
    assert_eq!(swapchain.pending_slot(), None);
    assert_eq!(swapchain.ready_slot(), None);
    assert_eq!(swapchain.worker_queued_slot(), None);
    assert_eq!(swapchain.free_slot_count(), free_before + 1);
    assert!(swapchain.render_target_available_for(NativeOutputPacingMode::PredictiveTriple));
    assert_eq!(swapchain.acquire_render_slot().unwrap(), slot);
    swapchain.validate_invariants().unwrap();
}
```

- [ ] **Step 2: Run the focused test to verify RED.**

Run:

```bash
rtk run -- cargo test --locked complete_unpresented_render_returns_slot_to_free_without_changing_output_ownership -- --exact --nocapture
```

Expected: compilation fails because `AtomicOutputSwapchain::complete_unpresented_render` does not yet exist. Do not alter the test to make the baseline pass.

## Task 2: GREEN nonfatal swapchain transition and Atomic fallback ownership

**Files:**
- Modify: `src/native_output/scanout/output_swapchain.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`

**Interfaces:**
- Consumes: active `rendering: Option<OutputSlotId>`, `AtomicEglGbmScanout::gl`, existing `restore_frame_batch_after_render_failure`, and `settle_failed_output_transaction`.
- Produces: `AtomicOutputSwapchain::complete_unpresented_render(slot) -> io::Result<()>`; an Atomic `LifecycleFallback` path that returns `AtomicFrameRenderOutcome::LifecycleFallback` without KMS ownership or fatal quarantine.

- [ ] **Step 1: Implement the smallest explicit swapchain transition.**

Add this method next to `cancel_render_before_gpu`:

```rust
pub(crate) fn complete_unpresented_render(&mut self, slot: OutputSlotId) -> io::Result<()> {
    if self.rendering != Some(slot) {
        return Err(io::Error::other(
            "completed unpresented output slot does not match active rendering ownership",
        ));
    }
    self.rendering = None;
    Ok(())
}
```

Document in a nearby comment that this method is valid only after the caller has proven GPU completion; it creates no ready/pending/worker ownership, does not advance frame or presentation serials or buffer-age history, and does not touch fatal quarantine. Keep `cancel_render_before_gpu` for the pre-GPU path.

- [ ] **Step 2: Run the focused swapchain test to verify GREEN.**

Run:

```bash
rtk run -- cargo test --locked complete_unpresented_render_returns_slot_to_free_without_changing_output_ownership -- --exact --nocapture
```

Expected: PASS, with the slot re-acquirable and no quarantine.

- [ ] **Step 3: Replace only the lifecycle-fallback ownership operations.**

In `AtomicEglGbmScanout::render_frame`, keep the existing `RenderExecution` failure terminal and replace the lifecycle-fallback closure body with the post-GPU retry sequence. Execute `unsafe { self.gl.finish() };` immediately before settling the transaction, then inside the existing obligations closure restore the exact frame batch and call `complete_unpresented_render(slot)`. The branch must remain before the fatal `Err(error)` branch and must continue returning the same fallback payload:

```rust
unsafe { self.gl.finish() };
settle_failed_output_transaction(
    output_transactions,
    transaction_id,
    OutputTransactionFailureStage::RenderExecution,
    MonotonicTimestampNs::new(monotonic_now_ns()?),
    |obligations| {
        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
            io::Error::other("lifecycle fallback transaction has no frame batch")
        })?;
        server.restore_frame_batch_after_render_failure(batch_id);
        self.swapchain_mut()?.complete_unpresented_render(slot)?;
        Ok(())
    },
)
```

Do not call `discard_frame_batch`, `quarantine_rendering`, `submit_ready`, `take_ready_for_worker`, or any KMS submit helper from this branch. Do not add synchronization to `EglFrameOutcome::Rendered`, `EglFrameOutcome::Skipped`, or the ordinary render-error path.

- [ ] **Step 4: Run focused Atomic/swapchain tests.**

Run:

```bash
rtk run -- cargo test --locked native_output::tests::scanout -- --nocapture
rtk run -- cargo test --locked complete_unpresented_render -- --nocapture
```

Expected: all selected tests pass; no selected test reports `PostDrawRenderFailure` for the new transition.

- [ ] **Step 5: Commit the production transition.**

```bash
rtk git add src/native_output/scanout/output_swapchain.rs src/native_output/scanout/atomic_egl_gbm.rs src/native_output/tests/scanout.rs
rtk git commit -m "fix: recover Atomic lifecycle render fallback"
```

## Task 3: RED/GREEN protocol and transaction regression coverage

**Files:**
- Modify: `src/native_output/tests/presentation_transactions.rs`
- Modify: `src/compositor/state/frame_tests.rs`
- Modify: `src/compositor/tests/surface_frames.rs`

**Interfaces:**
- Consumes: `settle_failed_output_transaction`, `OutputTransactionFailureStage::RenderExecution`, `restore_frame_batch_after_render_failure`, existing frame callback/presentation/dmabuf test helpers.
- Produces: tests proving one non-presenting transaction terminal and retryable protocol ownership with no retired-batch leak.

- [ ] **Step 1: Add the failed-render transaction test.**

In `src/native_output/tests/presentation_transactions.rs`, import `settle_failed_output_transaction` and add a test that creates/inserts a composited transaction, calls `settle_failed_output_transaction` at `RenderExecution`, asserts the closure receives the exact batch ID, asserts `take_settled_output_terminals()` returns exactly one terminal, asserts it is `OutputTransactionState::Terminal(OutputTransactionTerminal::Failed { stage: RenderExecution, .. })`, and asserts `ledger.counters().presented == 0` and `ledger.active_count() == 0`. This establishes that settling the failed render does not create KMS-submit ownership or physical presentation.

- [ ] **Step 2: Strengthen frame-state restoration assertions.**

In `frame_batch_captures_only_releases_pending_at_capture_and_restores_order`, assert immediately after `restore_frame_batch_after_render_failure(batch)` that `state.retired_frame_batches.is_empty()`. Keep the existing ordered pending-release assertions and restored metric. In the callback integration test `render_failure_requeues_callback_for_one_later_admission`, assert the restored callback is not counted in `callbacks_completed_after_abandonment` or `callbacks_in_discarded_rendered_batches` before the replacement batch admits it. Preserve the later immediate-admission assertions.

- [ ] **Step 3: Verify protocol tests GREEN.**

Run:

```bash
rtk run -- cargo test --locked native_output::tests::presentation_transactions -- --nocapture
rtk run -- cargo test --locked compositor::state::frame_tests -- --nocapture
rtk run -- cargo test --locked render_failure_requeues_callback_for_one_later_admission -- --exact --nocapture
rtk run -- cargo test --locked render_failure_restore_discards_feedback_for_a_stale_surface_commit -- --exact --nocapture
```

Expected: one failed-render transaction terminal, zero presented count, callbacks/feedback retry semantics preserved, dmabuf releases pending rather than retired, and stale feedback still discarded only because its commit identity is stale.

- [ ] **Step 4: Commit protocol regression coverage.**

```bash
rtk git add src/native_output/tests/presentation_transactions.rs src/compositor/state/frame_tests.rs src/compositor/tests/surface_frames.rs
rtk git commit -m "test: cover Atomic fallback protocol restoration"
```

## Task 4: Lifecycle physical-truth and reversal regression coverage

**Files:**
- Modify: `src/compositor/state/lifecycle_animation.rs`

**Interfaces:**
- Consumes: `publish_presented_lifecycle`, `apply_lifecycle_render_fallback`, `publish_presented_lifecycle_with_replacements`, `direct_scanout_scene_blockers`, `LifecycleTransitionId`, existing lifecycle test helpers.
- Produces: deterministic lifecycle tests showing an unpresented fallback cannot replace confirmed physical state or affect a newer reversal.

- [ ] **Step 1: Add the confirmed-physical-Lamp fallback test.**

Build a test state with an in-output minimize transition, sample and publish a qualified Lamp A as the previously pageflip-confirmed `presented_lifecycle`, then apply a fallback for the same transition without publishing a new frame. Assert the presented snapshot and frame ID remain Lamp A, the exact transition is retired, `has_pending_visible` remains true because the old physical Lamp is still visible, and the lifecycle scene requests more work. Then publish the canonical replacement with `publish_presented_lifecycle_with_replacements(..., &[], true)` and assert the old physical Lamp is removed and the transition remains correctly settled. Where the test fixture can provide a fullscreen owner, assert `direct_scanout_scene_blockers()` contains `DirectScanoutSceneRejection::LifecycleAnimation` while Lamp A remains visible and no longer contains it after replacement.

- [ ] **Step 2: Add or retain the reversal identity test.**

Keep the existing `stale_lifecycle_render_fallback_cannot_cancel_a_reversal` test as the direct A/B proof. If the new physical-state test shares setup, extend it with a reversal before applying A's fallback: assert A and B have different transition IDs, apply A's fallback, assert B remains active, and assert B's later replacement is the only state that can clear the physical Lamp. Do not change `apply_lifecycle_render_fallback` production logic.

- [ ] **Step 3: Run focused lifecycle tests.**

```bash
rtk run -- cargo test --locked compositor::state::lifecycle_animation -- --nocapture
rtk run -- cargo test --locked stale_lifecycle_render_fallback_cannot_cancel_a_reversal -- --exact --nocapture
rtk run -- cargo test --locked direct_scanout -- --nocapture
```

Expected: the old confirmed Lamp remains physical until a canonical replacement presentation, stale A cannot alter B, and Direct Scanout remains blocked while the old Lamp is visible.

- [ ] **Step 4: Commit lifecycle regression coverage.**

```bash
rtk git add src/compositor/state/lifecycle_animation.rs
rtk git commit -m "test: preserve lifecycle physical truth after fallback"
```

## Task 5: Full verification and handoff

**Files:**
- Verify: all changed files and current worktree state

- [ ] **Step 1: Run focused suites with visible pass counts.**

```bash
rtk run -- cargo test --locked native_output::tests::scanout -- --nocapture
rtk run -- cargo test --locked native_output::tests::presentation_transactions -- --nocapture
rtk run -- cargo test --locked compositor::state::frame_tests -- --nocapture
rtk run -- cargo test --locked compositor::state::lifecycle_animation -- --nocapture
rtk run -- cargo test --locked compositor::tests::surface_frames -- --nocapture
```

- [ ] **Step 2: Run the required full verification.**

```bash
rtk run -- cargo fmt --check
rtk run -- cargo check --locked --all-targets
rtk run -- cargo clippy --locked --all-targets -- -D warnings
rtk run -- cargo test --locked
rtk git diff --check
rtk run -- bash bin/check-source-layout
```

Do not run `cargo clean`. Reuse the existing target/build artifacts.

- [ ] **Step 3: Inspect the final diff and answer the ownership checklist.**

Use `rtk git diff`, `rtk git status --short`, and direct source inspection to verify: lifecycle fallback has no KMS submit edge, no fatal quarantine call, a preceding `glFinish`, retryable batch restoration, reusable slot, no retired dmabuf obligation, no false callback/feedback completion, unchanged physical lifecycle ledger until replacement presentation, stale reversal safety, Direct Scanout blocking, and no normal-path synchronization.

- [ ] **Step 4: Report native qualification availability accurately.**

If an Atomic EGL/GBM failure-injection harness or Typhon hardware is available, run the requested repeated minimize/restore qualification and report its observed screen/quarantine/endpoint/other-window results. If unavailable, explicitly state that native failure injection was not available and do not imply hardware qualification.
