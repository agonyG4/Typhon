# KMS Worker Pacing ABA Exact Reservation Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make KMS worker pacing success and terminal events settle only the exact asynchronous reservation that created them, including same-logical-ID Predictive O1 overlap.

**Architecture:** Add a dedicated per-`NativeFramePacing` reservation-ID sequence and immutable `WorkerPacingTicket`. Store exact active/ready lane affiliations separately from scheduler frame IDs; move the affiliation during same-work active→ready migration and detach it when a new render attempt reuses the active logical frame. Transport the ticket through `KmsCommitJob` and every worker terminal path.

**Tech Stack:** Rust, Cargo, existing Typhon native KMS worker/runtime, white-box unit tests, `rtk` command proxy.

## Global Constraints

- Preserve `PredictiveO1AttemptId` as a namespace distinct from `NativeOutputFrameId`.
- Preserve immutable `OutputFrameKey` physical identity and its validation behavior.
- Keep `PREDICTIVE_O1_LIFECYCLE_CAPACITY` at `4`.
- Do not change fullscreen, CU-DAG, screenshot, effects, Eclipse, worker-disable, or triple-buffer-disable behavior.
- Work directly in the current shared worktree and preserve unrelated dirty files.
- Use `rtk` for shell commands where available; do not spawn subagents.
- Compile in the repository directory so build artifacts remain on the same SSD.
- Create focused commits, staging only task files.

---

### Task 1: Add the failing same-logical-ID worker ABA regression

**Files:**
- Modify: `src/native_output/pacing.rs` test module near the existing worker reservation tests
- Modify: `src/native_output/predictive_o1_tests.rs` only if the complete lifecycle assertions need the existing physical helper

**Interfaces:**
- Consumes: current `NativeFramePacing` test API and `OutputFrameIdentitySnapshot` helper.
- Produces: a test that requires `reserve_worker_submission` to return a distinct exact ticket while the logical frame ID is reused.

- [ ] **Step 1: Add the production-order RED test**

Add a focused test that performs this exact sequence:

```rust
let mut pacing = NativeFramePacing::from_env();
pacing.enabled = true;
pacing.queue_visual(1, 1);
pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, false);
let predecessor_ticket = pacing
    .reserve_worker_submission(false)
    .expect("predecessor reservation")
    .expect("predecessor ticket");

// The successor intentionally reuses the still-active logical L.
pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true)
    .expect("predictive successor attempt");
let successor_attempt = pacing.active_predictive_attempt.expect("successor attempt");
let successor_physical = physical_identity(5_263);
pacing.bind_predictive_o1(successor_physical).expect("bind successor");
pacing.note_render_ready();
pacing.note_ready_frame(2, true);

assert_eq!(pacing.ready, Some(predecessor_ticket.frame_id()));
assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));
assert_eq!(pacing.ready_physical_key, Some(OutputFrameKey::from(&successor_physical)));
assert_ne!(pacing.ready_worker_reservation_id, Some(predecessor_ticket.reservation_id()));

pacing
    .note_worker_submit_exact(
        Some(predecessor_ticket),
        41,
        3,
        NativeOutputPacingMode::PredictiveTriple,
    )
    .expect("predecessor worker success");

assert_eq!(pacing.pending, Some(predecessor_ticket.frame_id()));
assert!(pacing.pending_predictive_attempt.is_none());
assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));
assert_eq!(pacing.ready_physical_key, Some(OutputFrameKey::from(&successor_physical)));
```

Use the wished-for immutable ticket API; do not make the test pass by comparing a frame ID or by querying a second snapshot from pacing.

- [ ] **Step 2: Extend the RED test through both pageflips**

After predecessor success, pageflip the predecessor physical identity and assert the successor lifecycle remains live. Reserve the ready successor as a second ticket, submit it, pageflip its physical identity, and assert:

```rust
assert_eq!(pacing.predictive_o1_presented, 1);
assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
```

- [ ] **Step 3: Add the cancellation ABA case**

Repeat the overlap setup, call `cancel_worker_submission(Some(predecessor_ticket))`, and assert the successor remains ready with its attempt and physical key. Then cancel the successor ticket and assert the lifecycle drains exactly once.

- [ ] **Step 4: Run the new tests before production edits**

Run:

```bash
rtk cargo test --locked native_output::pacing::tests::worker_success_does_not_clear_same_logical_id_predictive_successor
rtk cargo test --locked native_output::pacing::tests::worker_cancel_does_not_clear_same_logical_id_predictive_successor
```

Expected: compilation/test failure because the wished-for ticket return type and exact settlement behavior do not exist; the old implementation must fail the behavioral assertions rather than a test typo.

