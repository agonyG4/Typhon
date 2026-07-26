# Direct Scanout 2.0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make direct scanout a job-owned `PrimaryPlaneAssignment::ClientFramebuffer` inside the existing output transaction pipeline, with worker-side atomic validation, exact pageflip settlement, and nonfatal composited fallback.

**Architecture:** Reuse `OutputTransactionLedger`, `OutputPlanePlan`, `KmsCommitJob`, `AtomicCommitArbiter`, and the Stage 3 bounded worker as the only lifecycle authorities. Replace the direct path's compositor-thread `TEST_ONLY` and parallel queued/submitted frame structures with a `DirectPrimaryLease` carried by the worker job, a small physical submitted/presented ownership state, and a positive validation cache.

**Tech Stack:** Rust 2024, Wayland server state, linux-dmabuf, DRM/KMS Atomic API, GBM framebuffer import, explicit synchronization, Cargo tests, shell qualification scripts.

## Global Constraints

- Begin implementation only after real Stage 3 TTY/DRM KMS-worker qualification passes.
- Keep the KMS worker bounded to one in-flight job plus one queued job.
- Keep `OBLIVION_ONE_KMS_COMMIT_WORKER` default `Off` until its qualification policy changes in a separate reviewed task.
- Keep `OBLIVION_ONE_DIRECT_SCANOUT` default `Off` throughout Stage 4 development and qualification.
- Direct scanout requires the Stage 3 KMS worker; do not retain a synchronous direct-submit fallback.
- Do not run direct `TEST_ONLY` on the compositor thread.
- Only a validated pageflip may mark a transaction `Presented` or emit presentation feedback.
- A direct worker job must own the exact dmabuf and imported DRM framebuffer resource it names.
- Preserve job-owned input fences and cursor pins through every `EBUSY` retry.
- Preserve cursor-epoch consumption only after successful ioctl acceptance.
- Preserve pacing submit accounting only after successful ioctl acceptance.
- Preserve exact one-time frame-batch, callback, feedback, surface-damage, and buffer-release settlement.
- Preserve the current conservative single opaque fullscreen XRGB dmabuf candidate policy.
- Do not add overlay planes, VRR, tearing, multi-output, hotplug, scaling, transforms, color conversion, HDR, or multi-GPU scanout.
- Do not expand primary-plane formats or modifiers without a separate hardware-backed qualification result.
- Keep every production Rust module at or below 1,500 lines and every `mod.rs` at or below 800 lines.
- Use test-driven development for each task and make one focused commit per task.

---

### Task 0: Qualify the Stage 3 prerequisite

**Files:**
- Inspect: `bin/qualify-kms-worker`
- Produce: `artifacts/qualification/kms-worker-stage3-2026-07-26.log`
- Produce: `artifacts/qualification/kms-worker-stage3-2026-07-26.md`

**Interfaces:**
- Consumes: commit `3fcb18f` and the current Stage 3 worker policy.
- Produces: a pass/fail gate for every later task in this plan.

- [ ] **Step 1: Verify the software gate before native testing**

Run:

```bash
cargo fmt --check
cargo test --locked
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
./bin/check-source-layout
git diff --check
bash -n bin/qualify-kms-worker
```

Expected: every command exits zero and the worktree is clean.

- [ ] **Step 2: Start Typhon on a real TTY with the worker forced on**

Use the repository's normal TTY launch path and add:

```bash
OBLIVION_ONE_KMS_COMMIT_WORKER=force
OBLIVION_ONE_DIRECT_SCANOUT=off
TYPHON_PERF_LOG=1
```

Expected: Typhon starts on real DRM/KMS, direct scanout remains disabled, and the worker reports a bounded queue.

- [ ] **Step 3: Run the application matrix**

Exercise each application for at least five minutes:

```text
Palworld
Steam UI, context menus, and popups
Firefox browsing and fullscreen video
Kitty typing, moving, and resizing
one additional Vulkan game
```

For every application, perform fullscreen entry/exit, Alt+Tab, cursor movement, cursor image changes, and a shell overlay.

Expected: no frozen input, no cursor teleport, no black frame, no stuck pageflip, and no worker queue overflow.

- [ ] **Step 4: Exercise shutdown and recovery boundaries**

Perform:

```text
shutdown while a game is presenting
shutdown while a cursor update is queued
VT switch away and back
session suspend and resume
restart Typhon after the previous cases
```

Expected: the worker joins before KMS restore, no session-owned process survives, and Typhon starts cleanly again.

- [ ] **Step 5: Run the qualification script and save raw output**

Run:

```bash
mkdir -p artifacts/qualification
./bin/qualify-kms-worker 2>&1 | tee artifacts/qualification/kms-worker-stage3-2026-07-26.log
```

Expected counters:

```text
queue overflow = 0
ready superseded = 0
publication rejected = 0
callbacks requested = callbacks completed
unpublished callback owners = 0
shutdown join failures = 0
cursor ownership mismatches = 0
pacing identity mismatches = 0
```

- [ ] **Step 6: Write and commit the qualification report**

Create the report header from measured system data:

```bash
COMMIT=$(git rev-parse HEAD)
GPU=$(lspci -nnk | awk '/VGA compatible controller|3D controller/{print; capture=1; next} capture && /Kernel driver in use/{print; exit}')
CONNECTOR=$(for status in /sys/class/drm/card*-*/status; do [ "$(cat "$status")" = connected ] && printf '%s ' "${status%/status}"; done)
MODE=$(for mode in /sys/class/drm/card*-*/modes; do [ -s "$mode" ] && { head -n1 "$mode"; break; }; done)
cat > artifacts/qualification/kms-worker-stage3-2026-07-26.md <<EOF_REPORT
# KMS Commit Worker Stage 3 Qualification

- Commit: \`$COMMIT\`
- Date: 2026-07-26
- GPU and driver: \`$GPU\`
- Connected DRM path: \`$CONNECTOR\`
- Advertised mode: \`$MODE\`
- Worker policy: \`force\`
- Direct scanout policy: \`off\`

## Application matrix

| Application | Fullscreen | Alt+Tab | Cursor | Overlays | Result |
|---|---|---|---|---|---|
After the header, append one row for each required application. Every cell must contain the literal observed value `PASS` or `FAIL`; do not commit an empty cell or descriptive placeholder.

## Shutdown and recovery

Record the observed shutdown-under-load, VT switch, suspend/resume, and restart results using one PASS or FAIL line per case.

## Qualification counters

Copy the final counters from \`bin/qualify-kms-worker\` verbatim.

## Verdict

Write \`PASS\` only when every required case and counter passes; otherwise write \`FAIL\` and stop the plan.
EOF_REPORT
```

Before committing, verify the table contains exactly five application rows and only measured `PASS` or `FAIL` values.

```bash
git add artifacts/qualification/kms-worker-stage3-2026-07-26.log \
        artifacts/qualification/kms-worker-stage3-2026-07-26.md
git commit -m "test(native): qualify the Stage 3 KMS worker"
```

Stop this plan if the report verdict is not `PASS`.

---

### Task 1: Freeze current direct behavior with characterization tests

**Files:**
- Create: `src/native_output/tests/direct_scanout_stage4.rs`
- Modify: `src/native_output/tests/mod.rs`

**Interfaces:**
- Consumes: current `DirectScanoutCandidateKey`, `OutputTransaction::direct`, `KmsCommitJob`, and `AtomicCommitKind::DirectPrimary` behavior.
- Produces: regression names used as the safety net for later structural changes.

- [ ] **Step 1: Connect a dedicated Stage 4 test module**

Add to `src/native_output/tests/mod.rs`:

```rust
mod direct_scanout_stage4;
```

Create `src/native_output/tests/direct_scanout_stage4.rs` with imports for the existing direct candidate key, transaction, ledger, and pageflip helpers.

