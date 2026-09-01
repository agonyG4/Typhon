# Typhon Native Wake Authority v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task with review checkpoints. The user explicitly requested inline execution and no subagents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace stale native timer polling with one decision-aware absolute timer deadline plus one coalesced continuation eventfd, while preserving native output, protocol, input, DMA-BUF, KMS worker, and shutdown behavior.

**Architecture:** A pure `NativeWakePlan` owns a fixed continuation bitset and one earliest, owner-labelled real deadline. The scheduler produces its temporal wake contract beside its action and pipeline wait reason; the runtime merges that contract only with independent genuine future deadlines. `NativeEventLoop` owns one internal nonblocking continuation eventfd and exposes its coalesced reasons alongside all other epoll readiness.

**Tech Stack:** Rust, Linux `epoll`, `eventfd`, `timerfd`, `CLOCK_MONOTONIC`, existing `BoundedSamples`, native scheduler/pipeline tests, `rtk cargo` verification.

**Spec:** `docs/superpowers/specs/2026-09-01-typhon-native-wake-authority-design.md`

## Global Constraints

* Preserve DMA-BUF GPU release ownership, correlation, retry debt, and qualification behavior unchanged.
* Preserve O1 render-ahead, adaptive buffering, KMS worker ownership, output transactions, protocol semantics, input epochs, pointer constraints, XWayland ownership, session safety, and shutdown SafeDisable behavior.
* Keep `CLOCK_MONOTONIC` and `TFD_TIMER_ABSTIME` for genuine temporal deadlines.
* Use exactly one internal nonblocking continuation eventfd and one native timerfd.
* Use fixed-size bitsets and bounded existing storage; add no thread, mutex, polling loop, sleep, GPU wait, `glFinish`, or per-event stdout.
* Never clamp expired deadlines into the future.
* Never arm a visual render-start deadline for `WaitForWorkerQueue`, `WaitForPageFlip`, or `WaitForBuffer`.
* Run each new behavior through RED → GREEN → REFACTOR, observing the RED failure before production implementation.
* Commit each independently reviewable task with `rtk git`.

## File map

* Create `src/native_output/runtime/wake_plan.rs` for pure deadline-owner and continuation planning.
* Modify `src/native/event_loop.rs` for continuation eventfd ownership, readiness reporting, truthful timer consumption, and event-loop tests.
* Modify `src/native/scheduler/pipeline.rs` for decision-linked wake deadlines and tests; modify `src/native/scheduler.rs` only when the new contract needs an existing scheduler accessor to expose its current absolute boundary.
* Modify `src/native_output/runtime/mod.rs`, `cycle.rs`, `cycle/pageflip.rs`, `session_io.rs`, `bootstrap.rs`, `work_domains.rs`, `xwayland.rs`, and `planner.rs` to route runtime ownership through the plan and continuation reasons.
* Modify `src/native_output/runtime/metrics.rs` to remove timer orchestration and retain metrics/export responsibility.
* Modify `src/native_output/pacing.rs` for wake-authority counters and active pageflip samples.
* Modify existing runtime/input tests only when their expected wake source changes; do not change `src/native_output/input/epoch.rs` semantics.
* Create `REPORT-2026-09-01-typhon-native-wake-authority-closure.md` only after verification is complete.

### Task 1: Add RED coverage for pure scheduler wake authority

**Files:**

* Modify: `src/native/scheduler/pipeline.rs` tests and scheduler test helpers.
* Create: `src/native_output/runtime/wake_plan.rs` with the initial test module and the test-facing wished-for API used to express the pure contract; production bodies are added in Task 2.
* Modify: `src/native_output/runtime/mod.rs` only to declare the new testable module.

**Interfaces:**

