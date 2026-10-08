# Presentation trace exporter

## Source finding

The October 8 capture showed a sharp increase in unattributed reactor CPU near
the time the 65,536-event trace ring would fill. The supplied analysis report
named `TYPHON_CYBERPUNK_CPU_SATURATION_ANALYSIS_2026-10-08.md` was not present in
this checkout; the numeric capture summary is recorded in
[`cpu-saturation-2026-10-08-followup.md`](cpu-saturation-2026-10-08-followup.md).
This is strong temporal correlation, not a measurement that attributes every
long cycle to trace export.

The source-level defect was confirmed in
`src/native_output/presentation/trace.rs` and
`src/native_output/runtime/cycle.rs`: after ring overwrite, the export cursor
compared both retained length and dropped count, so each subsequent update
serialized the entire retained history as `TraceExport::Replace`. The
`run_cycle() -> flush_presentation_trace() -> export_delta() ->
write_presentation_trace_export()` path then synchronously rewrote the whole
JSONL file from the native reactor. At the captured capacity, that history was
about 5.3 MB. The independent renderer and cursor-planning stalls recorded
before the expected saturation point remain unexplained by this repair.

## Event and ownership model

The ring now assigns every retained event a 128-bit monotonic sequence number
within a trace source. Each record carries schema version, process ID, a
128-bit random source ID, event sequence, source name, transaction ID, and
`CLOCK_MONOTONIC` nanosecond timestamp. Cursor-epoch events also retain output
and cursor-epoch identity. A JSONL event is independently parseable. Retention
overwrites emit an explicit `loss` record with the inclusive missing sequence
range and reason `retention_overwrite`.

The ring and producer are owned by `NativeRuntime`. The KMS worker's
`run_worker()` receives worker state and a KMS executor; it publishes typed
completion events back to runtime-owned queues. Runtime code applies those
completions and records presentation trace events. The ring itself is not
shared with the KMS thread. The graph reports some false-positive `push`
call edges because unrelated `Vec::push` calls have the same method name; the
source signatures and `KmsCommitWorkerHandle::start()` / `run_worker()` bodies
confirm the single producer. Recheck this ownership if a future change gives
the ring to another thread.

`PresentationTransactionTraceRing::append_delta_records()` copies only newer
retained entries into a caller-owned preallocated `Vec`; its cost follows the
new records, plus at most one loss record. The normal overwrite path no longer
re-serializes retained history. The existing bounded ring remains the
in-memory flight recorder.

## Persistence path

When both trace collection and `OBLIVION_ONE_PRESENTATION_TRACE_FILE` are
enabled, the runtime starts one `typhon-trace-writer` thread. The reactor
collects typed records into one of five preallocated batches and sends through
bounded `std::sync::mpsc::sync_channel` queues using `try_send`. This is
single-producer/single-consumer ownership, but the standard channel is not
lock-free; benchmark its producer cost before replacing it. A full queue keeps
the scratch batch bounded, records a stall counter once per stall period, and
continues to run. If the in-memory ring overwrites records before the writer
catches up, the next exported batch names the lost range.

Batches flush at 128 records or after 20 ms. JSON serialization and one file
write per batch run on the writer. The active file and rotated files are
bounded by `OBLIVION_ONE_PRESENTATION_TRACE_MAX_BYTES` and
`OBLIVION_ONE_PRESENTATION_TRACE_MAX_FILES` (defaults: 16 MiB per file and four
files). The active file counts toward the file limit. Configuration is clamped
to 1 MiB–1 GiB per file and one to 16 files. A writer restart resumes from the
highest persisted sequence for the same process/source identity across the
active and retained rotated files. A partial final JSONL record in the active
file is truncated before appending.

When rotation or retention pruning removes a nonempty file, the next successful
batch begins with a versioned `retention_expired` JSONL metadata record scoped
to the trace file set. It has no event sequence or producer identity and is
never counted as a `loss` record. This marks that an older history prefix
expired without inventing a producer-loss range. Event and `loss` records
retain their source identities and monotonic sequence order; a retained set
can therefore start above sequence one while remaining internally continuous.
Each later file eviction emits a fresh marker, so the marker itself remains
within the same file-count and byte limits. If storage fails permanently before
that batch can be written, the writer reports the I/O failure and cannot promise
that the marker reached disk; trace files are diagnostic output, not
crash-safe storage.