- [ ] **Step 2: Add green characterization tests for existing logical behavior**

Add the following tests using the same `OutputContentKey::new`, `OutputTransaction::direct`, `OutputTransactionLedger::with_capacities`, and `PageFlipToken::new` constructors already used in `src/native_output/tests/presentation_transactions.rs`:

| Test | Exact assertion |
|---|---|
| `direct_transaction_uses_client_primary_assignment` | `transaction.planes().primary()` equals `PrimaryPlaneAssignment::ClientFramebuffer { key, framebuffer_id: 92 }`. |
| `same_buffer_new_content_epoch_has_a_distinct_candidate_key` | Two keys with the same buffer identity and epochs 3 and 4 are not equal. |
| `identical_content_epoch_reuses_the_same_candidate_key` | Two keys with identical fields are equal. |
| `direct_test_rejection_never_marks_transaction_presented` | A direct transaction settled as `Failed(KmsSubmit)` increments no presented counter and leaves no active record. |
| `direct_pageflip_is_the_only_presented_transition` | `mark_presented` fails before `mark_submitted`, then succeeds after submission with the exact token and generation. |

Each test must contain concrete key, transaction ID, framebuffer ID, token, generation, timestamp, and frame-batch values; reuse the numeric fixture pattern from `test_direct_key()` rather than creating DRM objects.

- [ ] **Step 3: Run the characterization suite**

```bash
cargo test --locked native_output::tests::direct_scanout_stage4 -- --nocapture
cargo test --locked native_output::tests::presentation_transactions -- --nocapture
cargo test --locked native_output::tests::fullscreen_cadence -- --nocapture
```

Expected: all characterization tests pass against the baseline. Tests for new job-owned resources and cursor fallback are added as RED tests in Tasks 3 and 7, immediately before their implementations.

- [ ] **Step 4: Commit the green characterization boundary**

The commit must contain no ignored or intentionally failing Stage 4 tests.

```bash
git add src/native_output/tests/mod.rs \
        src/native_output/tests/direct_scanout_stage4.rs
git commit -m "test(native): characterize Direct Scanout 2.0 invariants"
```

---

### Task 2: Split the near-limit direct path and add blocker diagnostics

**Files:**
- Create: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Create: `src/native_output/runtime/direct_plan.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/planner.rs`
- Modify: `src/compositor/fullscreen.rs`
- Modify: `src/compositor/state/fullscreen.rs`
- Modify: `src/compositor/server.rs`
- Test: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Test: `src/native_output/runtime/direct_plan.rs`
- Test: `src/compositor/state/fullscreen.rs`

**Interfaces:**
- Produces: `DirectScanoutRuntimeBlocker`, non-generic `DirectScanoutDecision`, and `plan_direct_scanout`.
- Produces: diagnostics-only `OwnCompositorServer::direct_scanout_scene_blockers()`.
- Preserves: `OwnCompositorServer::direct_scanout_scene_candidate()` as the fast first-rejection API.

- [ ] **Step 1: Extract the existing direct method without changing behavior**

Declare in `src/native_output/scanout/atomic_egl_gbm.rs`:

```rust
mod direct;
```

Move `AtomicEglGbmScanout::try_direct_scanout` and its direct-only helpers into `src/native_output/scanout/atomic_egl_gbm/direct.rs` as another inherent `impl AtomicEglGbmScanout` block.

Do not change behavior in this step.

- [ ] **Step 2: Verify extraction is behavior-neutral**

```bash
cargo fmt --check
cargo test --locked direct_scanout
cargo test --locked native_output::tests::fullscreen_cadence
./bin/check-source-layout
```

Expected: all commands exit zero and `atomic_egl_gbm.rs` falls comfortably below 1,500 lines.

- [ ] **Step 3: Add the pure runtime blocker model**

Create `src/native_output/runtime/direct_plan.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectScanoutRuntimeBlocker {
    PolicyOff,
    WorkerUnavailable,
    WorkerQueueFull,
    SessionInactive,
    ShutdownActive,
    OutputTransition,
    PrimaryCommitPending,
    SoftwareCursorVisible,
    CursorAssignmentUnsupported,
    AcquireNotReady,
    BufferDeviceUnproven,
    SameContent,
}

impl DirectScanoutRuntimeBlocker {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyOff => "policy_off",
            Self::WorkerUnavailable => "worker_unavailable",
            Self::WorkerQueueFull => "worker_queue_full",
            Self::SessionInactive => "session_inactive",
            Self::ShutdownActive => "shutdown_active",
            Self::OutputTransition => "output_transition",
            Self::PrimaryCommitPending => "primary_commit_pending",
            Self::SoftwareCursorVisible => "software_cursor_visible",
            Self::CursorAssignmentUnsupported => "cursor_assignment_unsupported",
            Self::AcquireNotReady => "acquire_not_ready",
            Self::BufferDeviceUnproven => "buffer_device_unproven",
            Self::SameContent => "same_content",
        }
    }
}

#[derive(Debug)]
pub(crate) enum DirectScanoutDecision {
    Eligible,
    Blocked(DirectScanoutRuntimeBlocker),
}
```

Add a `DirectScanoutPlanInput` containing only copyable booleans and current/pending content keys. Implement `plan_direct_scanout(input)` with stable first-rejection ordering matching the design document.

- [ ] **Step 4: Write and pass blocker-order tests**

Add these tests with complete `DirectScanoutPlanInput` literals:

| Test | Exact assertion |
|---|---|
| `policy_blocker_precedes_scene_work` | An otherwise eligible input with policy disabled returns `Blocked(PolicyOff)`. |
| `worker_unavailable_precedes_candidate_import` | Enabled policy without a running worker returns `Blocked(WorkerUnavailable)`. |
| `shutdown_precedes_primary_admission` | A shutting-down input returns `Blocked(ShutdownActive)` even when admission is available. |
| `visible_software_cursor_blocks_direct_scanout` | A visible software cursor returns `Blocked(SoftwareCursorVisible)`. |
| `identical_presented_content_returns_same_content` | A candidate key equal to the presented key returns `Blocked(SameContent)`. |

Run:

```bash
cargo test --locked native_output::runtime::direct_plan
```

Expected: all tests pass.

- [ ] **Step 5: Add diagnostics-only scene blocker collection**

In `src/compositor/fullscreen.rs`, add:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectScanoutSceneBlockers {
    reasons: Vec<DirectScanoutSceneRejection>,
}

impl DirectScanoutSceneBlockers {
    pub fn reasons(&self) -> &[DirectScanoutSceneRejection] {
        &self.reasons
    }

    fn push(&mut self, reason: DirectScanoutSceneRejection) {
        if !self.reasons.contains(&reason) {
            self.reasons.push(reason);
        }
    }
}
```

Refactor individual candidate checks into pure predicates used by both:

```rust
pub(in crate::compositor) fn direct_scanout_scene_candidate(&self) -> Result<DirectScanoutSceneCandidate, DirectScanoutSceneRejection>
pub(in crate::compositor) fn direct_scanout_scene_blockers(&self) -> DirectScanoutSceneBlockers
```

Expose the diagnostics method through `OwnCompositorServer`.

- [ ] **Step 6: Prove diagnostics do not change the hot path**

Add tests that construct multiple simultaneous blockers and assert:

```rust
assert_eq!(state.direct_scanout_scene_candidate(), Err(DirectScanoutSceneRejection::OverlayVisible));
assert_eq!(
    state.direct_scanout_scene_blockers().reasons(),
    &[
        DirectScanoutSceneRejection::OverlayVisible,
        DirectScanoutSceneRejection::PopupVisible,
        DirectScanoutSceneRejection::ResizePreviewActive,
    ]
);
```

Use the actual ordering produced by the extracted predicates.

```bash
cargo test --locked compositor::state::fullscreen
cargo test --locked direct_scanout_scene
```

Expected: all tests pass.

- [ ] **Step 7: Commit the extraction and decision boundary**

```bash
git add src/native_output/scanout/atomic_egl_gbm.rs \
        src/native_output/scanout/atomic_egl_gbm/direct.rs \
        src/native_output/runtime/direct_plan.rs \
        src/native_output/runtime/mod.rs \
        src/native_output/runtime/planner.rs \
        src/compositor/fullscreen.rs \
        src/compositor/state/fullscreen.rs \
        src/compositor/server.rs
