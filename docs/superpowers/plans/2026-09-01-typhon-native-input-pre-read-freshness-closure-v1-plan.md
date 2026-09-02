# Typhon Native Input Pre-Read Freshness Closure v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Promote native input that is readable at the final Wayland-only pre-read boundary into the existing semantic epoch before client input-resource topology can change.

**Architecture:** Keep the current Native Input Semantic Epoch, explicit libinput dispatch/drain split, Native Wake Authority, and post-transition latency guard. Add one production late-readiness arbitration at the Wayland-only branch of `NativeRuntime::dispatch_wayland_and_input()`, and carry a bounded observation of that decision into the transition timing summary. Correct timing service-attempt boundaries without changing input semantics.

**Tech Stack:** Rust, libinput/raw evdev native input backends, epoll readiness, OwnCompositorServer Wayland tests, Cargo/rtk verification.

## Global Constraints

- Do not introduce an input thread, timer, sleep, busy loop, unconditional `libinput.dispatch()`, or app-specific branch.
- Preserve `NATIVE_INPUT_DRAIN_BUDGET = 256`, one `libinput.dispatch()` per semantic epoch, stable topology during an epoch, exact motion sums, and consecutive-motion coalescing.
- Perform a late readiness probe only for `dispatch_wayland && !service_input`; combined and input-only turns perform zero pre-read probes.
- The probe uses `NativeEventLoop::input_ready_nonblocking()` and does not consume any fd or mutate the current wake snapshot.
- The probe is the semantic cut: ready at the cut belongs to the old topology; readiness after it belongs to a later epoch.
- Timing diagnostics are bounded, disabled-path zero-cost after cached enablement, and never scheduling authority.
- Use strict RED -> GREEN and commit each independently testable change.

---

### Task 1: Establish the failing timing and pre-read arbitration tests

**Files:**
- Modify: `src/native_output/runtime/pointer_timing.rs:tests`
- Modify: `src/native_output/runtime/cycle_dispatch.rs:tests`

**Interfaces:**
- Tests will use the existing `NativePointerTimingTrace` test constructor and the production pre-read decision function that will be called by `dispatch_wayland_and_input()`.
- The timing test will require distinct `first_input_service_attempt_at_ns` and `first_nonempty_input_service_duration_ns` summary fields.
- The arbitration test will require `NativePreReadInputDecision::{ReadWayland, PromoteInputEpoch, NoGate}` or equivalent names with the same behavior.

- [ ] **Step 1: Write the failing timing regression.**

Add a test that records a transition at `100`, an empty service attempt from `200` to `250`, then a non-empty attempt from `450` to `500`, and asserts the summary reports the first attempt offset as `100` and the non-empty attempt duration as `50`. Assert the old ambiguous `input_service_duration_ns` field is absent.

```rust
#[test]
fn timing_summary_separates_service_attempts() {
    let mut trace = NativePointerTimingTrace::enabled_for_test();
    trace.record_routing_transition_committed(test_transition(), 100);
    trace.record_input_service_start(200);
    trace.observe_first_batch(NativePointerTimingBatch::default(), 225);
    trace.record_input_service_end(250);
    trace.record_input_service_start(450);
    trace.observe_first_batch(test_batch(), 475);
    trace.record_input_service_end(500);

    let summary = format_summary(&trace.records[0].expect("completed record"));
    assert!(summary.contains("transition_to_first_input_service_attempt_ns=100"));
    assert!(summary.contains("first_nonempty_input_service_duration_ns=50"));
    assert!(!summary.contains("input_service_duration_ns="));
}
```

- [ ] **Step 2: Run the timing test and record RED.**

Run:

```bash
rtk cargo test --locked timing_summary_separates_service_attempts -- --exact --nocapture
```

Expected: compile/test failure because the current observer has one spanning `input_service_duration_ns` and no separate first-attempt/non-empty fields.

- [ ] **Step 3: Write the production-seam late-readiness RED test.**

Add a unit test in the existing `cycle_dispatch` test module for the production arbitration helper. It must represent the real ordering with an explicit recorder and a single readiness result:

```rust
#[test]
fn late_input_at_wayland_pre_read_cut_promotes_one_epoch_before_read() {
    let mut order = Vec::new();

    assert_eq!(
        decide_native_pre_read_input(true, false, true),
        NativePreReadInputDecision::PromoteInputEpoch,
    );
    order.push("pre_read_promote");
    order.push("native_epoch");
    order.push("wayland_read");

    assert_eq!(order, vec!["pre_read_promote", "native_epoch", "wayland_read"]);
}
```

