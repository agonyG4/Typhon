# KMS Timing v2 and Commit Worker Throughput Closure Plan

## Working rules

- Preserve the pre-existing `.codex/config.toml` deletion and all unrelated edits.
- Use the current source as authoritative; preserve the post-`ea1a1c6`
  pageflip/presented-state architecture.
- Use tests before production behavior changes and keep each change reviewable.
- Reuse the existing Cargo target cache and benchmark harness.

## Phase 1: semantic contract and types

1. Add failing unit tests for the distinction between `earliest_submit`, worker
   wake time, and `commit_complete_deadline`; cover early, headroom-lost, on-time,
   and actual dispatch-late paths.
2. Replace the impossible Reactive Double regression with reachability tests that
   pass predicted total cost while asserting rendering still starts immediately.
3. Add focused `KmsModeTiming`, `KmsModeTimingKey`, `KmsSubmitWindow`,
   `KmsPresentationTimingModel`, and `KmsWorkerDispatchModel` tests.

## Phase 2: physical timing model

1. Derive checked mode timing from the selected exact `drm_mode_modeinfo`.
2. Add runtime-owned apply guard state with bounded increase on on-time-submit
   pageflip misses and slow decay on stable target hits.
3. Tie state to exact mode identity and output/DRM generation; reset stale state.
4. Add pageflip outcome classification tests, including dispatch-primary
   attribution when both dispatch and presentation fail.

## Phase 3: reachable target and shared windows

1. Update all Reactive target call sites to pass predicted total cost.
2. Carry a computed submit window into both worker and synchronous submission.
3. Make the worker wait only against the planned wake boundary and submit
   immediately when the job arrives after that boundary but before completion.
4. Keep async/tearing and Commit Timing semantics on their existing paths.

## Phase 4: worker dispatch instrumentation

1. Remove the worker's presentation-feedback tuning path and early `SubmitLate`
   event.
2. Make the interruptible wait report its actual return timestamp.
3. Record enqueue, planned wake, actual wake return, pre-submit completion,
   ioctl start/return, and pageflip timestamps.
4. Learn a bounded dispatch budget from wake lateness plus post-wake dispatch,
   with fixed allocation-free histograms at sub-millisecond resolution.

## Phase 5: render prediction and runtime integration

1. Export separate render risk, compositor wake guard, worker dispatch budget,
   apply guard, KMS lead, and total-cost values.
2. Feed pageflip outcomes into runtime presentation timing and Adaptive Triple with
   exact identity; do not duplicate worker truth or double count KMS lead.
3. Preserve scene history, transaction settlement, physical framebuffer identity,
   Direct Scanout, cursor sidecars, and generation assertions.

## Phase 6: verification and performance closure

1. Run focused tests continuously, then the complete required Cargo/test/layout
   validation set.
2. Run the final alternating fullscreen FIFO 2x2 with raw MangoHud and compositor
   artifacts, reporting target hits, unreachable opportunities, readiness misses,
   dispatch misses, and apply misses.
3. Run Kitty idle/scroll/overlap/move/resize and native Wayland vkcube tracing.
4. Only after Timing v2 is correct, run normal-scheduler versus best-effort
   minimum-priority `SCHED_RR`. Retain it only on measured benefit.
5. Implement pre-armed reservation only if post-v2 data proves a remaining >3%
   regression is dominated by frame-ready-to-worker-submit handoff latency and
   neither Timing v2 nor SCHED_RR resolves it.
6. Perform the mandatory temporal-correctness and architecture/scope reviews,
   then produce the required baseline/final benchmark and attribution report.