* Consume: `SchedulerDecision`, `PipelineWaitReason`, `ExplicitAtomicSchedulerContext`, `PresentationPipelineView`, `PresentationTarget`.
* Produce for later tasks: `SchedulerWakeDeadlineKind`, `SchedulerWakeDeadline`, `ExplicitAtomicSchedulerDecision::wake_deadline`, `NativeDeadlineOwner`, `NativeDeadline`, `NativeContinuationReasons`, `NativeWakePlan`, and a pure `build_wake_plan`/deadline-selection function with fixed arguments.

- [ ] **Step 1: Write the failing scheduler tests.** Add tests with names and assertions equivalent to:

```rust
#[test]
fn worker_queue_wait_drops_expired_visual_deadline() {
    let decision = decision_for_worker_queue_with_target(5_000_000, 4_000_000);
    assert_eq!(decision.action, SchedulerDecision::WaitForWorkerQueue);
    assert_eq!(decision.wake_deadline, None);
}

#[test]
fn pageflip_wait_drops_obsolete_visual_deadline() {
    let decision = decision_for_pageflip_with_target(5_000_000, 4_000_000);
    assert_eq!(decision.action, SchedulerDecision::WaitForPageFlip);
    assert_eq!(decision.wake_deadline, None);
}

#[test]
fn refresh_wait_keeps_exact_render_start_boundary() {
    let decision = decision_for_refresh_with_target(4_000_000, 5_000_000);
    assert_eq!(decision.wake_deadline, Some(SchedulerWakeDeadline::render_start(5_000_000)));
}

#[test]
fn ready_frame_uses_submit_not_before_boundary() {
    let decision = decision_for_ready_frame(4_000_000, 3_000_000, 5_000_000);
    assert_eq!(decision.wake_deadline, Some(SchedulerWakeDeadline::submit_not_before(5_000_000)));
}

#[test]
fn actionable_scheduler_decisions_have_no_rediscovery_deadline() {
    for action in [
        SchedulerDecision::Render,
        SchedulerDecision::RenderAhead,
        SchedulerDecision::SubmitReady,
        SchedulerDecision::SubmitReadyLate,
        SchedulerDecision::CompleteProtocolOnly,
    ] {
        assert_eq!(scheduler_wake_deadline_for(action, 5_000_000), None);
    }
}
```

Use the existing pipeline fixture style and real `PresentationPipelineView` implementations. Do not mock the scheduler decision itself.

- [ ] **Step 2: Add RED tests for pure wake-plan selection.** Cover all four categories and the required owner behavior:

```rust
#[test]
fn expired_visual_deadline_is_not_selected_for_worker_blocker() {
    let plan = build_wake_plan(WakePlanInputs {
        scheduler: SchedulerWakeContract::wait_for_worker_queue(4_000_000),
        ..WakePlanInputs::default_at(5_000_000)
    });
    assert_eq!(plan.deadline, None);
    assert_eq!(plan.continuation, NativeContinuationReasons::default());
}

#[test]
fn future_refresh_deadline_is_selected_with_owner() {
    let plan = build_wake_plan(WakePlanInputs {
        scheduler: SchedulerWakeContract::render_start(5_000_000),
        ..WakePlanInputs::default_at(4_000_000)
    });
    assert_eq!(plan.deadline, Some(NativeDeadline::frame_scheduler(5_000_000)));
}

#[test]
fn input_backlog_is_continuation_not_now_deadline() {
    let plan = build_wake_plan(WakePlanInputs {
        input_backlog: true,
        ..WakePlanInputs::default_at(10_000_000)
    });
    assert!(plan.continuation.contains(NativeContinuationReason::InputBacklog));
    assert_ne!(plan.deadline.map(|deadline| deadline.at_ns), Some(10_000_000));
}
```

Add equivalent cases for Astrea publication, commit-timing planning, XWayland continuation, ready submit boundary, pageflip watchdog, explicit-sync fallback, future XWayland timeout, control timeout, cursor response, surface pacing, and DMA-BUF retry.

