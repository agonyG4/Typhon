# CPU saturation responsiveness qualification

## Current result

The October 8 Cyberpunk capture supplied with this task shows substantial native
reactor contention and 22,613 repeated effect-owner warnings. It is evidence of
contention, not proof that every reported stall had the same cause. The follow-up
records the capture limits, the effects diagnostic repair, opt-in cycle CPU-time
instrumentation, the RT safety decision, and the still-unrun hardware A/B:
[October 8 follow-up](cpu-saturation-2026-10-08-followup.md).

The earlier V2 idle-only result is historical. No Cyberpunk A/B run has yet
qualified the code changes or established that CPU starvation is resolved. See
the [V2 architecture review](cpu-saturation-architecture-review-v2.md) for the
original idle trial and its evidence.

## Probe

Run [`scripts/typhon_cpu_saturation_probe.py`](../scripts/typhon_cpu_saturation_probe.py)
while the native Typhon process is alive:

```bash
findmnt -T /mnt/Aether/Desktop/GitHub
mkdir -p /mnt/Aether/Desktop/GitHub/Typhon-cpu-saturation/idle-1
python3 scripts/typhon_cpu_saturation_probe.py \
  --pid "$TYPHON_PID" --duration 300 --interval 0.5 \
  --output /mnt/Aether/Desktop/GitHub/Typhon-cpu-saturation/idle-1/os.jsonl
```

The probe refuses to write unless `/mnt/Aether` is mounted, the Aether GitHub
directory exists on that same filesystem, and the new output file resolves
beneath that directory. It does not overwrite an existing trial file. Set
`TYPHON_PID` only after checking that `ps -p "$TYPHON_PID" -o pid,comm,args`
names the native Typhon process.

Each JSON Lines sample records per-thread scheduler policy, nice value, affinity,
runtime and `/proc/<pid>/task/<tid>/schedstat` runnable-wait/timeslice deltas;
system CPU, memory and I/O PSI; memory available and swap used; cgroup v2 quota,
visible-ancestor CPU weights, `cpu.stat` counters and pressure; and available
CPU frequency and thermal-zone readings. Each sample includes `cpu.stat`
deltas keyed by visible cgroup path so parent quota throttling can be seen. The
final record reports per-thread-instance p50/p95/p99 CPU use and runnable-wait
milliseconds per second with sample counts. Summary keys include the TID and
thread start time so reused TIDs do not merge samples. It also reports whether
the original process identity was still present at the end and whether it was
still alive. The probe checks `/proc/<pid>/stat` start time so a reused PID is
not mistaken for the original target, and treats zombie/dead states as exited.
CPU use is normalized to one wall-time second, so one fully busy thread is
about 100%. Missing counters remain `null`; they are not treated as zero.

Check `kernel_schedstats_enabled` in the samples. If it is `0`, missing, or
unreadable, treat zero or absent scheduler-wait counters as unavailable rather
than evidence against H1. Cgroup fields can also be absent when the host does not
expose cgroup v2 data to the process.

The probe is separate from Typhon's frame path and samples at 0.5-second or
slower intervals. Running it adds a small amount of procfs/sysfs and output I/O;
use the same interval for every comparison. Do not run it for more than 30
minutes per invocation. It aggregates only scheduler sample values in memory;
the full samples are streamed to the Aether output file.

## Typhon traces

Use a fresh output directory for every trial. `bin/start-oblivion-one-tty` selects
1920x1080@165 Hz. Launch on the target TTY with:

```bash
TRIAL_DIR=/mnt/Aether/Desktop/GitHub/Typhon-cpu-saturation/idle-1
mkdir -p "$TRIAL_DIR"
TYPHON_SLOW_CYCLE_TRACE=1 \
TYPHON_POINTER_TIMING_TRACE=1 \
OBLIVION_ONE_PRESENTATION_TRACE=1 \
OBLIVION_ONE_PRESENTATION_TRACE_FILE="$TRIAL_DIR/presentation.jsonl" \
OBLIVION_ONE_LOG_FILE="$TRIAL_DIR/session.log" \
OBLIVION_ONE_PROFILE=release \
bin/start-oblivion-one-tty
```

