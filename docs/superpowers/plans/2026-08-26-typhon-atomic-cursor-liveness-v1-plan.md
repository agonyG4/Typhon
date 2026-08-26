# Typhon Atomic Cursor Liveness v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use inline execution with `superpowers:executing-plans`; sub-agents are disabled by the user. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restore bounded liveness for visible atomic hardware cursor changes after input quiescence while preserving non-primary input wakes and existing KMS ownership.

**Architecture:** Add an O(1) `NativeAtomicCursor` predicate that compares desired state with the future output baseline (worker-owned, in-flight submitted, or current). Reconcile that debt into the existing `NativeCursorOutputArbitration` without adding a scheduler. Invoke the observer once at the final native input-batch synchronization boundary; presentation code continues to choose and submit the final cursor policy.

**Tech Stack:** Rust, Cargo tests, existing `NativeFrameScheduler`, atomic KMS cursor state, `rtk` command wrappers.

## Global Constraints

- Preserve all unrelated dirty work in the current branch; do not reset, clean, stash, restore, branch, or create a worktree.
- Stage only closure files in each commit.
- Keep hardware cursor movement independent from primary-scene repaint.
- Reuse `AtomicCursorVisualState::kms_equivalent()` and the existing `NativeCursorOutputArbitration`.
- Keep the input observer O(1), allocation-free, and free of scene traversal, Wayland dispatch, KMS submission, and policy selection.
- Use `NativeFrameScheduler::next_refresh_deadline_ns` for the first response deadline.
- Keep surface pacing, eager event-index formatting, shortcut inhibition, and narrow key/button full-tick behavior out of scope.
- Run all shell commands through `rtk`; do not use sub-agents.

---

### Task 1: Add the failing cursor output-debt regression tests

**Files:**
- Modify: `src/native_output/output/cursor_tests.rs`
- Modify: `src/native_output/output/cursor.rs` only after the tests fail

**Interfaces:**
- Produces: `NativeAtomicCursor::needs_output_liveness(&self) -> bool`.
- Uses: `AtomicCursorVisualState::kms_equivalent()`, the existing `test_cursor()` fixture, `PageFlipToken`, and existing submission helpers.

- [ ] **Step 1: Write the failing tests**

Add tests that describe the output debt rather than `needs_submission()`:

```rust
#[test]
fn cursor_output_liveness_uses_kms_visible_state() {
    let mut cursor = test_cursor();
    assert!(!cursor.needs_output_liveness());

    cursor.set_position(100, 200);
    assert!(!cursor.needs_output_liveness(), "hidden movement is not KMS debt");

    cursor.set_visible(true);
    assert!(cursor.needs_output_liveness(), "showing the plane is KMS debt");

    cursor.current = cursor.desired.clone();
    cursor.set_position(120, 220);
    assert!(cursor.needs_output_liveness(), "visible movement is KMS debt");
}

#[test]
fn cursor_output_liveness_survives_a_newer_state_during_inflight_submission() {
    let mut cursor = test_cursor();
    cursor.current.visible = true;
    cursor.desired.visible = true;
    cursor.set_position(100, 200);
    let submitted_epoch = cursor.desired_epoch();
    let submitted_state = cursor.desired().clone();
    let token = PageFlipToken::new(90).unwrap();

    cursor.begin_submission_at_revision(
        token,
        submitted_state,
        submitted_epoch,
        cursor.desired_revision(),
    );
    cursor.set_position(0, 0);

    assert!(cursor.needs_output_liveness(), "desired A differs from in-flight B");
}
```

Add a worker-owned variant using `queue_worker_submission()` before changing desired state, and assert that desired state is compared with the queued visual state. Keep the test fixture's existing resource ownership and transaction identity intact.

- [ ] **Step 2: Run the focused tests to verify the expected failure**

Run: `rtk cargo test cursor_output_liveness -- --nocapture`

Expected: compilation failure because `needs_output_liveness` is not yet defined. If the test passes, the test is not exercising the new behavior and must be corrected before implementation.

