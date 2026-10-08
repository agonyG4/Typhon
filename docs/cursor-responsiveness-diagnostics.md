# Hardware cursor responsiveness diagnostics

## Purpose and scope

This diagnostic extension measures ordinary pointer motion through input dispatch, hit testing, effect-scene resolution, cursor arbitration, Atomic KMS submission, and pageflip observation. It is opt-in, bounded, and records to memory during the session. It does not change scheduling, cursor priority, Predictive O1, KMS ownership, or rendering behavior.

Pageflip timestamps are compositor/kernel presentation observations. They are not optical measurements of when pixels became visible.

## Existing telemetry and blind spots

| Stage | Existing evidence | Blind spot |
|---|---|---|
| CPU and scheduler | `scripts/typhon_cpu_saturation_probe.py` samples per-thread runtime, runnable wait, timeslices, scheduler policy, affinity, CPU pressure, and visible cgroup counters. Its report calculates per-thread p50/p95/p99 and preserves missing/reset counters as unknown. | It does not observe pointer batches, hit tests, effects, cursor epochs, or KMS operations. |
| Native input | `cycle_dispatch.rs` counts all raw and coalesced input events and captures libinput timestamps. `NativePointerTimingTrace` retains eight pointer-constraint transition observations, including input service and dispatch spans. | Those timing records are transition based. Input counts are not motion-only, and normal motion does not receive a per-batch latency sample. Hardware event timestamps use a different clock domain from `CLOCK_MONOTONIC`; subtracting them directly would be invalid. |
| Hit testing | `pointer_scene_hit_at` tracks calls, cache hits/misses, full scans, groups/surfaces inspected, locality fast hits, and cumulative CPU nanoseconds when pointer instrumentation is enabled. The uncached path scans visual-stack groups and surfaces from front to back. | There is no production percentile distribution or ordinary-motion correlation for these metrics. |
| Effect scene | `resolved_effect_scene` builds and orders visible effect instances, walking active scene surfaces when the background effect is enabled. A test-only call counter exists. | Production duration and per-motion association are not measured. |
| Cursor arbitration | `NativeCursorOutputArbitration` keeps desired epoch, response-window/coalescing counts, plane-delta plan/submission counts, primary deferrals, piggyback counts, and software updates. Primary work wins; a due idle hardware cursor update can use a plane delta when the plane is usable and software overlay work is absent. | Existing values are cumulative counters. There is no per-epoch time series for the actual disposition or an input timestamp attached to the desired epoch. A newer desired epoch can supersede an older pending epoch. |
| Atomic KMS and presentation | `PresentationTransactionTraceRing` retains transaction build, worker queue, KMS submit start/return, and pageflip observations, keyed by output transaction ID. Its ordinary default capacity is 4096 events. `trace_cursor_kms_submit` can log cursor epoch, transaction ID, token, and plane assignment when pointer presentation logging is enabled. | Existing events do not tie ordinary motion to a cursor epoch. The separate cursor submit diagnostic is synchronous text output and is not used by this protocol. A pageflip observation must not be described as optical presentation. |
| Slow cycles | `NativeSlowCycleTrace` retains only cycles that exceed the refresh interval, with 96 records by default. It describes cycle phases, input counts, rendering/submission state, and wake context. | It is not a distribution of ordinary input cycles and cannot isolate per-motion hit-test or effect-scene cost. |

## Diagnostic design

Add a dedicated `NativeCursorResponsivenessTrace` with `TYPHON_CURSOR_RESPONSIVENESS_TRACE=1`. It retains every 16th ordinary pointer-motion batch in a fixed-capacity ring (default 32,768 samples, maximum 65,536); counters cover every batch, including unsampled batches. Capacity and eviction counts are reported. The disabled path checks the cached enable flag before taking diagnostic clocks, scanning input batches for motion counts, or formatting records. Sampled batches record the direct thread CPU time for creating and retaining the sample.

Each retained sample carries:

- a monotonically increasing sample ID;
- the source-clock oldest/newest libinput motion timestamps in microseconds, without converting them to monotonic time;
- raw and coalesced pointer-motion event counts for that native input batch;
- monotonic input-batch materialization, compositor dispatch start/end, and arbitration timestamps. Libinput event timestamps remain in their source clock and are never subtracted from monotonic timestamps;
- hit-test call/cache/scan/group/surface deltas and cumulative hit-test CPU time for that batch;
- exact effect-scene resolution duration for the output scene built in the same cycle;
- the output cursor epoch, arbitration disposition, and whether a later epoch superseded it as the desired cursor state.

The disposition is observed from the existing arbitration inputs and is recorded as one of `PiggybackPrimary`, `DeferForPrimary`, `SubmitPlaneDelta`, `SoftwareOverlay`, or `Noop`. The instrumentation does not feed a decision back into the scheduler.