- [ ] **Step 5: Commit only the RED tests**

```bash
rtk git add src/native_output/pacing.rs src/native_output/predictive_o1_tests.rs
rtk git commit -m "test(pacing): reproduce worker ABA on reused logical frame id"
```

### Task 2: Implement exact pacing reservation ownership

**Files:**
- Modify: `src/native_output/pacing.rs`

**Interfaces:**
- Consumes: Task 1 RED tests and existing active→ready/stale reservation tests.
- Produces: `WorkerPacingReservationId`, `WorkerPacingTicket`, `reserve_worker_submission(ready_submit) -> Result<Option<WorkerPacingTicket>, _>`, `note_worker_submit_exact(Option<WorkerPacingTicket>, token, now_ns, pacing_mode)`, and `cancel_worker_submission(Option<WorkerPacingTicket>)`.

- [ ] **Step 1: Add the dedicated reservation ID and sequence**

Define near the existing pacing identity types:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WorkerPacingReservationId(u64);

impl WorkerPacingReservationId {
    pub(crate) const fn get(self) -> u64 { self.0 }
}
```

Add a private `WorkerPacingReservationIdSequence { next: u64 }` with the same nonzero, wrap-to-one behavior as the existing identity sequences. Add it to `NativeFramePacing` and initialize it to `1`.

- [ ] **Step 2: Replace the split reservation snapshot with an immutable ticket**

Replace `WorkerPacingReservation` with a `pub(crate)` `WorkerPacingTicket` containing the reservation ID, `NativeOutputFrameId`, captured `ready_submit`, predictive attempt, physical identity, and physical key. Derive `Debug, Clone, Copy, PartialEq, Eq` and expose read-only accessors for runtime logging and rejection handling. Keep all fields immutable after construction.

- [ ] **Step 3: Add exact lane affiliations and initialize them**

Add `active_worker_reservation_id` and `ready_worker_reservation_id` to `NativeFramePacing`, initialized to `None`. The one outstanding-ticket slot remains, but lane mutation authority is represented by these exact IDs rather than by comparing `NativeOutputFrameId` values.

- [ ] **Step 4: Make reservation allocation return the full ticket**

Change `reserve_worker_submission` to allocate a fresh reservation ID, capture all source payload in one ticket, store it, and set only the selected lane affiliation. Return `Option<WorkerPacingTicket>`. Remove production use of `worker_submission_output_identity`, `worker_submission_output_key`, and frame-ID-based worker snapshot queries.

- [ ] **Step 5: Preserve migration and detach replacement work**

In `note_ready_frame`, move `active_worker_reservation_id` to `ready_worker_reservation_id` as part of the active→ready transfer. In `begin_render_attempt`, clear the active lane affiliation before installing the new attempt origin/attempt ID. This ensures a new predictive attempt on the same active logical frame cannot inherit the predecessor ticket while a same-work migration retains it.

- [ ] **Step 6: Consume by exact ticket ID and ticket payload**

Replace `take_worker_submission_frame(expected)` with an exact ticket operation. It must reject a missing/mismatched ID, emit a trace event containing returned/current reservation IDs, remove only the matching outstanding ticket, clear only lanes affiliated with that ID, clear only timing owned by those lanes, and return the captured ticket payload. No mutation decision may use frame-ID equality.

- [ ] **Step 7: Build pending state from the consumed ticket**

Make `note_worker_submit_exact` pass the consumed ticket’s frame ID, physical identity, predictive attempt, physical key, and captured `ready_submit` to `note_submit_frame`. Record ready-wait timing before consumption only when the exact ticket owns the ready lane. Keep ordinary non-worker `note_submit` timing behavior local to its current lane so asynchronous worker completion cannot clear newer colliding timing state.

- [ ] **Step 8: Make cancellation consume the exact ticket**

Change cancellation to accept only `Option<WorkerPacingTicket>`, consume by reservation ID, terminalize Predictive O1 using the ticket payload, clear only exact-lane timing/state, and log reservation ID plus captured metadata. `None` remains the valid no-pacing-owner case.

- [ ] **Step 9: Add trace observability**

For reservation, worker submit, worker cancellation, and stale ticket events, include `worker_pacing_reservation_id`, logical `frame_id`, captured `ready_submit`, predictive attempt ID, and physical output frame ID when present. Keep all output behind the existing trace sink.

- [ ] **Step 10: Run pacing tests GREEN**

Run:

```bash
rtk cargo test --locked native_output::pacing::tests
rtk cargo test --locked native_output::predictive_o1_tests
```

Expected: Task 1 and all existing same-work migration/stale tests pass without lifecycle capacity changes.

- [ ] **Step 11: Commit the focused production pacing fix**

```bash
rtk git add src/native_output/pacing.rs
rtk git commit -m "fix(pacing): carry exact worker reservation ownership"
```

### Task 3: Transport tickets through KMS and direct worker paths

**Files:**
- Modify: `src/native_output/kms_worker/payload.rs`
- Modify: `src/native_output/runtime/presentation_ready.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/kms_worker.rs`
- Modify: `src/native_output/runtime/direct_rejection.rs`
- Modify: `src/native_output/runtime/kms_worker/rejection.rs`
- Modify: `src/native_output/runtime/kms_worker_teardown.rs` only if its terminal delegation needs explicit ticket handling

**Interfaces:**
- Consumes: `WorkerPacingTicket` API from Task 2.
- Produces: `KmsCommitJob.pacing_ticket: Option<WorkerPacingTicket>` and exact ticket transport for explicit, compatibility, direct, rejection, replan, quiesce, fatal, and shutdown paths.

- [ ] **Step 1: Replace KMS job split fields**

Import `WorkerPacingTicket` into `payload.rs` and replace `pacing_frame_id` plus `predictive_output_identity` with `pacing_ticket`. Keep `ready_submit` only if KMS behavior needs the independent flag; assert/use the ticket’s captured value for pacing settlement.

- [ ] **Step 2: Capture one ticket in explicit and compatibility admission**

In `submit_explicit_ready_for_presentation` and the compatibility branch, call `reserve_worker_submission` once, pass the returned ticket to queue helpers, construct `KmsCommitJob` with that ticket, and use the same ticket for every immediate error/unavailable rollback. Remove all second identity queries.

- [ ] **Step 3: Carry the ticket in direct admission and rollback guard**

In `finish_direct_worker_queued`, reserve once after the job’s backend data is valid, assign the ticket to the job, and make `DirectWorkerAdmissionGuard` retain the `Option<WorkerPacingTicket>`. Its rollback must call `cancel_worker_submission(ticket)` exactly; it must not reconstruct from frame ID or `ready_submit`.

- [ ] **Step 4: Settle success from the job ticket**

In `process_kms_worker_event` for `Submitted`, pass `ownership.job.pacing_ticket` directly to `note_worker_submit_exact`. The pacing pending state must come from that ticket even if current active/ready state has a same-logical-ID successor.

- [ ] **Step 5: Update all known rejection/cancellation paths**

Replace pacing cancellation arguments in direct rejection and KMS rejection helpers (`fail_queued_worker_job`, `drop_queued_worker_job_with_reason_parts`, `replan_invalidated_worker_job`, and wrappers) with `job.pacing_ticket`. Read predictive physical identity for safe-abandonment accounting from the ticket accessors, not a job-level duplicate.

- [ ] **Step 6: Audit teardown and fatal retention**

Verify that known-failure/drop paths invoked by `handle_fatal_worker_jobs`, quiesce, `join_kms_worker`, and shutdown preserve the `KmsCommitJob` unchanged until the exact ticket is consumed or the job is intentionally quarantined. No teardown path may downgrade the job ticket to a frame ID.

- [ ] **Step 7: Update all job constructors and tests**

Replace every `KmsCommitJob` initializer’s old pacing fields with `pacing_ticket: None` or the captured ticket. Do not touch unrelated transaction or cursor ownership fields.

- [ ] **Step 8: Run transport and rejection tests**

Run:

```bash
rtk cargo test --locked native_output::runtime::kms_worker_tests
rtk cargo test --locked native_output::runtime::cycle::pageflip_tests
rtk cargo test --locked native_output::tests::frame
```

Expected: compilation and focused tests pass, including existing worker queue/rejection ownership tests.

### Task 4: Add integration regressions, timing coverage, and stress

**Files:**
- Modify: `src/native_output/pacing.rs` test module for migration, stale, timing, and long overlap tests
- Modify: `src/native_output/runtime/kms_worker_tests.rs` for real job transport/rejection coverage, or the narrowest existing worker test module that can construct the payload
- Modify: `src/native_output/predictive_o1_tests.rs` only for complete lifecycle/stress helpers

**Interfaces:**
- Consumes: exact ticket API and `KmsCommitJob.pacing_ticket`.
- Produces: deterministic regression coverage proving no stale worker terminal path mutates newer same-logical-ID work.

- [ ] **Step 1: Preserve same-work active→ready settlement**

Retain/update `worker_submit_settles_reserved_frame_after_active_becomes_ready` and its cancellation counterpart so the same ticket moves from active affiliation to ready affiliation and still settles that work.

- [ ] **Step 2: Add stale delayed-result replacement coverage**

Reserve and terminalize R1, create R2 with a colliding logical frame ID, then deliver R1. Assert the stale ID is rejected and R2’s active/ready/pending/timing state is unchanged. Do not rely on a different logical frame ID to prove staleness.

- [ ] **Step 3: Add exact timing ownership assertions**

Create newer active or ready timing state with the same logical frame ID as an older detached ticket. Deliver success and cancellation for the old ticket and assert `active_queued_ns`, `ready_waiting_started_ns`, and `ready_waiting_frame_id` remain owned by the newer state where that path applies.

- [ ] **Step 4: Add the real `KmsCommitJob` transport test**

Construct a real job with `pacing_ticket: Some(ticket)`, move it through the existing worker ownership/result representation, and call the same pacing settlement entry point used by `process_kms_worker_event`. Assert the reservation ID, predictive attempt, and physical key survive without reconstructing from `NativeOutputFrameId`.

- [ ] **Step 5: Add a real rejection-wiring test**

Use the existing rejection helper or worker event path with R1 in the job and a newer same-logical-ID ready successor. Assert rejection consumes/cancels only R1 and leaves the successor’s attempt and physical key ready.

- [ ] **Step 6: Add deterministic 10,000-iteration stress**

For 10,000 iterations, reuse the same logical frame ID for a normal predecessor and Predictive O1 successor, allocate distinct reservation/attempt/physical IDs, alternate success and rejection/cancel, complete predecessor then successor pageflips, and assert after each iteration:

```rust
assert!(pacing.predictive_o1_lifecycle.active_entries() <= PREDICTIVE_O1_LIFECYCLE_CAPACITY);
assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
assert_eq!(pacing.worker_reservation, None);
```

At the end assert zero active lifecycle entries, exact successful-presented accounting, zero invalid transitions, and no outstanding reservation. Do not change the capacity.

- [ ] **Step 7: Run the focused integration set**

```bash
rtk cargo test --locked native_output::pacing::tests
rtk cargo test --locked native_output::predictive_o1_tests
rtk cargo test --locked native_output::runtime::kms_worker_tests
rtk cargo test --locked native_output::runtime::presentation_ready
rtk cargo test --locked native_output::runtime::cycle::pageflip
rtk cargo test --locked native_output::tests::frame
```

- [ ] **Step 8: Commit the test coverage**

```bash
rtk git add src/native_output/pacing.rs src/native_output/predictive_o1_tests.rs src/native_output/runtime/kms_worker_tests.rs
rtk git commit -m "test(native): cover worker rejection and predictive overlap stress"
```

### Task 5: Full verification and handoff

**Files:**
- Inspect only: all changed files and `docs/SOURCE_LAYOUT.md`
- Modify: none unless verification exposes a target-path defect

**Interfaces:**
- Consumes: Tasks 1–4 commits and the unchanged shared-worktree files.
- Produces: evidence-backed verification report and hardware-validation status.

- [ ] **Step 1: Check changed-file scope and whitespace**

```bash
rtk git status --short
git diff --check HEAD~3..HEAD
rtk git diff --stat HEAD~3..HEAD
```

Confirm the three unrelated pre-existing dirty files remain unstaged/uncommitted by this work.

- [ ] **Step 2: Run formatting and compilation**

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
```

- [ ] **Step 3: Run linting and full tests**

```bash
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
```

Report any unrelated shared-worktree failure exactly instead of claiming a full pass.

- [ ] **Step 4: Run the source-layout gate**

Use the repository’s source-layout command if present. Report the existing `pacing.rs` baseline exception separately; do not mix a layout refactor into this fix.

- [ ] **Step 5: Re-check graph coverage for changed paths**

After edits, query Codebase Memory for the changed pacing/KMS symbols and call `check_index_coverage` for every changed source path. If indexing is stale or partial, inspect the reported source ranges directly before making exhaustive claims.

- [ ] **Step 6: Record native hardware validation honestly**

Run the Cyberpunk native-Wayland scenario only if the hardware/session is available, with KMS worker auto, triple buffering auto, direct scanout off, and frame-pacing trace enabled. Report predictive created/presented/invalid/active and worker reservation leak results. If it cannot run here, explicitly leave it as remaining validation and do not claim the production crash is resolved on hardware.

- [ ] **Step 7: Final report**

List the ABA mechanism, exact identity/ticket model, every terminal path, test RED/GREEN evidence, stress totals, focused/full verification results, source-layout result, commits, changed files, and remaining hardware validation.
