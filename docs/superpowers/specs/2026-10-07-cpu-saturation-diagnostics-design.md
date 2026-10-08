# Typhon CPU Saturation Diagnostics Design

## Investigation snapshot

The Codebase Memory project `home-agony-GitHub-Typhon` was indexed at generation
`2026-10-07T23:54:27Z`; the relevant source paths had no recorded coverage gaps.
This is a best-effort graph signal, not a completeness guarantee.

### Source-proven behavior

- `NativeRuntime::bootstrap()` blocks `SIGCHLD` before native initialization;
  `NativeRuntime::run()` enters `run_native_cycle()` and loops over
  `run_cycle()` (`src/native_output/runtime/mod.rs:852-873`,
  `src/native_output/runtime/cycle.rs:187-193`). The KMS commit worker starts
  during `finish_bootstrap()` (`src/native_output/runtime/bootstrap.rs:359-368`);
  several other helpers start during bootstrap, and the KMS worker can restart.
- `NativeEventLoop::wait()` blocks in `epoll_wait` and reports `blocked_ns` as
  the elapsed wait and `timer_lateness_ns` as lateness relative to the armed deadline
  (`src/native/event_loop.rs:675-881`). The cycle trace starts after that wait
  returns (`src/native_output/runtime/cycle/pageflip.rs:600-620`), so neither
  value identifies runnable-queue delay.
- `TYPHON_SLOW_CYCLE_TRACE` enables a 96-record ring. It retains post-wake cycles
  longer than the refresh interval, records phase timings and wake context, and
  dumps on runtime teardown (`src/native_output/runtime/slow_cycle.rs:5,
  148-166,326-367,377-415`; `src/native_output/runtime/mod.rs:945-950`).
- `TYPHON_POINTER_TIMING_TRACE` records pointer-routing transition timing,
  service/checkpoint durations, cycle return and next reactor wake. Its
  `transition_to_cycle_return_ns` is not a physical presentation timestamp
  (`src/native_output/runtime/pointer_timing.rs:559-758`).
- `run_cycle()` services every queued screen-capture request in one loop
  (`src/native_output/runtime/cycle.rs:638-642`). Each request resolves the
  scene, captures through the renderer, creates a sealed memfd, then replies
  (`src/native_output/runtime/cycle.rs:1272-1331`). This bounds neither the
  number of capture operations in a cycle nor the cost of one capture; no trace
  currently demonstrates that it caused the reported slowdown.
- Input draining has a 256-event budget
  (`src/native_output/input/batch.rs:3`); routing transitions have at most six
  checkpoints (`src/native_output/runtime/input_transition_guard.rs:3-14`).
  Work-domain planning and KMS presentation ownership remain outside this
  diagnostics change.
- A source search found no scheduler API or `CPUWeight` configuration. App
  scope setup does not establish a compositor scheduling policy.

### Observed machine state and qualification gap

The active display reports 1920x1080 at 164.999 Hz; the installed GPU is an
NVIDIA RTX 3060 Ti. Typhon was not running, the desktop was running a game, only
about 1.8 GiB of memory was available, about 11 GiB of swap was in use, and
memory PSI was elevated. These observations do not describe Typhon behavior.
No target thread `schedstat`, Typhon phase trace, input timing, frame cadence,
or target cgroup measurements were available. A CPU saturation trial was not
run on this already pressured session.

H1 (reactor runnable-queue delay) and H2 (long or unfair in-thread work) are
both unqualified. H1 needs per-thread scheduler wait evidence during a reproduced
stall. H2 needs contemporaneous native-cycle and phase timing. `blocked_ns` is
not a substitute for either.

## Decision

Add an optional external Python collector and an English qualification runbook.
The collector samples per-thread runtime and scheduler wait from
`/proc/<pid>/task/<tid>/schedstat`, thread nice/policy/affinity, CPU/memory/I/O
pressure, cgroup CPU quota/throttling, CPU frequency and thermal sensors. It
writes outside the checkout and reports p50/p95/p99 sample distributions.
Existing Typhon slow-cycle and pointer-transition traces supply application
phase context; the runbook explains that they do not independently establish
physical presentation cadence or kernel runnable delay.

No compositor hot-path instrumentation, screenshot budget, `nice`, `SCHED_RR`,
cgroup weight, KMS ownership, or event-loop changes are justified without target
measurements. Default startup and effective policy therefore remain unchanged.

## Verification

Use deterministic Python unit tests for `/proc` and PSI parsing and percentile
summaries, syntax validation, and diff review. Do not run a stress workload in
the current session. A later hardware qualification must compare repeated idle,
interactive, animation/video, and bounded CPU-load trials on the same display
and configuration, with the load generator at normal scheduling priority and an
automatic timeout.