git commit -m "refactor(native): isolate direct scanout planning"
```

---

### Task 3: Make direct primary resources job-owned

**Files:**
- Create: `src/native_output/scanout/direct_lease.rs`
- Modify: `src/native_output/scanout/mod.rs`
- Modify: `src/native_output/kms_worker/payload.rs`
- Modify: `src/native_output/kms_worker/thread.rs`
- Modify: `src/native_output/kms_worker/tests.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/kms_worker.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`

**Interfaces:**
- Produces: `DirectPrimaryLease`.
- Changes: `KmsCommitJob::direct_primary_lease: Option<DirectPrimaryLease>`.
- Changes: `KmsWorkerEvent::Submitted` transfers `direct_primary_lease` back to the runtime.
- Produces: `KmsCommitPayloadError::DirectPrimaryResourceMismatch`.

- [ ] **Step 1: Write failing lease ownership tests**

In `src/native_output/kms_worker/tests.rs`, add these RED tests using the existing `test_job(token)` fixture and a new `test_direct_transaction(token, key, framebuffer_id)` fixture:

| Test | Exact assertion |
|---|---|
| `direct_job_requires_matching_owned_primary_resource` | A `DirectPrimary` job with `direct_primary_lease: None` returns `DirectPrimaryResourceMismatch`. |
| `direct_job_rejects_lease_with_wrong_framebuffer` | A lease naming framebuffer 43 against transaction/job framebuffer 42 returns `DirectPrimaryResourceMismatch`. |
| `direct_job_rejects_lease_with_wrong_candidate_key` | A lease whose content epoch differs from the transaction key returns `DirectPrimaryResourceMismatch`. |
| `composited_and_cursor_jobs_reject_direct_leases` | A non-direct job containing a direct lease returns `DirectPrimaryResourceMismatch`. |

Expected RED: compilation fails only because the lease field, fixture, and validation error do not exist yet.

- [ ] **Step 2: Implement `DirectPrimaryLease`**

Create `src/native_output/scanout/direct_lease.rs`:

```rust
use std::sync::Arc;

use oblivion_one::compositor::{DirectScanoutSceneCandidate, SurfaceDamagePresentation};
use oblivion_one::render_backend::buffer::DmabufBufferHandle;

use super::{DirectScanoutCandidateKey, ImportedDirectFramebuffer};

#[derive(Debug)]
pub(crate) struct DirectPrimaryLease {
    key: DirectScanoutCandidateKey,
    surface_id: u32,
    _buffer: DmabufBufferHandle,
    framebuffer: Arc<ImportedDirectFramebuffer>,
    surface_damage: Option<SurfaceDamagePresentation>,
}

impl DirectPrimaryLease {
    pub(crate) fn new(
        candidate: DirectScanoutSceneCandidate,
        key: DirectScanoutCandidateKey,
        framebuffer: Arc<ImportedDirectFramebuffer>,
        surface_damage: SurfaceDamagePresentation,
    ) -> Self {
        Self {
            key,
            surface_id: candidate.surface_id,
            _buffer: candidate.buffer,
            framebuffer,
            surface_damage: Some(surface_damage),
        }
    }

    pub(crate) const fn key(&self) -> DirectScanoutCandidateKey { self.key }
    pub(crate) const fn surface_id(&self) -> u32 { self.surface_id }
    pub(crate) fn framebuffer_id(&self) -> u32 { self.framebuffer.framebuffer.get() }
}
```

Implement exactly these additional methods:

```rust
pub(crate) fn take_surface_damage(&mut self) -> io::Result<SurfaceDamagePresentation> {
    self.surface_damage
        .take()
        .ok_or_else(|| io::Error::other("direct surface damage already settled"))
}

pub(crate) const fn has_surface_damage(&self) -> bool {
    self.surface_damage.is_some()
}
```

Store `surface_damage` as `Option<SurfaceDamagePresentation>`. Keep the dmabuf handle and framebuffer `Arc` private and immutable so the lease remains alive after damage settlement while KMS still scans out the framebuffer.

- [ ] **Step 3: Prove the lease crosses the worker safely**

Add compile-time assertions beside the existing payload assertions:

```rust
fn _assert_send<T: Send>() {}

_assert_send::<DirectPrimaryLease>();
_assert_send::<KmsCommitJob>();
```

Do not require `DirectPrimaryLease: Sync` unless the worker architecture actually shares references across threads.

Run:

```bash
cargo check --locked --all-targets
```

Expected: the lease and job are `Send`.

- [ ] **Step 4: Add the job field and exact payload validation**

Add to `KmsCommitJob`:

```rust
pub(crate) direct_primary_lease: Option<DirectPrimaryLease>,
```

Add `DirectPrimaryResourceMismatch` and validate:

```rust
match (self.kind, self.direct_primary_lease.as_ref()) {
    (AtomicCommitKind::DirectPrimary { framebuffer_id, .. }, Some(lease))
        if lease.framebuffer_id() == framebuffer_id
            && matches!(
                transaction.content(),
                OutputTransactionContent::Direct { key, .. } if key == lease.key()
            ) => {}
    (AtomicCommitKind::DirectPrimary { .. }, _) => {
        return Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch);
    }
    (_, None) => {}
    (_, Some(_)) => return Err(KmsCommitPayloadError::DirectPrimaryResourceMismatch),
}
```

Use the existing transaction accessors rather than adding a duplicate content parser when possible.

- [ ] **Step 5: Transfer ownership in every worker event path**

Change `KmsWorkerEvent::Submitted` to include:

```rust
direct_primary_lease: Option<DirectPrimaryLease>,
test_only_policy: KmsTestOnlyPolicy,
```

The policy field preserves whether the successful job performed `TEST_ONLY`, allowing exact cache and timing metrics without reconstructing worker history.

After a successful ioctl, destructure the owned job and move its lease into the event. Preserve the unchanged job in:

```text
TestRejected
SubmitRejected
BusyExhausted
Quiesced.returned_jobs
```

`BusyDeferred` must retain the job inside the worker queue and must not clone or release the lease.

- [ ] **Step 6: Build direct jobs with the lease**

In the direct preparation path:

1. capture `SurfaceDamagePresentation`;
2. create `DirectPrimaryLease` after import and transaction construction;
3. pass the lease into `build_worker_direct_job`;
4. set `direct_primary_lease: Some(lease)`;
5. set `None` for every composited, compatibility, and cursor-only job.

The runtime must no longer place the candidate buffer or framebuffer `Arc` into a separate worker-queued direct structure.

- [ ] **Step 7: Add queue, retry, rejection, and shutdown ownership tests**

Add real job tests with a drop-probed `ImportedDirectFramebuffer` fixture:

| Test | Exact assertion |
|---|---|
| `queued_direct_job_keeps_dmabuf_and_framebuffer_alive` | Dropping importer-side references does not run the framebuffer drop probe while the queued job exists. |
| `direct_ebusy_retry_keeps_the_same_lease_identity` | The lease key, framebuffer ID, dmabuf identity, and drop-probe count are unchanged after one `BusyDeferred` retry. |
| `direct_test_rejection_returns_the_lease_once` | `TestRejected.job.direct_primary_lease` is `Some`, and dropping the event increments the drop probe once. |
| `direct_submit_rejection_returns_the_lease_once` | `SubmitRejected` has the same one-time ownership result. |
| `direct_shutdown_quiesce_returns_the_queued_lease` | The returned job in `Quiesced` owns the lease and no other owner remains. |
| `successful_direct_submit_transfers_the_lease_to_submitted_event` | The worker queue no longer owns the lease and `Submitted.direct_primary_lease` is `Some`. |

Use drop probes or `Arc` strong-count assertions around the actual imported-resource wrapper. Do not use a marker-only test that leaves DRM cleanup disabled as the sole proof.

- [ ] **Step 8: Run focused tests and commit**

```bash
cargo test --locked native_output::kms_worker
cargo test --locked direct_primary_lease
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
./bin/check-source-layout
git diff --check
```

```bash
git add src/native_output/scanout/direct_lease.rs \
        src/native_output/scanout/mod.rs \
        src/native_output/kms_worker/payload.rs \
        src/native_output/kms_worker/thread.rs \
        src/native_output/kms_worker/tests.rs \
        src/native_output/runtime/presentation_worker.rs \
        src/native_output/runtime/kms_worker.rs \
        src/native_output/scanout/atomic_egl_gbm/direct.rs