- [ ] **Step 3: Run only the new focused tests and verify a correct RED failure.** Run `rtk cargo test --lib native::scheduler::pipeline` and the focused runtime wake-plan test filter. Expected result: compilation/test failure because the wake contract and pure wake-plan API do not exist yet, not a fixture typo.

- [ ] **Step 4: Commit the RED tests.** Run `git diff --check`, inspect the diff, then commit with:

```bash
rtk git add src/native/scheduler/pipeline.rs src/native/scheduler.rs src/native_output/runtime/mod.rs src/native_output/runtime/wake_plan.rs
rtk git commit -m "test: specify native wake authority contracts"
```

### Task 2: Implement the pure scheduler contract and wake plan

**Files:**

* Modify: `src/native/scheduler/pipeline.rs`.
* Modify: `src/native/scheduler.rs` only if helper accessors need to be made contract-safe.
* Modify: `src/native_output/runtime/mod.rs`.
* Modify: `src/native_output/runtime/wake_plan.rs`.

**Interfaces:**

* Consume: the RED tests and current pipeline snapshot.
* Produce: a copyable `SchedulerWakeDeadlineKind`, `SchedulerWakeDeadline`, decision-linked `ExplicitAtomicSchedulerDecision`, fixed `NativeContinuationReasons`, fixed `NativeWakePlan`, and deterministic earliest-deadline selection.

- [ ] **Step 1: Implement the minimum scheduler contract.** Compute the wake deadline from the same `action`, `context`, and pipeline state that produces `wait_reason`. Return no visual deadline for external blockers or actionable decisions. Use exact `render_start_deadline`, `submit_not_before`, refresh, and watchdog values; do not normalize past values.

- [ ] **Step 2: Implement fixed wake-plan types and pure selection.** Use a `u32` bitset with named reason bits and a deterministic owner rank for equal timestamps. Make `NativeWakePlan` `Copy`, `Default`, `PartialEq`, and allocation-free. Include constructors/helpers used by tests, and ensure external blocker contracts contribute no visual timer.

- [ ] **Step 3: Run the focused tests and confirm GREEN.** Run the exact Task 1 commands through `rtk cargo test`. Expected result: all new scheduler and wake-plan tests pass.

- [ ] **Step 4: Refactor only after GREEN.** Remove duplicate test helpers, document the contract invariants, and rerun the focused tests.

- [ ] **Step 5: Commit.**

```bash
rtk git add src/native/scheduler.rs src/native/scheduler/pipeline.rs src/native_output/runtime/mod.rs src/native_output/runtime/wake_plan.rs
rtk git commit -m "feat: add decision-aware native wake plan"
```

### Task 3: Add RED/GREEN continuation eventfd behavior

**Files:**

* Modify: `src/native/event_loop.rs`.

**Interfaces:**

* Consume: `NativeContinuationReasons`/reason bits from Task 2 and existing `NativeEventSource`, `NativeWakeup`, `WakeReasons`, and registration generation logic.
* Produce: internal `RuntimeContinuation` source, `request_continuation`, continuation reason take/drain behavior, `NativeWakeup.continuation`, and truthful timer state.

- [ ] **Step 1: Write RED event-loop tests.** Add deterministic tests for coalescing, combined readiness, external registration rejection, and timer consumption:

```rust
#[test]
fn continuation_reasons_coalesce_into_one_eventfd_wake() {
    let mut event_loop = NativeEventLoop::new().unwrap();
    event_loop.request_continuation(NativeContinuationReason::InputBacklog).unwrap();
    event_loop.request_continuation(NativeContinuationReason::AstreaPublication).unwrap();
    event_loop.request_continuation(NativeContinuationReason::CommitTimingPlanning).unwrap();
    event_loop.request_continuation(NativeContinuationReason::XwaylandContinuation).unwrap();

    let wakeup = event_loop.wait().unwrap();
    assert!(wakeup.reasons.runtime_continuation());
    assert!(wakeup.continuation.contains(NativeContinuationReason::InputBacklog));
    assert!(wakeup.continuation.contains(NativeContinuationReason::XwaylandContinuation));
    assert_eq!(event_loop.continuation_requests(), 4);
    assert_eq!(event_loop.continuation_coalesced(), 3);
}

#[test]
fn continuation_and_real_fd_readiness_share_one_epoll_turn() {
    let input = event_fd();
    let mut event_loop = NativeEventLoop::new().unwrap();
    event_loop
        .register(input.as_raw_fd(), NativeEventSource::Input(0))
        .unwrap();
    event_loop
        .request_continuation(NativeContinuationReason::InputBacklog)
        .unwrap();
    signal(input.as_raw_fd());

    let wakeup = event_loop.wait().unwrap();
    assert!(wakeup.reasons.runtime_continuation());
    assert!(wakeup.reasons.input());
    assert!(wakeup.continuation.contains(NativeContinuationReason::InputBacklog));
}

#[test]
fn consumed_timer_is_not_still_armed() {
    let mut event_loop = NativeEventLoop::new().unwrap();
    event_loop
        .arm_deadline(Some(monotonic_now_ns().unwrap()))
        .unwrap();

    let wakeup = event_loop.wait().unwrap();
    assert!(wakeup.timer_lateness_ns.is_some());
    assert_eq!(event_loop.armed_deadline_ns(), None);
}
```

The combined-readiness test must assert the real source and continuation source both appear; it must not rely on ordering.

- [ ] **Step 2: Run the event-loop RED tests.** Run `rtk cargo test --lib native::event_loop::tests::continuation` and the timer-state filter. Expected result: the new API/tests fail because the internal eventfd and wake fields do not exist.

- [ ] **Step 3: Add the internal eventfd.** Create it with `EFD_CLOEXEC | EFD_NONBLOCK`, register it internally as `NativeEventSource::RuntimeContinuation`, reject external registration of both `Timer` and `RuntimeContinuation`, and retain no per-request allocation. Add a dedicated `WakeReasons` bit and a `NativeWakeup` continuation bitset.

- [ ] **Step 4: Implement coalescing and draining.** OR reason bits before signaling. Write one `u64` only when not already signaled; preserve reason bits and count coalescing on already-signaled/EAGAIN. On wake, drain once, take and clear the bitset, and clear the signaled flag. Keep all other ready event records in the same `NativeWakeup`.

- [ ] **Step 5: Make timer state truthful.** Capture `fired_deadline_ns` before draining, drain the one-shot timer, clear active `armed_deadline_ns`, and calculate lateness from the captured fired deadline. Expose only the active state through any accessor used by tests/metrics.

- [ ] **Step 6: Run event-loop GREEN tests and existing event-loop tests.** Use `rtk cargo test --lib native::event_loop`; verify no busy loop and no warning/error output.

- [ ] **Step 7: Commit.**

```bash
rtk git add src/native/event_loop.rs
rtk git commit -m "feat: add coalesced native continuation readiness"
```

### Task 4: Migrate runtime immediate work to continuation reasons

**Files:**

* Modify: `src/native_output/runtime/cycle.rs`.
* Modify: `src/native_output/runtime/cycle_dispatch.rs` if bounded input completion needs to request the next turn at the narrowest existing boundary.
* Modify: `src/native_output/runtime/work_domains.rs`.
* Modify: `src/native_output/runtime/xwayland.rs`.
* Modify: `src/native_output/runtime/session_io.rs`.
* Modify: `src/native_output/runtime/bootstrap.rs`.
* Modify: `src/native_output/runtime/mod.rs`.
* Modify: `src/native_output/input/epoch.rs` only if a read-only continuation query is required; preserve its state transitions and budget.

**Interfaces:**

* Consume: `NativeWakeup.continuation`, event-loop request/take methods, fixed wake plan, and current `NativeWorkDomains`.
* Produce: immediate continuation for input backlog, Astrea publication, commit-timing planning, and XWayland continuation; real-only deadline aggregation at every runtime rearm path.