Also add tests that `true/true` and `false/true` return `NoGate`, and `true/false/false` returns `ReadWayland`.

- [ ] **Step 4: Run the arbitration tests and record RED.**

Run:

```bash
rtk cargo test --locked late_input_at_wayland_pre_read_cut_promotes_one_epoch_before_read -- --exact --nocapture
```

Expected: compile failure because the production decision function and enum do not yet exist. Preserve the failure output in the implementation report as the primary pre-read RED point.

- [ ] **Step 5: Commit the RED tests only.**

```bash
git add src/native_output/runtime/pointer_timing.rs src/native_output/runtime/cycle_dispatch.rs
git commit -m "test: expose native input pre-read freshness gap"
```

### Task 2: Make timing service-attempt boundaries truthful

**Files:**
- Modify: `src/native_output/runtime/pointer_timing.rs:NativePointerTimingRecord`
- Modify: `src/native_output/runtime/pointer_timing.rs:NativePointerTimingTrace`

**Interfaces:**
- `record_input_service_start(at_ns)` starts one attempt and records the first attempt separately.
- `observe_first_batch(batch, at_ns)` ignores empty converted batches and marks the first non-empty attempt start from the active attempt.
- `record_input_service_end(at_ns)` ends only the active attempt and records the first non-empty duration.

- [ ] **Step 1: Add distinct record fields and update the summary.**

Replace the spanning `input_service_start_at_ns`/`input_service_end_at_ns` pair with:

```rust
active_input_service_start_at_ns: Option<u64>,
first_input_service_attempt_at_ns: Option<u64>,
first_nonempty_input_service_start_at_ns: Option<u64>,
first_nonempty_input_service_end_at_ns: Option<u64>,
```

Format only:

```text
transition_to_first_input_service_attempt_ns
first_nonempty_input_service_duration_ns
```

Retain `unknown` when a boundary is unavailable.

- [ ] **Step 2: Make first-batch observation non-empty aware.**

In `observe_first_batch`, return for `raw_events == 0`, and for the first real batch set `first_nonempty_input_service_start_at_ns` from the active attempt without marking an empty attempt complete. Keep the existing fixed ring and one-summary-per-transition behavior.

- [ ] **Step 3: Reset the active attempt at every service end.**

Set the first non-empty end only when the active attempt produced the first non-empty batch, then clear `active_input_service_start_at_ns`. Do not make a later attempt inherit an earlier start.

- [ ] **Step 4: Run focused GREEN.**

Run:

```bash
rtk cargo test --locked timing_summary_separates_service_attempts actual_input_service_time_is_not_reactor_wake_time empty_input_service_does_not_complete_transition_observation -- --nocapture
```

Expected: PASS, with no per-motion output.

- [ ] **Step 5: Commit the timing correction.**

```bash
git add src/native_output/runtime/pointer_timing.rs
git commit -m "fix: separate native input service attempts in timing trace"
```

### Task 3: Add the production pre-read decision and promote the real input path

**Files:**
- Modify: `src/native_output/runtime/cycle_dispatch.rs:NativePreReadInputDecision, dispatch_wayland_and_input`
- Modify: `src/native_output/runtime/cycle_dispatch.rs:tests`

**Interfaces:**
- `decide_native_pre_read_input(dispatch_wayland: bool, service_input: bool, input_ready: bool) -> NativePreReadInputDecision` is the production decision used immediately after the real non-consuming probe.
- `NativeWaylandInputDispatchOutcome` carries the bounded pre-read observation for timing/tests.

- [ ] **Step 1: Implement the smallest decision helper.**

Use this behavior:

```rust
fn decide_native_pre_read_input(
    dispatch_wayland: bool,
    service_input: bool,
    input_ready: bool,
) -> NativePreReadInputDecision {
    if !dispatch_wayland || service_input {
        NativePreReadInputDecision::NoGate
    } else if input_ready {
        NativePreReadInputDecision::PromoteInputEpoch
    } else {
        NativePreReadInputDecision::ReadWayland
    }
}
```

- [ ] **Step 2: Move raw-input observation to actual service entry.**

Remove the initial `NativeSessionIo::observe` based only on the incoming boolean. After the pre-read decision has promoted the turn, call it immediately before the existing native input service block. This makes promoted input real input service even when the original wake snapshot lacked input.

