# CPU contention follow-up: October 8, 2026

## Decision and limits

The supplied Cyberpunk 2077 capture is evidence of substantial CPU contention
around the native reactor, plus a separate high-frequency effect-owner logging
problem. It does not establish that every long cycle or presentation gap was a
scheduler delay, and it does not identify Cyberpunk's per-process CPU use. The
CPU-starvation issue is **not qualified as resolved**: repeated baseline and
candidate Cyberpunk trials have not been run.

The report named `TYPHON_CYBERPUNK_CPU_SATURATION_ANALYSIS_2026-10-08.md` was not
present in the checkout or the searched Aether documentation paths. The figures
below are the capture summary supplied with this task. The report's per-surface
details were not available, so surface IDs 3, 4, 7, and 8 cannot be assigned to
layer-shell, popup, subsurface, or another category here.

## Capture evidence

The recording used a 1920x1080@165 Hz native Wayland session and an NVIDIA RTX
3060 Ti. It reported:

- Reactor CPU use rose from 19.21% idle p50 to 52.37% during the game session.
- Per-thread runnable-wait, accumulated and normalized by observation interval,
  rose from 2.33 to 339.72 ms/s at p95. These are not individual frame delays.
- CPU PSI `some avg10` reached 29.62%.
- The reactor remained `SCHED_OTHER`, nice -4.
- The 96-record slow-cycle ring was full: 74 Slow and 22 Severe records. The
  longest was about 111.01 ms, with about 108.38 ms unattributed to the existing
  wall-time phases. That residual is not automatically scheduler delay.
- The final presentation window covered about 5.55 seconds and had pageflip
  gaps up to about 503 ms. A gap is not a dropped frame if no frame was pending
  or the session was tearing down.
- A libinput warning reported 60 ms keyboard event-processing lag. KMS submit
  time was comparatively short in the sampled presentation window.
- Memory pressure, temperature, and observed session-cgroup throttling did not
  dominate this capture. The capture did not isolate the game process.
- The session log contained 22,613 repeated effect-owner warnings.

These findings support reactor contention as a contributor. They do not make
the cycle residual, aggregate runnable-wait, and pageflip gaps interchangeable
measurements.

## Milestone A: effects ownership and diagnostics