git commit -m "feat(native): make direct primary resources job-owned"
```

---

### Task 4: Move direct `TEST_ONLY` into the KMS worker

**Files:**
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/kms_worker/thread.rs`
- Modify: `src/native_output/kms_worker/tests.rs`
- Modify: `src/native/kms/submitter.rs`
- Test: `src/native_output/kms_worker/tests.rs`

**Interfaces:**
- Consumes: Task 3's job-owned lease.
- Produces: direct jobs with `KmsTestOnlyPolicy::Required` on validation-cache misses.
- Removes: compositor-thread calls to `test_atomic_primary_flip_with_cursor` from the direct path.

- [ ] **Step 1: Write a recording executor test**

Add a test executor that records each request:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedAtomicRequest {
    framebuffer_id: u32,
    cursor: KmsCursorUpdate,
    test_only: bool,
    request_out_fence: bool,
}
```

Add these tests around the recording executor:

| Test | Exact assertion |
|---|---|
| `direct_worker_tests_then_submits_the_same_primary_and_cursor_state` | Exactly two records exist; the first has `test_only=true`, the second `false`, and framebuffer/cursor fields are equal. |
| `direct_test_rejection_prevents_real_submit` | Exactly one record exists and it has `test_only=true`; the event is `TestRejected`. |
| `direct_test_only_does_not_consume_input_fence` | The real-submit record observes the still-open job-owned fence and the fd closes only when the terminal event/job is dropped. |

Expected RED: direct jobs currently set `Skip` and the old main-thread path performs the test.

- [ ] **Step 2: Remove synchronous direct validation**

Delete the direct-path block that calls:

```rust
kms.test_atomic_primary_flip_with_cursor(framebuffer.framebuffer, cursor)
```

Delete the stale-candidate re-query that exists only to close the gap between main-thread `TEST_ONLY` and submit. The candidate snapshot becomes immutable once the transaction and lease are built.

Keep pre-build candidate validation and content-key checks.

- [ ] **Step 3: Require worker validation for unvalidated plans**

When constructing the direct job, set:

```rust
test_only: KmsTestOnlyPolicy::Required,
```

Task 5 later changes this to `Skip` only for an exact positive cache hit.

Reject direct eligibility when no worker admission permit exists. Do not call the old inline direct submit path.

- [ ] **Step 4: Verify the worker uses one immutable request**

In `AtomicKmsWorkerExecutor::submit`, keep the existing ordering:

```text
borrow job state
→ TEST_ONLY
→ borrow the same job-owned input fence
→ real submit
```

Ensure the test path never takes ownership of `in_fence` and never requests an out-fence.

- [ ] **Step 5: Separate `TestRejected` from real-submit rejection metrics**

Ensure `KmsWorkerEvent::TestRejected` is emitted only when the required atomic test fails. `SubmitRejected` is reserved for the real ioctl. Preserve `BusyDeferred` and `BusyExhausted` semantics for the real submit.

- [ ] **Step 6: Run focused tests and prove the compositor thread no longer tests**

```bash
cargo test --locked direct_worker_tests_then_submits_the_same_primary_and_cursor_state
cargo test --locked direct_test_rejection_prevents_real_submit
cargo test --locked native_output::kms_worker
rg -n "test_atomic_primary_flip_with_cursor" src/native_output/scanout src/native_output/runtime
```

Expected: tests pass and the search returns no direct compositor-thread caller.

- [ ] **Step 7: Commit**

```bash
git add src/native_output/scanout/atomic_egl_gbm/direct.rs \
        src/native_output/runtime/presentation_worker.rs \
        src/native_output/kms_worker/thread.rs \
        src/native_output/kms_worker/tests.rs \
        src/native/kms/submitter.rs
git commit -m "feat(native): validate direct assignments in the KMS worker"
```

---

### Task 5: Add an exact positive direct-plane validation cache

**Files:**
- Create: `src/native_output/scanout/direct_validation.rs`
- Modify: `src/native_output/scanout/mod.rs`
- Modify: `src/native_output/scanout/atomic_direct.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Modify: `src/native_output/runtime/kms_worker.rs`
- Modify: `src/native_output/runtime/session_io.rs`
- Modify: `src/native_output/runtime/session.rs`
- Test: `src/native_output/scanout/direct_validation.rs`

**Interfaces:**
- Produces: `DirectPlaneValidationKey` and `DirectPlaneValidationCache`.
- Changes: direct job policy is `Skip` only for an exact positive cache hit.
- Removes: `DirectPlanePlanKey`, `TestedDirectPlanePlan`, and `DirectScanoutState::tested_plane_plan`.

- [ ] **Step 1: Write cache tests before implementation**

Create these tests with full `DirectPlaneValidationKey` literals:

| Test | Exact assertion |
|---|---|
| `validation_key_changes_with_output_generation` | Keys differing only in `output_generation` are not equal. |
| `validation_key_changes_with_cursor_assignment` | Keys differing only in `cursor_plan_key` are not equal. |
| `validation_key_changes_with_modifier_and_layout` | Keys differing in modifier or layout hash are not equal. |
| `positive_cache_is_bounded_to_eight_entries` | Inserting nine unique keys evicts key 1 and retains keys 2 through 9. |
| `real_submit_rejection_invalidates_matching_entry` | Invalidating key 4 removes only key 4. |
| `output_rebuild_invalidates_all_entries` | `invalidate_all()` makes every previous lookup return false. |

Expected RED: types do not exist.

- [ ] **Step 2: Implement the exact key**

Create:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DirectPlaneValidationKey {
    pub(crate) output_generation: u64,
    pub(crate) crtc_id: u32,
    pub(crate) primary_plane_id: u32,
    pub(crate) mode_width: u32,
    pub(crate) mode_height: u32,
    pub(crate) format: u32,
    pub(crate) modifier: u64,
    pub(crate) buffer_width: u32,
    pub(crate) buffer_height: u32,
    pub(crate) plane_layout_hash: u64,
    pub(crate) cursor_plan_key: Option<u64>,
    pub(crate) synchronization_key: u64,
}
```

Calculate `plane_layout_hash` from plane count, offsets, strides, and modifier data already present in `DmabufBufferHandle`. Do not hash file descriptor numbers.

Calculate `synchronization_key` from the required input-fence property and requested release mode, not from ephemeral fd values.

- [ ] **Step 3: Implement a bounded positive-only cache**

```rust
#[derive(Debug, Default)]
pub(crate) struct DirectPlaneValidationCache {
    entries: VecDeque<DirectPlaneValidationKey>,
}

