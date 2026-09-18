# Presentation Geometry Micro-Closure Implementation Plan

> **For agentic workers:** This plan is executed inline in the current session because the repository instructions prohibit sub-agents. Each task is kept independently testable.

**Goal:** Make presentation output identity explicit and prevent effective geometry no-ops from allocating or retaining presentation work.

**Architecture:** Keep `PendingPresentationGeometryTransaction` responsible for canonical first-start/final-target merging. Put effective-work filtering in `PresentationEngine::commit`, after deterministic validation and before transaction/revision ID reservation. Preserve active-track sampling, velocity-retaining retargets, exact physical ACK retirement, and existing SceneNode ownership.

**Tech Stack:** Rust, Cargo, native compositor unit/integration tests, Codebase Memory MCP, `rtk` command wrapper.

## Global Constraints

- Keep build artifacts in the repository's existing `target/` directory.
- Use explicit caller-supplied `OutputId` for every empty presentation sample or frame snapshot.
- Validate owner, duplicate property, and geometry validity before no-op filtering.
- Do not allocate transaction/revision IDs, update metrics, or mutate engine state for a fully elided request.
- Preserve active moving-track retargeting and settled-but-unACKed exact-revision lifecycle.
- Treat layout `PresentationTransactionError::Empty` as a valid effective no-op; retain invariant handling for every other error.
- Do not alter canonical layout, physical `NativeSceneHistory` authority, input authority, curve/timing semantics, signatures, Direct Scanout policy, or deferred generalized properties.

---

### Task 1: Make empty presentation identity explicit

**Files:**
- Modify: `src/presentation_animation/frame.rs`
- Modify: `src/native_output/tests/fullscreen_frame_scene.rs`
- Modify: `src/native_output/tests/output_retry.rs`
- Modify: `src/native_output/tests/output.rs`
- Modify: `src/native_output/runtime/frame_tests.rs`
- Modify: `src/native_output/runtime/frame_scene_identity_tests.rs`
- Modify: `src/native_output/runtime/scene_history.rs`
- Test: `src/presentation_animation/transactions_tests.rs`

**Interfaces:**
- Preserve `PresentationSceneSample::empty_for_output(output_id, sampled_at, sample_time_source)`.
- Remove `PresentationSceneSample::empty(sampled_at)`.
- Add `PresentationFrameSnapshot::empty_for_output(output_id) -> Self`, using zero time and `ZeroFallback` without manufacturing an output identity.
- Keep test-only synthetic IDs explicit at their call sites and use each surrounding snapshot/frame's authoritative output ID.

- [ ] **Step 1: Add failing identity and API tests.** Add tests that construct empty samples for two explicit output IDs and assert each ID is preserved; construct empty frame snapshots for both IDs and assert each ID is preserved. Add a source/API assertion that the implicit empty sample/snapshot constructors are absent through compilation call-site migration and a final source search.

- [ ] **Step 2: Run the focused presentation tests and observe the intended failure.**

Run: `rtk cargo test --locked presentation_animation::transactions_tests`

Expected: the new empty-snapshot API tests fail to compile or the old constructors remain available until the implementation is changed.

- [ ] **Step 3: Replace the production helpers and migrate fixtures.** Remove the implicit sample helper, add `empty_for_output`, and update every production-compiled test fixture using the exact `output_id` field/variable already present in its enclosing frame or history record. Keep unrelated explicit `OutputId::from_raw(1)` test data unchanged.

- [ ] **Step 4: Run the focused identity suites.**

Run: `rtk cargo test --locked presentation_animation::transactions_tests native_output::runtime::frame_tests native_output::runtime::frame_scene_identity_tests native_output::runtime::scene_history native_output::tests::fullscreen_frame_scene native_output::tests::output_retry`

Expected: all selected tests pass and no implicit empty constructor call remains.

- [ ] **Step 5: Commit the identity closure.**

```bash
git add src/presentation_animation/frame.rs src/presentation_animation/transactions_tests.rs src/native_output/tests/fullscreen_frame_scene.rs src/native_output/tests/output_retry.rs src/native_output/tests/output.rs src/native_output/runtime/frame_tests.rs src/native_output/runtime/frame_scene_identity_tests.rs src/native_output/runtime/scene_history.rs
git commit -m "fix: require explicit presentation output identity"
```

### Task 2: Elide effective geometry no-ops in the engine

**Files:**
- Modify: `src/presentation_animation/engine.rs`
- Modify: `src/presentation_animation/transactions_tests.rs`

**Interfaces:**
- `PresentationEngine::commit` remains the validation and effective-work boundary.
- `PresentationRect::is_identity_with` remains the semantic geometry identity helper.
- Existing test-only ID/track/transaction inspection helpers may be extended narrowly for allocator and membership assertions.

- [ ] **Step 1: Add failing engine regressions.** Cover inactive `A -> A`, mixed inactive identity plus effective member, active moving same-target canonical mutation with sampled rect/velocity preservation, and mathematically-settled same-target mutation that retains the old revision and transaction membership until exact ACK. Assert metrics, active tracks, records, and allocator peeks remain unchanged for fully elided work.

- [ ] **Step 2: Run the new tests before changing production code.**

Run: `rtk cargo test --locked presentation_animation::transactions_tests`

