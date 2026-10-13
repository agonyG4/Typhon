# Cursor responsiveness diagnostics and hardware qualification

## Scope

The opt-in cursor trace follows sampled pointer-motion batches through input dispatch, hit testing, effect-scene resolution, cursor arbitration, and attributable Atomic KMS transactions. It observes existing decisions and does not change scheduling, cursor priority, rendering, or KMS ownership.

Pageflip timestamps are compositor/kernel transaction observations. They do not measure optical pixel visibility or hardware-cursor scanout latency.

## Trace costs and timing boundaries

Set `TYPHON_CURSOR_RESPONSIVENESS_TRACE=1` to enable the bounded trace. It samples one in every 16 nonempty pointer-motion batches by default. The sample ring defaults to 32,768 entries and is clamped to 1–65,536. The separate cursor-transaction history is bounded at 65,536 entries by default. The output is serialized at clean runtime teardown rather than synchronously for each motion.

Sample IDs increase by one for each retained sample. The trace stops assigning IDs after `u64::MAX` rather than repeating an ID. FIFO lookup uses the offset from the oldest retained ID and verifies that the record at that offset has the requested ID; absent and evicted IDs return no record. The per-output latest-epoch index is updated when a sample is completed and cleared when that indexed sample is evicted. Its size is bounded by the retained records, so epoch supersession does not walk the history.

`trace_write_cpu_ns` starts immediately before the sampled timing-capture closure. It ends after record construction, FIFO eviction/insertion, and the O(1) lookup of the newly appended record. The final clock read, storing that measured value and adding it to the aggregate are outside the interval because they occur after its endpoint. The per-output latest-epoch index update happens when dispatch completion is recorded, after this interval. Per-batch counters updated before the sampling branch, libinput dispatch, semantic input routing, and work after `record_motion_batch()` returns are also outside this field. Compare trace-off and trace-on trials to measure total instrumentation impact; this field is only the sampled record-retention cost.

With the trace disabled, `record_motion_batch()` returns before invoking its lazy capture closure or reading diagnostic clocks. The ring is constructed with zero capacity and the empty hash map allocates no buckets; the disabled call path performs no trace-owned heap allocation. The runtime caller also gates pointer-motion event scanning on the cached trace-enabled flag.

The published effect-demand summary is refreshed when effect registrations, material selection or parameters, background blur configuration, fullscreen ownership, XDG geometry, active-scene membership, or effect-relevant surface geometry changes. Incremental scene updates compare the fields used by effect bounds and blur assignment before refreshing the summary. A surface-tree publication defers its scene-geometry invalidation and refreshes at the transaction boundary after its nodes become authoritative; explicit effect-configuration changes may also trigger a refresh during publication. Pure cursor motion never invalidates this summary. Geometry-preserving content commits can still take the conservative end-of-transaction refresh path. Static scenes can answer frame-demand queries from the published summary; when the summary reports a continuous effect, the compositor still resolves the live scene to preserve time-dependent dirty regions. Tests compare cached demand against a fresh resolution across registration/removal, material changes, workspace visibility changes, blur toggles, internal-anchor destruction, fullscreen/XDG geometry updates, and static/continuous demand.

The retained records distinguish these clock domains and stages:

- Libinput's source-clock oldest/newest event timestamps in microseconds. They are not subtracted from monotonic timestamps.
- Monotonic input-batch materialization, dispatch start/completion, and cursor-arbitration times.
- Hit-test counter and CPU-time deltas for the sampled dispatch.
- Bounded pointer-hit invalidation, interaction-topology invalidation, and visual-stack cache-rebuild deltas. The JSONL fields are `hit_test_generation_invalidations`, `hit_test_topology_invalidations`, and `hit_test_visual_stack_groups_cache_rebuilds`; these distinguish invalidated hit knowledge from topology changes and actual group reconstruction without per-motion logging.
- Effect-scene resolution duration where resolution is associated with that cycle. A missing value is unknown, not zero.
- Cursor epoch, policy (`auto`, `piggyback`, or `software`), arbitration reason, pending-since age, freshness deadline, overdue age, retry-not-before, and supersession.
- Primary carrier/ownership state, physical cursor target sequence and presentation time, worker wake time, and commit-complete deadline.
- A transaction joined by output and epoch, subject to matching arbitration and build-time constraints; worker queue, KMS submit start/return, and pageflip observation values appear only when present.