impl DirectPlaneValidationCache {
    pub(crate) const CAPACITY: usize = 8;
    pub(crate) fn contains(&self, key: DirectPlaneValidationKey) -> bool;
    pub(crate) fn record_success(&mut self, key: DirectPlaneValidationKey);
    pub(crate) fn invalidate(&mut self, key: DirectPlaneValidationKey);
    pub(crate) fn invalidate_all(&mut self);
}
```

Move a repeated key to the newest position and evict from the front.

- [ ] **Step 4: Carry the validation key with the direct job**

Add the key to `DirectPrimaryLease` or the direct job metadata so that both success and rejection events identify the exact tested state. Do not reconstruct the key after the worker returns.

- [ ] **Step 5: Select `Required` or `Skip` deterministically**

Before building the job:

```rust
let test_only = if validation_cache.contains(validation_key) {
    KmsTestOnlyPolicy::Skip
} else {
    KmsTestOnlyPolicy::Required
};
```

After a successful `Submitted` event for a job that performed `TEST_ONLY`, record the key as positive. After a real-submit rejection, invalidate the exact key. A `TEST_ONLY` rejection is not inserted.

- [ ] **Step 6: Invalidate on output/session boundaries**

Call `invalidate_all()` on:

```text
DRM generation change
modeset or output reconstruction
session resume
primary or cursor plane capability reconstruction
scanout backend recovery
```

Use one helper on the scanout backend so all boundary paths share the same behavior.

- [ ] **Step 7: Run tests and commit**

```bash
cargo test --locked direct_validation
cargo test --locked direct_scanout
cargo test --locked native_output::runtime::session
cargo check --locked --all-targets
./bin/check-source-layout
git diff --check
```

```bash
git add src/native_output/scanout/direct_validation.rs \
        src/native_output/scanout/mod.rs \
        src/native_output/scanout/atomic_direct.rs \
        src/native_output/scanout/atomic_egl_gbm/direct.rs \
        src/native_output/runtime/kms_worker.rs \
        src/native_output/runtime/session_io.rs \
        src/native_output/runtime/session.rs
git commit -m "feat(native): cache validated direct plane assignments"
```

---

### Task 6: Remove queued direct lifecycle duplication

**Files:**
- Modify: `src/native_output/scanout/atomic_direct.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm/worker.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/kms_worker.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Test: `src/native_output/scanout/atomic_egl_gbm/worker.rs`
- Test: `src/native_output/runtime/presentation_tests.rs`

**Interfaces:**
- Produces: `DirectPrimaryOwnership`, `SubmittedDirectPrimary`, `PresentedDirectPrimary`, and `DirectScanoutControl`.
- Removes: `PreparedDirectFrame`, `WorkerQueuedDirectFrame`, `SubmittedDirectFrame`, and worker-queue authority from `DirectScanoutState`.
- Preserves: `DirectFramebufferCache`, counters, qualification state, and presented-resource retention.

- [ ] **Step 1: Write lifecycle ownership tests**

Add these tests using two leases with separate drop probes:

| Test | Exact assertion |
|---|---|
| `worker_queue_owns_direct_resource_before_submit` | Before a `Submitted` event, `DirectPrimaryOwnership` has no submitted or presented lease. |
| `submitted_event_transfers_direct_resource_to_physical_ownership` | `accept_submitted` stores the exact transaction ID, token, key, and framebuffer ID. |
| `pageflip_promotes_submitted_direct_resource_to_presented` | `complete_pageflip` clears submitted and stores the same lease as presented. |
| `replacement_pageflip_releases_previous_presented_resource` | Lease A remains live while B is submitted and drops exactly once after B is presented. |
| `rejected_queued_direct_job_never_enters_submitted_ownership` | Dropping a returned rejected job leaves both ownership slots empty. |

Expected RED: current `DirectScanoutState` owns `worker_queued` and duplicates the worker.

- [ ] **Step 2: Replace the direct lifecycle structs**

Define in `atomic_direct.rs`:

```rust
pub(crate) struct SubmittedDirectPrimary {
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) lease: DirectPrimaryLease,
    pub(crate) submit_started_at: MonotonicTimestampNs,
    pub(crate) submit_returned_at: MonotonicTimestampNs,
    pub(crate) out_fence: Option<OwnedFd>,
}

pub(crate) struct PresentedDirectPrimary {
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) token: PageFlipToken,
    pub(crate) lease: DirectPrimaryLease,
    pub(crate) presented_at: MonotonicTimestampNs,
}

#[derive(Default)]
pub(crate) struct DirectPrimaryOwnership {
    submitted: Option<SubmittedDirectPrimary>,
    presented: Option<PresentedDirectPrimary>,
    suspended: Vec<DirectPrimaryLease>,
}

pub(crate) struct DirectScanoutControl {
    pub(crate) ownership: DirectPrimaryOwnership,
    pub(crate) framebuffer_cache: DirectFramebufferCache,
    pub(crate) validation_cache: DirectPlaneValidationCache,
    pub(crate) inhibit_until_composited_present: bool,
    pub(crate) counters: DirectScanoutCounters,
    pub(crate) drm_generation: u64,
    pub(crate) identity_viewport_metadata_logged: bool,
    pub(crate) last_debug_candidate: Option<(u32, u64, u64, u64)>,
}
```

Replace the scanout field type from `DirectScanoutState` to `DirectScanoutControl`. Add exact-token methods to `DirectPrimaryOwnership`:

```rust
pub(crate) fn accept_submitted(&mut self, submitted: SubmittedDirectPrimary) -> io::Result<()>;
pub(crate) fn complete_pageflip(&mut self, transaction_id: OutputTransactionId, token: PageFlipToken, presented_at: MonotonicTimestampNs) -> io::Result<(PresentedDirectPrimary, Option<PresentedDirectPrimary>)>;
pub(crate) fn abandon_submitted_for_restore(&mut self, token: PageFlipToken) -> io::Result<()>;
pub(crate) fn clear_after_restore(&mut self);
```

- [ ] **Step 3: Remove compositor-side worker-queued storage**

Delete:

```text
DirectScanoutState.worker_queued
WorkerQueuedDirectFrame
promote_worker_submission based on worker_queued
suspend_worker_queued
worker_queued_token
```

The worker job is the only queued physical owner.

- [ ] **Step 4: Transfer successful worker submission into physical ownership**

When handling `KmsWorkerEvent::Submitted` for `DirectPrimary`:

1. require `direct_primary_lease: Some`;
2. build `SubmittedDirectPrimary` from the event;
3. call `DirectPrimaryOwnership::accept_submitted`;
4. mark the ledger transaction submitted;
5. mark arbiter kernel submission;
6. confirm scheduler and pacing submission.

Perform resource transfer before replaying a deferred early pageflip so the replay can complete against valid physical ownership.

- [ ] **Step 5: Route rejection and quiesce solely through returned jobs**

`fail_queued_worker_job` and `drop_queued_worker_job_with_reason` must no longer call direct-state methods to retrieve a queued frame batch or physical resource. The returned job already contains its lease; the ledger contains frame-batch obligations.

Settle with:

```rust
settle_failed_output_transaction(
    &mut self.output_transactions,
    job.transaction_id,
    OutputTransactionFailureStage::KmsSubmit,
    MonotonicTimestampNs::new(monotonic_now_ns()?),
    |obligations| {
        let batch = obligations.frame_batch_id().ok_or_else(|| {
            io::Error::other("rejected direct transaction has no frame batch")
        })?;
        self.server.restore_frame_batch_after_render_failure(batch);
        Ok(())
    },
)?;
```

The lease drops after the settlement closure unless it is retained for uncertain kernel ownership.

- [ ] **Step 6: Derive metrics from real authorities**

Replace `direct_scanout_pending()` and `direct_scanout_pending_transaction_id()` implementations so they query:

- worker/arbiter transaction identity for queued/submitted state;
- `DirectPrimaryOwnership` only for physical submitted/presented resource data.

Do not infer logical state from a duplicated direct frame.