- [ ] **Step 3: Implement the minimal cursor-owned predicate**

In `NativeAtomicCursor`, add:

```rust
pub(crate) fn needs_output_liveness(&self) -> bool {
    let future_output = self
        .worker_queued
        .as_ref()
        .map(|queued| &queued.visual_state)
        .or_else(|| self.pending_token.as_ref().map(|_| &self.submitted))
        .unwrap_or(&self.current);
    !self.desired.kms_equivalent(future_output)
}
```

Do not use raw epochs or `needs_submission_for()`; the latter intentionally suppresses a second submission while a token is pending.

- [ ] **Step 4: Run the focused tests to verify they pass**

Run: `rtk cargo test cursor_output_liveness -- --nocapture`

Expected: PASS for hidden movement, visible movement, in-flight submission, and worker-owned state.

- [ ] **Step 5: Commit the predicate and tests**

```bash
rtk git add src/native_output/output/cursor.rs src/native_output/output/cursor_tests.rs
rtk git commit -m "fix: model atomic cursor output debt"
```

---

### Task 2: Add failing arbitration reconciliation and input-boundary tests

**Files:**
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/cursor_cycle.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/native_output/output/cursor_tests.rs`
- Modify: `src/native_output/tests/frame.rs`

**Interfaces:**
- Produces: `NativeCursorOutputArbitration::request_hardware`, `NativeCursorOutputArbitration::reconcile_hardware_cursor_liveness`, and `observe_atomic_cursor_output_liveness`.
- Consumes: `NativeAtomicCursor::needs_output_liveness`, `NativeFrameScheduler::next_refresh_deadline_ns`, and the existing arbitration disposition/consumption methods.

- [ ] **Step 1: Write failing arbitration tests**

Add tests for independent hardware debt and software-overlay preservation:

```rust
#[test]
fn stale_atomic_cursor_debt_is_cleared_without_clearing_software_work() {
    let mut arbitration = NativeCursorOutputArbitration::default();
    let scheduler = NativeFrameScheduler::new(165, 0);

    arbitration.request_hardware(7, 1_000, scheduler.next_refresh_deadline_ns(1_000));
    arbitration.set_software_overlay_pending(true);
    arbitration.reconcile_hardware_cursor_liveness(false);

    assert!(arbitration.pending());
    assert_eq!(arbitration.disposition(1_000, false, false), NativeCursorOutputDisposition::SoftwareOverlay);
}
```

Add a test that `observe_atomic_cursor_output_liveness()` arms one finite scheduler-derived deadline, leaves the caller's scene-redraw flag untouched, and coalesces repeated desired epochs without moving the first deadline. Add A→B→A before submission and in-flight A→B→A cases.

- [ ] **Step 2: Run the focused tests to verify the expected failure**

Run: `rtk cargo test stale_atomic_cursor_debt -- --nocapture`

Expected: compilation failure because the new arbitration source/reconciliation methods and observer do not exist.

- [ ] **Step 3: Implement arbitration source tracking**

In `NativeCursorOutputArbitration`:

- Add `hardware_cursor_pending: bool`.
- Keep `software_overlay_pending` independent.
- Make `pending()` remain true while either source has an active response window.
- Add an internal request helper that opens one window, retains its first deadline, updates the latest epoch, and increments coalescing only when the epoch changes.
- Implement `request_hardware()` through that helper and mark hardware debt.
- Implement `reconcile_hardware_cursor_liveness(false)` to clear only hardware debt and call `clear_pending()` only when software-overlay work is also absent.
- Reset the new source bit in `clear_pending()`.
- Preserve exact-epoch `consume()` and `consume_submitted_epoch()` behavior, including re-arming for newer in-flight epochs.

Update `update_cursor_output_arbitration()` to use the source-aware helper for hardware work while preserving its software-overlay path and return values.

- [ ] **Step 4: Implement the input-boundary observer**

In `src/native_output/runtime/cursor_cycle.rs`, add:

```rust
pub(crate) fn observe_atomic_cursor_output_liveness(
    atomic_cursor: Option<&NativeAtomicCursor>,
    arbitration: &mut NativeCursorOutputArbitration,
    frame_scheduler: &NativeFrameScheduler,
    now_ns: u64,
) -> bool {
    let Some(cursor) = atomic_cursor else {
        return false;
    };
    let needed = cursor.needs_output_liveness();
    arbitration.reconcile_hardware_cursor_liveness(needed);
    if !needed {
        return false;
    }
    arbitration.request_hardware(
        cursor.desired_epoch(),
        now_ns,
        frame_scheduler.next_refresh_deadline_ns(now_ns),
    );
    true
}
```

Export the observer from `runtime/mod.rs`. Call it exactly once in `dispatch_wayland_and_input()` after the final batch-end `synchronize_cursor_state_for_server()` succeeds and before `end_native_input_batch()`. Pass the existing runtime arbitration and frame scheduler fields. Do not alter `redraw_requested`.

- [ ] **Step 5: Run focused tests to verify they pass**

Run: `rtk cargo test stale_atomic_cursor_debt -- --nocapture`

Run: `rtk cargo test observe_atomic_cursor_output_liveness -- --nocapture`

Run: `rtk cargo test cursor_requests_coalesce -- --nocapture`

Expected: PASS, including finite deadline, stable first deadline, latest epoch, stale debt cancellation, and software-overlay preservation.

- [ ] **Step 6: Commit arbitration and input-boundary integration**

```bash
rtk git add src/native_output/runtime/frame.rs src/native_output/runtime/cursor_cycle.rs src/native_output/runtime/cycle_dispatch.rs src/native_output/runtime/mod.rs src/native_output/output/cursor_tests.rs src/native_output/tests/frame.rs
rtk git commit -m "fix: arm atomic cursor liveness from input"
```

---

### Task 3: Add the complete deterministic runtime acceptance coverage

**Files:**
- Modify: `src/native_output/tests/frame.rs`
- Modify: `src/native_output/tests/input.rs`
- Modify: `src/native_output/runtime/work_domains.rs` only if a missing seam is proven
- Modify: `src/native_output/output/cursor_tests.rs` only for cursor ownership cases

**Interfaces:**
- Consumes: the production observer and existing runtime work-domain/deadline APIs.
- Produces: regression coverage for CursorOnly maturity, 1000 Hz-style coalescing, primary piggyback, software/legacy paths, and interaction preservation.

- [ ] **Step 1: Add and run failing acceptance tests before any additional production code**

Add deterministic tests with no sleeps:

```rust
#[test]
fn input_side_cursor_liveness_matures_as_cursor_only_work() {
    let mut arbitration = NativeCursorOutputArbitration::default();
    let scheduler = NativeFrameScheduler::new(165, 0);
    let deadline = scheduler.next_refresh_deadline_ns(1_000);
    arbitration.request_hardware(9, 1_000, deadline);

    assert_eq!(arbitration.disposition(999, false, true), NativeCursorOutputDisposition::DeferForPrimary);
    assert_eq!(arbitration.disposition(deadline, false, true), NativeCursorOutputDisposition::SubmitPlaneDelta);
}