The slow-cycle trace keeps at most 96 cycles that exceeded the refresh interval
by default and dumps them on clean runtime teardown. Set
`TYPHON_SLOW_CYCLE_TRACE_CAPACITY` to a value from 1 through 65,536 to raise the
bounded ring size. Set `TYPHON_SLOW_CYCLE_THREAD_CPU=1` to add opt-in
`CLOCK_THREAD_CPUTIME_ID` samples; the reported wall-minus-CPU value is an
off-CPU estimate, not scheduler run-queue delay. These options are described in
the [October 8 follow-up](cpu-saturation-2026-10-08-followup.md). The trace gives
post-wake phase durations, input counts, render/submission state, timer lateness
and wake context; it is not a percentile distribution of all cycles. The pointer
trace keeps eight transition records and logs completed observations. It covers
routing-to-input service, phase duration, cycle return and next reactor wake; it
is not a generic input-event-to-physical-presentation latency measurement.

The presentation transaction trace is separately opt-in and defaults to a
4096-event bounded ring. Its JSONL `PageflipPresented.timestamp_ns` events give
Typhon's pageflip-observed presentation cadence; use deltas between successive
events and report the event count. `ImmediatePresented` is a distinct event.
These timestamps are compositor/kernel presentation observations, not an
independent optical measurement of scanout. The trace file is written during the
session; use a new file for each run. Keep trace settings identical in compared
trials because optional tracing has some runtime and file-I/O cost.

`blocked_ns` means elapsed time inside `epoll_wait`; a long value may simply mean
the reactor was asleep. `timer_lateness_ns` is deadline lateness observed when
the timer fires. Neither measures scheduler runnable-queue delay. The probe's
per-thread `schedstat` runnable-wait delta is the kernel evidence for H1.

## Workload protocol

Keep the Typhon build, display mode, VRR/presentation settings, GPU driver, power
profile, application set and trace switches unchanged across trials. Use a
separate trial directory and a clean Typhon start for each run. Record the
sample count, the individual trial summaries and any interruptions. Three
300-second trials at a 0.5-second probe interval provide 600 scheduler samples
per trial for each of these conditions:

1. **Idle desktop:** leave the same desktop and windows open, with no interaction.
2. **Pointer/window interaction:** continuously move the pointer, switch focus,
   and resize or move the same windows for the full trial.
3. **Video or animation:** replay the same local clip or animation, in the same
   window size and audio configuration, for the full trial.

Repeat each condition three times. Keep the three files separate and report each
trial's sample count and percentile summary; at 600 samples per run, p99 still
represents only the upper tail, so include run-to-run spread.

For saturation, repeat three 120-second trials. Run the probe for 140 seconds
and start the load command immediately after it begins. In a separate terminal,
run the following with `stress-ng` installed:

```bash
bash -c '
  renice -n 0 -p "$$" >/dev/null || exit 1
  exec chrt --other 0 timeout --signal=TERM --kill-after=5s 130s \
    stress-ng --cpu "$1" --cpu-method matrixprod --timeout 120s --metrics-brief
' _ "$(nproc)" > "$TRIAL_DIR/stress.log" 2>&1
```

