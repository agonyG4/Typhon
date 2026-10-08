# Hardware cursor responsiveness diagnostics

## Purpose and scope

This diagnostic extension measures ordinary pointer motion through input dispatch, hit testing, effect-scene resolution, cursor arbitration, Atomic KMS submission, and pageflip observation. It is opt-in, bounded, and records to memory during the session. It does not change scheduling, cursor priority, Predictive O1, KMS ownership, or rendering behavior.

Pageflip timestamps are compositor/kernel presentation observations. They are not optical measurements of when pixels became visible.

## Existing telemetry and blind spots

| Stage | Existing evidence | Blind spot |
|---|---|---|
| CPU and scheduler | `scripts/typhon_cpu_saturation_probe.py` samples per-thread runtime, runnable wait, timeslices, scheduler policy, affinity, CPU pressure, and visible cgroup counters. Its report calculates per-thread p50/p95/p99 and preserves missing/reset counters as unknown. | It does not observe pointer batches, hit tests, effects, cursor epochs, or KMS operations. |
| Native input | `cycle_dispatch.rs` counts all raw and coalesced input events and captures libinput timestamps. `NativePointerTimingTrace` retains eight pointer-constraint transition observations, including input service and dispatch spans. | Those timing records are transition based. Input counts are not motion-only, and normal motion does not receive a per-batch latency sample. Hardware event timestamps use a different clock domain from `CLOCK_MONOTONIC`; subtracting them directly would be invalid. |
| Hit testing | `pointer_scene_hit_at` tracks calls, cache hits/misses, full scans, groups/surfaces inspected, locality fast hits, and cumulative CPU nanoseconds when pointer instrumentation is enabled. The uncached path scans visual-stack groups and surfaces from front to back. | There is no production percentile distribution or ordinary-motion correlation for these metrics; the cumulative snapshot is currently exposed through test support. |
| Effect scene | `resolved_effect_scene` builds and orders visible effect instances, walking active scene surfaces when the background effect is enabled. A test-only call counter exists. | Production duration and per-motion association are not measured. |
| Cursor arbitration | `NativeCursorOutputArbitration` keeps desired epoch, response-window/coalescing counts, plane-delta plan/submission counts, primary deferrals, piggyback counts, and software updates. Primary work wins; a due idle hardware cursor update can use a plane delta when the plane is usable and software overlay work is absent. | Existing values are cumulative counters. There is no per-epoch time series for the actual disposition or an input timestamp attached to the desired epoch. A newer desired epoch can supersede an older pending epoch. |
| Atomic KMS and presentation | `PresentationTransactionTraceRing` retains transaction build, worker queue, KMS submit start/return, and pageflip observations, keyed by output transaction ID. Its default capacity is 4096 events. `trace_cursor_kms_submit` can log cursor epoch, transaction ID, token, and plane assignment when pointer presentation logging is enabled. | The structured transaction trace does not currently associate a cursor epoch with a cursor-only transaction. The separate cursor submit diagnostic is synchronous text output. A pageflip observation must not be described as optical presentation. |
| Slow cycles | `NativeSlowCycleTrace` retains only cycles that exceed the refresh interval, with 96 records by default. It describes cycle phases, input counts, rendering/submission state, and wake context. | It is not a distribution of ordinary input cycles and cannot isolate per-motion hit-test or effect-scene cost. |

## Diagnostic design

Add a dedicated `NativeCursorResponsivenessTrace` with `TYPHON_CURSOR_RESPONSIVENESS_TRACE=1`. It retains every 16th ordinary pointer-motion batch in a fixed-capacity ring (default 32,768 samples, maximum 65,536); counters cover every batch, including unsampled batches. Capacity and eviction counts are reported. The disabled path checks the cached enable flag before taking clocks, scanning input batches for motion counts, or formatting records.

Each retained sample carries:

- a monotonically increasing sample ID;
- the source-clock oldest/newest libinput motion timestamps in microseconds, without converting them to monotonic time;
- raw and coalesced pointer-motion event counts for that native input batch;
- monotonic input-batch materialization, compositor dispatch start/end, and arbitration timestamps;
- hit-test call/cache/scan/group/surface deltas and cumulative hit-test CPU time for that batch;
- exact effect-scene resolution duration for the output scene built in the same cycle;
- the output cursor epoch, arbitration disposition, and whether a later epoch superseded it as the desired cursor state.