#[test]
fn high_rate_cursor_updates_keep_one_window_and_latest_epoch() {
    let mut arbitration = NativeCursorOutputArbitration::default();
    let scheduler = NativeFrameScheduler::new(165, 0);
    let first_deadline = scheduler.next_refresh_deadline_ns(1_000);
    arbitration.request_hardware(10, 1_000, first_deadline);
    for epoch in 11..=1_010 {
        arbitration.request_hardware(epoch, 1_000 + epoch, first_deadline + 10_000);
    }
    assert_eq!(arbitration.deadline_ns(), Some(first_deadline));
    assert_eq!(arbitration.desired_epoch(), 1_010);
    assert_eq!(arbitration.response_windows_opened(), 1);
    assert!(arbitration.changes_coalesced() >= 1_000);
}
```

Use the existing work-domain fixtures to prove input-only state remains non-primary before maturity and becomes `CursorOnly` at maturity. Add/retain primary piggyback and interaction tests; add explicit assertions that software cursor movement does not arm hardware arbitration and that legacy `move_to()` remains the immediate path.

Run each new test selector through `rtk cargo test ... -- --nocapture` and confirm each newly added test fails for the intended missing behavior before changing production code.

- [ ] **Step 2: Implement only missing test seams**

If the acceptance tests cannot reach the production observer without a seam, extract only a pure, allocation-free helper already called by `dispatch_wayland_and_input()`. Do not construct a second runtime, scheduler, or cursor policy in tests. Do not alter work-domain semantics to make tests pass.

- [ ] **Step 3: Run focused acceptance tests**

Run:

```bash
rtk cargo test input_side_cursor_liveness_matures_as_cursor_only_work -- --nocapture
rtk cargo test high_rate_cursor_updates_keep_one_window_and_latest_epoch -- --nocapture
rtk cargo test cursor_output_work_waits_for_the_next_output_deadline -- --nocapture
rtk cargo test primary_work_has_right_of_first_refusal_at_the_cursor_deadline -- --nocapture
rtk cargo test native_input_pointer_motion_can_skip_frame_repaint_with_hardware_cursor -- --nocapture
```

Expected: all pass, with no test using real time sleeps.

- [ ] **Step 4: Commit deterministic acceptance coverage**

```bash
rtk git add src/native_output/tests/frame.rs src/native_output/tests/input.rs src/native_output/runtime/work_domains.rs src/native_output/output/cursor_tests.rs
rtk git commit -m "test: cover atomic cursor liveness boundaries"
```

---

### Task 4: Verify the closure, review adversarial cases, and write the report

**Files:**
- Create: `REPORT-2026-08-26-typhon-atomic-cursor-liveness-v1.md`

**Interfaces:**
- Consumes: all committed closure changes, test output, git diff, and available Linux/KMS environment evidence.
- Produces: an English evidence-backed closure report with root cause, implementation, exact test results, hardware qualification status, review answers, and separate follow-ups.

- [ ] **Step 1: Run focused and broad verification through `rtk`**

Run:

```bash
rtk cargo fmt --check
rtk cargo check
rtk cargo test cursor_output_liveness -- --nocapture
rtk cargo test observe_atomic_cursor_output_liveness -- --nocapture
rtk cargo test native_output::output::cursor -- --nocapture
rtk cargo test native_output::tests::frame -- --nocapture
rtk cargo test native_output::tests::input -- --nocapture
rtk cargo test
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk git diff --check
```

Record exact exit status and any platform/toolchain blocker; do not claim unavailable Linux/KMS evidence.

- [ ] **Step 2: Perform the two review passes**

Review Pass 1 must explicitly answer ownership, refresh-derived timing, stable deadlines, hidden suppression, software transition clearing, in-flight epochs, exact consumption, legacy immediacy, non-primary input behavior, primary piggyback, and unchanged transaction lineage.

Review Pass 2 must exercise the deterministic equivalents of high-rate idle input, in-flight cursor/primary work, worker busy, direct scanout, hide/show, image transitions, interaction move/resize, fallback, session/recovery/shutdown, A→B→A before submission, A→B→A while B is committed, and observer O(1) behavior.

- [ ] **Step 3: Write the final report**

Create `REPORT-2026-08-26-typhon-atomic-cursor-liveness-v1.md` with the exact root-cause chain, changed files and ownership boundaries, test commands/results, truthful hardware qualification status, review answers, and separate residual follow-ups for surface pacing, eager `event_index` formatting, shortcut-inhibition scanning, and key/button full-tick behavior.

- [ ] **Step 4: Commit the report**

```bash
rtk git add REPORT-2026-08-26-typhon-atomic-cursor-liveness-v1.md
rtk git commit -m "docs: report atomic cursor liveness closure"
```