- [ ] **Step 3: Insert exactly one late probe at the existing Wayland-only branch.**

Make `service_input` local and mutable. After initial constraint settlement and cursor synchronization, immediately before the current `if dispatch_wayland && !service_input` read branch, execute:

```rust
let mut pre_read_input_promoted = false;
if dispatch_wayland && !service_input {
    let input_ready = event_loop.input_ready_nonblocking()?;
    match decide_native_pre_read_input(dispatch_wayland, service_input, input_ready) {
        NativePreReadInputDecision::PromoteInputEpoch => {
            service_input = true;
            pre_read_input_promoted = true;
        }
        NativePreReadInputDecision::ReadWayland | NativePreReadInputDecision::NoGate => {}
    }
}
```

The existing control flow must then run the native input block before the after-input Wayland read. Do not add a loop or call `libinput.dispatch()` from the gate.

- [ ] **Step 4: Preserve combined and input-only ownership.**

Ensure the gate is skipped when `service_input` is already true or `dispatch_wayland` is false. A combined wake keeps its existing old-input epoch before Wayland read, and an input-only turn has no pre-read probe.

- [ ] **Step 5: Run focused GREEN.**

Run:

```bash
rtk cargo test --locked late_input_at_wayland_pre_read_cut_promotes_one_epoch_before_read -- --exact --nocapture
rtk cargo test --locked ordinary_pointer_motion_never_enters_full_server_progression constraint_sensitive_input_keeps_its_narrow_follow_up -- --nocapture
```

Expected: PASS; the production path now promotes exactly one epoch for Wayland-only late readiness and does not probe other work classes.

- [ ] **Step 6: Commit the production gate.**

```bash
git add src/native_output/runtime/cycle_dispatch.rs
git commit -m "fix: promote fresh native input before Wayland reads"
```

### Task 4: Carry pre-read timing evidence without changing scheduling authority

**Files:**
- Modify: `src/native_output/runtime/pointer_timing.rs:NativePointerPreReadObservation, NativePointerTimingRecord`
- Modify: `src/native_output/runtime/cycle_dispatch.rs:dispatch_wayland_and_input`

**Interfaces:**
- `NativePointerPreReadObservation` is a small `Copy` value containing probe/promotion booleans and one optional `NativePointerTimingBatch`.
- Transition commit recording accepts the observation only for the transition produced by the same dispatch flow.

- [ ] **Step 1: Add bounded pre-read observation fields.**

Add `pre_read_probe`, `pre_read_input_promoted`, and `pre_transition_input` to the fixed timing record. Record only one promoted pre-read batch. Do not maintain an unbounded history or make the observation available to the scheduler.

- [ ] **Step 2: Capture the promoted batch after existing coalescing.**

When the promoted epoch materializes its batch, capture the same raw/coalesced/hardware timestamp values already used by `observe_first_batch`. Keep all physical events and existing coalescing unchanged.

- [ ] **Step 3: Attach the observation to the real final transition commit.**

Pass the local observation to the final settlement’s `record_routing_transition_committed` call. Initial/pre-existing transitions pass the default observation. If no transition follows, the local bounded observation is discarded at return.

- [ ] **Step 4: Extend the summary truthfully.**

Add the booleans and pre-transition raw/coalesced/hardware span fields to the one-line transition summary. Use `unknown` for absent batch/timestamps. Do not emit per-event lines.

- [ ] **Step 5: Run timing GREEN.**

Run:

```bash
rtk cargo test --locked timing_probe_does_not_change_recorded_batch_values timing_summary_does_not_invent_a_hardware_timestamp_span timing_summary_separates_service_attempts -- --nocapture
```

Expected: PASS with fixed-capacity records and no disabled-path clock/output activity.

- [ ] **Step 6: Commit observer integration.**

```bash
git add src/native_output/runtime/pointer_timing.rs src/native_output/runtime/cycle_dispatch.rs
git commit -m "feat: trace pre-read native input promotion"
```

### Task 5: Close regression coverage around production ordering and existing epochs

**Files:**
- Modify: `src/native_output/runtime/cycle_dispatch.rs:tests`
- Modify: `src/native_output/runtime/pointer_timing.rs:tests`
- Modify: `src/native_output/tests/input.rs`
- Modify: `src/compositor/tests/input_output/relative_and_constraints.rs` where the existing real-resource fixture can accept the narrow assertion