The disposition is observed from the existing arbitration inputs and is recorded as one of `PiggybackPrimary`, `DeferForPrimary`, `SubmitPlaneDelta`, `SoftwareOverlay`, or `Noop`. The instrumentation does not feed a decision back into the scheduler.

Add a bounded `CursorEpochTransactionBuilt` event to the existing presentation transaction trace for cursor transactions. It carries transaction ID and cursor epoch. Existing KMS submit start/return and `PageflipPresented` events then join by transaction ID. This makes attribution valid for the transaction that carries the epoch; missing or evicted links remain unknown. The trace does not infer an epoch for unrelated primary commits.

The response trace is dumped at clean runtime teardown, not synchronously per input event. Sampled ring-write CPU time is recorded so its direct cost is visible. The protocol also compares otherwise identical trace-disabled and trace-enabled trials to measure total runtime overhead.

## Reproducible 1920x1080@165Hz protocol

Run three 300-second trials per condition from a clean Typhon start. Confirm `bin/start-oblivion-one-tty` selects 1920x1080@165Hz and record the actual mode. Use one release build and hold GPU driver, VRR/presentation settings, cursor settings, power profile, applications, input device, and all trace flags constant. Use the same recorded HID/uinput motion path and speed for every trial.

Prepare two conditions with the same application/window set and pointer route:

1. **Empty active workspace:** move the same windows to an inactive workspace and verify the active workspace has no visible windows.
2. **Populated active workspace:** show 24 windows in the active workspace, using the same geometry and stacking order for every run. Leave the same applications running in both conditions.

For each trial, write all data to a new directory beneath `/mnt/Aether/Desktop/GitHub/Typhon-cursor-responsiveness/`. Run the CPU probe for 300 seconds at a 0.5-second interval and enable the same slow-cycle, cursor-responsiveness, and presentation transaction traces in all six trials. Set cursor trace capacity to 32,768 or higher. Keep the pointer route, dwell time, input sample rate, and window state fixed within each condition.

Report each trial separately and then the three-run spread. Include:

- p50/p95/p99 and sample counts for dispatch duration, dispatch-to-arbitration, per-batch hit-test CPU, effect-scene resolution, KMS queue wait, KMS submit duration, and submit-return-to-pageflip-observation;
- raw and coalesced pointer-motion counts, the ratio `(raw - coalesced) / raw`, retained samples, sampled fraction, evictions, and missing/unattributed joins;
- cumulative-counter deltas for hit-test cache hits/misses and scans, groups/surfaces inspected, arbitration response windows/coalescing/piggyback/plane-delta/defer/software counts, output transaction built/submitted/presented counts, per-thread runnable-wait/runtime/timeslices, and visible cgroup `cpu.stat` counters;
- the per-sample trace-record CPU cost and a paired trace-off/trace-on comparison of reactor CPU, pointer percentiles, and runnable wait under the same motion workload.

The libinput timestamps are source timestamps, not a calibrated hardware-to-monotonic measurement. Report monotonic stages from the compositor's first sample timestamp. Label submit-to-pageflip as an observed transaction interval; do not call it optical latency. Do not compare a percentile with fewer than 1000 retained samples without reporting that limitation.

## Evidence required before selecting a fix

Use the populated-versus-empty deltas to identify which measured stage grows with active-window count. High dispatch-to-arbitration time with high reactor runnable wait points toward input service/scheduling contention. High hit-test duration accompanied by increased full scans/groups/surfaces points toward hit testing. High effect-scene resolution time that scales with scene/effect count points toward scene resolution. Long epoch-to-arbitration or repeated supersession/piggyback/defer decisions points toward cursor arbitration. Long KMS queue wait, submit duration, or submit-return-to-pageflip intervals points toward KMS/driver timing. These are attribution clues, not diagnoses by themselves; reproduce the same result across trials and inspect whether tracing changes it before proposing a behavior change.

## Initial implementation record

At the design stage, no runtime code had been changed and no tests or hardware A/B trials had been run. The final commit updates this section with implementation details, executed test commands, and remaining hardware-only uncertainties.