The wrapper explicitly sets nice 0 and `SCHED_OTHER`; `stress-ng` uses one CPU
worker per online logical CPU. Its own 120-second timeout is backed by an outer
130-second timeout that sends `TERM` and then `KILL` after five seconds. The
outer timeout uses its default process-group mode so its signals cover the load
process and its workers; do not add `--foreground`, which excludes children from
timeout supervision ([GNU Coreutils `timeout`](https://www.gnu.org/software/coreutils/manual/html_node/timeout-invocation.html)).
Check `ps -C stress-ng -o pid,cls,rtprio,ni,comm` during a run; workers should
show normal time-sharing policy and nice 0. Do not raise the workload's priority.

Optionally sample NVIDIA telemetry in the same trial directory, using the
installed driver's supported query fields:

```bash
timeout --signal=INT --kill-after=3s 140s nvidia-smi \
  --query-gpu=timestamp,utilization.gpu,temperature.gpu,clocks.current.graphics,power.draw \
  --format=csv -l 1 > "$TRIAL_DIR/gpu.csv"
```

Capture process/thread CPU and context-switch activity alongside the probe with
`pidstat` from the `sysstat` package:

```bash
timeout --signal=INT --kill-after=3s 140s pidstat -t -u -w 1 \
  > "$TRIAL_DIR/pidstat.log"
```

This helps identify competing external tasks. If `pidstat` is unavailable, mark
external process attribution as missing rather than inferring it from aggregate
CPU pressure alone. Also note
memory PSI, swap use, cgroup throttling, frequency/thermal changes, GPU activity,
and any audio, driver or application-throughput regressions. A high GPU busy
percentage or long renderer phase alone does not prove a GPU wait caused the
stall.

## Reading results

- **H1, reactor runnable delay:** supported when the native reactor thread has
  repeatably increased `runnable_wait_delta_ns` during user-visible stalls, and
  the delay tracks input or pageflip timing. Low runnable wait during a reproduced
  stall weakens H1.
- **H2, long or unfair in-thread work:** supported when slow-cycle phase or
  presentation trace durations align with the stall while runnable wait stays
  low. Short relevant phases during the same stall weaken H2. Inspect screenshot
  volume and render/readback timing separately; bounding the request count cannot
  eliminate the cost of one expensive capture.
- **Other constraints:** compare target-thread CPU use with system CPU PSI and
  competing task activity; inspect memory PSI/swap, cgroup quota/throttling,
  thermal zones/frequency, and NVIDIA utilization/temperature/clocks. A `100%`
  CPU label by itself cannot distinguish these cases.

Report absolute p50/p95/p99 values, sample counts, individual trial spread, and
whether counter/trace coverage was complete. The pointer trace is transition
based and the slow-cycle ring is bounded; do not present either as complete
per-event percentiles. A scheduler policy experiment should be considered only
after H1 is supported and thread/child inheritance can be controlled. The
October 8 capture supports contention as a contributor, but the RT safety gate
remains closed; see the follow-up for the specific recovery and inheritance
blockers.

## Pre-October 8 V2 evidence ledger

- **Observed in V2:** native Typhon was running with the reactor and worker
  threads on `SCHED_OTHER`. One 300-second idle probe trial recorded 598 valid
  samples; the game, saturation, and interaction workloads were not run. No
  input timing, slow-cycle trace, or presentation cadence was collected.
- **Source-proven:** `NativeRuntime::bootstrap()` blocks `SIGCHLD` before native
  initialization and `run()` enters the reactor loop
  (`src/native_output/runtime/mod.rs:852-873`,
  `src/native_output/runtime/cycle.rs:187-193`). Helpers start during bootstrap;
  the KMS worker starts in `finish_bootstrap()` and can restart
  (`src/native_output/runtime/bootstrap.rs:359-368`,
  `src/native_output/runtime/kms_worker.rs:849-893`).
- **Source-proven:** epoll wait accounting and post-wake trace start are in
  `src/native/event_loop.rs:675-881` and
  `src/native_output/runtime/cycle/pageflip.rs:600-620`.
- **Source-proven:** all pending captures are serviced in one `run_cycle()` loop;
  each capture resolves the scene, renders/captures, creates a sealed memfd, and
  replies (`src/native_output/runtime/cycle.rs:638-642,1272-1331`).
- **Hypothesis:** the screenshot burst or runnable-queue delay might contribute
  during an actual saturation event. Neither is demonstrated by this session.
- **Not hardware-qualified:** scheduler policy, screenshot budgeting, and any
  claimed responsiveness improvement. Default startup and scheduling remain
  unchanged.
