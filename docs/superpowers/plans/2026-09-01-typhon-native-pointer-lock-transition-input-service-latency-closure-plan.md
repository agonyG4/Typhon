# Typhon Native Pointer-Lock Transition Input-Service Latency Closure v1 Implementation Plan

> **For agentic workers:** Execute this plan inline in the current checkout as requested. Do not dispatch sub-agents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add observer-neutral transition timing and a bounded, production-owned input-routing transition barrier without changing accepted pointer semantics or losing unrelated reactor work.

**Architecture:** First add a disabled-by-default transition-local timing probe so runtime qualification can attribute wall time without per-motion logging. Then make native constraint settlement return a typed routing-transition outcome, add an input-only nonblocking epoll checkpoint, and integrate a one-shot barrier that can run one fresh input microturn through the existing dispatch seam before cycle-tail work.

**Tech Stack:** Rust, libc epoll/eventfd/timerfd, Smithay/Wayland server resources, libinput/raw evdev backends, Cargo unit/integration tests, repository `rtk` wrappers.

## Global Constraints

- Preserve `NATIVE_INPUT_DRAIN_BUDGET = 256`.
- Preserve one `libinput.dispatch()` per semantic epoch.
- Preserve no client read-side Wayland dispatch inside an active semantic epoch.
- Preserve activation-time anchor resolution from current compositor pointer state and backend/compositor anchor equality.
- Preserve exact physical relative motion, consecutive-motion coalescing, raw evdev bounds, and all previously closed pointer behavior.
- Do not add sleeps, busy loops, permanent polling timers, per-motion output, per-motion allocation, motion thresholds, delta clamps, motion drops, or application-specific branches.
- Do not consume unrelated readiness during the transition checkpoint.
- Do not commit or revert the pre-existing unrelated working-tree deletions; preserve the existing dirty scheduler/wake-authority changes while editing overlapping files.
- Use strict RED → GREEN for every production behavior change.
- Use `rtk` wrappers for Cargo and Git commands where available.

---

### Task 1: Establish observer-neutral transition timing

**Files:**
- Create: `src/native_output/runtime/pointer_timing.rs`
- Modify: `src/native_output/runtime/mod.rs:20-60,268-379`
- Modify: `src/native_output/runtime/bootstrap.rs:650-690`
- Test: `src/native_output/runtime/pointer_timing.rs`

**Interfaces:**
- Produces `NativePointerTimingTrace::from_env()` keyed by `TYPHON_POINTER_TIMING_TRACE`.
- Stores a fixed-capacity `[NativePointerTimingRecord; 8]` ring and one active transition slot.
- Exposes transition/batch/phase observation methods that accept compositor-monotonic timestamps and counters.
- Exposes test-only record/emission counters so disabled-path behavior is testable without intercepting process output.

- [ ] **Step 1: Write the failing timing-probe tests.** Test disabled construction, transition completion, deterministic ring replacement, and no formatting/emission on the disabled path.

```rust
#[test]
fn disabled_timing_probe_does_not_format_or_emit() {
    let mut trace = NativePointerTimingTrace::disabled_for_test();
    trace.observe_transition(test_transition(), 10);
    trace.observe_first_batch(test_batch(), 20);

    assert_eq!(trace.formatted_summary_count(), 0);
    assert_eq!(trace.emitted_summary_count(), 0);
    assert_eq!(trace.retained_capacity(), 8);
}

#[test]
fn timing_ring_replaces_oldest_record_deterministically() {
    let mut trace = NativePointerTimingTrace::enabled_for_test();
    for timestamp in 1..=9 {
        trace.observe_transition(test_transition(), timestamp);
        trace.observe_first_batch(test_batch(), timestamp + 1);
    }

    assert_eq!(trace.completed_record_count(), 9);
    assert_eq!(trace.oldest_retained_transition_timestamp(), 2);
}
```

- [ ] **Step 2: Run the tests to verify RED.**

Run: `rtk cargo test --locked pointer_timing --lib`

Expected: FAIL because the timing probe and record APIs do not exist.