Add a bounded `CursorEpochTransactionBuilt` observation for a primary or cursor-plane transaction whose plane assignment carries an Atomic cursor epoch. It carries output ID, transaction ID, epoch, and build timestamp. Existing worker queue, KMS submit start/return, and `PageflipPresented` observations join by transaction ID. A compact cursor-transaction history is retained separately from the general presentation-event ring, with the same configurable capacity (default 65,536 when this diagnostic is enabled, maximum 65,536). The general event ring remains bounded independently. Attribution requires matching output and epoch, a matching arbitration epoch, and a transaction built no earlier than that arbitration. Evicted or missing cursor transactions remain unknown. The trace does not infer an epoch for unrelated transactions.

For synchronous Atomic primary submission, the start observation is taken immediately before `submit_ready_frame`; the existing primary registration records the return timestamp after the call succeeds. Plane-delta start and return are observed around the cursor submit call. The KMS worker already supplies its queue, submit-start, and submit-return timestamps. The per-sample output includes these absolute monotonic observation timestamps and their derived intervals where available. The queue-wait percentile is based only on worker-submitted transactions that have both endpoints.

The response trace is dumped at clean runtime teardown, not synchronously per input event. Sampled ring-write CPU time is recorded so its direct cost is visible. The protocol also compares otherwise identical trace-disabled and trace-enabled trials to measure total runtime overhead.

## Reproducible 1920x1080@165Hz protocol

Run three 300-second trials per condition from a clean Typhon start. Confirm `bin/start-oblivion-one-tty` selects 1920x1080@165Hz and record the actual mode. Use one release build and hold GPU driver, VRR/presentation settings, cursor settings, power profile, applications, input device, and all trace flags constant. Use the same recorded HID/uinput motion path and speed for every trial.

Prepare two conditions with the same application/window set and pointer route:

1. **Empty active workspace:** move the same windows to an inactive workspace and verify the active workspace has no visible windows.
2. **Populated active workspace:** show 24 windows in the active workspace, using the same geometry and stacking order for every run. Leave the same applications running in both conditions.

Use output scale 1x. Replay one saved uinput motion sequence at the same event rate and timestamps in both conditions. A fixed route is a continuous rectangular loop through logical coordinates `(96,96)`, `(1824,96)`, `(1824,984)`, `(96,984)`, and back to `(96,96)`. Store the replay fixture and its SHA-256 with each trial; keep pointer acceleration/speed and replay rate unchanged. Let the loop run for all 300 seconds. This makes the pointer cross the same screen regions while visible-window count changes.

For each trial, write all data to a new directory beneath `/mnt/Aether/Desktop/GitHub/Typhon-cursor-responsiveness/`. Run the CPU probe for 300 seconds at a 0.5-second interval. Use the same release build, display mode, VRR/presentation configuration, GPU driver, power profile, cursor settings, applications, and recorded HID/uinput motion path for every run. Use the same pointer path, dwell time, input sample rate, and window geometry/stacking order within each workspace condition. The active workspace is empty in one condition; in the other, show the same 24 windows while keeping those applications running in both.

Enable the same traces in all six workspace trials. The cursor diagnostic also enables its bounded presentation transaction ring. Set `TYPHON_CURSOR_RESPONSIVENESS_TRACE=1`, `TYPHON_CURSOR_RESPONSIVENESS_TRACE_CAPACITY=32768`, `TYPHON_CURSOR_RESPONSIVENESS_TRACE_FILE="$TRIAL_DIR/cursor-responsiveness.jsonl"`, and `OBLIVION_ONE_PRESENTATION_TRACE_CAPACITY=65536`. Keep the existing `TYPHON_SLOW_CYCLE_TRACE=1`, `TYPHON_POINTER_TIMING_TRACE=1`, and `OBLIVION_ONE_PRESENTATION_TRACE=1` settings consistent. Do not set `OBLIVION_ONE_PRESENTATION_TRACE_FILE`; the cursor trace includes valid joined KMS timestamps and writes once at runtime teardown without synchronous per-motion logging. The CPU probe's output path must be beneath Aether as required by its own path validation.

Launch each trial through the 165 Hz TTY entry point from a separate terminal using the same settings:

```bash
findmnt -T /mnt/Aether/Desktop/GitHub
TRIAL_DIR=/mnt/Aether/Desktop/GitHub/Typhon-cursor-responsiveness/populated-1
mkdir -p "$TRIAL_DIR"
export TYPHON_SLOW_CYCLE_TRACE=1
export TYPHON_POINTER_TIMING_TRACE=1
export TYPHON_CURSOR_RESPONSIVENESS_TRACE=1
export TYPHON_CURSOR_RESPONSIVENESS_TRACE_CAPACITY=32768
export TYPHON_CURSOR_RESPONSIVENESS_TRACE_FILE="$TRIAL_DIR/cursor-responsiveness.jsonl"
export OBLIVION_ONE_PRESENTATION_TRACE=1
export OBLIVION_ONE_PRESENTATION_TRACE_CAPACITY=65536
export OBLIVION_ONE_LOG_FILE="$TRIAL_DIR/session.log"
export OBLIVION_ONE_PROFILE=release
bin/start-oblivion-one-tty
```