Expected: inactive identity requests currently allocate tracks/IDs and settled same-target requests currently churn revisions, so the new assertions fail.

- [ ] **Step 3: Filter only after validation and active-track sampling.** In `commit`, validate owner/duplicate/finite geometry first. For an absent track, skip only when `mutation.start.is_identity_with(mutation.target)`. For an existing track, sample at `request.started_at`; skip only when the sample is mathematically settled and its sampled rect equals the target. Otherwise push the sampled start, velocity, target, curve, and preserve-velocity flag into `prepared`.

- [ ] **Step 4: Return effective-empty before ID reservation.** After preparation, return `Err(PresentationTransactionError::Empty)` if `prepared.is_empty()`. Leave allocator state, tracks, transactions, metrics, and prior settled tracks untouched. Keep the existing prepare-then-commit reservation and mutation ordering for effective members.

- [ ] **Step 5: Run the engine suite and refactor only after green.**

Run: `rtk cargo test --locked presentation_animation::transactions_tests`

Expected: all existing transaction, retarget, exact ACK, and ID exhaustion tests plus the new no-op tests pass.

- [ ] **Step 6: Commit the engine closure.**

```bash
git add src/presentation_animation/engine.rs src/presentation_animation/transactions_tests.rs
git commit -m "fix: elide ineffective geometry presentation work"
```

### Task 3: Preserve layout batch semantics and blocker invariants

**Files:**
- Modify: `src/compositor/state/surfaces.rs`
- Modify: `src/compositor/state/tiled_layout_tests.rs`

**Interfaces:**
- `PendingPresentationGeometryTransaction` continues preserving its first canonical start and final canonical target.
- `finish_layout_reflow_batch` accepts `Ok(record)` and `Err(PresentationTransactionError::Empty)` as valid presentation outcomes and keeps every other error under the existing debug invariant.
- Direct Scanout candidate policy is unchanged; tests observe `PresentationEngine::active_count`, `has_track`, `has_pending_visible`, and existing compositor blocker queries.

- [ ] **Step 1: Add the failing net-zero layout regression.** Build a one-window layout batch that merges `A -> B` and `B -> A`, finish the batch, and assert no track, transaction, pending visible work, or animation Direct Scanout blocker is created. Keep the canonical layout/render-generation assertions appropriate to the existing helper.

- [ ] **Step 2: Run the layout test before the completion change.**

Run: `rtk cargo test --locked compositor::state::tiled_layout_tests::nested_layout_batch_merges_duplicate_geometry_before_outer_commit compositor::state::tiled_layout_tests::net_zero_layout_batch_elides_presentation_work`

Expected: the new test fails because the engine currently installs a no-op track and `finish_layout_reflow_batch` asserts that the commit must be `Ok`.

- [ ] **Step 3: Accept only effective-empty in layout completion.** Capture the commit result, keep `Ok(_)` and `Err(PresentationTransactionError::Empty)` as valid outcomes, and preserve the debug assertion/panic for `Disabled`, owner, duplicate, invalid geometry, and ID exhaustion errors.

- [ ] **Step 4: Run the focused layout, Direct Scanout, and native presentation suites.**

Run: `rtk cargo test --locked compositor::state::tiled_layout_tests compositor::state::direct_scanout native_output::runtime::presentation native_output::runtime::scene_history native_output::tests::presentation_transactions`

Expected: the net-zero batch is inert while effective reflows, blockers, physical ACKs, and cleanup remain unchanged.

- [ ] **Step 5: Commit the layout closure.**

```bash
git add src/compositor/state/surfaces.rs src/compositor/state/tiled_layout_tests.rs
git commit -m "fix: accept effective-empty layout presentation batches"
```

### Task 4: Update status and run the complete verification gate

**Files:**
- Modify: `docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md`

- [ ] **Step 1: Search and classify forbidden constructor patterns.** Search presentation production code for `OutputId::from_raw(1)`, `PresentationSceneSample::empty(`, and `PresentationFrameSnapshot::empty()`. Confirm only explicit test fixtures or test-only compatibility helpers remain, and no production presentation constructor manufactures output 1.

- [ ] **Step 2: Run focused regression suites.** Run presentation transactions/math/retarget tests, Dwindle/layout batch tests, physical ACK and scene-history tests, Direct Scanout presentation blocker tests, XWayland geometry continuity, input/presented geometry, native frame tests, and scheduler/presentation runtime tests using the repository's current module filters.

- [ ] **Step 3: Run the fresh full gate in the checkout.**

```bash
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk run ./bin/check-source-layout
```

Record pre-existing source-layout debt separately and confirm no new debt was introduced.

- [ ] **Step 4: Update the approved design status.** Record explicit output identity removal and effective no-op elision as implemented, mark Geometry v2 closed, and leave Opacity, Clip, retained lifecycle/Lamp migration, and new effects pending. Do not claim the generalized roadmap is complete.

- [ ] **Step 5: Verify the final diff and commit only owned files.** Confirm unrelated dirty files remain unstaged and unchanged, review the final diff, rerun any required checks after documentation edits, and commit the task-owned changes.

```bash
git status --short
git diff --check
git diff -- src/presentation_animation src/compositor/state/surfaces.rs src/compositor/state/tiled_layout_tests.rs src/native_output docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md
git add docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md
git commit -m "docs: close geometry presentation engine v2"
```
