# Typhon Native Content Frame Clock v1.2 Implementation Plan

## Constraints and invariants

- Work directly in `C:\Users\vitor_crispim\Documents\GitHub\Typhon` on Windows.
- Use `rtk` for repository, Cargo, search, and verification commands.
- Do not spawn subagents.
- Do not mutate immutable `PrimaryRefreshClaim` or `PresentationTarget` identities.
- Do not mutate `PresentationDeadlinePlanner` before physical claim revalidation.
- Recover only exact READY or still-worker-queued ownership; execution/kernel ownership remains a distinct state.
- Preserve existing O1, DMA-BUF, cursor, callback, pacing, direct-scanout, SHM, and shutdown policies.

## Task 1: Establish RED tests for planner transactionality

Files:

- `src/native/presentation_deadline.rs`

Tests:

- Add a test proving candidate presentation preparation does not change `last_presented_sequence`, `last_presented_at`, or scheduled-target abandonment.
- Add a test proving commit applies the same timestamp-to-sequence result exactly once.
- Add a test proving a failed physical revalidation leaves the scheduled target and planner frontier unchanged.

Implementation after RED:

- Split the existing `note_presented` calculation into side-effect-free preparation and explicit commit operations.
- Keep `note_presented` as a compatibility wrapper where needed, delegating to prepare then commit.
- Keep target abandonment and identity-reuse accounting in the commit phase.

Verification: run the focused planner tests and confirm the new tests fail for the intended pre-implementation reason before changing production code.

## Task 2: Establish RED tests for typed physical claim revalidation

Files:

- `src/native_output/scanout/output_swapchain.rs`
- `src/native_output/scanout/atomic_egl_gbm.rs`

Tests:

- Replace the current generic-error READY collision test with a typed `OvertakesReady` assertion carrying exact frame and claim identity.
- Replace the worker collision test with a typed `OvertakesWorkerQueued` assertion carrying exact token, transaction, and frame identity.
- Add fatal assertions for generation mismatch, regression, pre-claim presentation, and identity mismatch.
- Add the multi-refresh overtake case where N+1 is presented after READY N+2 and a future N+3 claim exists.

Implementation after RED:

- Introduce the typed revalidation result and fatal violation payload.
- Preserve current strict ordering checks for valid claims.
- Add exact-owner identity checks without modifying live target fields.
- Thread the typed result through `AtomicEglGbmScanout`.

## Task 3: Establish RED tests for READY recovery at the runtime boundary

Files:

- `src/native_output/runtime/cycle/pageflip.rs`
- `src/native_output/runtime/kms_worker/rejection.rs`
- `src/native_output/runtime/presentation_transactions.rs`
- `src/compositor/state/frames.rs`

Tests:

- Add a runtime-level pageflip test in the real pageflip handling path where a predecessor overtakes READY.
- Assert the runtime does not terminate, the predecessor advances normally, the stale READY frame is safely abandoned, its frame batch and DMA-BUF release are settled once, callbacks are not duplicated/lost, and a redraw/replan is requested.
- Assert target identity remains immutable and the replacement target has a new identity.

Implementation after RED:

- Reorder pageflip handling to prepare the physical claim, revalidate/reconcile ownership, commit the presentation clock, and only then settle compositor/output obligations.
- Use existing `suspend_abandon_ready`/safe-abandonment and transaction settlement paths for READY recovery.
- Add the recovery-to-redraw/replan signal through the existing runtime cycle state.
- Keep unreconcilable errors on the fatal path.

## Task 4: Establish RED tests for worker-queued recovery and races

Files:

- `src/native_output/kms_worker/queue.rs`
- `src/native_output/kms_worker/thread.rs`
- `src/native_output/runtime/kms_worker.rs`
- `src/native_output/runtime/kms_worker/rejection.rs`

Tests:

- Add deterministic tests for a job still in the worker queue, a job dequeued before cancellation, a frozen/test-owned job, a submit/kernel-owned job, and token/transaction/frame identity mismatch.
- Add an exact KMS transport regression for worker-queued physical overtake.
- Assert only cancellable queued work is reported as `OvertakesWorkerQueued`; other states are not falsely cancelled or reclassified.
- Assert exact transaction, pacing, cursor, swapchain, and arbiter ownership is settled once.

Implementation after RED:

- Add an exact-owner queued-job cancellation/invalidation operation to worker shared state/handle.
- Integrate it with the existing worker rejection/replan lifecycle and `KmsCommitJob` identity.
- Reuse existing safe abandonment and `replan_invalidated_worker_job` machinery where compatible.
- Surface cancellation races as typed fatal recovery failures rather than generic queued-state assumptions.

## Task 5: Add overtake observability

Files:

- `src/native_output/pacing.rs`
- `src/native_output/runtime/metrics.rs` (only if the runtime summary is the authoritative export point)
- Relevant native runtime tests.

Implementation:

- Add bounded counters for READY overtakes, worker-queued overtakes, successful recoveries, recovery failures, and fatal physical violations.
- Emit them in the existing native content frame clock summary/perf diagnostics.
- Ensure normal valid presentation does not increment any overtake/fatal counter.

Tests:

- Assert every counter is exported and increments only for its matching outcome.
- Preserve all existing pacing and attribution fields.

## Task 6: Close callback timing lifecycle and attribution

Files:

- `src/compositor/state/frame_callbacks.rs`
- `src/compositor/state/surfaces.rs`
- `src/compositor/state/client_lifecycle.rs`
- `src/compositor/state/frames.rs`
- `src/compositor/frame_batch.rs`
- `src/compositor/protocols/core.rs`
- `src/native_output/pacing.rs`

Tests first:

- Add consume-once admission tests: first callback-requesting commit consumes the admission, the second has no evidence, and a no-callback commit does not consume it.
- Preserve and extend cross-surface isolation tests.
- Add teardown tests for explicit destroy, role teardown, and client disconnect, including no pending callbacks and no unrelated surface clock removal.
- Add exact single-surface batch attribution and ambiguous multi-surface attribution tests.
- Assert ambiguous evidence does not produce Chromium fast-client samples or arbitrary surface timing.
- Assert summary output no longer aliases sequence mutation and target identity reuse metrics.

Implementation after RED:

- Change `SurfaceFrameClockState::note_commit` to consume admission with `Option::take`.
- Remove surface timing state on every terminal teardown while keeping disconnect’s callback-discard bypass.
- Replace `max_by_key(commit_ns, surface_id)` with exact-single-surface attribution and explicit ambiguity accounting.
- Add the bounded ambiguity counter and thread only exact evidence into pacing observations.
- Remove/rename the misleading `presentation_target_sequence_mutations` export.

## Task 7: Integrate, refactor, and run focused verification

- Run focused tests for planner, swapchain, worker queue/runtime, frame callbacks, frame batches, and pacing.
- Refactor duplicate recovery logic into small typed helpers only after all focused tests are green.
- Run `rtk cargo fmt --check`.
- Run `rtk cargo check`.
- Run `rtk cargo clippy --all-targets --all-features -- -D warnings`.
- Run `rtk cargo test`.
- Run `rtk git diff --check`.
- Inspect `rtk git diff` and `rtk git status --short`.

## Task 8: Commit the implementation

- Create a focused implementation commit after tests and static checks pass.
- If verification requires a follow-up correction, commit the correction separately with the exact failure addressed.
- Report commit IDs, changed files, test commands, and exact verification outcomes.

