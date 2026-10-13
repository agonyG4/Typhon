# First-Class Output Epoch and Reconfiguration Barrier

## Current source and failure

This design is based on Typhon `main` at `3f2d43121d352cc09535f710969d9ffde56ec3f2`, including the current Display qualification path. Codebase Memory was indexed at that HEAD. Its coverage report marks `presentation_cycle.rs:371` and `cycle_dispatch.rs:2113` as parse-partial; both current source ranges were read directly.

The current `NativeRuntime::drm_file_generation` comment says the value changes on both DRM session-file replacement and successful modesets. The value flows into physical output work: render and KMS worker identities, pageflip and cursor validation, presentation timing, explicit-sync watches, and scanout setup. `OutputConfigurationGeneration` is already a separate Settings/API generation.

The observed `overtaken READY has no pacing owner` failure is consistent with a real current path: an Atomic render can leave a physical READY frame in the swapchain when immediate submission is deferred for an unsignaled async render fence. `NativeFramePacing::note_ready_frame` only runs on the target-wait path, so this frame remains under the active pacing owner. When the fence resolves, output reconfiguration can reach its safe boundary; `abandon_overtaken_ready` then accepts only pacing READY and errors, even though the physical READY identity is still exact. The retirement path also mutates pacing, scene, scheduler, and swapchain before settling the output transaction, so a later failure can leave partial ownership changes.

## Ownership map

- `runtime/output_configuration.rs` owns Display apply, rollback, candidate preparation, safe-boundary observation, TEST_ONLY, real commit, adoption, and output publication. It already retains the prepared modeset candidate through test, commit, and adoption.
- `runtime/cycle/pageflip.rs` handles physical READY retirement, pageflip identity and presentation completion.
- `runtime/presentation_cycle.rs`, `presentation_worker.rs`, and `render_worker.rs` create READY frames and process render-worker results.
- `runtime/kms_worker.rs` validates worker submissions and completions; `kms_worker/*` owns their transport identities.
- `NativeFramePacing`, `NativeFrameScheduler`, `NativeSceneHistory`, `AtomicOutputSwapchain`, and `OutputTransactionLedger` each track different parts of frame ownership.
- `native/explicit_sync.rs` parks and rearms acquire watches against the current native-output ownership lifetime.
- `runtime/session_io.rs` replaces session-owned resources and currently advances the same generation used by ordinary modesets.
- `presentation/*`, `scanout/atomic_egl_gbm.rs`, and `scanout/atomic_egl_gbm_transactions.rs` hold presentation timing, plane proof, candidate scanout, and KMS commit state.

## Design

Add small copyable `DrmSessionGeneration` and `OutputEpochId` newtypes alongside the existing `OutputConfigurationGeneration`. Bootstrap starts an output epoch independently of the session generation. A successful structural output modeset, including rollback, publishes the next output epoch; session recovery advances the session generation and also publishes a new output epoch after recovery succeeds. Buffer-pool generation remains a separate identity.

Rename the existing boundary to `NativeOutputReconfigurationBoundary`, keeping its current worker, arbitration, pageflip, cursor, render-fence, deferred-event, and explicit-sync checks as the only safety model. A `QuiescedOutputReconfiguration` proof is created only after old-frame retirement and a fresh boundary observation. The real structural commit method requires that proof and verifies it names the currently published old epoch. TEST_ONLY and real commit continue to use the same prepared modeset candidate.

Make `NativeRuntime::terminalize_unsubmitted_output_frame` the sole reconfiguration retirement operation. It captures one `OutputFrameIdentitySnapshot`, validates current epoch and every tracked owner before mutation, then settles the transaction and protocol obligations, scene READY, scheduler/pacing owner, and physical READY exactly once. A matching active pacing owner is valid for a physically READY frame whose immediate submission was deferred; a missing or mismatched active/READY owner is a typed invariant error before mutation. The operation does not reset pacing independently or lose protocol obligations.

After KMS commit and candidate adoption, one `publish_new_output_epoch` boundary receives the new target and epoch. It publishes scanout identity and capabilities, Settings generation, dimensions/input/cursor state, presented-plane baseline, Direct Scanout proof invalidation, scene damage baseline, timing/deadline state, scheduler, pacing, render/adaptive state, and visual-work admission in a fixed order. Old epoch work is fully retired before new visual work is queued.

Carry `OutputEpochId` in existing physical asynchronous identities (KMS jobs/completions, render worker identities/completions, frame/pageflip and cursor claims, presentation timing, explicit-sync watches, and validation keys). A stale result releases its own worker/fence/buffer ownership and telemetry, but does not publish presentation or mutate the new epoch. Pool generation remains independently checked.

## Tests and constraints

Add a deterministic regression that produces an unsignaled-fence READY frame with its exact active pacing identity, makes reconfiguration eligible after the fence settles, terminalizes it once, publishes the next epoch, and feeds a stale predecessor/pageflip event. Assert that transaction, protocol, scene, scheduler, pacing, and swapchain ownership are settled once and the new epoch remains unchanged. Add targeted old-epoch KMS, pageflip, render-worker, and explicit-sync regressions. Preserve the existing six Display qualification scenarios and the production qualification gate.

Ordinary frame work remains scalar and main-thread owned: no new lock, allocation, global lookup, or virtual dispatch. No hotplug implementation or full FrameLanes migration is part of this work. Live NVIDIA qualification is attempted only after deterministic checks; production Display qualification remains disabled until the full hardware matrix passes.