- [ ] **Step 3: Implement the bounded probe.** Cache the environment value once in `from_env()`. Keep disabled observation as a boolean branch with no clock read, formatting, allocation, output, or scheduling side effect. Use a `Copy` record and fixed ring. Define the active-record policy explicitly: a new transition replaces an incomplete active record; a completed record is retained until ring wraparound. Emit one compact summary only when the first post-transition batch completes the record.

- [ ] **Step 4: Run the focused tests to verify GREEN.**

Run: `rtk cargo test --locked pointer_timing --lib`

Expected: PASS with zero disabled-path formatting/emission and deterministic bounded retention.

- [ ] **Step 5: Commit the observer slice.**

```bash
rtk git add src/native_output/runtime/pointer_timing.rs src/native_output/runtime/mod.rs src/native_output/runtime/bootstrap.rs
rtk git commit -m "obs: add neutral pointer transition timing probe"
```

### Task 2: Add the typed backend settlement outcome

**Files:**
- Modify: `src/native_output/runtime/frame.rs:599-615,760-795`
- Modify: `src/native_output/input/routing.rs:1327-1442`
- Modify: `src/native_output/runtime/cycle_dispatch.rs:27-53,820-840,1127-1145`
- Test: `src/native_output/runtime/cycle_dispatch.rs:1169-1360`
- Test: `src/native_output/tests/input.rs`

**Interfaces:**
- Produces `NativeInputRoutingTransition` with `LockedActivated`, `LockedDeactivated`, `ConfinedActivated`, and `ConfinedDeactivated` variants carrying `PointerConstraintBackendId`.
- Produces `NativePointerConstraintSettlementOutcome { redraw_requested, routing_transition }`.
- Changes `settle_native_pointer_constraint_backend_requests()` and `process_native_pointer_constraint_backend_requests()` to return that outcome.
- Adds `deactivated_mode: Option<PointerConstraintMode>` to `NativePointerConstraintBackendAction` so deactivation classification comes from the action that removed the active backend constraint.

- [ ] **Step 1: Write the failing outcome tests.** Add backend tests for real locked/confined activation and deactivation, and assert `None` for visibility-only, warp, confined-region update, stale, rejected, failed, and queued-only actions.

```rust
#[test]
fn backend_activation_produces_one_locked_transition() {
    let id = PointerConstraintBackendId { constraint_id: 7, generation: 3 };
    let mut backend = NativePointerConstraintBackend::default();
    let action = backend.activate_locked(id, CompositorOutputPosition { x: 10.0, y: 20.0 });

    assert_eq!(
        NativeInputRoutingTransition::from_action(&action),
        Some(NativeInputRoutingTransition::LockedActivated(id))
    );
}

#[test]
fn visibility_only_action_has_no_routing_transition() {
    let mut backend = NativePointerConstraintBackend::default();
    let action = backend.handle_request(
        PointerConstraintBackendRequest::ApplyCursorVisibility { visible: false },
        CompositorOutputPosition { x: 10.0, y: 20.0 },
    );

    assert_eq!(NativeInputRoutingTransition::from_action(&action), None);
}
```

- [ ] **Step 2: Run the focused tests to verify RED.**

Run: `rtk cargo test --locked native_pointer_constraint --lib`

Expected: FAIL because the typed transition enum, action mapping, and settlement outcome do not exist.

- [ ] **Step 3: Implement the minimal typed result.** Add the enum and outcome, populate `deactivated_mode` only in `deactivate()`, and set the first real transition returned by the backend action. Keep redraw accumulation and request FIFO unchanged. Do not set a transition for queued, stale, rejected, failed, update, warp, or visibility actions.

- [ ] **Step 4: Run the focused tests to verify GREEN.**

Run: `rtk cargo test --locked native_pointer_constraint --lib`

Expected: PASS, including the existing anchor and epoch tests.

- [ ] **Step 5: Commit the typed outcome slice.**

```bash
rtk git add src/native_output/runtime/frame.rs src/native_output/input/routing.rs src/native_output/runtime/cycle_dispatch.rs src/native_output/tests/input.rs
rtk git commit -m "feat: expose native pointer routing transitions"
```

