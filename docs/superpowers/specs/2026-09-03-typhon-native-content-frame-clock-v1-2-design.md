# Typhon Native Content Frame Clock v1.2 Design

## Scope

Implement the v1.2 closure for the native content frame clock in the existing v1/v1.1 architecture:

`ReactiveDouble` remains advisory metadata, while `PrimaryRefreshClaim` remains the immutable physical reservation carried through planning, swapchain ownership, READY state, KMS worker state, and pageflip settlement.

This change covers both required areas:

1. Recoverable physical-frontier overtakes when a predecessor is presented after a future READY or worker-owned claim.
2. Callback evidence lifecycle closure: consume-once admission, bounded surface-local timing state, exact multi-surface attribution, and truthful metrics.

The existing DMA-BUF, O1, KMS worker, wake, cursor, input, direct-scanout, SHM, pacing, and shutdown policies remain unchanged unless a recovery path must explicitly abandon and replan a frame.

## Design

### 1. Physical claim validation is prepare/reconcile/commit

`PresentationDeadlinePlanner` will expose a side-effect-free preparation step that derives the logical sequence and candidate physical claim from the presented timestamp. A separate commit step will update `last_presented_sequence`, `last_presented_at`, and scheduled-target abandonment only after physical ownership revalidation succeeds.

The timestamp-to-sequence calculation remains single-sourced. Existing callers that need the normal presentation path will use the same prepare/commit pair; no path will mutate the planner before claim validation.

`AtomicOutputSwapchain` will replace the generic physical-claim error boundary with a typed revalidation result:

- `Valid`.
- `OvertakesReady`, carrying the exact READY frame/claim identity.
- `OvertakesWorkerQueued`, carrying the exact token, transaction, and frame identity.
- `Fatal`, carrying the existing physical-primary violation details.

The validator will continue to reject generation mismatches, regressions, pre-claim presentations, identity/token mismatches, and unreconcilable duplicates. A recoverable overtake is not converted to `io::Error`.

### 2. READY overtake recovery

READY targets remain immutable. Recovery will use the existing safe-abandonment/quarantine and replan lifecycle, preserving the `RenderedOutputFrame`, output transaction, frame batch, DMA-BUF release ownership, fence, damage, callback, and slot identity until their normal terminal path executes.

The recovery sequence is:

1. Identify and verify the exact READY owner.
2. Retire it through the explicit safe-abandonment path.
3. Settle its output transaction and protocol obligations exactly once.
4. Record an overtake recovery and request redraw/replan.
5. Plan a fresh target with a new immutable physical identity.

No target field is retargeted in place, and no already-admitted frame is presented as abandoned.

### 3. Worker-queued overtake recovery

Worker recovery will use the existing KMS job, pacing reservation, `OutputTransactionId`, `AtomicCommitArbiter`, swapchain, cursor-owner, and safe-abandonment identities.

The worker transport will expose an exact-owner cancellation/invalidation operation for a still-queued job. The operation verifies token, transaction, and frame identity before removing the job and returning its resources to the normal dropped/replan lifecycle. The worker event path remains authoritative for jobs that have crossed into execution, test-only, submit, or kernel ownership; those states are never reported as queued.

Reachable race states will be tested deterministically: queued and cancellable, dequeued but not cancellable, frozen/test ownership, kernel ownership, and identity mismatch. A cancellation or replan failure is a fatal recovery failure and increments the corresponding diagnostic counter.

### 4. Pageflip ordering

The native pageflip path will follow this ordering:

1. Observe the kernel pageflip.
2. Derive a candidate physical claim without planner mutation.
3. Revalidate and reconcile predecessor ownership against the exact READY/worker owner.
4. Commit the physical presentation phase.
5. Complete the predecessor output transaction and compositor protocol/scene obligations.
6. Continue normal planning and pacing.

Direct and composited primary presentation paths will share the same planner-side-effect ordering where physical claim validation applies.

### 5. Callback evidence lifecycle

`SurfaceFrameClockState::note_commit` will take the pending admission with `Option::take`. A commit requesting callbacks consumes exactly one admission; a second callback commit before a new admission has no timing evidence. Commits without callbacks do not consume admission state. Existing cross-surface isolation is preserved.

Surface timing state will be removed on every terminal surface teardown, including explicit destruction, role teardown, and client disconnect. The disconnect path will continue bypassing the normal callback-discard behavior and will remove only the disconnected surface's timing state.

Frame-batch capture will attribute callback timing only when all callback evidence belongs to one exact surface. Evidence spanning multiple distinct surfaces is represented as ambiguous, with no arbitrary newest-surface selection, no averaging, and no fast Chromium attribution sample. A bounded `content_callback_attribution_ambiguous` counter will record these cases.

The legacy `presentation_target_sequence_mutations` summary field will be removed or renamed so it no longer aliases `target_identity_reuse_after_abandonment`; the latter remains truthful.

### 6. Metrics

Add bounded diagnostic counters:

- `physical_claim_overtake_ready`
- `physical_claim_overtake_worker_queued`
- `physical_claim_overtake_recoveries`
- `physical_claim_overtake_recovery_failures`
- `physical_claim_fatal_violations`
- `content_callback_attribution_ambiguous`

Acceptance requires recovery failures and fatal violations to remain zero in the native qualification scenarios. Existing pacing and attribution metrics remain intact.

## Tests

Tests will be written RED before production changes, then driven GREEN and refactored. Coverage includes:

- Side-effect-free planner preparation and commit.
- READY physical overtake with immutable target identity, callback/DMA-BUF ownership, and exactly-once safe abandonment.
- Worker transport overtake for every reachable ownership race state.
- Miss by more than one refresh with all overtaken reservations reconciled.
- Real pageflip-boundary recovery through the native runtime, including no runtime termination, predecessor phase advance, stale READY retirement, redraw/replan, and no duplicate obligation settlement.
- Callback admission consumption and cross-surface isolation.
- Timing cleanup for all teardown reasons.
- Exact single-surface versus ambiguous multi-surface frame-batch attribution.
- Truthful summary metrics.

Verification will use the Windows checkout and `rtk`:

```text
rtk cargo fmt --check
rtk cargo check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk cargo test
rtk git diff --check
rtk git status --short
```

Native qualification will be reported only after the static and test checks are green.

## Alternatives rejected

- Fatal runtime termination on an overtake: fails the recoverability requirement.
- In-place retargeting of READY or worker-owned claims: breaks immutable physical identity and exact lifecycle ownership.
- Disabling O1 or permanently degrading pacing: changes an accepted policy rather than closing the race.