- [ ] **Step 1: Write RED runtime-domain tests.** Update/add tests proving continuation reasons, not `WakeReasons::TIMER`, cause input/Astrea/commit planning service; prove no continuation is requested once input debt is gone; prove a continuation and DRM/KMS readiness are both classified.

- [ ] **Step 2: Run the domain RED tests.** Run `rtk cargo test --lib native_output::runtime::work_domains` and the XWayland reactor test filter. Expected failure is missing continuation classification/request behavior.

- [ ] **Step 3: Migrate input backlog.** Keep `NATIVE_INPUT_DRAIN_BUDGET`, `NativeInputEpoch::begin/finish`, and the current bounded `drain_events_into` behavior. After `finish`, request one `InputBacklog` continuation only when `backlog_pending()` remains true. Do not write once per input event and do not require a new libinput fd event for materialized backlog.

- [ ] **Step 4: Migrate Astrea and commit-timing planning.** Set `NativeRuntimeState` due flags from the continuation reasons as well as any existing readiness where appropriate. Remove `then_some(now_ns)`/`client_pacing_now_ns()` planning timestamps from timer aggregation. Keep the resulting presentation constraint/future pacing boundary as a real deadline after planning.

- [ ] **Step 5: Migrate XWayland immediate continuation.** Replace the direct `arm_deadline(Some(monotonic_now_ns()?))` path with `request_continuation(XwaylandContinuation)`. Leave startup/backoff/adoption/resize timeouts on their real deadline APIs.

- [ ] **Step 6: Route bootstrap, rearm, suspended, and shutdown-safe paths through one real-deadline helper.** Preserve shutdown-specific 50 ms/restore/watchdog behavior. Immediate suspended Astrea publication uses continuation; external blockers remain their fds/timeouts.

- [ ] **Step 7: Run GREEN focused suites and commit.**

```bash
rtk cargo test --lib native_output::runtime::work_domains
rtk cargo test --lib native_output::runtime::xwayland_reactor
rtk git add src/native_output/runtime src/native_output/input
rtk git commit -m "fix: migrate native immediate work to continuation readiness"
```

### Task 5: Make runtime timer aggregation decision-aware

**Files:**

* Modify: `src/native_output/runtime/cycle.rs`.
* Modify: `src/native_output/runtime/cycle/pageflip.rs`.
* Modify: `src/native_output/runtime/presentation_cycle.rs`.
* Modify: `src/native_output/runtime/presentation_pipeline.rs` only if it needs a stable current pipeline snapshot accessor.
* Modify: `src/native_output/runtime/planner.rs` to stop exposing raw visual deadline ownership to runtime aggregation, retaining target planning behavior.
* Modify: `src/native_output/runtime/metrics.rs` to remove `arm_runtime_deadline` orchestration and raw visual/immediate timestamp imports.
* Modify: `src/native_output/runtime/session_io.rs` and `bootstrap.rs` to use the same helper for their rearm paths.

**Interfaces:**

* Consume: current `ExplicitAtomicSchedulerDecision`, current pipeline snapshot, independent real owner deadlines, and continuation request API.
* Produce: exactly one `NativeWakePlan` at every runtime arm boundary and no context-free `frame_scheduler.next_deadline_ns()`/`visual_target_deadline_for_target()` ownership in runtime orchestration.

- [ ] **Step 1: Write RED integration tests for stale deadline ownership.** Add tests around the existing presentation-cycle/scheduler fixture for:

```text
expired target + WaitForWorkerQueue -> no visual timer and zero stale rearm
expired target + WaitForPageFlip -> no visual timer; watchdog allowed
future WaitForRefresh -> exact render-start timer
ready frame at now=4 ms -> exact submit_not_before=5 ms, never old render-start=3 ms
actionable Render/RenderAhead/SubmitReady/SubmitReadyLate/CompleteProtocolOnly -> no rediscovery timer
```