### Task 3: Add an input-only nonblocking epoll checkpoint

**Files:**
- Modify: `src/native/event_loop.rs:562-700`
- Test: `src/native/event_loop.rs` test module near `simultaneous_sources_are_returned_in_one_wakeup`

**Interfaces:**
- Produces `NativeEventLoop::input_ready_nonblocking() -> io::Result<bool>`.
- The method calls `epoll_wait(..., timeout=0)`, examines only `NativeEventSource::Input(_)`, and leaves every fd unread.

- [ ] **Step 1: Write the failing readiness-preservation test.** Register eventfds as input and control sources, signal both, assert the checkpoint returns only input readiness, then assert the normal wait still reports input readiness.

```rust
#[test]
fn input_readiness_checkpoint_peeks_without_consuming_input_or_other_sources() {
    let input = event_fd();
    let control = event_fd();
    let mut event_loop = NativeEventLoop::new().unwrap();
    event_loop.register(input.as_raw_fd(), NativeEventSource::Input(0)).unwrap();
    event_loop.register(control.as_raw_fd(), NativeEventSource::ControlClient).unwrap();

    signal(input.as_raw_fd());
    signal(control.as_raw_fd());

    assert!(event_loop.input_ready_nonblocking().unwrap());
    let wakeup = event_loop.wait().unwrap();
    assert!(wakeup.reasons.input());
}
```

- [ ] **Step 2: Run the test to verify RED.**

Run: `rtk cargo test --locked input_readiness_checkpoint --lib`

Expected: FAIL because the checkpoint method is missing.

- [ ] **Step 3: Implement the input-only peek.** Reuse stable registration-token lookup, accept input `EPOLLIN`, `EPOLLERR`, `EPOLLHUP`, and `EPOLLRDHUP`, and use the existing interrupted-wait retry helper. Do not drain timer, control, XWayland, cursor, KMS, or continuation sources.

- [ ] **Step 4: Run the focused tests to verify GREEN.**

Run: `rtk cargo test --locked input_readiness_checkpoint --lib`

Expected: PASS.

- [ ] **Step 5: Commit the readiness slice.**

```bash
rtk git add src/native/event_loop.rs
rtk git commit -m "feat: peek native input readiness without consuming work"
```

### Task 4: Add the one-shot transition barrier decision

**Files:**
- Create: `src/native_output/runtime/routing_barrier.rs`
- Modify: `src/native_output/runtime/mod.rs:20-60, public re-exports`
- Test: `src/native_output/runtime/routing_barrier.rs`

**Interfaces:**
- Produces `NativeInputRoutingTransitionBarrier` with `observe(transition)`, `armed()`, and `checkpoint(input_ready) -> NativeRoutingBarrierDecision`.
- Produces `NativeRoutingBarrierDecision::{NoBarrier, ContinueCycle, ServiceFreshInput}`.
- `checkpoint()` clears the intervention state exactly once.

- [ ] **Step 1: Write the failing production-seam tests.** Test no transition, a transition with no fresh input, a transition with fresh input, repeated checkpoints, and the deterministic ordering `settlement → checkpoint → fresh input → tail`.

```rust
#[test]
fn a_real_transition_requires_one_checkpoint_before_tail_work() {
    let mut barrier = NativeInputRoutingTransitionBarrier::default();
    barrier.observe(NativeInputRoutingTransition::LockedActivated(test_id()));

    assert_eq!(
        barrier.checkpoint(true),
        NativeRoutingBarrierDecision::ServiceFreshInput
    );
    assert_eq!(barrier.checkpoint(true), NativeRoutingBarrierDecision::NoBarrier);
}
```

- [ ] **Step 2: Run the test to verify RED.**

Run: `rtk cargo test --locked real_transition_requires_one_checkpoint --lib`

Expected: FAIL because the barrier module and decision type do not exist.

- [ ] **Step 3: Implement the minimal state machine.** Store an armed bit and transition metadata for diagnostics. `observe()` arms only a real transition. `checkpoint(false)` returns `ContinueCycle`; `checkpoint(true)` returns `ServiceFreshInput`; an unarmed checkpoint returns `NoBarrier`. Each path clears the armed bit.