The production path freezes `NativeEffectDamageFrameSnapshot` before it builds
presentation-effect influences in
[`ResolvedNativeFrameScene::from_server_at_with_lifecycle()`](../src/native_output/runtime/frame.rs#L430).
The physical damage snapshot covers the effect instance and its aggregate
footprint. Partial repaint expansion consumes old and new physical effect
snapshots independently in the physical damage path. It already provides the
damage evidence needed when a surface has no window-animation owner.

Presentation effect influences have a narrower role: they carry a window-group
scene node so presentation transforms and animation can affect the region. The
production resolver first resolves popup and subsurface anchors to their owner
root, then looks up that root in window-group presentation ownership. A
layer-shell root or another non-window visual can legitimately have no such
owner. Its canonical surface identity and physical damage remain valid; it must
not be assigned a fabricated window animation node. Lifecycle exit anchors use
the captured exit scene node explicitly. See
[`freeze_presentation_effect_influences()`](../src/native_output/runtime/frame.rs#L75),
[`presentation_owner_root_for_surface()`](../src/compositor/state/hit_testing.rs#L158),
and [`presentation_scene_node_id_for_root()`](../src/compositor/state/active_scene.rs).

The repeated warning was emitted synchronously in that per-frame path. Commit
`2aff812c` keeps window-group ownership unchanged, leaves no-owner effects out
of the presentation influence map, and removes recurring default stderr
output. Tests use the same group-only
presentation owner lookup in test and production builds; a missing group does
not receive a synthetic scene node.
`TYPHON_EFFECT_OWNER_DIAGNOSTICS=1` enables two atomic occurrence counters; the
runtime reports at most one aggregate line per category during teardown.
Missing programs continue to use output bounds for conservative presentation
damage. The flag is read during native runtime bootstrap; the frame path performs
no formatting or synchronous output. No physical effect damage is suppressed.
Commit `70e903d5` moves the opt-in report from the frame loop to
`NativeRuntime::drop()` and records bounded occurrence totals instead of writing
on the first affected frame.

Focused tests cover a toplevel owner, missing non-window owner with preserved
physical damage, shared owners, missing registry program fallback, and disabled
or bounded diagnostics. The broader physical-effect suite also passed, including
replacement/removal footprints and registry-generation transitions.

## Milestone B: cycle CPU and off-CPU attribution

The slow-cycle trace now supports:

```text
TYPHON_SLOW_CYCLE_TRACE=1
TYPHON_SLOW_CYCLE_TRACE_CAPACITY=4096
TYPHON_SLOW_CYCLE_THREAD_CPU=1
```

The default ring remains 96. The optional capacity is clamped to 1–65,536 and
evictions are counted in the teardown header. CPU-clock sampling is disabled
unless both the slow-cycle trace and `TYPHON_SLOW_CYCLE_THREAD_CPU` are enabled.
With diagnostics disabled there are no additional thread-CPU clock calls. The
trace uses `CLOCK_MONOTONIC` for cycle wall timestamps and the existing Linux
`CLOCK_THREAD_CPUTIME_ID` helper for reactor-thread CPU time. It reports
`thread_cpu_ns` and `off_cpu_estimate_ns = max(total_wall_ns - thread_cpu_ns, 0)`.

The off-CPU value includes sleeping, kernel blocking, descheduling, and
measurement error. It is **not** a run-queue wait measurement. The external
probe's per-TID `schedstat` runnable-wait counter is aggregate kernel scheduling
evidence over its sampling interval; use `perf sched` or sched-switch/wakeup
tracing when a per-TID scheduling timeline is needed. Do not infer an individual
cycle's run-queue delay from the probe's 0.5-second deltas or from wall-minus-CPU.

The diagnostic clock calls add measurement overhead when explicitly enabled.
Use the same trace settings for diagnostic comparisons, and use a separate
low-overhead run for responsiveness and FPS comparisons. The bounded presentation
trace already supports a configurable ring up to 65,536 events and does not need
a second buffering system.

## Milestone C: RT scheduling remains disabled

No scheduler adapter or RT scheduling code was added. The normal/inherited
policy remains the production default. The capture reported
`RLIMIT_RTTIME=unlimited` and global RT runtime equal to its scheduling period,
so it did not provide a fair-class recovery reserve for a runaway RT reactor.
Changing the hard `RLIMIT_RTTIME` limit from an unprivileged process is
irreversible without `CAP_SYS_RESOURCE`, and the resource limit is inherited by
child processes. A finite RT-time budget sends `SIGXCPU` at the soft limit and
can send `SIGKILL` at the hard limit; it is not a safe recovery plan without a
signal-ownership and child-limit design.

Thread and child isolation is also unresolved. New pthreads default to
`PTHREAD_INHERIT_SCHED`, while `SCHED_RESET_ON_FORK` resets fork children but
does not prevent new threads from inheriting the creator's scheduler attributes.
Typhon creates and recovers KMS workers, starts other helpers after bootstrap,
and launches XWayland and supervised commands. Renderer and driver calls can
block the reactor; a higher-priority reactor can also delay services needed to
release a dependency or lock. A lower-priority monitor cannot recover a reactor
that runs without yielding.

Current upstream code illustrates the costs rather than proving a benefit for
Typhon: [KWin](https://github.com/KDE/kwin/blob/master/src/utils/realtime.cpp#L276)
attempts minimum-priority `SCHED_RR | SCHED_RESET_ON_FORK` on its caller;
[Hyprland](https://github.com/hyprwm/Hyprland/blob/main/src/init/initHelpers.cpp#L644)
tries direct scheduling and RTKit, with an RTKit-specific `SIGXCPU` demotion and
finite `RLIMIT_RTTIME` path; [Mutter](https://github.com/GNOME/mutter/blob/main/src/backends/native/meta-thread.c#L2601)
uses per-thread roles, RTKit priority requests, and real-time inhibition;
[Gamescope](https://github.com/ValveSoftware/gamescope/blob/master/src/Utils/Process.cpp#L2811)
keeps `--rt` optional and saves a scheduler state for restoration. These do not
establish that an RT reactor is safe or faster on this host.

Relevant Linux constraints are documented in
[`pthread_attr_setinheritsched(3)`](https://man7.org/linux/man-pages/man3/pthread_attr_setinheritsched.3.html),
[`sched(7)`](https://man7.org/linux/man-pages/man7/sched.7.html),
[`getrlimit(2)`](https://man7.org/linux/man-pages/man2/getrlimit.2.html),
[RT group scheduling](https://docs.kernel.org/scheduler/sched-rt-group.html),
and [cgroup v2 CPU controls](https://docs.kernel.org/admin-guide/cgroup-v2.html).
In particular, `cpu.weight` and `cpu.max` apply to fair-class tasks; they do not
provide RT recovery. Do not add direct RR, RTKit, a custom `SIGXCPU` handler,
SCHED_FIFO, or SCHED_DEADLINE until signal ownership, hard-limit handling,
thread/child inheritance, driver blocking, and a reliable recovery reserve have
an explicit design.

## Independent adversarial review

- Non-window effects still receive no fabricated window-animation owner.
  Physical blur, replacement, retained-frame, and partial-repaint damage remains
  represented by the existing physical effect snapshot and expansion path.
- Stable missing-owner scenes do not write to stderr by default. Opt-in
  diagnostics aggregate in atomic counters and print during teardown; no
  unbounded queue, per-frame string formatting, synchronous frame-path output,
  or new hot-path mutex was introduced.
- No RT policy, capability, ambient capability, resource limit, signal handler,
  or cgroup weight is changed. Consequently there is no new RT inheritance path
  into KMS workers, XWayland, shell commands, or applications.
- Renderer/driver blocking, lock-owner priority inversion, RT signal ownership,
  hard-limit recovery, and global fair-class recovery remain unresolved. Those
  are the reasons an RT experiment was not implemented.
- The CPU trace is opt-in. Its thread-clock calls and larger ring can affect a
  diagnostic run, so diagnostic and low-overhead responsiveness trials must be
  separate. No hardware improvement or regression has been measured.
- Input/event ownership, frame callbacks, explicit-sync obligations, KMS
  transaction ownership, pageflip ownership, and suspend/recovery code were not
  changed by these fixes. The full suite has unrelated/unclassified failures,
  so those areas are not claimed as newly requalified.

## Verification snapshot

Compilation and tests used
`CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target`.

- Before the teardown-only diagnostic adjustment, slow-cycle tests (16),
  presentation-effect tests (4), and physical effect-damage tests (23) passed.
  The updated binary-target tests could not be rerun because the unrelated
  cursor visibility error below prevents that target from compiling.
- After removing the test-only synthetic owner fallback, the missing-owner
  regression test passed; direct-scanout tests (17), tiled-layout tests (19),
  window-decoration tests (62), and fullscreen occlusion tests (20) passed.
- Fresh `cargo check --locked --lib`: succeeded with 2 existing warnings.
- A prior all-target check succeeded with 16 warnings, before the current
  uncommitted `presentation_cursor.rs` change was present in the live worktree.
- A prior `cargo test --locked --all-targets` run had 3,813 passed, 27 failed,
  and 2 ignored.
  The failures span unrelated persistence, compositor window/XWayland,
  selection, and render-graph cases. The live checkout contained unrelated
  edits, so the failures are unclassified without a clean baseline run.
- A prior `cargo clippy --locked --all-targets -- -D warnings` failed on 26
  warnings across existing modules, including dead code, argument counts, and
  identity maps. It was not rerun on the final tree because the binary target
  currently stops at the cursor visibility error above.
- Workspace formatting checks are limited by uncommitted source edits:
  `src/native_output/runtime/mod.rs` reports module-order formatting around the
  new cursor-responsiveness module, and the untracked
  `src/compositor/tests/data_device_workspace.rs` also differs from rustfmt.
  Direct rustfmt checks passed for `active_scene.rs`, `frame.rs`, and
  `slow_cycle.rs`; `mod.rs` still has module-order formatting differences from
  the uncommitted cursor module additions, which were preserved. The launcher
  passed `bash -n bin/start-oblivion-one` and a dry run using the Aether binary
  path.
- A later focused binary-target test attempt could not compile the current live
  checkout because an unrelated, uncommitted change in
  `src/native_output/runtime/presentation_cursor.rs` refers to crate-private
  `ProtocolCursorShape` through the public compositor module. The fresh
  `cargo check --locked --all-targets` also stops there. That file was left
  untouched, so binary-target suites were not rerun after the ownership test
  alignment.

## Milestone D: hardware qualification still required

At review time the graphical session was running Astrea Shell and XWayland, not
Typhon, and no Cyberpunk process was running. The game installation was present.
The native launcher dry run selected the Aether release binary correctly but
warned that the user is not in the `input` group. The configured native TTY,
device access, and configured Astrea Shell launch were not changed. A valid
Cyberpunk trial was therefore not run from the current session.

The launcher accepts optional `OBLIVION_ONE_BINARY` so an Aether build can be
selected without placing `target/` under the source checkout. Build and run from
the native test TTY using the same configured Astrea Shell command and Steam
launch action. Do not add an explicit shell command after the compositor's
`--` argument.

The effects-fix source baseline is commit `4eca3c90`, the parent of the ownership
repair `2aff812c`. The diagnostic teardown follow-up is `70e903d5`; the
measurement commit is `026b6e8d`. The supplied capture report and its build
revision were not found in the checkout, so the October capture cannot be
treated as a reproducible source-matched baseline. For a quantitative A/B,
build and record a fresh baseline from `4eca3c90` and a candidate containing
both effects commits plus the measurement commit, then use the same launcher
settings and workload below. Keep the original capture as initial context only.

First verify the output directory, build the candidate, and retain each trial in
its own Aether directory:

```bash
export CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target
cargo metadata --locked --no-deps --format-version 1 | jq -r .target_directory
cargo build --locked --release
test -x "$CARGO_TARGET_DIR/release/oblivion-one"

TRIAL_DIR=/mnt/Aether/Desktop/GitHub/Typhon-cpu-saturation/candidate-r1
mkdir -p "$TRIAL_DIR"
export OBLIVION_ONE_VRR=on
export OBLIVION_ONE_DMABUF_KMS_PREFERRED=auto
export ASTREA_COMPOSITOR_BACKEND=typhon
export TYPHON_XWAYLAND=eager
export OBLIVION_ONE_MODE=1920x1080@165
export OBLIVION_ONE_KMS_MODE=atomic
export OBLIVION_ONE_SCANOUT_BACKEND=native-egl-gbm
export OBLIVION_ONE_CURSOR=auto
export OBLIVION_ONE_KMS_COMMIT_WORKER=auto
export OBLIVION_ONE_TRIPLE_BUFFERING=auto
export OBLIVION_ONE_DIRECT_SCANOUT=experimental-auto
export OBLIVION_ONE_PROFILE=release
export OBLIVION_ONE_BINARY="$CARGO_TARGET_DIR/release/oblivion-one"
export OBLIVION_ONE_LOG_FILE="$TRIAL_DIR/session.log"

# Run on the configured native TTY, not inside the currently active Wayland session.
bin/start-oblivion-one-tty
```

For the low-overhead comparison, leave the Typhon trace switches unset and run
the same external probe for each baseline and candidate trial from a separate
terminal while Typhon is alive:

```bash
TRIAL_DIR=/mnt/Aether/Desktop/GitHub/Typhon-cpu-saturation/candidate-r1
TYPHON_PID=$(pgrep -xo oblivion-one)
ps -T -p "$TYPHON_PID" -o pid,tid,comm,cls,rtprio,ni
python3 scripts/typhon_cpu_saturation_probe.py \
  --pid "$TYPHON_PID" --duration 300 --interval 0.5 \
  --output "$TRIAL_DIR/os.jsonl"
```

Start Cyberpunk with the same Steam launch action, graphics settings, startup
sequence, and timed interaction for every run. Keep CPU/memory PSI, swap I/O,
thermal conditions, cgroup throttling, GPU utilization, frame pacing, and FPS
collection identical. Run at least three baseline and three candidate trials;
report each trial's sample count and p50/p95/p99 plus run-to-run spread. The
October capture may serve as initial evidence but its short presentation window
and missing game PID attribution do not replace repeated trials.

For a separate attribution run, add these variables at startup and use the same
values for every compared diagnostic trial:

```bash
export TYPHON_SLOW_CYCLE_TRACE=1
export TYPHON_SLOW_CYCLE_TRACE_CAPACITY=4096
export TYPHON_SLOW_CYCLE_THREAD_CPU=1
export TYPHON_POINTER_TIMING_TRACE=1
export OBLIVION_ONE_PRESENTATION_TRACE=1
export OBLIVION_ONE_PRESENTATION_TRACE_FILE="$TRIAL_DIR/presentation.jsonl"
```

Run a separate probe/diagnostic session from the low-overhead performance
comparison. Correlate per-TID kernel scheduling evidence with cycle wall/CPU
records, input timing, pageflip events, KMS-worker state, and game/GPU telemetry.
The changes should not be called a CPU-starvation fix until those repeated
hardware comparisons show a user-visible improvement without stability or frame
pacing regressions.
