# Typhon CPU contention responsiveness: V2 review

**Review date:** 2026-10-07

**Decision:** Outcome A — no justified runtime change.

## Decision

Typhon was running in the native session, but the requested gaming condition was
not run. The user explicitly said not to run Cyberpunk 2077. I collected one
five-minute idle baseline and reviewed the live source and upstream scheduling
mechanisms. The reported gaming stall was not reproduced, so there is no basis
to select H1–H4 as its cause or to change runtime behavior. I did not substitute
a synthetic CPU load for the missing game workload.

No runtime, scheduling, input, KMS, explicit-sync, or Predictive O1 code changed.
The idle capture is a baseline, not a qualification result or an input/frame
latency measurement.

## Observed baseline

The native Typhon reactor was TID 1041. Its six observed threads used
`SCHED_OTHER`, static priority 0, nice -4, and CPUs 0–11. The reactor's current
resource limits showed `RLIMIT_NICE=31`, `RLIMIT_RTPRIO=99`, and
`RLIMIT_RTTIME=unlimited`; effective and ambient capabilities were empty. These
limits make some priority changes technically possible on this host, but do
not make a real-time policy safe.

The external probe ran for 300 seconds at 0.5-second sampling and retained 598
valid samples for the reactor. For those coarse samples, reactor CPU use was
p50/p95/p99 **19.2/42.3/45.8% of one CPU**. Kernel per-thread runnable-wait
counter deltas, normalized by sample wall time, were p50/p95/p99
**0.31/2.33/7.16 ms per wall-clock second**. The maximum sample was 71.6 ms/s.
These are sampled CPU and run-queue counters, not per-frame or per-input
latencies. `/proc/sys/kernel/sched_schedstats` was `0`, but the task-level
`/proc/<pid>/task/<tid>/schedstat` values were nonzero and advanced during the
trial; the kernel exposes that file's run-queue delay counter separately from
the system-wide scheduler statistics switch ([kernel proc implementation](https://github.com/torvalds/linux/blob/master/fs/proc/base.c),
[scheduler statistics documentation](https://docs.kernel.org/scheduler/sched-stats.html)).

During the capture, available memory remained at least 8.94 GiB, swap use was
0.47–0.79 MiB, and memory PSI was zero. The session cgroup had `cpu.max=max`
and no increase in `nr_throttled`. System CPU PSI `some avg10` peaked at 1.46%;
I/O PSI `some avg10` had isolated peaks up to 7.99%. The host's
`sched_rt_period_us` and `sched_rt_runtime_us` were both 1,000,000 us, so the
global RT bandwidth configuration did not reserve the usual 5% for fair-class
recovery. CPU package temperature ranged from 33–55°C, and the probe's sampled
mean CPU frequency ranged from 800–4,031 MHz. No competing-task attribution or
time-aligned GPU telemetry was collected. These observations do not correlate
with an input or presentation stall because none was reproduced.

No input-transition trace, slow-cycle trace, or presentation trace was enabled
at startup. Consequently this run has no input service delay, cycle phase
durations, deadline misses, pageflip cadence, renderer blocking, or game FPS.
The per-thread probe samples every 0.5 seconds and cannot be treated as a
per-frame timeline.

## Typhon source evidence

Code discovery used CodebaseMCP against the live `main` checkout. Its index was
current at `45389c17`; coverage was complete for the reviewed paths except
parse-partial graph ranges in `cycle_dispatch.rs:2017` and
`presentation_cycle.rs:298`, which were read directly from source. Coverage is a
best-effort signal, not proof of completeness.

- [`NativeRuntime::run()`](../src/native_output/runtime/mod.rs#L861) enters
  [`run_native_cycle()`](../src/native_output/runtime/cycle.rs#L187), which
  repeatedly runs [`run_cycle()`](../src/native_output/runtime/cycle.rs#L195)
  on the reactor thread.
- [`NativeEventLoop::wait()`](../src/native/event_loop.rs#L675) waits in
  `epoll_wait(..., -1)`. Its `blocked_ns` measures time asleep in epoll; timer
  lateness is a separate deadline observation. Neither is runnable-queue delay.
  The probe's per-thread `schedstat` wait delta is the available kernel measure
  for runnable delay.
- `run_cycle()` classifies the wake, reaps supervised children when needed,
  dispatches XWayland and Wayland/input work, checks input-transition routing
  checkpoints, services timer and deferred completions, prepares scene and
  explicit-sync work, admits presentation, renders/submits, and updates cycle
  traces. Input dispatch tracks batch-budget exhaustion and continues backlog
  in later cycles; the transition guard has a bounded number of checkpoints.
- One optional path drains all pending Astrea screen captures inside a
  `while` loop. Each request resolves scene state, captures through the renderer,
  creates a sealed memfd, and replies. This is a candidate for measurement, not
  evidence of a bottleneck. It was not changed.
- Slow-cycle traces use a bounded ring of 96 slow cycles; pointer-transition
  traces retain eight records; the presentation trace is separately opt-in and
  bounded. These traces were not active for the live process. Pageflip
  observations are not optical scanout measurements.
- Worker creation is separated for KMS commit, DMABuf foreground work, cursor
  I/O, keyboard persistence, output-configuration persistence, and trace
  writing. Both native and compositor pacing trace sinks create threads. KMS
  worker recovery can create a replacement thread. Application-scope
  registration also starts a helper thread. These use ordinary Rust thread
  creation; none sets an explicit scheduler policy. Supervised
  applications and shell commands use `Command::spawn`; no scheduler or
  capability promotion was found in those launch paths.
- A literal search of Rust source found no `sched_setscheduler`,
  `pthread_setschedparam`, `setpriority`, `SIGXCPU`, `RLIMIT_RTTIME`, or
  `SCHED_RESET_ON_FORK` implementation. Current worker creation uses normal
  scheduling, as confirmed by the live thread query.
- Predictive O1, KMS commit admission/recovery, output-transaction identity,
  pageflip completion, and explicit-sync obligations are coupled across the
  pacing, presentation-transaction, and KMS-worker paths. Nothing in this
  review changes those paths or moves EGL/DRM work between threads.

## Upstream mechanisms

The source and documentation below confirm implementation choices, not a
cross-compositor hardware performance comparison. No directly comparable
latency/FPS results were found in the inspected primary sources.

| Project | Confirmed behavior and default | Privileges, failure path, and limitations | Hardware evidence in reviewed sources |
|---|---|---|---|
| **KWin** | Wayland startup calls `gainRealTime()`. When built with `SCHED_RESET_ON_FORK`, it attempts minimum-priority `SCHED_RR | SCHED_RESET_ON_FORK` on the calling thread. Failure logs a warning and startup continues. Its render loop estimates compositing time from the render journal, refresh interval, and a safety margin; it includes a 1 ms timer/scheduler margin and caps its expected compositing duration at two refresh intervals. This is presentation scheduling, not CPU-pressure detection. ([startup](https://github.com/KDE/kwin/blob/master/src/main_wayland.cpp#L2078), [policy](https://github.com/KDE/kwin/blob/master/src/utils/realtime.cpp#L276), [render loop](https://github.com/KDE/kwin/blob/master/src/core/renderloop.cpp#L979)) | Direct kernel permission is required: `CAP_SYS_NICE` or a sufficient `RLIMIT_RTPRIO`. Runtime support is compile-time gated by `HAVE_SCHED_RESET_ON_FORK`. The helper changes its caller only; this code does not establish that later threads are isolated from scheduling inheritance. | The code establishes the policy attempt and frame-timing algorithm only; it supplies no comparable hardware trial. |
| **Hyprland** | Startup calls `gainRealTime()` on the main thread. `HAS_RTKIT` is compiled on Linux unless `NO_RTKIT` is selected; that default build path requires `sdbus-c++>=2.0.0`. It first uses a raw per-TID syscall for minimum `SCHED_RR | SCHED_RESET_ON_FORK`, then tries RealtimeKit if direct scheduling fails. With `HAS_RTKIT`, it installs a `SIGXCPU` handler and applies a finite `RLIMIT_RTTIME`; the D-Bus call has a two-second timeout. Without `HAS_RTKIT`, it uses `pthread_setschedparam(SCHED_RR)` without the kernel reset flag and registers an at-fork child demotion. Failure leaves the compositor running normally. ([build paths](https://github.com/hyprwm/Hyprland/blob/main/CMakeLists.txt#L2290), [direct path](https://github.com/hyprwm/Hyprland/blob/main/src/init/initHelpers.cpp#L644), [RTKit and signal path](https://github.com/hyprwm/Hyprland/blob/main/src/init/initHelpers.cpp#L684), [startup attempt](https://github.com/hyprwm/Hyprland/blob/main/src/init/initHelpers.cpp#L865)) | Direct promotion requires `CAP_SYS_NICE` or `RLIMIT_RTPRIO`; RTKit mediates unprivileged requests for an active local session. In the RTKit path, lowering a hard real-time limit is not reversible without `CAP_SYS_RESOURCE`, and the limit is inherited by child processes. The reset flag covers fork children, not subsequently created threads. Signal disposition is process-wide even though the handler targets a cached reactor TID, so the signal path needs a careful thread-target audit before reuse. | The implementation demonstrates a safety strategy and its inheritance/limit costs; it provides no comparable hardware trial. |
| **Mutter** | `MetaThread` distinguishes user threads, kept normal, from kernel-role threads with a preferred priority. It requests either RTKit high priority (nice level) or real-time scheduling for a specific TID. A realtime-inhibit count suppresses RT and scheduling is synchronized back to the role's selected priority. Mutter's current NEWS says the KMS thread defaults to high priority rather than RT. ([thread scheduling](https://github.com/GNOME/mutter/blob/main/src/backends/native/meta-thread.c#L2601), [role and inhibit selection](https://github.com/GNOME/mutter/blob/main/src/backends/native/meta-thread.c#L2834), [KMS default change](https://github.com/GNOME/mutter/blob/main/NEWS)) | RealtimeKit handles both priority paths. Its RT path sets both soft and hard `RLIMIT_RTTIME` to RTKit's maximum before making the request; a failed request can therefore still leave a changed process-wide limit. Failure is logged and normal scheduling remains available. | Thread-role policy is confirmed; no comparable hardware result is supplied by the reviewed sources. |
| **Gamescope** | `--rt` is optional and off by default. At startup, if `CAP_SYS_NICE` is present, Gamescope sets nice -20 regardless of `--rt`; with `--rt`, it also attempts minimum-priority `SCHED_RR`. Without the capability it logs that it will use regular-priority compute and threads. It saves and has a restore path for the previous scheduler state. ([startup choice](https://github.com/ValveSoftware/gamescope/blob/master/src/main.cpp), [RT policy](https://github.com/ValveSoftware/gamescope/blob/master/src/Utils/Process.cpp#L2811)) | The policy path checks for `CAP_SYS_NICE`. The shown RR request does not set `SCHED_RESET_ON_FORK`, and a caller-thread promotion is not a general per-worker policy framework. Gamescope's gaming-session choices are not evidence that the same policy benefits a general-purpose desktop compositor. | The reviewed implementation contains no comparable result isolating `--rt` or nice from other gaming-session changes. |
| **Smithay / calloop** | `calloop::EventLoop::dispatch()` waits up to its supplied timeout, synchronously processes ready sources, then fires idle callbacks. `run()` repeats dispatch and a callback. The API provides event and timer organization, not CPU-time budgets or scheduler starvation protection. ([dispatch and run](https://github.com/Smithay/calloop/blob/master/src/loop_logic.rs#L3342)) | No special privilege is required or requested by calloop itself. A long callback still occupies the loop thread until it returns. | The library source makes no hardware performance claim. |
| **COSMIC** | COSMIC's backend initializes around a Smithay-reexported `calloop::EventLoop` passed into backend setup. This is event-loop structure, not automatic CPU protection. ([backend setup](https://github.com/pop-os/cosmic-comp/blob/master/src/backend/mod.rs)) | The reviewed path has no scheduler policy request or per-callback execution budget. | No comparable CPU-contention trial was identified in the reviewed source. |
| **Sway** | Sway's server runs through Wayland's `wl_display_run()` / event-loop dispatch organization. Dispatch callbacks remain synchronous work on that server loop; the event-loop API does not make callback work bounded. ([Sway main](https://github.com/swaywm/sway/blob/master/sway/main.c)) | No special scheduling permission is required by this event-loop structure; this review did not find a reactor scheduling promotion in the inspected entry path. | No comparable scheduling-policy result was supplied by the reviewed source. |

## Linux constraints relevant to Typhon

- `SCHED_OTHER` uses fair time-sharing. A more favorable nice value changes
  weight within the fair class; it does not isolate the reactor or guarantee a
  deadline. `SCHED_RR` is a fixed-priority real-time policy and preempts runnable
  fair-class work at lower priority. A reactor can still block in EGL, DRM,
  synchronization, or a driver dependency; a higher policy does not guarantee
  the lock owner or service thread will run promptly.
- `SCHED_RESET_ON_FORK` changes the policy of fork children and resets negative
  nice to zero. It does not prevent new threads from inheriting scheduler
  attributes. Typhon's worker and recovery paths currently use ordinary thread
  creation, so elevating only the reactor requires explicit inheritance handling
  at every such boundary.
- `RLIMIT_RTPRIO` can permit unprivileged RT promotion. `RLIMIT_RTTIME` limits
  continuous RT CPU use between blocking system calls: reaching the soft limit
  raises `SIGXCPU`; reaching the hard limit can kill the task. A process-wide,
  inherited limit has real consequences for supervised applications. This host's
  RT-time hard limit is unlimited, so there is no configured runaway bound. The
  host's global RT runtime equals its one-second period, leaving no fair-class
  reserve if RT work consumes that budget.
  ([Linux scheduling limits and reset-on-fork](https://www.man7.org/linux/man-pages/man7/sched.7.html),
  [RT-time limit](https://www.man7.org/linux/man-pages/man2/getrlimit.2.html))
- Kernel RT bandwidth throttling is separate from fair-class cgroup controls.
  The cgroup v2 `cpu.max` and `cpu.weight` controls affect fair-class tasks;
  do not infer RT protection from those values. PSI is aggregate stall evidence,
  not a per-thread scheduler-delay measure. ([RT bandwidth](https://docs.kernel.org/scheduler/sched-rt-group.html),
  [cgroup v2 CPU controls](https://docs.kernel.org/admin-guide/cgroup-v2.html),
  [PSI](https://docs.kernel.org/accounting/psi.html))

## Strategy decision and adversarial review

1. **Keep normal scheduling and fix measured work first.** There is no reproduced
   workload and no phase trace, so neither an in-thread bottleneck nor a
   run-queue problem is established.
2. **Do not change reactor nice in production.** The host's `RLIMIT_NICE` means a
   small adjustment may be permitted, but no H1 evidence supports it. A nice
   change would also need a deliberate policy for threads created later by the
   reactor.
3. **Do not add RealtimeKit or minimum-priority RR.** The live process has
   `RLIMIT_RTTIME=unlimited`; the existing thread-creation paths would require
   inheritance control, and there is no demonstrated safety/recovery path for a
   runaway reactor. `SCHED_RESET_ON_FORK` alone does not solve thread
   inheritance. No custom signal handler is justified.
4. **Leave Predictive O1 and presentation margins unchanged.** There is no
   measured scheduler-jitter correlation with deadline misses or measured input
   penalty from rendering earlier.

The independent review found no runtime changes to audit for lost input,
callbacks, frame obligations, KMS transaction ownership, commit generation,
pageflip token, fence handling, VT recovery, or shutdown behavior. The screenshot
drain remains an unproven candidate, not a fix target. The probe is external to
the frame path, sampled at 0.5 seconds, and wrote its 4.0 MB JSONL capture under
`/mnt/Aether/Desktop/GitHub`; its sampling is too coarse to qualify individual
stalls.

## Qualification and next evidence

The 598-sample idle capture provides no comparable trial under a capped game,
the reported game settings, or repeated interaction during that workload. There
are no measured input or presentation p50/p95/p99 values, frame misses, dropped
frames, cadence, game FPS, renderer blocking, or repeat-trial variation. No
performance improvement is claimed or accepted.

To reopen implementation, capture the native reactor's policy and per-thread
run-queue deltas during the actual stall, with Typhon's slow-cycle,
pointer-transition, and presentation traces enabled from startup. Align those
with interaction timestamps, pageflip cadence, game FPS/GPU telemetry, competing
tasks, CPU/memory/I/O PSI, swap, thermals, and cgroup throttling. If sampled
run-queue counters cannot localize a stall, collect `perf sched` evidence with
permissions confirmed. Repeat comparable trials before considering a scheduling
experiment. Keep `SCHED_OTHER` as Typhon's production default unless those
measurements and an inheritance/recovery audit support a narrowly scoped change.

No Rust tests, formatter, Clippy, or build were run because V2 made documentation
changes only and deliberately left the live runtime untouched.