Each sampled input record is followed by an `arbitration_evidence` JSONL record. Cursor age at arbitration is recorded directly; age at transaction build, worker queue, KMS submit, and pageflip is derived from the preserved pending-since timestamp and the exact same output/epoch transaction join. These ages retain the original pending-since timestamp across coalesced epochs and retries. A missing join is reported as missing data, not zero latency. Pageflip-derived cursor age is still an observed presentation interval, not optical visibility.

Pointer-hit validity follows interaction state rather than render churn. Repeated physical presentation at unchanged authority and root rectangles, including changed frame IDs and sample timestamps, should report zero pointer-hit invalidations, topology invalidations, and group-cache rebuilds. Opacity- or clip-only presentation changes also leave hit validity unchanged. A changed physical rectangle invalidates pointer mapping while preserving topology. Empty or Minimize-only lifecycle publications do not invalidate pointer hits; a successful settled Restore acknowledgement does when it removes logical suppression. These counters are bounded aggregates and do not establish hardware latency improvement.

Hit-test counters distinguish exact-coordinate cache hits, `hit_test_certificate_hits`, `hit_test_certificate_misses`, successful `hit_test_certificate_builds`, conservative `hit_test_certificate_rejections`, owner-locality hits, and full scene scans. The exact-coordinate cache serves repeated coordinates. A certificate serves motion at new coordinates inside a proven client-safe cell. Owner locality is the fallback proof that still traverses scene/group state; the full scan remains the canonical exact result. V1 certificates intentionally reject custom input regions, visual apertures, server-side decoration complexity, and ambiguous overlapping blocker envelopes. Those rejections preserve normal hit testing and are not failures. Eligible ordinary non-fullscreen client motion can be O(1) with respect to scene size while a certificate remains valid; dominant fullscreen still constructs `fullscreen_composition_plan()` before cache reuse, so pointer processing is not universally O(1). Hardware latency improvement requires separate measurement.

Percentile records include their valid sample count. Do not interpret a percentile from fewer than 1,000 observations as a stable tail estimate. KMS queue wait uses only worker transactions with both queue and submit-start timestamps. Submission-to-pageflip is an observed transaction interval, not optical latency.

Run the manual capacity benchmark with:

```sh
CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target rtk cargo test --locked --bin oblivion-one native_output::runtime::cursor_responsiveness::tests::benchmark_lookup_cost_across_retained_sample_capacities -- --ignored --nocapture
```

The benchmark reports `sample`, `sample_mut`, and `note_cursor_epoch` cost at capacities 16, 1,024, 16,384, and 32,768. It has no wall-clock assertion; compare its output on the same machine and build profile. Verify `CARGO_TARGET_DIR` resolves under `/mnt/Aether/Desktop/GitHub/Typhon-target` before building.

Compare the trace API's disabled and enabled path with:

```sh
CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target cargo test --locked --bin oblivion-one native_output::runtime::cursor_responsiveness::tests::benchmark_trace_disabled_vs_enabled_motion_batch_cost -- --ignored --nocapture
```

