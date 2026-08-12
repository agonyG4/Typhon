# Typhon Realtime Commit Timing Closure Implementation Plan

> Approved implementation brief: preserve FIFO, surface transactions, frame claims, Direct Scanout, and existing pacing behavior while closing Realtime clock-domain and identity gaps.

**Goal:** Make Commit Timing correct for Monotonic and Realtime presentation clocks, use exact ordered-transaction identity, preserve active claims through native submission, fix non-evictable queue admission, and extract ordered readiness from `frames.rs`.

**Architecture:** Keep `CommitTimingConstraint` as the protocol-domain value. Add a typed clock sample and scheduler mapping that produces an explicit Monotonic target, and pass a planning candidate containing transaction ID plus original constraint into native planning. Add `SurfaceTreeTransactionId` allocated by `CompositorState`, arm readiness by ID, revalidate Realtime mappings before release and pre-submit, and expose only a narrow active-guard query. Move ordered transaction progression into `surface_tree_readiness.rs` without changing publication semantics.

**Tech Stack:** Rust 2024, existing compositor/native scheduler, deterministic unit/integration tests, cargo formatting/checking/linting/testing, and source-layout validation.

## Global Constraints

- Preserve FIFO v1 and existing surface transaction semantics.
- Preserve the original `(seconds, nanoseconds)` protocol timestamp in the advertised presentation-clock domain.
- Never compare Realtime absolute timestamps directly with Monotonic scheduler timestamps.
- Use exact `SurfaceTreeTransactionId`; timestamp equality is never identity.
- Keep target invalidation/replanning finite and overflow-safe.
- Use patch-based edits and avoid unrelated refactors.

---

### Task 1: Baseline and deterministic mapping tests

**Files:**
- Modify: `src/compositor/state/surface_pacing.rs`
- Test: existing `surface_pacing` unit-test module

- [ ] Add synthetic clock-sample tests for future, due, backward, forward, repeated backward, maximum protocol timestamp, and unchanged Monotonic mapping behavior.
- [ ] Run the focused pacing tests and record the baseline/current failures before changing production behavior.

### Task 2: Exact surface-tree transaction identity

**Files:**
- Modify: `src/compositor/mod.rs`, `src/compositor/state/mod.rs`, `src/compositor/state/surface_transactions.rs`, `src/compositor/state/subsurfaces.rs`, `src/compositor/state/xdg_lifecycle.rs`, `src/compositor/state/surface_pacing.rs`, `src/compositor/state/frame_tests.rs`

- [ ] Add `SurfaceTreeTransactionId(u64)` and a monotonic `CompositorState` allocator with explicit overflow behavior.
- [ ] Add the ID to every `PendingSurfaceTreeTransaction` constructor and test fixture.
- [ ] Add uniqueness/lifecycle tests and replace any timestamp-based transaction lookup with ID-based lookup.

### Task 3: Planning candidate and exact target arming

**Files:**
- Modify: `src/compositor/state/surface_pacing.rs`, `src/compositor/state/mod.rs`, `src/compositor/server_frames.rs`, `src/native_output/runtime/planner.rs`
- Test: compositor pacing tests and native planner tests

- [ ] Define `CommitTimingPlanningCandidate` with transaction ID, original constraint, and typed Monotonic target.
- [ ] Select only current root heads; choose earliest mapped target and deterministic transaction-ID tie break.
- [ ] Replace `next_commit_timing_requested_ns` and both timestamp searches with candidate/ID APIs.
- [ ] Preserve original constraint in `CommitTimingReadiness` and active `CommitTimingTargetClaim`.

### Task 4: Realtime revalidation and native guard

**Files:**
- Modify: `src/compositor/state/surface_pacing.rs`, `src/compositor/server_frames.rs`, `src/native_output/runtime/planner.rs`, and the native submission boundary identified by the current claim flow
- Test: deterministic release, backward-jump, forward-jump, active-claim, pre-submit, and final presentation invariant tests

- [ ] Re-sample clocks before early render release and invalidate/replan stale backward mappings.
- [ ] Treat forward jumps as due without changing ordering or protocol truth.
- [ ] Add a narrow active-claim guard query and pre-submit validation that defers stale Realtime targets while retaining frame/claim ownership.
- [ ] Keep completion validation in the original presentation-clock domain and add bounded identity/mapping diagnostics.

### Task 5: Queue admission correction

**Files:**
- Modify: `src/compositor/state/subsurfaces.rs`, pacing metrics/tests

- [ ] Make a full non-evictable root queue request fatal resource exhaustion for every incoming transaction, including ordinary commits.
- [ ] Keep legal ordinary unready supersession and assert cleanup/absence of fake surface errors.

### Task 6: Extract ordered readiness

**Files:**
- Create: `src/compositor/state/surface_tree_readiness.rs`
- Modify: `src/compositor/state/mod.rs`, `src/compositor/state/frames.rs`, `src/compositor/state/subsurfaces.rs`

- [ ] Move only ordered root-head discovery, readiness, supersession, callback/resize carry, and publication progression helpers.
- [ ] Keep frame-batch/render ownership in `frames.rs` and leave `subsurfaces.rs` below the source-layout limit.

### Task 7: Documentation and full verification

**Files:**
- Modify: `docs/wayland/FRAME_PACING_V1.md`

- [ ] Document protocol/scheduler clock domains, sampled Realtime revalidation, exact transaction identity, and fatal admission behavior only as proven by tests.
- [ ] Run focused tests, then `cargo fmt --check`, `cargo check --locked --all-targets`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, `./bin/check-source-layout`, and `git diff --check` where the checkout supports Git.
- [ ] Inspect the final diff for stale mappings, untyped timestamps, identity aliasing, silent queue drops, FIFO changes, and source-layout regressions.
