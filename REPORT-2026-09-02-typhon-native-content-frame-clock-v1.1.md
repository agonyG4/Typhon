# Typhon Native Content Frame Clock v1.1 Closure Report

Date: 2026-09-02

## Outcome

The physical-claim propagation and attribution-integrity closure is implemented and verified with the current checkout. The closure does not include native DRM/KMS qualification; that remains the next step on a machine with the required TTY/DRM seat.

The accepted Native Content Frame Clock v1 model is preserved:

* ReactiveDouble targets remain advisory prediction metadata.
* Commit Timing and predictive/O1 targets remain binding reservations.
* `PrimaryRefreshClaim` is the immutable physical ownership value.
* The claim follows a primary through rendering, READY, worker queue, kernel pending state, and pageflip validation.
* Cursor-only PlaneDelta completion does not advance the primary presentation phase.
* Surface callback/client timing is frozen from exact surface-local lifecycle state.

## Root cause

The v1 RED evidence confirmed that a conservative ReactiveDouble `PresentationTarget` could describe a future diagnostic target such as sequence 4 while the same in-flight primary physically owned the next opportunity, sequence 2. O1 then correctly selected a binding successor at sequence 3, but downstream ownership code still compared the raw advisory sequence 4. The planner and swapchain consequently used different physical frontiers.

The same raw comparison in `plan_visual_target_for_budget()` could abandon and recreate a live O1 target on a subsequent planning turn. That made target identity and physical ownership unstable even though the authoritative pipeline had not changed.

Callback timing had a separate integrity issue: global “last callback” fields could pair one surface's callback admission with another surface's commit. Content attribution therefore could not reliably identify a fast or client-limited surface in a real multi-surface desktop.

## Before and after wake/ownership model

Before the closure, presentation metadata was used as both prediction and physical ordering:

```text
Reactive metadata target (possibly N+3/N+4)
        |
        +--> planner advisory interpretation
        +--> swapchain ordering interpretation  <-- inconsistent frontier
        +--> possible O1 abandon/recreate churn
```

After the closure, all physical ownership boundaries use one immutable claim:

```text
physical primary N
      |
      +--> PrimaryRefreshFrontier
              |
              +--> ReactiveDouble metadata: advisory only
              |       physical claim: actual owned opportunity N+1
              |
              +--> O1 / Predictive / CommitTiming target:
                      reserved target + immutable claim N+2
                                      |
                                      v
             render -> READY -> worker -> kernel -> pageflip
                                      |
                                      v
                         physical primary phase advances
```

The timer and continuation architecture remains unchanged: genuine future times use the absolute timerfd, external owners use their readiness, and immediate bounded work uses the coalesced continuation eventfd.

## Physical claim propagation

`PrimaryRefreshClaim` contains sequence, phase-aligned presentation time, and clock generation. `PresentationTarget` retains its diagnostic target and now carries the claim plus bounded target-selection evidence.

The claim is used for:

* planner predecessor/successor ordering;
* O1 successor planning;
* `OutputPipelineSnapshot` validation;
* READY and worker-queued ordering;
* `OutputSwapchain` invariants and presentation-opportunity frontier construction;
* physical pageflip validation and last-presented-primary state.

Binding targets require metadata and claim identity to agree. Advisory targets may retain conservative diagnostic metadata, but their physical claim is the actual opportunity owned by that primary. Claims are checked for clock-generation consistency, strict monotonic order, and duplicate live ownership. A physical predecessor miss is explicitly revalidated against READY and worker-owned successors; the successor claim is not silently rewritten in place.

The planner's target-reuse counter is now real. `presentation_target_sequence_mutations` is sourced from the planner rather than exported as a literal zero, and `target_identity_reuse_after_abandonment` makes reuse after abandonment directly observable.

## Callback and content attribution integrity

Frame callback timing is now surface-local. Callback admission and the next callback-requesting commit are paired through per-surface lifecycle state. The resulting `FrameCallbackTimingEvidence` is attached to the exact `CompositorFrameBatch` and carried into rendered output observations; global “latest callback” state is not used for content-frame attribution.

`TargetLimited` is no longer inferred from selected and actual distances alone. It requires frozen evidence that the selected target was binding, an earlier opportunity was measured feasible, and the selected opportunity was actually used. An advisory prediction that selected a later diagnostic target but physically presented at the next opportunity is prediction overestimation/target hit, not compositor target limitation.