This microbenchmark processes one million synthetic motion batches with the default 16:1 sample stride and 32,768-record ring. It includes sampled record creation, completion, and epoch-index maintenance when enabled; it excludes input decoding, caller-side pointer-event scanning, hit testing, effects, rendering, and KMS. On this host, a single unoptimized test-profile run on 2026-10-08 measured 11 ns/batch disabled and 105 ns/batch enabled (94 ns/batch added), with 62,500 samples, 29,732 FIFO evictions, and 26.94 ms total `trace_write_cpu_ns`. The separate capacity run measured `sample` at 38–42 ns/op, `sample_mut` at 39–44 ns/op, and `note_cursor_epoch` at 275–302 ns/op across all four ring sizes. These are single-run microbenchmark observations, not end-to-end overhead or stable tail estimates. The paired trace-off/on hardware trials below remain necessary before trusting compositor latency attribution.

## Host preflight

Run these read-only checks from the existing desktop session before planning a native TTY trial. They do not start Typhon:

```fish
hyprctl monitors -j | jq -er '[.[] | select(.focused)][0] | {name, description, width, height, refreshRate, scale}'
set active_mode (hyprctl monitors -j | jq -er '[.[] | select(.focused)][0] | "\(.width)x\(.height)@\(.refreshRate | round)"')
printf 'Record this mode for the later TTY trial: %s\n' "$active_mode"
nvidia-smi --query-gpu=name,driver_version --format=csv,noheader
findmnt -T /mnt/Aether/Desktop/GitHub
```

On the host used to prepare this procedure, the focused Hyprland output was DP-1 at 1920×1080, scale 1, `164.99899` Hz; the selected NVIDIA device was GeForce RTX 3060 Ti with driver 615.71.09. Treat these as a dated host observation, not a requirement. Record the actual output, refresh, GPU, and driver for each qualification session. Typhon's `OBLIVION_ONE_MODE` parser rounds a fractional refresh to integer Hz; for that observed mode, the corresponding exact selector is `1920x1080@165`.

The active desktop in this environment is not Typhon. Do not invoke either Typhon startup script during this preflight or while doing the current desktop work. Hardware runs are a later, explicit operator action from a TTY only when replacing the current session is intended.

## Auto versus Piggyback hardware A/B

Run the same release binary and keep every setting other than `OBLIVION_ONE_CURSOR_SCHEDULING` fixed. Do not let a script restart the active desktop compositor. Start Typhon manually from the intended TTY session, use the same recorded 165 Hz mode for both runs, and keep `OBLIVION_ONE_CURSOR=hardware`.

First compare with low-overhead tracing disabled:

1. Steam visible, continuous UI updates and scrolling, and several visible windows, at 165 Hz with the hardware cursor.
2. Repeat that same desktop workload with Cyberpunk running.

Use matched run lengths and repeat the order in both directions where practical. Record the binary SHA, mode, VRR/tearing settings, GPU/driver, pointer device/rate, applications, and workload duration. For each policy, run the same Fish launch block from the TTY, changing only the marked scheduling value:

```fish
set -gx OBLIVION_ONE_PROFILE release
set -gx OBLIVION_ONE_BINARY /mnt/Aether/Desktop/GitHub/Typhon-target/release/oblivion-one
set -gx OBLIVION_ONE_MODE '1920x1080@165'
set -gx OBLIVION_ONE_CURSOR hardware
set -gx OBLIVION_ONE_VRR auto
set -gx OBLIVION_ONE_TEARING off
set -gx OBLIVION_ONE_CURSOR_SCHEDULING auto # set to piggyback for the baseline run
exec /home/agony/GitHub/Typhon/bin/start-oblivion-one-tty
```

The second run uses the identical block and binary with `OBLIVION_ONE_CURSOR_SCHEDULING=piggyback`. If the actual display mode or VRR/tearing policy differs from these example values, record the real settings and use those same values in both runs. Do not compare different binaries or change cursor, mode, VRR, or tearing settings between arms.

After the subjective low-overhead comparison, repeat both workloads with the existing cursor-responsiveness and presentation traces enabled. Keep all trace settings identical between the Auto and Piggyback runs, write each run to a separate trace file, and do not restart the compositor automatically.

