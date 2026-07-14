# Frame-Owned Client Buffer Release Matrix

The explicit Atomic path captures protocol release work into the same
`CompositorFrameBatch` that owns callbacks and presentation feedback. A batch
is never restored after GPU sampling begins. The output slot retains the batch
identity through ready, KMS-pending, and pageflip completion; suspend and fatal
teardown first prove that the corresponding fence or KMS ownership has ended.

## State-transition test matrix

| State transition | Ownership rule | Regression coverage |
| --- | --- | --- |
| pending protocol queue → captured batch | Capture only work present at capture time | `frame_batch_captures_only_releases_pending_at_capture_and_restores_order` |
| captured → restored before rendering | Restore in original order only before sampling | `frame_batch_captures_only_releases_pending_at_capture_and_restores_order` |
| captured → rendering with fence | Keep the batch out of global pending ownership | `failed_frame_retains_release_until_safe_teardown_without_duplication` |
| rendering → ready | Carry the batch ID in `RenderedOutputFrame` | `explicit_output_swapchain_valid_current_pending_ready_transition` |
| ready → KMS pending | Submission transfers ownership; no restore path remains | `explicit_output_swapchain_valid_current_pending_ready_transition` |
| KMS pending → presented | Only the matching token/generation completes the batch | `mismatched_frame_and_batch_identity_completes_nothing`, `pageflip` tests |
| ready abandonment before fence signal | Retain the frame in the suspend quarantine | `suspended_ready_slot_cannot_be_reused_before_fence_proof` |
| ready abandonment after fence signal | Retire the batch, then make the slot reusable | `suspended_ready_fence_requires_an_observed_signal_before_recovery` |
| suspend while rendering | Keep the rendering slot owned; do not restore protocol work | `suspend_while_rendering_retains_slot_ownership` |
| suspend while ready | Retain the complete frame until fence proof | `suspended_ready_slot_cannot_be_reused_before_fence_proof` |
| suspend while KMS pending | Retire only after recovery modeset ends KMS ownership | `recovery_retires_unpresented_pending_frame_without_promoting_it` |
| Atomic submit failure | Quarantine the slot and retain the batch as retired work | `atomic_submit_failure_quarantines_ready_slot_and_poison_rejects_operations` |
| client destruction in pending/captured/retired queues | Scrub dead protocol resources; never send a duplicate release | `destroyed_clients_scrub_pending_captured_and_retired_releases` |
| fatal teardown in rendering/ready/pending states | Drain only after scanout/GLES teardown and ownership retirement | `failed_frame_retains_release_until_safe_teardown_without_duplication` |

The protocol-side tests additionally cover empty batches, disjoint pending and
ready release sets, late commits, one-presentation dma-buf deferral, exact
identity matching, and a bounded three-buffer client over 100 cycles.
