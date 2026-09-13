# Atomic Recoverable Fallback Closure

## Goal

Make an Atomic EGL/GBM lifecycle renderer fallback recoverable without submitting the incomplete framebuffer, poisoning the explicit output, or retiring protocol ownership that must participate in the replacement render.

## Root cause

`AtomicEglGbmScanout::render_frame` already receives `EglFrameOutcome::LifecycleFallback` after `draw_scene_to_target` has marked GPU sampling as started. Its current branch treats that recoverable renderer result as a fatal post-draw output failure: it discards the compositor frame batch and calls `quarantine_rendering(..., PostDrawRenderFailure)`. The slot then fails `ensure_operational`, while callbacks, presentation feedback, commit timing, FIFO, explicit-sync, and dmabuf-release ownership no longer have the ordinary retry path.

## Design

The exceptional lifecycle-fallback branch will synchronously prove completion of the current EGL context's preceding GLES work with the established `glFinish` safety principle already used during Atomic scanout teardown. This synchronization is limited to the fallback branch; successful renders and ordinary no-visual skips are unchanged.

After completion is proven, the branch will:

1. settle the exact allocated output transaction once with `OutputTransactionFailureStage::RenderExecution`;
2. restore the transaction's frame batch through `restore_frame_batch_after_render_failure`, returning callbacks, presentation feedback, FIFO/commit-timing ownership, and dmabuf release obligations to retryable compositor ownership;
3. call `AtomicOutputSwapchain::complete_unpresented_render(slot)` to clear only the active rendering ownership.

`complete_unpresented_render` will require `rendering == Some(slot)`. It will not create ready, pending, worker, or KMS ownership; advance a frame or presentation serial; update buffer-age history; or touch fatal quarantine. Once it returns, the slot is eligible for a later full/repair-safe render and the output remains operational.

The failed framebuffer is never passed to `submit_ready`, `take_ready_for_worker`, or any KMS submission path. The failed frame cannot update confirmed physical presentation state. Runtime fallback handling remains responsible for applying only the exact still-current `LifecycleTransitionId`, marking immediate scheduler completion, and requesting the canonical replacement redraw. The previously pageflip-confirmed lifecycle snapshot remains physical truth until that replacement pageflip promotes the new snapshot, so Direct Scanout remains blocked while the old Lamp is visible.

## Testing

The test-first cycle will add focused coverage for:

- Atomic swapchain unpresented-render ownership: no fatal quarantine, unchanged ready/pending/worker/current ownership and serials, safe re-acquisition, and explicit transition preconditions;
- output transaction failure settlement: one terminal, no KMS-submit ownership, and unchanged confirmed output state;
- frame-batch restoration: retryable callbacks and presentation feedback, pending dmabuf release ownership rather than retired-batch storage, and preserved explicit-sync timing ownership until GPU completion is proven;
- lifecycle physical truth and reversals: a fallback does not replace the last pageflip-confirmed Lamp, requests a canonical redraw, preserves Direct Scanout blocking, promotes the replacement normally, and cannot affect a newer reversal;
- source-level/structural safeguards that the lifecycle fallback branch cannot call fatal quarantine or KMS submit and that the normal successful Atomic branch does not invoke the rare-path synchronization.

No Eclipse files or runtime policy, renderer, partial-repaint, or mesh-admission behavior will be changed.