Assert the selected owner and timestamp, not only `Option<u64>`.

- [ ] **Step 2: Run the integration RED tests.** Run the native scheduler/presentation focused filters with `rtk cargo test`; expected failure is the current independent raw aggregation selecting stale visual deadlines.

- [ ] **Step 3: Add a runtime helper that builds current wake inputs.** The helper obtains the current pipeline/decision at the same point used for presentation action, then adds only genuine independent deadlines: atomic watchdog, explicit-sync fallback, real XWayland timeout, cursor response, control timeout, future surface pacing, and DMA-BUF retry. Use `NativeDeadlineOwner` for diagnostics.

- [ ] **Step 4: Remove raw visual aggregation.** Delete the `visual_target_deadline_for_target` use from `arm_runtime_deadline`/metrics and stop merging `frame_scheduler.next_deadline_ns()` without current pipeline context. Keep scheduler-local APIs used by scheduler tests and planning only.

- [ ] **Step 5: Handle actionable and terminal decisions before rearming.** `Render`, `RenderAhead`, submit, and protocol-only decisions execute in the current cycle; terminal/error decisions are handled before any rearm. Blocked worker/pageflip/buffer decisions wait on readiness plus only genuine watchdogs.

- [ ] **Step 6: Recompute after ownership transitions.** Call the helper after render completion, ready-frame creation, worker queueing/acknowledgment, kernel submission, pageflip, target replanning, and buffer changes. Do not cache a scheduler decision across these transitions.

- [ ] **Step 7: Run GREEN scheduler/event-loop/presentation tests and commit.**

```bash
rtk cargo test --lib native::scheduler
rtk cargo test --lib native_output::runtime::presentation
rtk cargo test --lib native_output::runtime::cycle
rtk git add src/native/scheduler.rs src/native/scheduler/pipeline.rs src/native_output/runtime
rtk git commit -m "fix: bind native timer ownership to pipeline decisions"
```

### Task 6: Add wake-authority and active-pageflip observability

**Files:**

* Modify: `src/native_output/pacing.rs`.
* Modify: `src/native_output/runtime/metrics.rs`.
* Modify: `src/native_output/runtime/cycle/pageflip.rs` and runtime rearm helper only where counters must be observed.
* Modify: `src/native/event_loop.rs` accessors only if runtime metrics require a stable bounded snapshot.

**Interfaces:**

* Consume: `NativeWakePlan`, continuation event-loop counters, current/fired timer state, existing `BoundedSamples`, and existing `is_active_refresh_interval` policy.
* Produce: compatibility counters plus runtime arms/disarms, continuation requests/coalescing/wakes, per-source continuation counts, stale/past arms, deadline-owner counts, and active pageflip percentile fields.

- [ ] **Step 1: Write RED metrics tests.** Add tests for same expired deadline rearm accounting, one-shot timer state, and the required pageflip sequence:

```rust
for interval in [6060, 6061, 12120, 60000, 6060] {
    // feed timestamps using the existing note_pageflip API
}
assert_eq!(legacy_samples_semantics, expected_legacy);
assert_eq!(active_percentiles, expected_without_idle_gap);
assert_eq!(idle_intervals_excluded, 1);
```

Add tests asserting the shutdown summary contains the new fields without per-wake output.

- [ ] **Step 2: Run metrics RED tests.** Run `rtk cargo test --lib native_output::pacing`; expected failure is absent active samples/counters/summary fields.

- [ ] **Step 3: Add bounded counters and owner accounting.** Keep all counters fixed `u64` fields or fixed arrays. Increment arm/disarm, continuation request/coalesce/wake, migrated-source continuation, stale rearm, past arm, and deterministic owner fields at the plan/event-loop boundaries. Do not let metrics alter the selected plan.