- [ ] **Step 7: Run focused lifecycle tests and commit**

```bash
cargo test --locked worker_queue_owns_direct_resource_before_submit
cargo test --locked submitted_event_transfers_direct_resource_to_physical_ownership
cargo test --locked pageflip_promotes_submitted_direct_resource_to_presented
cargo test --locked replacement_pageflip_releases_previous_presented_resource
cargo test --locked native_output::runtime::presentation_tests
cargo test --locked native_output::kms_worker
./bin/check-source-layout
```

```bash
git add src/native_output/scanout/atomic_direct.rs \
        src/native_output/scanout/atomic_egl_gbm/direct.rs \
        src/native_output/scanout/atomic_egl_gbm/worker.rs \
        src/native_output/runtime/presentation_worker.rs \
        src/native_output/runtime/kms_worker.rs \
        src/native_output/runtime/cycle.rs \
        src/native_output/runtime/metrics.rs
git commit -m "refactor(native): unify direct worker ownership"
```

---

### Task 7: Unify pageflip settlement and fallback transitions

**Files:**
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/kms_worker.rs`
- Modify: `src/native_output/runtime/presentation_direct.rs`
- Modify: `src/native_output/runtime/presentation_transactions.rs`
- Modify: `src/native_output/runtime/presentation.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Modify: `src/native_output/output/damage.rs`
- Test: `src/native_output/runtime/presentation_tests.rs`
- Test: `src/native_output/scanout/atomic_egl_gbm/confirmed_pageflip_tests.rs`

**Interfaces:**
- Produces: one direct pageflip settlement path using `complete_presented_output_transaction`.
- Produces: nonfatal `TestRejected`/`SubmitRejected` composited fallback.
- Produces: confirmed assignment transition reporting.
- Removes: direct entry/damage side effects at worker queue admission.

- [ ] **Step 1: Add transition and fallback tests**

Add these runtime tests:

| Test | Exact assertion |
|---|---|
| `composed_to_direct_becomes_active_only_after_pageflip` | Queue and submit leave entry count unchanged; confirmed direct pageflip increments it once. |
| `direct_to_direct_retains_old_resource_until_replacement_pageflip` | Lease A's drop probe remains zero until B's confirmed pageflip. |
| `direct_to_composed_releases_direct_resource_after_composed_pageflip` | Direct ownership remains active through composed submit and clears on composed pageflip. |
| `direct_test_rejection_restores_batch_and_requests_composition` | The batch returns to the server, the transaction is terminal without feedback, and `queued_redraw_requested` is true. |
| `direct_real_submit_rejection_invalidates_cache_and_requests_composition` | The exact validation key is removed and the same fallback conditions hold. |
| `rejected_direct_attempt_does_not_invalidate_presented_damage_history` | Damage-history generation is unchanged after pre-submit rejection. |

- [ ] **Step 2: Complete direct pageflips through the transaction helper**

In the pageflip route for `AtomicCommitKind::DirectPrimary`:

1. obtain the submitted direct resource from `DirectPrimaryOwnership`;
2. verify transaction ID and token;
3. call `complete_presented_output_transaction` with actual DRM timestamp and sequence;
4. in the settlement closure:
   - obtain the direct frame batch;
   - call `server.complete_direct_presented_frame_batch`;
   - apply the lease's `SurfaceDamagePresentation`;
5. promote the lease to presented;
6. release the previous presented lease after promotion;
7. notify pacing and metrics exactly once.

Do not call any presented helper before arbiter validation.

- [ ] **Step 3: Move transition side effects to confirmed presentation**

Remove from worker queue admission:

```text
entries increment
scene.invalidate_presented_damage_history()
direct-active transition
```

After confirmed direct presentation:

- if the previous confirmed primary assignment was composed, increment `entries`;
- if it was direct, increment direct replacement/steady-state metrics;
- invalidate composed presented-damage history for the next composed transition.

After confirmed composed presentation replacing direct:

- increment `exits`;
- clear direct-active state;
- invalidate composed damage history before relying on buffer age.

- [ ] **Step 4: Implement rejection-specific fallback without global cursor demotion**

Split worker rejection handling by event kind and commit kind:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerRejectionKind {
    TestOnly,
    RealSubmit,
}

match (event_kind, job.kind) {
    (WorkerRejectionKind::TestOnly, AtomicCommitKind::DirectPrimary { .. }) => {
        self.reject_direct_worker_job(job, false)?;
    }
    (WorkerRejectionKind::RealSubmit, AtomicCommitKind::DirectPrimary { .. }) => {
        self.reject_direct_worker_job(job, true)?;
    }
    _ => self.reject_non_direct_worker_job(job, error)?,
}
```

For direct rejection:

- cancel exact pacing/scheduler/arbiter identity;
- settle the transaction without feedback;
- restore the frame batch for composition;
- request an immediate redraw;
- retain hardware cursor policy;
- do not set `NativeCursorRenderMode::Software` solely from direct combined-state rejection.

- [ ] **Step 5: Preserve software-cursor fallback only in the composed cursor path**

Keep the existing software fallback when a cursor-only or normal composed-primary cursor commit independently fails. Add a metric distinguishing:

```text
direct_combined_cursor_rejection
composed_cursor_fallback
```

- [ ] **Step 6: Handle suspension and forced abandonment**

For a kernel-submitted direct job during session loss or shutdown timeout:

- move its lease to `DirectPrimaryOwnership.suspended` before dropping logical state;
- settle the transaction with the existing `SessionSuspended` or `SafeAbandonment` reason;
- do not release the lease until KMS restore/disarm proves scanout ownership ended;
- clear suspended leases after restore exactly once.

- [ ] **Step 7: Run transition tests and commit**

```bash
cargo test --locked composed_to_direct_becomes_active_only_after_pageflip
cargo test --locked direct_to_direct_retains_old_resource_until_replacement_pageflip
cargo test --locked direct_to_composed_releases_direct_resource_after_composed_pageflip
cargo test --locked direct_test_rejection_restores_batch_and_requests_composition
cargo test --locked rejected_direct_attempt_does_not_invalidate_presented_damage_history
cargo test --locked direct_combined_cursor_rejection_does_not_latch_software_cursor
cargo test --locked native_output::scanout::atomic_egl_gbm::confirmed_pageflip_tests
```

```bash
git add src/native_output/runtime/cycle.rs \
        src/native_output/runtime/kms_worker.rs \
        src/native_output/runtime/presentation_direct.rs \
        src/native_output/runtime/presentation_transactions.rs \
        src/native_output/runtime/presentation.rs \
        src/native_output/scanout/atomic_egl_gbm.rs \
        src/native_output/scanout/atomic_egl_gbm/direct.rs \
        src/native_output/output/damage.rs
git commit -m "fix(native): settle direct assignments by confirmed pageflip"
```

---

### Task 8: Make same-content and protocol obligations exact

**Files:**
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Modify: `src/native_output/presentation/transaction.rs`
- Modify: `src/native_output/presentation/ledger.rs`
- Modify: `src/native_output/runtime/presentation_transactions.rs`
- Modify: `src/compositor/state/surface_commits.rs`
- Modify: `src/compositor/state/surfaces.rs`
- Test: `src/native_output/tests/direct_scanout_stage4.rs`
- Test: `src/native_output/tests/presentation_transactions.rs`
- Test: `src/compositor/state/surfaces.rs`

**Interfaces:**
- Preserves: `DirectScanoutCandidateKey` content-epoch authority.
- Produces: direct `NoVisualChange` settlement without KMS submission or fake feedback.
- Produces: exact callback and surface-damage behavior for same-buffer reattachment.

- [ ] **Step 1: Add content-identity regressions**

Add these content and protocol tests:

| Test | Exact assertion |
|---|---|
| `same_buffer_and_same_content_epoch_does_not_submit` | Worker admission and KMS submit counters remain unchanged. |
| `same_buffer_with_new_content_epoch_submits_new_direct_transaction` | The new key differs and one new direct transaction is built. |
| `metadata_only_commit_keeps_content_epoch` | Metadata-only state leaves `ContentEpochId` unchanged. |
| `no_visual_change_does_not_emit_presentation_feedback` | The terminal reason is `NoVisualChange` and the feedback count is zero. |
| `no_visual_change_callbacks_follow_existing_refresh_contract` | The original callback owner is settled once and no unpublished owner remains. |

- [ ] **Step 2: Centralize direct content comparison**

Add a pure helper:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirectContentDisposition {
    NewContent,
    MatchesPresented,
    MatchesQueuedOrSubmitted,
}

pub(crate) fn classify_direct_content(
    candidate: DirectScanoutCandidateKey,
    presented: Option<DirectScanoutCandidateKey>,
    pending: Option<DirectScanoutCandidateKey>,
) -> DirectContentDisposition;
```