- [ ] **Step 4: Run the focused tests to verify GREEN.**

Run: `rtk cargo test --locked routing_barrier --lib`

Expected: PASS.

- [ ] **Step 5: Commit the barrier-decision slice.**

```bash
rtk git add src/native_output/runtime/routing_barrier.rs src/native_output/runtime/mod.rs
rtk git commit -m "feat: model one-shot pointer routing barrier"
```

### Task 5: Propagate outcomes and integrate the real barrier

**Files:**
- Modify: `src/native_output/runtime/cycle_dispatch.rs:752-1160`
- Modify: `src/native_output/runtime/cycle.rs:112-355`
- Modify: `src/native_output/runtime/mod.rs:132-379`
- Modify: `src/native_output/runtime/bootstrap.rs:650-690`
- Modify: `src/native_output/runtime/routing_barrier.rs`
- Test: `src/native_output/runtime/cycle_dispatch.rs:1214-1360`
- Test: `src/native_output/runtime/cycle.rs`
- Test: `src/native_output/tests/input.rs`

**Interfaces:**
- Produces `NativeDispatchWaylandAndInputOutcome { pacing_readiness_changed, routing_transition }`.
- `NativeRuntime` owns `routing_barrier: NativeInputRoutingTransitionBarrier`.
- Adds private `NativeRuntime::run_fresh_input_microturn(&mut self, cycle: &mut NativeCycleState)` using the real dispatch function with `service_input=true` and `dispatch_wayland=false`.

- [ ] **Step 1: Write the failing real-seam tests.** Extend epoch tests to assert pre-existing activation is effective before already-ready input, Wayland-created activation is reported only after the old epoch, and the combined old-input-plus-Wayland sequence remains ordered. Add a production cycle scheduling test with `NativeWorkDomains` and the production barrier decision that requires `settlement → checkpoint → optional microturn → tail`.

- [ ] **Step 2: Run the focused tests to verify RED.**

Run: `rtk cargo test --locked cycle_dispatch --lib`

Expected: FAIL because dispatch returns only a pacing boolean and `run_cycle()` does not consult a routing barrier.

- [ ] **Step 3: Propagate the settlement outcome.** Change both settlement calls to accumulate the first real routing transition alongside redraw state. Return the typed outcome with the existing pacing flag. Preserve the input epoch gate, deferred Wayland progression, cursor synchronization, anchor resolution, request FIFO, and batch counters.

- [ ] **Step 4: Integrate the one-shot checkpoint.** After dispatch returns and before commit timing, pacing, control/cursor, XWayland, acquire/prepare, or presentation tail work, observe the outcome and call `input_ready_nonblocking()` only when the barrier is armed. If it returns `ServiceFreshInput`, invoke one real input microturn and merge only additive input/redraw/shutdown fields; do not overwrite original pageflip, accepted-client, presentation, or work-domain fields. Do not recursively checkpoint a transition observed by that microturn. If it returns `ContinueCycle`, retain the current cycle’s non-input ownership and continue.

- [ ] **Step 5: Integrate timing marks without authority coupling.** Mark real wake return, input-service start/end, Wayland read-side dispatch, settlement return, cursor synchronization, named cycle-tail phase durations, cycle return, and the next wake/input service using the already-created timing probe. Capture raw/coalesced counts and oldest/newest hardware timestamps at batch boundaries. The probe must not select work domains, deadlines, epochs, anchors, or motion values.

- [ ] **Step 6: Run focused GREEN tests.**

Run: `rtk cargo test --locked cycle_dispatch --lib`

Expected: PASS, including no barrier on ordinary motion, no boundary in a >256 continuation, exact fresh locked input, and already-ready input in the same cycle.

- [ ] **Step 7: Commit the integrated scheduling slice.**

```bash
rtk git add src/native_output/runtime/cycle_dispatch.rs src/native_output/runtime/cycle.rs src/native_output/runtime/mod.rs src/native_output/runtime/bootstrap.rs src/native_output/runtime/routing_barrier.rs
rtk git commit -m "feat: bound native cycle tail after routing transitions"
```