The writer retries recoverable open and write failures with bounded backoff.
If a write partially succeeds, it truncates the file to the batch start before
retrying. Failure to roll back is fatal to that trace writer, not to the
compositor. Writer errors, queue stalls, abandoned event counts, and shutdown
timeouts are aggregated into one diagnostic line. Runtime shutdown attempts
to hand off and flush for at most 50 ms; a forced process termination may lose
queued trace data. The trace is diagnostic output, not crash-safe storage.

The normal cycle only calls nonblocking `try_send`/`try_recv`. The bounded
shutdown drain is the sole producer-side wait and occurs after termination
starts; if its deadline expires, the writer is detached and normal teardown
continues. A blocked filesystem write can outlive that deadline, but cannot
hold the reactor or compositor process open.

## Writer startup lifecycle

File recovery is asynchronous. `PresentationTraceWriter::start_with_config()`
creates the existing bounded queues and one writer thread, then returns in the
`Initializing` state without waiting for filesystem recovery. The worker sends
its recovered source sequence through a one-slot readiness channel and waits
for an explicit activation message before consuming trace batches. Only the
reactor can send that message, so a timed-out or abandoned worker cannot
activate itself later.

The reactor polls readiness from its existing trace submission work; startup
adds no timer or polling source. The reactor must accept readiness within one
second of writer creation. The deadline is checked at those existing polls, so
a first poll after the deadline fails closed even if recovery completed
earlier. If readiness is an error, the worker panics, the result misses the
deadline, or shutdown begins first, file export becomes `Disabled`. The
in-memory trace ring remains available, and the runtime does not retry or spawn
another writer for that trace source. Startup outcomes join the existing
aggregated writer counters.

Cancellation closes the readiness and activation channels and sets the worker
stop flag. A filesystem call already in progress cannot generally be
interrupted, so the reactor drops the `JoinHandle` and continues; that thread
owns only its paths, file handles, channels, stop flag, and shared counters. It
does not borrow runtime or renderer state. If the filesystem call returns, the
worker observes the closed channel or stop flag and exits without entering the
writer loop. Shutdown during initialization does not wait. Active-writer
shutdown retains the existing 50 ms total budget, and a worker is joined only
after its completion signal and `JoinHandle::is_finished()` both confirm it
has finished; otherwise it is detached.

Tracing remains opt-in through `OBLIVION_ONE_PRESENTATION_TRACE=1` or the
existing cursor-responsiveness diagnostic setting. Without a trace output path,
the ring can still provide bounded in-memory diagnostics. With diagnostics
disabled, no writer is created, no source-ID entropy read occurs, and no
additional syscall, allocation, channel operation, or file write is added to
the presentation cycle.

## Architecture choices

The design borrows a few focused ideas rather than importing a tracing
framework:

- KWin's render journal uses measured render time with a scheduling safety
  margin. Typhon already has Predictive O1 and render-duration estimates, so
  this work does not add a second frame scheduler. See the
  [KWin render-loop source](https://sources.debian.org/src/kwin-x11/4%3A6.7.4-1/src/core/renderloop.cpp/).
- Perfetto separates producers from persistence, uses bounded buffers, and
  makes buffer loss visible. Typhon uses the same boundary at much smaller
  scale, with per-source event sequences and explicit loss records. See
  [Perfetto buffers and dataflow](https://perfetto.dev/docs/concepts/buffers).
- Linux `sched_wakeup` and `sched_switch` tracepoints can establish actual
  scheduling intervals. Wall-minus-thread-CPU remains only an off-CPU
  estimate; use kernel scheduling evidence for run-queue attribution. See
  [Linux event tracing](https://docs.kernel.org/trace/events.html) and
  [ftrace](https://docs.kernel.org/trace/ftrace.html).
- COSMIC exposes optional Tracy and GPU profiling features. Typhon retains its
  existing named slow-cycle/render diagnostics and does not add a mandatory
  profiler dependency. See the [COSMIC compositor features](https://github.com/pop-os/cosmic-comp/blob/master/Cargo.toml).

No RT policy, thread-priority change, new event loop, generic executor, custom
IPC protocol, rendering backend, or per-frame worker thread was added. KWin,
Mutter, Gamescope, and Hyprland scheduling policies were reviewed but are not
needed to repair an unbounded persistence path. Normal inherited Linux
scheduling remains the production default.

## Synthetic saturation measurements

The ignored benchmark ran against the candidate test binary in Cargo's test
profile. Each mode recorded 12 times the configured capacity. `after wraps`
uses the samples after the first overwrite through the remaining 11 capacities.
The producer measurements use `Instant` wall time and include its measurement
overhead. The full-history column measures JSONL serialization only, using the
current ring contents as a legacy-replacement reference; it does not run the
old binary or include the old synchronous file write. Values below are one
representative run, and the table is a microbenchmark rather than a desktop
latency result.

| Capacity | Disabled push p99 after wraps | Retained push p50/p95/p99 | Full-history JSONL size; median serialization | Streaming push + handoff p50/p95/p99 | Writer result (persisted / attempted; losses; batches; stall periods; total file bytes) |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 64 | 23 ns | 71 / 82 / 86 ns | 16,202 B; 49.6 µs | 283 / 379 / 1,206 ns | 709 / 768; 1 range / 59 events; 6; 1; 177,346 B |
| 4,096 | 23 ns | 73 / 84 / 91 ns | 1,032,396 B; 3.54 ms | 139 / 157 / 176 ns | 15,782 / 49,152; 77 ranges / 33,370 events; 123; 88; 3,988,087 B |
| 65,536 | 23 ns | 72 / 135 / 151 ns | 16,580,813 B; 52.4 ms | 138 / 171 / 222 ns | 246,293 / 786,432; 1,263 ranges / 540,139 events; 1,920; 1,407; 62,528,051 B |

The final column's loss ranges were parsed in sequence order across the active
and rotated files. The benchmark asserts that every attempted event is either
present as an event record or covered by exactly one loss range. `retention_drops`
is a separate ring-overwrite counter: it also counts events that had already
been persisted, so it is not the writer loss count. The producer deliberately
ran much faster than a 165 Hz workload; the writer fell behind at the two larger
capacities, but output stayed within the configured 16 MiB × 4 file bound and
the loss was explicit.

The benchmark also records pre-saturation and first-overwrite latency.
First-overwrite results have one sample and are single observations, not
meaningful percentiles. `batches_written` is a rough handoff/wakeup proxy, not
a kernel thread-wakeup count.

At 65,536 entries, the retained event records occupy 13,631,488 bytes and the
five reusable export batches occupy 134,160 bytes. These are `size_of` buffer
estimates, not process RSS; they exclude ring bookkeeping, thread stacks, and
allocator metadata. The max-size run wrote 62,528,051 bytes across rotated
files. A separate `perf stat` around the whole benchmark process reported
1.458 s task-clock and 15,633 minor faults; these aggregate figures include
serialization, output I/O, and the test harness, so they do not attribute CPU
cost to individual modes. The available perf invocation reported no usable
kernel context-switch count. Heap allocation counts and syscall counts were
not captured (`strace` was unavailable), and the benchmark does not claim
zero-cost tracing. Producer source inspection shows preallocated batch vectors
with no per-event formatting or vector growth; JSON formatting and file writes
remain on the diagnostic thread.

Runs were repeated, and scheduling variation affected stream loss and tail
latency: at capacity 65,536, streaming p99 ranged from 222–267 ns, and the
explicit loss count ranged from 531,393–540,139 events across three ordinary
benchmark runs. Treat the p50/p95/p99 values as host-local indicative data.

The CPU saturation probe now samples per-TID minor/major faults and voluntary /
involuntary context switches from `/proc`, plus system `pswpin`/`pswpout` page
deltas from `/proc/vmstat`. These are sampled diagnostic counters, not per-frame
events; swap counters are system-wide page counts and do not identify which
process caused I/O. Existing slow-cycle wall/CPU measurements still make
wall-minus-thread-CPU only an off-CPU estimate. Use `sched_wakeup` and
`sched_switch` tracing for run-queue attribution where host policy permits it.

## Rendering stall attribution

`TYPHON_SLOW_CYCLE_TRACE=1` keeps the bounded slow-cycle ring. Its
`CLOCK_MONOTONIC` cycle timestamps and `std::time::Instant` monotonic phase
durations are emitted at shutdown; phase durations are elapsed intervals, not
absolute timestamps. `TYPHON_SLOW_CYCLE_THREAD_CPU=1` also samples
`CLOCK_THREAD_CPUTIME_ID` around each cycle. The log header names each phase
array position. Positions 0–13 retain their existing meaning. Positions 14–17
are nested render details and are excluded from the top-level phase sum:

- 14, `egl_context_activation`: the `eglMakeCurrent` call.
- 15, `render_target_preparation`: swapchain slot acquisition, framebuffer and
  buffer-age lookup, and construction of the scene draw request.
- 16, `scene_drawing`: `draw_scene_to_target`, including scene traversal,
  effects, and GL command encoding.
- 17, `native_render_fence_export`: EGL native-fence creation, `glFlush`, fd
  duplication, and EGL sync destruction.

The existing parent phases remain useful: `OutputTransactionPreparation`
includes positions 14–15, while `RendererAndFence` includes positions 16–17.
The detailed phase timers are sampled only when the slow-cycle trace is
enabled; they add no clock reads when disabled. They measure wall intervals,
not per-phase CPU or scheduler wait. `AcquirePrepare`,
`PresentationCursorPlanning`, `PageflipDrain`, and `ReadyFrameKmsHandoff` remain
the existing boundaries for synchronization, cursor planning, completion
drain, and KMS handoff. Rendering and fence semantics are unchanged. The
`render_fence_export` effect trace now emits its end marker even if fence
creation fails.

## Validation and limitations

The regression suite covers incremental overwrite deltas, explicit loss ranges,
sequence continuity beyond `u64`, more than ten ring wraps, producer behavior
with a paused consumer, batch flush on shutdown, writer restart without
duplicates, partial-file recovery, bounded rotation, partial-write rollback,
path failures, and `/dev/full` where available. An ignored manual benchmark
reports p50/p95/p99 producer latency before saturation, on the first overwrite,
and after more than ten wraps at capacities 64, 4,096, and 65,536. It reports
retained record memory, batch and stall counts, audited event loss, and file
size.

The tests do not inject a fixed slow-writer delay or measure producer percentiles
for permission and `/dev/full` failures. The paused-consumer case checks that a
full bounded queue does not block and that resumed output reports evicted
sequences. Restart coverage reconstructs a writer against an existing file in
the same process; it does not simulate a kernel crash or power loss. Shutdown
flush is tested at the writer boundary, not by terminating a live compositor.
Those microbenchmarks include `Instant` timing overhead and do not measure
allocator counts or kernel syscall counts by themselves.

Verification on this worktree:

- `cargo fmt --all -- --check`: passed.
- `cargo check --locked --all-targets`: passed with 18 warnings in other
  compositor/runtime code; no trace-writer warning remained.
- Focused trace tests: 67 passed, 2 ignored.
- CPU saturation probe tests: 22 passed.
- Full `oblivion-one` unit suite, both default parallel and serial: 2,312
  passed, 15 failed, 3 ignored on each run. The same failures repeated serially.
  They were in EGL effects/damage/lifecycle and native readiness/diagnostic
  tests, not trace export. The checkout also has unrelated uncommitted source
  changes, so I did not claim these failures are pre-existing at HEAD.
- `cargo clippy --locked --all-targets -- -D warnings`: failed with 25
  worktree-wide lint errors and one warning in other modules (unused items,
  complexity, and style lints); the captured report has no diagnostic in the
  trace files.

The hardware A/B has not been run. The Cyberpunk process was not running in the
current session, and restarting the active compositor on the configured native
TTY would risk ending the user's desktop session. The exact candidate command,
configuration variables, and three-run protocol are in
[`cpu-saturation-2026-10-08-followup.md`](cpu-saturation-2026-10-08-followup.md).
The source changes must not be called a CPU-starvation fix until those runs
compare baseline and candidate with the preserved workload settings.

The trace exporter does not prove or repair the pre-overflow rendering and
cursor-planning stalls. The original Cyberpunk startup workload has not been
repeated against this candidate, so no user-visible latency or FPS improvement
is claimed. The next qualification must compare repeated baseline and
candidate runs at 1920x1080@165 Hz with the user's existing VRR, Direct
Scanout, buffering, game settings, and driver configuration. Keep diagnostic
and low-overhead responsiveness trials separate.