Do not compare only wl_buffer identity. Use the complete candidate key, including content epoch and output generation.

- [ ] **Step 3: Settle same-content without creating a KMS job**

When callbacks or protocol obligations require a transaction despite no visual change, build the existing logical transaction and settle it as:

```rust
OutputTransactionDropReason::NoVisualChange
```

When no obligations exist, return the direct attempt's unchanged result without allocating a transaction.

Never create a pageflip token or worker admission for identical content.

- [ ] **Step 4: Preserve same-buffer new-content publication**

Verify the surface commit path increments the content epoch when the client reattaches or republishes the same buffer with new visual content. Metadata-only state changes must not increment it.

Do not add application-specific exceptions for Steam, Wine, Proton, Firefox, or games.

- [ ] **Step 5: Keep presentation feedback pageflip-only**

Search every direct path:

```bash
rg -n "presentation_feedback|complete_direct_presented|mark_presented|accept_presented" \
    src/native_output src/compositor
```

Every direct feedback call must be reachable only after validated pageflip completion. Add assertions in tests that test rejection, submit rejection, no-visual-change, supersede, and safe abandonment produce zero presentation feedback.

- [ ] **Step 6: Run content and obligation suites**

```bash
cargo test --locked same_buffer_and_same_content_epoch_does_not_submit
cargo test --locked same_buffer_with_new_content_epoch_submits_new_direct_transaction
cargo test --locked no_visual_change_does_not_emit_presentation_feedback
cargo test --locked native_output::tests::presentation_transactions
cargo test --locked compositor::state::surfaces
```

Expected: all tests pass and `same_buffer_resubmissions` remains zero in deterministic cadence tests.

- [ ] **Step 7: Commit**

```bash
git add src/native_output/scanout/atomic_egl_gbm/direct.rs \
        src/native_output/presentation/transaction.rs \
        src/native_output/presentation/ledger.rs \
        src/native_output/runtime/presentation_transactions.rs \
        src/compositor/state/surface_commits.rs \
        src/compositor/state/surfaces.rs \
        src/native_output/tests/direct_scanout_stage4.rs \
        src/native_output/tests/presentation_transactions.rs
git commit -m "fix(presentation): suppress duplicate direct content exactly"
```

---

### Task 9: Align dmabuf feedback and add Stage 4 observability

**Files:**
- Modify: `src/compositor/dmabuf.rs`
- Modify: `src/native_output/scanout/feedback.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `src/native_output/scanout/atomic_direct.rs`
- Modify: `src/native_output/scanout/direct_policy.rs`
- Create: `bin/qualify-direct-scanout`
- Test: `src/compositor/tests/protocol_buffers.rs`
- Test: `src/native_output/tests/direct_scanout_stage4.rs`

**Interfaces:**
- Produces: primary-plane scanout tranche followed by renderer tranche.
- Produces: stable structured Stage 4 metric names.
- Produces: `bin/qualify-direct-scanout`.

- [ ] **Step 1: Add feedback ordering and capability tests**

Add tests asserting:

```text
scanout tranche precedes render tranche
scanout tranche uses the selected DRM device
scanout tranche excludes renderer-only formats
scanout tranche excludes unsupported modifiers
feedback rebuilds after output generation change
```

Expected RED: current feedback lacks one or more exact capability constraints.

- [ ] **Step 2: Build scanout feedback from qualified primary-plane capabilities**

Use the primary-plane format/modifier set already discovered by the KMS path. Intersect it with the direct importer and current conservative Stage 4 policy.

Do not advertise scaling, transform, cross-device, color-conversion, or overlay capabilities.

- [ ] **Step 3: Add stable metrics**

Expose at least:

```text
direct_scanout_first_blocker
direct_scanout_blocker_set
direct_scanout_worker_admission_rejected
direct_scanout_live_leases
direct_scanout_validation_cache_hits
direct_scanout_validation_cache_misses
direct_scanout_test_only_attempts
direct_scanout_test_only_rejections
direct_scanout_real_submit_rejections
direct_scanout_entries
direct_scanout_replacements
direct_scanout_exits
direct_scanout_composited_fallbacks
direct_scanout_fallback_cycles
direct_scanout_same_content_suppressed
direct_scanout_same_buffer_resubmissions
direct_scanout_duplicate_feedback
direct_scanout_duplicate_settlement
direct_scanout_early_release_prevented
direct_scanout_presented_surface
direct_scanout_presented_framebuffer
direct_scanout_presented_content_epoch
```

Use counters or bounded strings compatible with the existing perf logger. Do not allocate the full blocker set unless debug or qualification mode requests it.

- [ ] **Step 4: Implement the qualification script**

Create `bin/qualify-direct-scanout` with `set -euo pipefail`.

The script must:

1. accept a perf log path as its first argument;
2. fail when the file is missing;
3. parse the final structured counters;
4. require at least one direct entry, presentation, replacement, exit, and composited fallback;
5. require zero duplicate feedback, duplicate settlement, early release, same-buffer resubmission, queue overflow, and callback-owner leak;
6. print a compact pass/fail table;
7. exit nonzero on any failed invariant.

Use explicit `awk`/`rg` parsing matching the actual emitted field syntax. Add a `--self-test` mode containing a passing and failing embedded fixture so shell behavior is deterministic in CI.

- [ ] **Step 5: Validate metrics and shell syntax**

```bash
cargo test --locked dmabuf
cargo test --locked direct_scanout_stage4
bash -n bin/qualify-direct-scanout
./bin/qualify-direct-scanout --self-test
```

Expected: all commands pass.

- [ ] **Step 6: Commit**

```bash
git add src/compositor/dmabuf.rs \
        src/native_output/scanout/feedback.rs \
        src/native_output/runtime/metrics.rs \
        src/native_output/scanout/atomic_direct.rs \
        src/native_output/scanout/direct_policy.rs \
        src/native_output/tests/direct_scanout_stage4.rs \
        bin/qualify-direct-scanout