### Task 6: Complete the pointer and raw-input regression matrix

**Files:**
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/native_output/input/epoch.rs`
- Modify: `src/native_output/input/routing.rs`
- Modify: `src/native_output/tests/input.rs`
- Modify: `src/compositor/tests/input_output/relative_and_constraints.rs`
- Modify: `src/compositor/tests/input_output/pointer_cursor_lifecycle.rs`
- Modify: `src/compositor/tests/input_output/pointer_lock_warp.rs`
- Modify: `src/compositor/tests/input_output/pointer_warp_serial.rs`
- Modify: `src/compositor/tests/xwayland_pointer_batch.rs`
- Test: `src/native_output/runtime/pointer_timing.rs`

- [ ] **Step 1: Write failing regressions for exact behavior.** Cover locked first-fresh motion with exact relative delta and unchanged absolute position, confined transitions, already-ready activation/input, combined old input plus Wayland request, visibility-only no barrier, repeated lock/unlock liveness, raw evdev ordering, >256 continuation epoch ownership, timing probe wraparound, and disabled observer properties.

- [ ] **Step 2: Run each focused suite independently.**

Run these separately so each filter is valid and failures remain attributable:

```bash
rtk cargo test --locked relative_and_constraints --lib
rtk cargo test --locked pointer_cursor --lib
rtk cargo test --locked pointer_lock_warp --lib
rtk cargo test --locked pointer_warp_serial --lib
rtk cargo test --locked xwayland_pointer_batch --lib
rtk cargo test --locked native_input --lib
```

Expected: new tests fail before their production behavior exists and pass after the integrated seam is complete. Any unrelated failure is rerun with its exact test name and recorded.

- [ ] **Step 3: Implement only corrections demanded by focused failures.** Do not alter relative arithmetic, coalescing, anchor policy, cursor semantics, or unrelated work domains.

- [ ] **Step 4: Rerun all six focused suites independently.** Require exit status 0 for each before moving to full verification.

- [ ] **Step 5: Commit the regression slice.**

```bash
rtk git add src/native_output/runtime/cycle_dispatch.rs src/native_output/input/epoch.rs src/native_output/input/routing.rs src/native_output/tests/input.rs src/compositor/tests/input_output/relative_and_constraints.rs src/compositor/tests/input_output/pointer_cursor_lifecycle.rs src/compositor/tests/input_output/pointer_lock_warp.rs src/compositor/tests/input_output/pointer_warp_serial.rs src/compositor/tests/xwayland_pointer_batch.rs src/native_output/runtime/pointer_timing.rs
rtk git commit -m "test: close native pointer transition latency regressions"
```

### Task 7: Full verification and qualification handoff

**Files:**
- Inspect: all changed files, commits, and `git diff --check`
- Report: final findings in the assistant response; do not add unrelated report-file changes

- [ ] **Step 1: Run the full required verification.**

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

- [ ] **Step 2: Rerun any parallel failure independently.** Run the exact failing test or command without parallelism, then run the deterministic single-thread form before classifying a failure as unrelated.

- [ ] **Step 3: Inspect final ownership and scope.** Verify current HEAD, changed files, typed transition origin, input-only checkpoint behavior, no unrelated readiness consumption, no per-motion output/allocation, no app-specific code, no new thread, and preserved pre-existing dirty deletions.

- [ ] **Step 4: Provide the primary runtime command with semantic debug unset.**

```bash
TYPHON_POINTER_TIMING_TRACE=1 \
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
./bin/start-oblivion-one-tty
```

- [ ] **Step 5: Provide the optional semantic-debug command only for semantic inspection.**

```bash
TYPHON_POINTER_DEBUG=1 \
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
./bin/start-oblivion-one-tty
```

The final report must distinguish source/deterministic evidence from runtime
evidence, state whether real Sober was tested, summarize the observer-effect
analysis, compare KWin/Hyprland + Aquamarine/wlroots/Weston accurately, list
files and commits, and report only timing-supported residual hypotheses if the
symptom persists.