**Interfaces:**
- Use the real input backend epoch API and existing OwnCompositorServer fixtures; do not duplicate `run_cycle()` into a fake scheduler.

- [ ] **Step 1: Add the decision matrix tests.**

Assert exactly one decision/probe for Wayland-only late readiness, one negative decision for unavailable input, and zero gate decisions for combined/input-only turns. Add an order recorder proving that promotion occurs before the client read and that no second probe is allowed after the promoted epoch.

- [ ] **Step 2: Add the real resource ordering regression.**

Extend the existing relative-pointer / pointer-constraint production fixture to assert the old native delta is processed before the client resource exists, the Wayland request then creates the resource and lock, and a later D2 is delivered as locked relative motion while the absolute pointer remains anchored. Keep the assertion magnitude-independent and exact for D2.

- [ ] **Step 3: Add/retain >256 and exact-256 checks.**

Use the existing backend tests to assert one libinput dispatch, no Wayland read within continuation chunks, same epoch ownership, and client read only after exhaustion. Confirm the pre-read gate is not invoked inside an active epoch.

- [ ] **Step 4: Add raw evdev and input-peek regression coverage.**

Preserve bounded raw reads, suspend discard, no unbounded allocation, and pre-read promotion ordering. Retain simultaneous input/control/DRM/continuation checks proving `input_ready_nonblocking()` is non-consuming and terminal flags do not count as healthy input.

- [ ] **Step 5: Audit deferred Wayland progression.**

Run the existing `deferred_wayland_progression` behavior through the >256 test. If it is green, leave the local state architecture untouched and document that the debt remains within the production dispatch invocation. Only if a deterministic RED demonstrates loss across a continuation may epoch-owned state be introduced.

- [ ] **Step 6: Run focused GREEN suites.**

Run:

```bash
rtk cargo test --locked native_output::runtime::cycle_dispatch::tests -- --nocapture
rtk cargo test --locked native_output::runtime::pointer_timing::tests -- --nocapture
rtk cargo test --locked native_input -- --nocapture
rtk cargo test --locked relative_and_constraints -- --nocapture
rtk cargo test --locked raw_evdev -- --nocapture
```

Expected: all focused tests pass, including existing anchor/guard/warp/coalescing behavior.

- [ ] **Step 7: Commit the regression matrix.**

```bash
git add src/native_output/runtime/cycle_dispatch.rs src/native_output/runtime/pointer_timing.rs src/native_output/tests/input.rs src/compositor/tests/input_output/relative_and_constraints.rs
git commit -m "test: cover native input pre-read ownership"
```

### Task 6: Verify, audit, and hand off runtime qualification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-01-typhon-native-input-pre-read-freshness-closure-v1-design.md` only if implementation details need factual correction
- Create: `docs/superpowers/reports/2026-09-01-typhon-native-input-pre-read-freshness-closure-v1-report.md`

- [ ] **Step 1: Audit all native client-read call sites.**

Use graph traces and literal search to classify `dispatch_wayland_with_outcome()`, `tick_with_outcome()`, and `tick()`. Confirm only the native runtime path receives the pre-read policy and synthetic/headless/test server APIs remain independent.

- [ ] **Step 2: Run the required verification.**

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

Record every command, exit status, and any unrelated flake with isolated rerun evidence.

- [ ] **Step 3: Inspect the final diff and coverage.**

Run `git diff --check`, `git status --short`, `git log`, and the codebase-memory coverage check for every changed source path. Read any reported partial ranges directly before making completeness claims.

- [ ] **Step 4: Write the report.**

Include starting/ending HEAD, exact pre-read gate location, probe counts by work class, semantic cut, RED/GREEN evidence, timing fields, reference comparisons, >256/raw/guard behavior, files, verification, and explicit statement that Sober was not tested by the agent.

- [ ] **Step 5: Commit the report.**

```bash
git add docs/superpowers/reports/2026-09-01-typhon-native-input-pre-read-freshness-closure-v1-report.md
git commit -m "docs: report native input pre-read closure"
```

- [ ] **Step 6: Provide the user qualification commands.**

Primary latency run:

```bash
TYPHON_POINTER_TIMING_TRACE=1 \
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
./bin/start-oblivion-one-tty
```

Optional semantic debug only:

```bash
TYPHON_POINTER_DEBUG=1 \
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
./bin/start-oblivion-one-tty
```

Do not claim application-level Sober closure before the user’s manual native DRM/KMS qualification.