Report p50/p95/p99 and observation counts for input batch → dispatch complete, dispatch complete → arbitration, arbitration → transaction build, arbitration → worker queue, worker queue → KMS submit, KMS submit duration, submit return → pageflip observation, and cursor age at arbitration/worker queue/KMS submit/pageflip. Also report policy and reason ratios, independent PlaneDelta submissions per second, ready-primary piggybacks, unready-primary bypass decisions, EBUSY retries, target misses, primary presentation rate, and primary target misses. Pageflip remains a compositor/kernel observation, not optical cursor latency.

## Trial matrix

Use one release binary and keep the output mode, VRR/tearing policy, GPU driver, power profile, cursor setting, pointer device and polling rate, applications, and background services fixed. For each workload, perform three matched trace-off/trace-on pairs, alternating pair order where practical:

| Workload | Visible workspace | Activity |
|---|---|---|
| Empty | No visible application windows; keep the same applications running on another workspace | Same pointer route as other trials |
| Populated static | A recorded, fixed set of visible windows (prefer 20 where available) | No deliberate window movement, video, or animation |
| Populated dynamic | The same visible window set and geometry | Reproducible client updates or animation; record which applications and update source |

Keep the pointer route and duration identical within each trace-off/on pair. If no controlled uinput replay is available, use the same recorded manual route and mark it as a less controlled input source; this checkout does not provide a motion-replay utility. Record window count, visible surfaces if available, geometry/stacking, output scale, workload applications, input-device rate, actual output mode, and any environmental differences in every trial directory.

Each trial should last 300 seconds. At 16:1 sampling, 1,000 trace samples require at least 16,000 nonempty motion batches; report the actual count. If any metric has fewer than 1,000 valid observations, report its count and mark the percentile insufficient.

Trace-off turns off only `TYPHON_CURSOR_RESPONSIVENESS_TRACE` and `OBLIVION_ONE_PRESENTATION_TRACE`; keep `TYPHON_SLOW_CYCLE_TRACE` and `TYPHON_POINTER_TIMING_TRACE` identical in both runs. This paired comparison measures the cursor/presentation tracing increment on top of those common diagnostics. Do not claim that trace-off provides per-motion cursor-stage percentiles.

## Fish launch and capture workflow

Build the release binary only after verifying the Aether target path. Never build into the checkout's `target/` directory:

```fish
set target_dir /mnt/Aether/Desktop/GitHub/Typhon-target
realpath "$target_dir"
findmnt -T "$target_dir"
env CARGO_TARGET_DIR="$target_dir" rtk cargo build --locked --release
```

When a hardware trial is explicitly scheduled, start it from the TTY entry point using the mode recorded during preflight. The following Fish block shows the trace-on settings; do not run it in the active desktop session:

```fish
set trial_dir /mnt/Aether/Desktop/GitHub/Typhon-cursor-responsiveness/populated-static-on-01
mkdir -p "$trial_dir"
set -gx OBLIVION_ONE_PROFILE release
set -gx OBLIVION_ONE_BINARY /mnt/Aether/Desktop/GitHub/Typhon-target/release/oblivion-one
set -gx OBLIVION_ONE_MODE 'WIDTHxHEIGHT@HZ' # replace with the recorded output mode
set -gx OBLIVION_ONE_LOG_FILE "$trial_dir/session.log"
set -gx TYPHON_SLOW_CYCLE_TRACE 1
set -gx TYPHON_POINTER_TIMING_TRACE 1
set -gx TYPHON_CURSOR_RESPONSIVENESS_TRACE 1
set -gx TYPHON_CURSOR_RESPONSIVENESS_TRACE_CAPACITY 32768
set -gx TYPHON_CURSOR_RESPONSIVENESS_TRACE_FILE "$trial_dir/cursor-responsiveness.jsonl"
set -gx OBLIVION_ONE_PRESENTATION_TRACE 1
set -gx OBLIVION_ONE_PRESENTATION_TRACE_CAPACITY 65536
exec /home/agony/GitHub/Typhon/bin/start-oblivion-one-tty
```

