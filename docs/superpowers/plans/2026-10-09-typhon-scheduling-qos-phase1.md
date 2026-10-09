# Typhon Scheduling QoS Phase 1 Implementation Plan

> **For agentic workers:** Execute inline in this session. Use the checklist in order; each task ends with focused verification before committing.

**Goal:** Add safe, typed, opt-in scheduling QoS for the native runtime and KMS worker without changing render ownership or KMS queue semantics.

**Architecture:** A native scheduling module owns requested/effective policy snapshots, Linux scheduler operations, a bounded RTKit broker, RT budget safety, and guarded normal-thread creation. Native bootstrap starts the broker and all workers before promoting the current runtime thread. Existing continuation, doctor, and child-supervision boundaries carry bounded work, observability, and child-state restoration.

**Tech Stack:** Rust, libc, existing zbus and futures-lite dependencies, existing native eventfd continuation and control doctor APIs.

## Global Constraints

- `OBLIVION_ONE_SCHEDULING_QOS` defaults to `off`; production default does not change in Phase 1.
- Native realtime requests only `SCHED_RR` priority 1 with `SCHED_RESET_ON_FORK`.
- No RTKit realtime call, `SCHED_FIFO`, `SCHED_DEADLINE`, process-wide negative nice, or new D-Bus dependency.
- The original `RLIMIT_RTTIME` hard limit is retained; launched children restore the original RT-time soft limit and nice value.
- KMS queue capacity, wake timing, TEST_ONLY, EBUSY, generation, and settlement ownership stay unchanged.
- All Cargo output uses `/mnt/Aether/Desktop/GitHub/Typhon-target` after printing and checking that effective path.
- Preserve all pre-existing working-tree edits and commit only files/hunks created for this phase.

---

### Task 1: Lock down policy selection with deterministic fake-kernel tests

**Files:**
- Create: `src/native/scheduling_qos.rs`
- Modify: `src/native/mod.rs`

- [x] Define typed policy, role, requested/effective scheduling class, backend, and diagnostic snapshot types.
- [x] Add fake `SchedulingOperations` tests for `off`, direct RR success, RR permission failure, RTKit and direct FAIR fallbacks, total failure, `require`, actual-state verification, KMS/background no-RT, reset-on-fork, demotion evidence, and helper-creation refusal.
- [ ] Deliberate unimplemented-stub failure check was not run; implementation proceeded against the completed typed behavior tests.
- [x] Implement pure policy selection against the operations interface and run the focused QoS test set.

### Task 2: Implement Linux state queries, RT budget guard, and RTKit broker

**Files:**
- Modify: `src/native/scheduling_qos.rs`

- [x] Query TIDs with `gettid`, scheduler class/priority with `sched_getscheduler` and `sched_getparam`, nice with `getpriority`, and reset-on-fork from the returned policy flags.
- [x] Install the `SIGXCPU` handler and finite soft `RLIMIT_RTTIME` before direct RR. The handler uses only lock-free atomics, a scheduler syscall against the captured reactor TID, and `_exit` if demotion fails.
- [x] Restore the original soft RT-time limit and prior signal disposition after failed acquisition, overrun observation, shutdown demotion, and child pre-exec setup.
- [x] Start a bounded ordinary-priority broker before runtime promotion. Use zbus for `MakeThreadHighPriority`, query `MinNiceLevel`, clamp the request, and verify the actual kernel result afterward.
- [x] Add no-root tests for `RLIMIT_RTTIME` decisions, scheduler flag verification, diagnostic serialization, and policy failure reasons.

### Task 3: Make thread and child scheduling inheritance explicit

**Files:**
- Modify: `src/application_scope.rs`
- Modify: `src/compositor/pacing.rs`
- Modify: `src/cursor_manager.rs`
- Modify: `src/keyboard_persistence.rs`
- Modify: `src/native/dmem_foreground.rs`
- Modify: `src/native_output/kms_worker/thread.rs`
- Modify: `src/native_output/pacing.rs`
- Modify: `src/native_output/presentation/trace_writer.rs`
- Modify: `src/native_output/runtime/output_configuration.rs`
- Modify: `src/process.rs`

- [x] Route audited production `thread::spawn` and `thread::Builder::spawn` calls through the QoS helper; normalize RT creators before pthread creation and reset each helper's inherited nice value before role work.
- [x] Mark the KMS worker role and request high-priority FAIR before it starts commit work.
- [x] Capture startup child nice and RT-time limits, then restore both in the supervisor's async-signal-safe pre-exec setup. Preserve existing signal-mask and fd setup.
- [x] Add focused tests for normal helper inheritance, child limit/nice restoration, and unchanged process signal ownership.

### Task 4: Promote and demote the native reactor at lifecycle boundaries

**Files:**
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/shutdown_cycle.rs`
- Modify: `src/native_output/runtime/bootstrap.rs`

- [x] Initialize QoS after signal setup and before any native workers are created.
- [x] Apply native reactor policy immediately before entering the cycle loop; return the typed `require` error if verified RR is unavailable.
- [x] Demote before controlled shutdown joins and before emergency cleanup; do not reacquire after a budget overrun.
- [x] Keep all existing work-domain, input-epoch, cursor, presentation, and KMS ownership semantics intact.

### Task 5: Bound identified native work batches and report effective state

**Files:**
- Modify: `src/native/event_loop.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/compositor/state/shortcuts.rs`
- Modify: `src/compositor/screen_capture.rs`
- Modify: `src/native_output/launch.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`

- [x] Cap Wayland accepts, native process launches, and screen-capture requests per turn while preserving request order and using the existing continuation eventfd for remaining work. Record that per-client Wayland dispatch drains that client's full pending request set.
- [x] Add continuation coalescing regressions and verify input/routing guard order remains unchanged.
- [x] Include one doctor check per live QoS role with requested and effective kernel state, backend, failure reason, reset-on-fork, demotion count, and budget-overrun evidence.

### Task 6: Document the Phase 2 renderer split and verify the change

**Files:**
- Create: `docs/scheduling-qos-phase1.md`
- Modify as needed: `docs/superpowers/specs/2026-10-09-typhon-scheduling-qos-phase1-design.md`

- [x] Record the current renderer ownership boundary and the future bounded one-running/one-pending render queue contract.
- [x] Run `cargo fmt --all -- --check`, `git diff --check`, focused QoS/work-domain/wake/input/cursor/presentation/KMS/process/application-scope tests, then the broad suite.
- [x] Compare broad-suite failures against same-checkout baseline runs and run Clippy/check for changed targets.
- [x] Build the qualification binary under the verified Aether target, commit only this phase's files/hunks, and return exact `off`/`auto` hardware commands without restarting the compositor.
