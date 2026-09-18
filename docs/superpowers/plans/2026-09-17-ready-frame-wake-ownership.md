# Ready-Frame Wake Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task with inline review checkpoints. Sub-agents are prohibited for this task.

**Goal:** Preserve runtime ownership when an actionable native scheduler decision has no future deadline, so a physically READY frame receives a coalesced presentation continuation instead of leaving the native loop without a wake owner.

**Architecture:** Keep scheduler decisions and `SchedulerWakeDeadline` semantics unchanged. Add a runtime-only `NativeSchedulerWakeRequirement` with distinct `None`, `Deadline`, and `ImmediatePresentation` states; classify the guarded decision in the runtime, carry that typed state into the wake plan, and encode only the immediate state as the existing `FrameScheduler` eventfd continuation. Map that continuation to presentation-only work in `NativeWorkDomains`.

**Tech Stack:** Rust, Linux epoll/timerfd/eventfd, native scheduler pipeline, Cargo tests, `rtk` command proxy.

## Global Constraints

- Compile and run tests in `/home/agony/GitHub/Typhon` so build artifacts stay in the same folder.
- Preserve all existing unrelated dirty-worktree edits; stage only task-owned files.
- Use no sub-agents.
- Do not change `SchedulerDecision`, `SchedulerWakeDeadline`, `ready_submit_decision()`, or `actionable_scheduler_decisions_have_no_rediscovery_deadline`.
- Do not change deferred Predictive O1’s `max(bind_at, actual_claim.presentation_time + 100_000)` calculation or any frame/attempt/claim/target identity.
- Do not change WarmPaired, MissRecovery, render-risk, dispatch-tail, `fair_dispatch_chance`, KMS apply-guard, buffering credit, overlap, or target-selection arithmetic.
- Do not create polling, a second event source, fake past deadlines, per-cycle stderr logging, or scene work for a scheduler continuation.

---

### Task 1: Add RED runtime ownership regressions

**Files:**
- Modify: `src/native_output/runtime/wake_plan.rs` tests and runtime wake-plan types.
- Modify: `src/native_output/runtime/work_domains.rs` tests only.
- Modify: `src/native/event_loop.rs` tests only.

**Interfaces:**
- Consumes: `SchedulerDecision`, `SchedulerWakeDeadline`, `NativeDeadline`, `NativeWakePlan`, existing eventfd continuation machinery, and `NativeWorkDomains`.
- Produces: failing tests for future READY deadline ownership, boundary crossing, explicit TOCTOU ownership, presentation-only mapping, lane/worker external ownership, and frame-scheduler continuation coalescing.

- [ ] **Step 1: Add the runtime requirement and wake-plan assertions before its implementation exists**

Add tests in `wake_plan.rs` using one logical `PresentationTarget` with `submit_not_before = 5_000_000`:

```rust
#[test]
fn future_ready_frame_owns_submit_boundary_deadline() {
    let requirement = scheduler_wake_requirement_for_action(
        SchedulerDecision::WaitForRefresh,
        Some(SchedulerWakeDeadline {
            kind: SchedulerWakeDeadlineKind::SubmitNotBefore,
            at_ns: 5_000_000,
        }),
        NativePageflipTimeoutOwner::MainThread,
    );

    assert_eq!(
        requirement,
        NativeSchedulerWakeRequirement::Deadline(NativeDeadline {
            owner: NativeDeadlineOwner::PresentationTarget,
            at_ns: 5_000_000,
        })
    );
    let plan = build_native_wake_plan(NativeWakePlanInputs {
        scheduler_wake_requirement: requirement,
        ..NativeWakePlanInputs::default()
    });
    assert_eq!(plan.deadline.map(|deadline| deadline.at_ns), Some(5_000_000));
    assert!(!plan
        .continuation
        .contains(NativeContinuationReason::FrameScheduler));
}
```

Add the matching boundary test:

```rust
#[test]
fn actionable_ready_frame_owns_immediate_presentation_continuation() {
    let action = SchedulerDecision::SubmitReady;
    let requirement = scheduler_wake_requirement_for_action(
        action,
        None,
        NativePageflipTimeoutOwner::MainThread,
    );

    assert_eq!(
        requirement,
        NativeSchedulerWakeRequirement::ImmediatePresentation { action }
    );
    let plan = build_native_wake_plan(NativeWakePlanInputs {
        scheduler_wake_requirement: requirement,
        ..NativeWakePlanInputs::default()
    });
    assert_eq!(plan.deadline, None);
    assert!(plan
        .continuation
        .contains(NativeContinuationReason::FrameScheduler));
}
```

Add an explicit two-observation TOCTOU test asserting the first plan is a
`SubmitNotBefore` deadline and the second plan is not `deadline=None` plus an
empty continuation:

```rust
#[test]
fn ready_frame_boundary_transition_retains_a_runtime_wake_owner() {
    let first = build_native_wake_plan(NativeWakePlanInputs {
        scheduler_wake_requirement: NativeSchedulerWakeRequirement::Deadline(
            NativeDeadline {
                owner: NativeDeadlineOwner::PresentationTarget,
                at_ns: 5_000_000,
            },
        ),
        ..NativeWakePlanInputs::default()
    });
    let second = build_native_wake_plan(NativeWakePlanInputs {
        scheduler_wake_requirement: NativeSchedulerWakeRequirement::ImmediatePresentation {
            action: SchedulerDecision::SubmitReady,
        },
        ..NativeWakePlanInputs::default()
    });

    assert_eq!(first.deadline.map(|deadline| deadline.at_ns), Some(5_000_000));
    assert_eq!(second.deadline, None);
    assert!(second.continuation.contains(NativeContinuationReason::FrameScheduler));
}
```

- [ ] **Step 2: Add lane and worker ownership regressions**

Add tests proving the mapping is performed after the Atomic guard and that
worker readiness remains external:

```rust
#[test]
fn lane_blocked_ready_frame_does_not_request_scheduler_continuation() {
    let guarded = apply_atomic_commit_lane_guard(
        SchedulerDecision::SubmitReady,
        true,
        false,
    );
    assert_eq!(guarded, SchedulerDecision::WaitForPageFlip);
    assert_eq!(
        scheduler_wake_requirement_for_action(
            guarded,
            None,
            NativePageflipTimeoutOwner::MainThread,
        ),
        NativeSchedulerWakeRequirement::None
    );
}

#[test]
fn worker_queue_wait_does_not_request_scheduler_continuation() {
    assert_eq!(
        scheduler_wake_requirement_for_action(
            SchedulerDecision::WaitForWorkerQueue,
            None,
            NativePageflipTimeoutOwner::KmsWorker,
        ),
        NativeSchedulerWakeRequirement::None
    );
}
```

- [ ] **Step 3: Add presentation-only domain and eventfd coalescing regressions**

In `work_domains.rs`, construct a `NativeWakeup` whose continuation contains
`FrameScheduler` and assert `presentation` and `presentation_due` are true,
while `scene`, `visual_scene_debt`, and `service_acquire_and_prepare` are
false. In `event_loop.rs`, request `FrameScheduler` twice before `wait()` and
assert one continuation wake, two requests, one coalesced request, and the
reason in the returned bitset.

- [ ] **Step 4: Run the RED tests**

Run:

```bash
rtk cargo test --locked wake_plan
```

Expected: compile failure naming the missing `NativeSchedulerWakeRequirement`,
`scheduler_wake_requirement_for_action`, `FrameScheduler` bit, or wake-plan
input field. Correct only test setup errors; do not add production behavior in
this step.

---

### Task 2: Implement typed runtime requirement and wake-plan propagation

**Files:**
- Modify: `src/native_output/runtime/wake_plan.rs`.
- Modify: `src/native_output/runtime/mod.rs` exports.
- Modify: `src/native_output/runtime/metrics.rs` current scheduler wake method,
  wake-plan installation, and deadline-only pacing metric projection.
- Modify: `src/native_output/runtime/scene_liveness.rs` scheduler ownership and
  wake-plan input.
- Modify: `src/native_output/runtime/bootstrap.rs` initial wake-plan field name.
- Modify: `src/native_output/runtime/cycle.rs` shutdown/suspended wake-plan field
  name and scene-debt ownership check.