Use a distinct `$trial_dir` for every trace-off/on run. For trace-off, set both cursor and presentation trace variables to `0` and remove `TYPHON_CURSOR_RESPONSIVENESS_TRACE_FILE` with `set -e TYPHON_CURSOR_RESPONSIVENESS_TRACE_FILE`. Keep the slow-cycle and pointer timing variables unchanged. Do not set `OBLIVION_ONE_PRESENTATION_TRACE_FILE`; the cursor trace joins bounded presentation observations and writes its report at clean teardown.

After Typhon has been started manually for a trial, open a second terminal and confirm the PID before sampling. Do not make the capture script start or restart the compositor:

```fish
set trial_dir /mnt/Aether/Desktop/GitHub/Typhon-cursor-responsiveness/populated-static-on-01
mkdir -p "$trial_dir"
pgrep -a -x oblivion-one
set pid (read -P 'Enter the PID shown for the Typhon compositor: ')
ps -p "$pid" -o pid=,uid=,comm=,args=
python3 /home/agony/GitHub/Typhon/scripts/typhon_cpu_saturation_probe.py --pid "$pid" --duration 300 --interval 0.5 --output "$trial_dir/os.jsonl"
```

Confirm the displayed command and UID identify the intended native Typhon process before entering the PID. Keep the CPU probe output below `/mnt/Aether/Desktop/GitHub`; the probe rejects output elsewhere. Start the same pointer route/workload for the full capture interval. Stop Typhon through the intended session path so the bounded cursor trace can flush at clean teardown.

The Fish syntax above was checked with Fish 4.9.3. The only launch binary defaults in the TTY script are overridden above, so neither a build nor capture writes artifacts under the source checkout.

## Trial report contents

Keep each run's trace, `os.jsonl`, session log, and a short metadata file together. Report each run separately, then the three-pair spread. Include:

- p50/p95/p99 plus valid counts for input-batch-to-dispatch-complete, dispatch duration, dispatch-complete-to-arbitration, hit-test CPU, effect-scene resolution, KMS queue wait, KMS submit duration, and submit-return-to-pageflip observation;
- raw/coalesced motion counts, `(raw - coalesced) / raw`, retained samples, sampled fraction, sample evictions, retained/evicted cursor transactions, observation counts for transaction build/worker queue/KMS start/KMS return/pageflip, general presentation-event retention/evictions, and missing/unattributed joins;
- cursor-disposition ratios with the decision count as denominator, superseded samples, hit-test call/cache/miss/full-scan/group/surface/CPU deltas, and arbitration response-window/coalescing/piggyback/plane-delta/defer/software counters;
- CPU probe per-thread runtime, runnable wait and timeslices, scheduler policy/affinity, pressure, and available cgroup `cpu.stat` counters;
- `trace_write_cpu_ns` distribution and total from trace-on, plus paired trace-off/on changes in process CPU, runnable wait, and any comparable input/dispatch metric.

Compare empty to populated-static to identify costs that scale with visual scene size. Compare populated-static to populated-dynamic to separate scene complexity from ongoing client updates. Increased dispatch time, hit-test work, or effect-resolution time are attribution clues; higher runnable wait, KMS queue delay, driver submission time, or primary frame misses provide different clues. Repeat a difference across pairs and check trace overhead before recommending cursor scheduling work. Do not infer optical display latency from a pageflip.

## Current qualification status

The host GPU and focused output were inspected, and the Fish command forms and CPU-probe CLI were checked. No Typhon compositor was launched, no active session was displaced, and no trace-off/trace-on hardware trial has run. There are no hardware latency or trace-overhead results yet; cursor scheduling changes are not justified until these measurements identify a repeatable bottleneck.