The existing bounded diagnostic samples remain separate for callback handoff, client reaction, render, READY-to-submit, submit-to-pageflip, selected-target distance, actual distance, and target behavior. No predictor constants were lowered and no protocol callback timing semantics were changed.

## Invariants preserved

* ReactiveDouble remains an immediate low-latency path when the pipeline permits it.
* Commit Timing lower bounds remain binding.
* Predictive READY owns exact contents, batch, transaction, target, and claim.
* READY target identity is not mutated to repair an ordering comparison.
* O1 remains enabled and targets the successor of the real reserved frontier.
* Cursor PlaneDelta pageflips remain outside primary phase and primary pacing samples.
* KMS worker ordering and timeout ownership are unchanged.
* DMA-BUF release authority, exact correlation, Direct Scanout, SHM, input, pointer, surface pacing, XWayland, transactions, and SafeDisable shutdown are unchanged.
* No new thread, mutex, polling loop, sleep, forced repaint, blocking GPU wait, unbounded history, or per-event output was added.

## RED/GREEN evidence

The required regressions are covered by deterministic tests at the production boundaries:

* advisory predecessor claim 2 plus reserved O1 claim 3 passes planner and swapchain ordering;
* reserved claim 4 before claim 3 is rejected;
* duplicate live physical claims are rejected;
* physical predecessor misses revalidate READY and worker-owned successors without duplicate submission or silent claim mutation;
* repeated planning preserves an existing O1 target and does not count abandon/recreate churn;
* callback reactions remain surface-local in both event orderings;
* advisory selected distance 3 with actual distance 1 is not `TargetLimited`;
* a binding target that intentionally skips a measured-feasible opportunity remains `TargetLimited`;
* cursor-only pageflips do not advance the primary phase;
* the callback-driven 165 Hz virtual oracle remains green with bounded future-primary depth and active O1.

Focused results from the current checkout:

```text
native::presentation_deadline (lib)                         21 passed
OutputSwapchain                                               5 passed
pacing_mode_tests                                             34 passed
O1                                                              1 passed
KMS worker                                                    110 passed
frame_callbacks                                                5 passed
content attribution                                            2 passed
wake_plan                                                     12 passed
DMA-BUF                                                        32 passed
presentation_transactions                                     58 passed
shutdown                                                       28 passed
triple_buffering_model                                        22 passed
callback-driven 165 Hz oracle                                 1 passed
```

Full test suite:

```text
3239 passed, 5 ignored, 40 filtered out (30 suites)
```

## Verification

Fresh required checks are green:

```text
rtk cargo fmt --check                                      PASS
rtk cargo check                                            PASS
rtk cargo clippy --all-targets --all-features -- -D warnings PASS
rtk cargo test                                              PASS
git diff --check                                            PASS
```

The implementation is in commit `40b08b7` (`fix: propagate physical primary claims through output ownership`). This report is committed separately.

## Native qualification status

No native DRM/KMS qualification was attempted or claimed. The prior native residual `stale_deadline_rearms=6` / `past_deadline_arms=6` cannot be assigned to an exact owner from unit tests alone; the fixed per-owner wake counters remain available for the next hardware run. The next qualification should use a continuously producing client interval and review primary cadence together with callback/client reaction, target-selection, render, submit, KMS, and pageflip attribution.

## Final review answers

* Cursor PlaneDelta cannot change primary frame-clock phase: its pageflip completion remains non-primary.
* Pessimistic ReactiveDouble metadata cannot reserve extra physical refreshes: ownership compares its immutable claim.
* One in-flight primary cannot count as multiple physical reservations: each live primary has one claim and duplicate claims are rejected.
* An O1 READY target cannot silently mutate: identity and claim are validated and miss handling explicitly revalidates/replans.
* Commit Timing retains explicit lower-bound semantics and binding claim identity.
* A fast client is not classified as target-limited from advisory distance alone; exact surface-local reaction and binding selection evidence are required.
* Idle clients do not fail the 165 Hz criterion because no forced continuous rendering was added.
* Predictor protection and component telemetry remain; no constant tuning hides GPU/KMS tails.
* Frame callback timing was not moved; only its attribution was made exact, with callback tests covering cross-surface orderings.
* No unproven SurfacePacing change was made.
* DMA-BUF, Direct Scanout, cursor wake liveness, input pre-read semantics, and O1 were not redesigned or disabled.

Remaining limitation: only the real sustained 1920x1080@165 Hz run can prove hardware cadence, wake counters, DMA-BUF physical release, transaction safety, protocol compliance, and shutdown SafeDisable together.