**Interfaces:**
- Consumes: guarded scheduler decisions and existing deadline-owner helpers.
- Produces: `NativeSchedulerWakeRequirement`,
  `scheduler_wake_requirement_for_action()`, and wake plans that carry either a
  deadline or a frame-scheduler continuation.

- [ ] **Step 1: Implement the minimal typed requirement**

Define in `wake_plan.rs`:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum NativeSchedulerWakeRequirement {
    #[default]
    None,
    Deadline(NativeDeadline),
    ImmediatePresentation { action: SchedulerDecision },
}
```

Add `is_none()` and `deadline()` accessors as `const fn`s. Add
`scheduler_wake_requirement_for_action(action, deadline, timeout_owner)` that:

- converts `WaitForRefresh` to a deadline when one exists;
- accepts only `PageFlipWatchdog` deadlines for `WaitForBuffer` and
  `WaitForPageFlip`, subject to the existing timeout-owner filter;
- returns `None` for `WaitForWorkerQueue` and `Idle`;
- returns `ImmediatePresentation { action }` for `Render`, `RenderAhead`,
  `SubmitReady`, `SubmitReadyLate`, `ReadyTargetInvalidated`,
  `CompleteProtocolOnly`, and `PageFlipWatchdogExpired`.

An absent deadline on a waiting action remains `None`; it never becomes a fake
deadline.

- [ ] **Step 2: Carry the requirement into `NativeWakePlanInputs`**

Replace the scheduler-specific `scheduler_deadline` input with
`scheduler_wake_requirement`. Add `primary_deadline: Option<NativeDeadline>`
for shutdown/bootstrap call sites that need a non-scheduler primary timer. In
`build_native_wake_plan()`, insert `FrameScheduler` when the typed requirement
is `ImmediatePresentation`, use its deadline when it is `Deadline`, and merge
`primary_deadline` with all other deadline owners through `earliest_deadline`.
Keep `NativeWakePlan.deadline` as `Option<NativeDeadline>`; the typed
distinction is represented in the input and continuation bit, not by a timer
value.

- [ ] **Step 3: Change the runtime method to classify after all guards**

Rename `current_scheduler_wake_deadline()` to
`current_scheduler_wake_requirement()` and keep its decision computation
unchanged. In the explicit Atomic branch, call
`decision_with_pipeline_diagnostics()`, apply
`apply_atomic_commit_lane_guard()`, then classify the guarded action using the
raw diagnostic wake deadline. In the compatibility branch, classify the
existing `decision_with_context()` result using the existing target/watchdog
deadline construction. Do not change any scheduler or O1 code.

- [ ] **Step 4: Update runtime consumers without changing pacing semantics**

`arm_runtime_deadline()` passes the typed requirement into the wake plan. The
scene-debt continuation condition uses `scheduler_wake_requirement.is_none()`
so an immediate frame-scheduler owner suppresses only the duplicate scene
continuation while leaving independently dirty scene state intact.

`update_cycle_metrics()` projects only
`requirement.deadline().map(|deadline| deadline.at_ns)` into the existing
`note_deadline_state()` call. This preserves the scheduler-level
`wake_deadline=None` contract and avoids treating a continuation as a pacing
deadline.

- [ ] **Step 5: Run the wake-plan and compilation tests**

Run:

```bash
rtk cargo test --locked wake_plan
rtk cargo check --locked --all-targets
```

Expected: Task 1 ownership tests pass; existing wake-plan tests continue to
pass; compilation succeeds without touching unrelated dirty files.

---

### Task 3: Wire the continuation reason, metrics, and presentation-only domains

**Files:**
- Modify: `src/native/event_loop.rs` continuation enum/bit mapping and tests.
- Modify: `src/native_output/runtime/wake_plan.rs` authority metrics/summary and
  tests.
- Modify: `src/native_output/runtime/metrics.rs` installation loop.
- Modify: `src/native_output/runtime/work_domains.rs` classification and tests.

**Interfaces:**
- Consumes: the existing `NativeContinuationReasons` eventfd coalescing path.
- Produces: `NativeContinuationReason::FrameScheduler`, aggregate
  `frame_scheduler_continuations`, and `presentation=true` without automatic
  scene/acquire-prepare work.

- [ ] **Step 1: Add the new continuation bit**

Add `FrameScheduler` to `NativeContinuationReason`, assign a new unused bit in
`NativeContinuationReasons`, and map it in `bit()`. Do not add an event source;
`RuntimeContinuation` remains the only continuation fd.

- [ ] **Step 2: Add the existing metrics path**

Add `frame_scheduler_continuations: u64` to
`NativeWakeAuthorityMetrics`, increment it in `note_continuation()`, include
it in `summary_line()` as `frame_scheduler_continuations=...`, and include the
reason in `install_native_wake_plan()`’s existing request loop. Keep
`request_native_continuation()` coalescing behavior unchanged.

- [ ] **Step 3: Map only presentation in `NativeWorkDomains`**

Extend the `presentation` expression with
`wakeup.continuation.contains(NativeContinuationReason::FrameScheduler)`.
Leave the `scene` expression unchanged. Because
`operation_plan()` derives `service_acquire_and_prepare` from scene/explicit
sync/screen capture only, the new continuation produces
`presentation_due=true`, `scene=false`,
`service_acquire_and_prepare=false` unless independent runtime state says
otherwise.

- [ ] **Step 4: Run focused domain and event-loop tests**

Run:

```bash
rtk cargo test --locked work_domains
rtk cargo test --locked event_loop
rtk cargo test --locked wake_plan
```

Expected: presentation-only, coalescing, summary, existing scene-debt, timer,
and continuation tests pass.

---

### Task 4: Preserve scheduler contracts and verify all requested gates

**Files:**
- Modify only task-owned files from Tasks 1–3 if a verified test exposes a
  defect in this patch.
- Do not modify `src/native/scheduler.rs` or
  `src/native/scheduler/pipeline.rs` except to inspect the unchanged contract.

- [ ] **Step 1: Run the remaining focused suites**

Run:

```bash
rtk cargo test --locked scheduler
rtk cargo test --locked presentation_cycle
rtk cargo test --locked presentation_ready
rtk cargo test --locked kms_worker
rtk cargo test --locked pacing
rtk cargo test --locked native_output
```

Expected: all requested focused suites pass, including the unmodified
`actionable_scheduler_decisions_have_no_rediscovery_deadline` assertion.

- [ ] **Step 2: Run repository verification**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

Run `docs/SOURCE_LAYOUT.md`’s source-layout gate if the repository exposes an
executable gate. Record pre-existing failures separately from regressions.

- [ ] **Step 3: Review the final diff and dirty-worktree boundaries**

Confirm with `git diff --name-only`, `git diff --stat`, and focused diffs that:

- scheduler semantics and O1 timing are unchanged;
- no fake deadline or timer polling was added;
- lane-blocked and worker-occupied states have no frame-scheduler continuation;
- a physically READY actionable frame cannot produce empty deadline plus empty
  continuation with no external owner;
- continuation work is presentation-only;
- no fair=false samples are accepted by dispatch-tail code;
- unrelated existing modifications were not reset, reformatted, staged, or
  committed.

- [ ] **Step 4: Commit task-owned implementation**

Stage only the implementation files and commit:

```bash
rtk git add src/native/event_loop.rs src/native_output/runtime/cycle.rs src/native_output/runtime/metrics.rs src/native_output/runtime/mod.rs src/native_output/runtime/scene_liveness.rs src/native_output/runtime/wake_plan.rs src/native_output/runtime/work_domains.rs src/native_output/runtime/bootstrap.rs
rtk git commit -m "fix(native): preserve ready-frame wake ownership"
```

- [ ] **Step 5: Native qualification handoff**

Run the same blur-enabled native workload with Pacing v3.1 tracing enabled.
Record the command and inspect READY-frame submit lateness, `frame_scheduler_continuations`, runtime continuation requests/coalescing, scheduler wake lateness, WarmPaired/MissRecovery share, submit/render-limited frames, fast-client cadence, Predictive O1 lifecycle, and every remaining `binding=true && fair=false` sample. Unit tests alone do not establish the hardware result.