- [ ] **Step 4: Add active pageflip samples.** Add `active_pageflip_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>`. In `note_pageflip`, record all intervals in the legacy set, record active intervals only if `is_active_refresh_interval` accepts them, and retain `idle_intervals_excluded`. Add active p50/p95/p99 fields using the same percentile implementation.

- [ ] **Step 5: Emit one bounded shutdown summary.** Add `event=native_wake_authority_summary` through the existing summary path when enabled. Do not add stdout in per-wake or per-frame paths.

- [ ] **Step 6: Run GREEN metrics/pacing and native output regression filters, then commit.**

```bash
rtk cargo test --lib native_output::pacing
rtk cargo test --lib native_output::tests
rtk git add src/native_output/pacing.rs src/native_output/runtime/metrics.rs src/native_output/runtime src/native/event_loop.rs
rtk git commit -m "obs: expose native wake authority and active cadence"
```

### Task 7: Full regression verification and self-review

**Files:**

* Modify only if a test expectation precisely documents the new wake source: existing focused test files under `src/native`, `src/native_output`, `tests`, and compositor pacing tests.

- [ ] **Step 1: Re-read the spec and this plan.** Check every required RED test, timer-owner category, immediate source, fairness rule, and non-regression constraint against the implementation.

- [ ] **Step 2: Search for forbidden stale/immediate timer paths.** Run:

```bash
rtk rg -n --hidden --glob '!.git/**' --glob '!target/**' 'arm_deadline\(|then_some\(now_ns\)|then\(monotonic_now_ns\)|client_pacing_now_ns\(\)|visual_target_deadline_for_target|next_commit_timing_planning_deadline_ns|next_surface_pacing_deadline_ns' src
```

Review every remaining hit and classify it as a real future deadline, shutdown-specific timeout, test, protocol timestamp, or non-runtime clock read. Block completion if input backlog, XWayland continue-now, Astrea publication, commit-timing planning, or a blocked visual pipeline still arms the timer.

- [ ] **Step 3: Run the focused suites requested by the user.** Cover native scheduler, pipeline, triple buffering, O1, presentation deadline, KMS worker, event loop, input epoch/batching/routing, pointer constraints, surface pacing, commit timing, DMA-BUF GPU release, output transactions, session/VT, and shutdown.

- [ ] **Step 4: Run the mandatory fresh static verification.** Use exactly:

```bash
rtk cargo fmt --check
rtk cargo check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk cargo test
git diff --check
git status --short
```

Record exact pass/fail counts and identify any unrelated pre-existing failure without suppressing it.

- [ ] **Step 5: Review the final diff for the non-negotiable failure modes.** Confirm no deadline clamp, timer-based input/XWayland/Astrea/commit-planning continuation, blocked visual deadline, stale cached decision, eventfd spin/unbounded writes, continuation starvation, or active percentile idle-gap contamination.

- [ ] **Step 6: Commit verification/report artifacts.** Do not claim native qualification from unit tests. Create the report only with evidence available from the checkout and verification commands, then commit:

```bash
rtk git add REPORT-2026-09-01-typhon-native-wake-authority-closure.md
rtk git commit -m "docs: report native wake authority closure"
```

### Task 8: Native qualification handoff

**Files:**

* No source changes expected.
* Modify: `REPORT-2026-09-01-typhon-native-wake-authority-closure.md` only if the user supplies an actual qualifying native run.

- [ ] **Step 1: Do not run native DRM/KMS qualification from an environment without a TTY/DRM seat.** Do not use ydotool, desktop screenshots, or destructive system commands.

- [ ] **Step 2: If the user performs the real run, record the exact command, workload, and observed counters.** Check repeated timer wakes, expired waits, stale re-arms, continuation counts, active pageflip percentiles, O1, KMS worker, output transactions, DMA-BUF, protocol safety, and shutdown KMS-safe boundary.

- [ ] **Step 3: Update the report with native evidence only when actually supplied.** Explicitly separate unit/static evidence from native qualification evidence and list remaining limitations.