git commit -m "feat(native): qualify direct scanout ownership and feedback"
```

---

### Task 10: Remove the legacy direct state and run the full deterministic gate

**Files:**
- Modify: `src/native_output/scanout/atomic_direct.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Modify: `src/native_output/scanout/direct.rs`
- Modify: `src/native_output/runtime/presentation.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/kms_worker.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `docs/KNOWN_ISSUES.md` if present
- Modify: `README.md` only if it currently documents direct scanout flags

**Interfaces:**
- Consumes: Tasks 1 through 9.
- Produces: one authoritative Direct Scanout 2.0 implementation with no legacy parallel lifecycle.

- [ ] **Step 1: Delete superseded types and methods**

Remove every remaining use of:

```text
PreparedDirectFrame
WorkerQueuedDirectFrame
SubmittedDirectFrame
PresentedDirectFrame
TestedDirectPlanePlan
DirectPlanePlanKey
DirectScanoutState.worker_queued
DirectScanoutState.pending
main-thread direct TEST_ONLY
inline direct submit
```

Keep only:

```text
DirectFramebufferCache
DirectPlaneValidationCache
DirectPrimaryLease
DirectPrimaryOwnership
DirectScanoutControl
DirectScanoutCounters
Direct scanout policy and qualification state
```

- [ ] **Step 2: Prove there is one lifecycle authority per state**

Run:

```bash
rg -n "worker_queued.*Direct|PreparedDirectFrame|SubmittedDirectFrame|TestedDirectPlanePlan|test_atomic_primary_flip_with_cursor" src
rg -n "DirectPrimary" src/native_output | sort
```

Expected: the first search returns no legacy direct lifecycle or main-thread test call. Review the second search and confirm every occurrence belongs to transaction description, worker ownership, arbiter routing, physical submitted/presented ownership, metrics, or tests.

- [ ] **Step 3: Add final invariant assertions**

At runtime debug boundaries, assert:

```text
a DirectPrimary job always has one direct lease
non-direct jobs never have a direct lease
one transaction ID is not both queued and submitted
presented direct ownership matches the last presented direct transaction
no direct transaction remains active after terminal settlement
no replaced direct lease remains live after a confirming replacement pageflip
```

Use debug assertions plus structured counters; do not turn recoverable user-space scanout rejection into a release-build panic.

- [ ] **Step 4: Run the complete deterministic gate**

```bash
cargo fmt --check
cargo test --locked
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
./bin/check-source-layout
git diff --check
bash -n bin/qualify-kms-worker
bash -n bin/qualify-direct-scanout
./bin/qualify-direct-scanout --self-test
```

Expected: every command exits zero, with no ignored Stage 4 regression tests.

- [ ] **Step 5: Review source boundaries**

```bash
wc -l \
  src/native_output/scanout/atomic_egl_gbm.rs \
  src/native_output/scanout/atomic_egl_gbm/direct.rs \
  src/native_output/scanout/atomic_direct.rs \
  src/native_output/runtime/presentation.rs \
  src/native_output/runtime/kms_worker.rs
```

Expected: every production file is at or below 1,500 lines and each file has one clear responsibility.

- [ ] **Step 6: Update documentation and commit**

Document:

- direct scanout remains experimental and default `Off`;
- it requires the KMS worker during Stage 4 qualification;
- supported candidate constraints;
- explicit exclusions;
- qualification command and expected report;
- rejection always falls back to composition.

```bash
git add src docs README.md bin/qualify-direct-scanout
git commit -m "refactor(native): finalize Direct Scanout 2.0"
```

Use `git status --short` before committing and exclude unrelated files.

---

### Task 11: Run the real Direct Scanout 2.0 qualification matrix

**Files:**
- Produce: `artifacts/qualification/direct-scanout-stage4-2026-07-26.log`
- Produce: `artifacts/qualification/direct-scanout-stage4-2026-07-26.md`

**Interfaces:**
- Consumes: the fully verified Stage 4 implementation.
- Produces: the evidence required to close Stage 4 and begin Triple Buffering 2.0.

- [ ] **Step 1: Start the qualified configuration**

Launch on a real TTY with:

```bash
OBLIVION_ONE_KMS_COMMIT_WORKER=force
OBLIVION_ONE_DIRECT_SCANOUT=experimental-auto
TYPHON_PERF_LOG=1
TYPHON_DIRECT_SCANOUT_DEBUG=1
```

Use the normal atomic EGL/GBM backend, hardware cursor, one 1920x1080@165 Hz output, and the user's production-like shell session.

- [ ] **Step 2: Run the application matrix**

For each application, verify entry, steady state, overlays, fallback, re-entry, and exit:

```text
Palworld
Steam UI plus one Proton game
Firefox fullscreen video
Firefox fullscreen WebGL
Kitty fullscreen
one additional native Vulkan game
```

- [ ] **Step 3: Run transition stress**

Perform at least 50 iterations each:

```text
fullscreen enter/exit
Alt+Tab away/back
open/close Steam popup over fullscreen
open/close shell volume overlay
move hardware cursor continuously
change cursor image over client surfaces
resize then immediately enter fullscreen
```

Expected: no black frame, stale image, cursor disappearance, duplicate feedback, or stuck direct state.

- [ ] **Step 4: Re-run deterministic failure suites on the qualification commit**

Before collecting the real-session counters, run:

```bash
cargo test --locked direct_test_rejection
cargo test --locked direct_real_submit_rejection
cargo test --locked direct_ebusy_retry
cargo test --locked direct_shutdown
cargo test --locked direct_session_suspend
```

Expected: every known-no-submit failure returns to composition, shutdown and suspend retain unsafe resources until restore, and no test emits duplicate feedback or settlement.

- [ ] **Step 5: Run the qualification script**

```bash
mkdir -p artifacts/qualification
PERF_LOG=${TYPHON_PERF_LOG_PATH:?set TYPHON_PERF_LOG_PATH to the real session perf log}
./bin/qualify-direct-scanout "$PERF_LOG" \
  2>&1 | tee artifacts/qualification/direct-scanout-stage4-2026-07-26.log
```

Required results:

```text
direct entries > 0
direct presentations > 0
direct replacements > 0
direct exits > 0
composited fallbacks > 0
same-buffer resubmissions = 0
duplicate presentation feedback = 0
duplicate transaction settlement = 0
early resource release = 0
worker queue overflow = 0
callback owners leaked = 0
active submitted transactions after shutdown = 0
fallback latency <= 1 additional presentation cycle
```

- [ ] **Step 6: Write the final report**

Generate measured metadata and the report skeleton:

```bash
COMMIT=$(git rev-parse HEAD)
GPU=$(lspci -nnk | awk '/VGA compatible controller|3D controller/{print; capture=1; next} capture && /Kernel driver in use/{print; exit}')
CONNECTOR=$(for status in /sys/class/drm/card*-*/status; do [ "$(cat "$status")" = connected ] && printf '%s ' "${status%/status}"; done)
MODE=$(for mode in /sys/class/drm/card*-*/modes; do [ -s "$mode" ] && { head -n1 "$mode"; break; }; done)
cat > artifacts/qualification/direct-scanout-stage4-2026-07-26.md <<EOF_REPORT
# Direct Scanout 2.0 Stage 4 Qualification

- Commit: \`$COMMIT\`
- Date: 2026-07-26
- GPU and driver: \`$GPU\`
- Connected DRM path: \`$CONNECTOR\`
- Advertised mode: \`$MODE\`
- KMS worker policy: \`force\`
- Direct scanout policy: \`experimental-auto\`

## Application matrix

| Application | Entry | Steady | Overlay fallback | Re-entry | Exit | Result |
|---|---|---|---|---|---|---|
After the header, append exactly six application rows in the order listed in Step 2. Every result cell must contain the literal observed value `PASS` or `FAIL`.

## Deterministic failure suites

Record the exact Cargo commands and pass counts from Step 4.

## Counter summary

Copy every invariant checked by \`bin/qualify-direct-scanout\`.

## Visual defects

Write \`none\` or list each observed black frame, stale frame, cursor defect, or flicker with reproduction steps.

## Verdict

Write \`PASS\` only when every application, deterministic failure suite, and counter passes; otherwise write \`FAIL\`.
EOF_REPORT
```

Before committing, verify the application table contains six rows, no empty cells, and only measured `PASS` or `FAIL` result values.

- [ ] **Step 7: Commit qualification evidence**

```bash
git add artifacts/qualification/direct-scanout-stage4-2026-07-26.log \
        artifacts/qualification/direct-scanout-stage4-2026-07-26.md
git commit -m "test(native): qualify Direct Scanout 2.0"
```

Stage 4 is closed only after this commit reports `PASS`. The next roadmap item is Triple Buffering 2.0.