After checking that the process is the native Typhon instance and recording the actual mode, run the existing 300-second CPU probe from another terminal as shown in `docs/cpu-saturation-qualification.md`. Start the motion replay at the probe's start and retain its log and checksum beside `os.jsonl`.

To measure the added trace overhead, run an additional matched trace-off/trace-on pair for each workspace condition with the same probe and input workload. Leave the slow-cycle and pointer-transition settings unchanged; turn off both `TYPHON_CURSOR_RESPONSIVENESS_TRACE` and `OBLIVION_ONE_PRESENTATION_TRACE` for the off run. Compare reactor CPU, the probe's runnable-wait percentiles, pointer timing, and throughput. Report the diagnostic's `trace_write_cpu_ns` percentiles and total directly. Do not interpret an off run as providing the per-motion stage percentiles.

Report every trial separately and then the three-run spread. Include the count next to every percentile; do not compare any percentile based on fewer than 1,000 observations. Include:

- p50/p95/p99 and sample counts for input-batch-to-dispatch-complete, dispatch duration, dispatch-complete-to-arbitration, per-batch hit-test CPU, effect-scene resolution, KMS queue wait, KMS submit duration, and submit-return-to-pageflip-observation;
- raw and coalesced pointer-motion counts, the ratio `(raw - coalesced) / raw`, retained samples, sampled fraction, sample evictions, cursor transaction-history retention/evictions, retained cursor transaction counts for build/worker-queue/KMS-start/KMS-return/pageflip observations, general presentation event retention/evictions, and missing/unattributed joins;
- cumulative-counter deltas for hit-test calls/cache hits/cache misses/full scans/groups/surfaces/CPU nanoseconds and cursor-arbitration response windows/coalescing/piggyback/plane-delta/defer/software updates, plus per-thread runnable-wait/runtime/timeslices and visible cgroup `cpu.stat` counters from the CPU probe;
- the per-sample trace-record CPU cost and a paired trace-off/trace-on comparison of reactor CPU, pointer percentiles, and runnable wait under the same motion workload.

The libinput timestamps are source timestamps, not a calibrated hardware-to-monotonic measurement. Report monotonic stages from the compositor's input-batch materialization or dispatch-start timestamp. Label submit-return-to-pageflip as an observed transaction interval; do not call it optical latency. Keep KMS queue, submit, and pageflip percentiles restricted to valid joins and show the associated sample counts. Report insufficient counts instead of treating missing data as zero.

## Evidence required before selecting a fix

Use the populated-versus-empty deltas to identify which measured stage grows with active-window count. Longer dispatch duration points toward input handling; longer dispatch-complete-to-arbitration time isolates delay after event handling and can be compared with reactor runnable wait. High hit-test duration accompanied by increased full scans/groups/surfaces points toward hit testing. High effect-scene resolution time that scales with scene/effect count points toward scene resolution. Repeated supersession/piggyback/defer decisions or longer dispatch-complete-to-arbitration time points toward cursor arbitration progress. Long KMS queue wait, submit duration, or submit-return-to-pageflip intervals point toward KMS/driver timing. These are attribution clues, not diagnoses by themselves; reproduce the same result across trials and inspect whether tracing changes it before proposing a behavior change.

## Initial implementation record

Implementation adds bounded per-motion samples, session hit-test and arbitration counter deltas, and a compact cursor-epoch transaction history for valid KMS joins. Sampling is one batch in sixteen; the sample history defaults to 32,768 entries and the cursor transaction history defaults to 65,536 entries when enabled, with configurable sample capacity and eviction counts. The focused tests cover disabled lazy capture, empty batches, bounded retention, matching and rejected epoch/output/time attribution, event-ring eviction, supersession, and primary-piggyback/plane-delta disposition.

Focused verification passed on the current checkout with build output directed to `/mnt/Aether/Desktop/GitHub/Typhon-target`:

- `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo test --bin oblivion-one native_output::runtime::cursor_responsiveness::tests:: -- --nocapture` — 11 passed.
- `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo test --bin oblivion-one native_output::presentation::trace::tests:: -- --nocapture` — 7 passed.
- `rustfmt --config skip_children=true --edition 2024 --check` on the diagnostics source files passed; `git diff --check` passed. The repository-wide format check still reports pre-existing formatting in unrelated `compositor/effects.rs` and `runtime/termination.rs` edits.

No physical 1920x1080@165Hz A/B capture or trace-off/trace-on overhead trial has been run. The diagnostic therefore identifies no winning cause and selects no behavior fix. Pageflip observations remain kernel/compositor timestamps, not optical presentation measurements.
