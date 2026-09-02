# Typhon Native Input Pre-Transition Gap Decomposition v1

## Scope and checkout

Starting `HEAD`:

```text
e05d33f32f489c26e989546a6ca7ef7c6b22f48f
test: expose physical claim boundary regressions
```

The implementation commit is:

```text
017f15a native: decompose pointer transition timing
```

The transition-local overhead tightening follow-up is:

```text
053a21f native: keep timing probes transition-local
```

The worktree already contained unrelated presentation/KMS/frame-callback edits and deleted historical reports. They were not staged or changed by this task. The report commit follows these implementation commits, so `053a21f` is the reproducible code ending point recorded above.

This task is observability-only. It does not claim to fix the Sober/Roblox pointer jump.

## Source audit

The current production paths inspected were:

* `src/native_output/runtime/pointer_timing.rs`: bounded transition ring, existing summary lifecycle, and causal pre-read observation.
* `src/native_output/runtime/cycle_dispatch.rs`: native input/Wayland orchestration, pre-read gate, actual Wayland reads, constraint settlement, and transition commit.
* `src/native_output/runtime/cycle.rs`: post-transition latency-guard checkpoints and fresh input microturn ownership.
* `src/native_output/input/routing.rs`: backend-owned readiness, libinput/raw ingress, and pointer-constraint backend settlement.
* `src/native/event_loop.rs`: global reactor wait and `CLOCK_MONOTONIC` helper.
* `src/compositor/server.rs`: Wayland client dispatch and pointer-constraint state notification.
* `src/compositor/server_toplevel.rs`: Wayland client flush behavior and native-input batch flush deferral.
* `src/native_output/input/epoch.rs`: epoch-owned continuation and deferred Wayland progression.

The graph index was current for this checkout. `cycle_dispatch.rs` has the previously recorded parse-partial range at line 865; the affected production ranges were read directly as source fallback.

## Runtime evidence used

The supplied native qualification contained 66 `locked_activated` transitions:

* `pre_read_probe=true`: 65/66.
* `pre_read_input_promoted=true`: 0/66.
* `first_serviceable_checkpoint=0`: 66/66.
* Transition-to-first-input-service median: approximately 4.48 us; maximum approximately 5.61 us.
* First Locked hardware-span median: approximately 26.99 ms.
* 45/66 first Locked activations had at least 24 ms of hardware history.
* `wayland_read_duration_ns`: median approximately 34 us, p95 approximately 68 us, maximum approximately 97 us.
* `pre_read_probe_to_transition_ns`: median approximately 28.28 ms.

This means the post-transition guard is considered healthy for the supplied evidence: it reaches the first service attempt in microseconds. The earlier global-epoll-truncation hypothesis is rejected because production now uses targeted backend readiness. The long-Wayland-read hypothesis is rejected by the measured tens-of-microseconds read duration. The remaining approximately 28 ms owner was not assumed by this task; the new trace decomposes it into wall and compositor-thread CPU intervals.

## Timing architecture

`NativePointerTimingPoint` pairs:

```text
wall_ns = CLOCK_MONOTONIC
thread_cpu_ns = CLOCK_THREAD_CPUTIME_ID, or unknown on clock failure/platforms without it
```

`capture_timing_point()` is allocation-free and failure-tolerant for thread CPU time. The existing monotonic wall clock remains the authoritative wall-time source. CPU-clock failure produces `unknown` CPU fields while retaining wall measurements.

The bounded `NativePointerPreReadObservation` now carries the exact point pairs for the dispatch/settlement chain that can produce a transition:

* targeted pre-read probe start/end;
* Wayland read start/end;
* constraint settlement start/end;
* backend activation start/end;
* Wayland flush start/end;
* the optional pre-transition input batch.

The transition record receives those points only through the same dispatch-local observation that produced the final backend settlement. Initial transitions receive their own settlement observation. No global loose timestamps or temporal-proximity matching are used. Missing or causally unavailable phases remain `unknown`; incomplete observations continue to use the bounded supersession counter.

Production boundaries are instrumented at:

1. the exact targeted readiness call in the Wayland-only branch;
2. the actual Wayland client read, both before-input and after-epoch paths;
3. initial and final native pointer-constraint settlement;
4. the backend pointer-constraint state commit;
5. the flush immediately following that state commit;
6. the authoritative transition-committed point after settlement.

The summary now exposes wall and thread-CPU durations for the probe, probe-to-Wayland gap, Wayland read, read-to-settlement gap, constraint settlement, backend activation, Wayland flush, settlement-to-commit interval, and complete probe-to-commit interval. It also exposes the transition commit CPU timestamp and preserves the existing batch, checkpoint, guard, and hardware-span fields.

The old direct Wayland-read update of the active timing record was removed. Read points are first held locally and then attached to the transition record causally, preventing an unrelated later transition from inheriting them.

## Observer neutrality

When `TYPHON_POINTER_TIMING_TRACE` is disabled, the new capture calls are not executed. No per-motion clock, formatting, allocation, file write, or output was added. When enabled, settlement clocks are sampled only when a real pending settlement candidate exists; ordinary cycles with no candidate do not take the new phase clocks. The observer retains records in the existing fixed-capacity ring of eight entries. Summary output remains at most one compact `eprintln!` per completed transition; no per-motion diagnostics are used. Timing values never influence scheduling or semantic input decisions.

## TDD evidence

The new propagation tests were added and run before the production implementation. The recorded RED was a compile failure showing the missing `NativePointerTimingPoint`, timed transition method, phase fields, and CPU helper. The RED command was:

```text
rtk cargo test --locked --bin oblivion-one pointer_timing
```

After implementation, the focused GREEN results were:

* pointer timing: 16 passed;
* transition guard: 4 passed;
* native input tests: 83 passed;
* native event-loop tests: 34 passed;
* relative/constraint integration tests: 38 passed;
* backend-constraint focused tests: 12 passed.

The transition-local overhead tightening was separately checked with pointer-timing tests and binary clippy; both passed.

The new tests cover wall/CPU propagation, unknown CPU time with retained wall time, missing phase values, causal non-inheritance by an unrelated transition, and monotonic thread CPU readings when available. Existing disabled-trace, fixed-ring, service-time, empty-attempt, supersession, batch, and summary tests remain green.

## Verification

Passed:

```text
rtk cargo check --locked --bin oblivion-one
rtk cargo clippy --locked --bin oblivion-one -- -D warnings
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk git diff --check
```

The required `rtk cargo fmt --check` was run and failed only on unrelated pre-existing formatting differences in dirty presentation/scheduler/KMS files. Four of the five implementation source files pass standalone Rustfmt with edition 2024; the remaining standalone check reports an unrelated already-present import-format difference in `src/compositor/server.rs`.

The required `rtk cargo test --locked` ran all unit/integration binaries successfully (`3231 passed, 5 ignored, 32 filtered`) but exited during doctest compilation because the unrelated current checkout has `FrameCallbackTimingEvidence` unresolved at `src/compositor/server.rs:1669`. The requested single-threaded all-targets rerun was also attempted through:

```text
rtk cargo test --locked --lib --bins --tests -- --test-threads=1
```

and was blocked by unrelated missing `surface_callback_admission` and `surface_callback_commit_timing` symbols in `src/compositor/state/frame_callbacks.rs`. No pointer-timing or native-input failure was observed in the completed test binaries.

## Files changed by this task

* `src/native_output/runtime/pointer_timing.rs`
* `src/native_output/runtime/cycle_dispatch.rs`
* `src/native_output/input/routing.rs`
* `src/native_output/runtime/mod.rs`
* `src/compositor/server.rs`
* this report

## Runtime qualification

The agent did not run native DRM/KMS qualification and did not test Sober/Roblox. The user should run the observer-neutral command manually:

```bash
TYPHON_POINTER_TIMING_TRACE=1 \
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
./bin/start-oblivion-one-tty
```

The next trace should report raw wall and thread-CPU measurements rather than attributing the approximately 28 ms interval to an assumed phase. This report intentionally does not claim application-level closure. If the jump persists, the measured phase with the dominant wall/CPU delta is the next falsifiable owner.
