# Pending Surface Mapping State Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Project the exact ordered pending SurfaceTree mapping state into separate incoming transaction validation and attachment preparation.

**Architecture:** Extend the existing `EffectiveSurfaceMappingState` path with a read-only pending-prefix projection. Select predecessor transactions from same-root FIFO order, pending external Content Update dependencies, same-surface lineage, and transitive ordering edges; topologically replay their commit metadata before applying the candidate. Keep publication, cancellation, explicit-sync settlement, canonicalization, and merge boundaries unchanged.

**Tech Stack:** Rust, Cargo, Wayland compositor state tests, Codebase Memory graph.

## Global Constraints

- Preserve all F07, F08, and existing F09 coordinate/damage, cumulative, canonicalization, and merge semantics.
- Do not merge transactions to solve the defect; PacingProtected, Commit Timing, FIFO, merge-frozen, CU-DAG, and ready-transaction boundaries remain exact.
- Projection is side-effect-free: do not mutate `SurfaceData`, `CurrentSurfaceBuffer`, buffers, acquire watches, callbacks, feedbacks, publication lifetimes, or protocol-error state.
- Use actual same-root and Content Update dependency ordering, not raw global pending-vector position alone.
- Preserve historical `viewport_error_owner`, exactly-once explicit-sync ownership, and existing cancellation behavior.
- Work in the current checkout without creating or switching branches, preserve unrelated dirty changes, use `rtk`, and create one focused commit.

---

### Task 1: Add and verify separate-transaction regressions

**Files:**
- Modify: `src/compositor/state/subsurfaces.rs:120-2085` — test helpers and F09 regression tests.

**Interfaces:**
- Consumes: existing `EffectiveSurfaceMappingState`, `PendingSurfaceTreeTransaction`, queue/readiness helpers, and Wayland test resources.
- Produces: RED/GREEN coverage for separate destination, scale, transform, viewport composition, historical owner, and cancellation semantics.

- [x] **Step 1: Write the failing tests**

  Add tests that queue a real `fifo_set_barrier` predecessor behind an external Content Update dependency, submit a later same-root attachment or viewport operation, assert two transactions remain separate, and verify the expected projected mapping/error behavior. The tests cover:

  ```rust
  separate_pending_destination_prepares_later_attachment_against_predecessor
  separate_pending_scale_prepares_later_attachment_at_predecessor_scale
  separate_pending_transform_prepares_later_attachment_at_predecessor_transform
  separate_pending_viewport_reset_accepts_final_valid_composition
  separate_pending_viewport_reset_rejects_final_invalid_composition_with_source_owner
  canceling_surface_retires_predecessor_and_later_projected_transaction_together
  ```

- [x] **Step 2: Run focused tests to verify RED**

  Run:

  ```bash
  rtk cargo test --locked separate_pending -- --nocapture
  rtk cargo test --locked canceling_surface_retires_predecessor_and_later_projected_transaction_together -- --nocapture
  ```

  Expected: the new tests fail only on stale mapping/preparation or the resulting stale validation decision; the test target compiles.

### Task 2: Implement ordered pending-prefix projection

**Files:**
- Modify: `src/compositor/state/subsurfaces.rs:33-109, 2896-2988, 3485-3552` — projection selection and effective-state derivation.
- Modify: `src/compositor/state/surface_tree_readiness.rs:92-158` — invalidate dependent local queue entries after asynchronous predecessor rejection.
- Modify: `src/compositor/state/subsurfaces.rs:3065-3258` — pass the incoming root/dependency context through validation and preparation while retaining merge behavior.

**Interfaces:**
- Consumes: `EffectiveSurfaceMappingState::from_surface`, `EffectiveSurfaceMappingState::apply_commit`, `PendingSurfaceTreeTransaction::root_surface_id`, transaction external dependencies, and Content Update lineage references.
- Produces: a private read-only helper that returns pending transactions in scheduler-derived publication order and a `derive_surface_tree_surface_state` path that starts from that projected state.

- [x] **Step 1: Define the projection contract**

  Add a helper with an explicit root/dependency interface, for example:

  ```rust
  fn pending_surface_tree_mapping_prefix(
      &self,
      root_surface_id: u32,
      nodes: &[(u32, CachedSubsurfaceCommit)],
      external_content_update_dependencies: &[ContentUpdateRef],
  ) -> Vec<&PendingSurfaceTreeTransaction>
  ```

  Select all existing transactions for the incoming root, pending transactions covering incoming external/lineage references, and recursively selected transactions' own predecessors. Add scheduler edges for same-root FIFO order and Content Update predecessor ownership, then topologically order the selected prefix with the original vector index only as a deterministic tie-break. Do not select unrelated roots or unrelated same-surface work.

- [x] **Step 2: Replay only metadata into effective states**

  Seed each candidate surface from `EffectiveSurfaceMappingState::from_surface`. Replay selected transaction nodes using `apply_commit`, ignoring returned mappings for predecessor attachments. Treat an invariant violation in an already-admitted predecessor as a debug assertion/error return; do not post an error, release a buffer, or mutate any live state.

- [x] **Step 3: Thread the projected base through validate/prepare**

  Change `validate_surface_tree_surface_state` and `prepare_surface_tree_surface_state` to accept the incoming root and external dependency slice. Have both call the projected `derive_surface_tree_surface_state`, so validation and `PendingSurfaceBuffer::apply_content_mapping` see the same base. Pass transaction context in the merged re-preparation call; the merged target is removed before re-preparation, so its own nodes remain the candidate state rather than being replayed as a predecessor.

- [x] **Step 4: Cover the earlier unsynchronized admission check**

  Update `surface_mapping_error_for_commit` to use the same projected state for the one commit (including its captured same-surface lineage) instead of validating only against committed `SurfaceData`. Preserve the existing error tuple and historical owner routing.

- [x] **Step 5: Run the focused GREEN cycle**

  Run:

  ```bash
  rtk cargo test --locked separate_pending -- --nocapture
  rtk cargo test --locked canceling_surface_retires_predecessor_and_later_projected_transaction_together -- --nocapture
  rtk cargo test --locked pending_transaction_merge_reprepares_against_its_effective_mapping -- --nocapture
  ```

  Expected: all new tests and the existing merge re-preparation regression pass.

### Task 3: Audit lifecycle/order callers and run relevant suites

**Files:**
- Inspect and, only if required by the projection contract, modify: `src/compositor/state/surface_tree_readiness.rs`, `src/compositor/state/surface_transactions.rs`, `src/compositor/state/surfaces.rs`, `src/compositor/state/frames.rs`, and relevant compositor tests.

**Interfaces:**
- Consumes: `commit_ready_surface_tree_transactions`, `publish_surface_tree_nodes`, `publish_surface_tree`, `apply_cached_subsurface_commit`, surface/root cancellation paths, and explicit-sync readiness.
- Produces: verified agreement between prepared metadata and ordered publication, with no readiness, cancellation, callback, feedback, or acquire ownership regression.

- [x] **Step 1: Trace the complete lifecycle**

  Re-query Codebase Memory callers for `derive_surface_tree_surface_state`, `prepare_surface_tree_surface_state`, `merge_or_queue_surface_tree_transaction`, `queue_waiting_surface_tree_with_lifetimes`, `commit_ready_surface_tree_transactions`, `publish_surface_tree_nodes`, and `publish_surface_tree`; inspect the exact source and coverage for every relied-on path.

- [x] **Step 2: Verify cancellation and explicit-sync invariants**

  Confirm that surface/root teardown retires every transaction containing a projected predecessor surface before any later transaction can publish, and that projection never touches acquire state. Run focused lifecycle, explicit-sync, and cancellation tests.

- [x] **Step 3: Run relevant suites**

  Run focused suites for SurfaceTree/subsurface lifecycle, PacingProtected, Commit Timing/FIFO, explicit sync, viewport protocol, surface mapping, F07, F08, and F09 rendering/damage. Record exact command results and counts.

### Task 4: Final verification and focused commit

**Files:**
- Modify: `docs/superpowers/specs/2026-09-17-pending-surface-mapping-state-design.md` and `docs/superpowers/plans/2026-09-17-pending-surface-mapping-state.md` are already written; include them only with this focused change if they are retained.
- Modify: production/test files from completed tasks.

**Interfaces:**
- Consumes: all completed implementation and regression changes.
- Produces: one focused commit with fresh verification evidence and an exact final report.

- [x] **Step 1: Run repository verification**

  Run:

  ```bash
  rtk cargo fmt --check
  rtk cargo check --locked --all-targets
  rtk cargo clippy --locked --all-targets -- -D warnings
  rtk cargo test --locked
  rtk git diff --check
  ```

- [x] **Step 2: Review source layout observationally**

  Inspect the changed regions for layout only; do not repair unrelated historical layout debt.

- [x] **Step 3: Commit the focused change**

  ```bash
  git add docs/superpowers/specs/2026-09-17-pending-surface-mapping-state-design.md docs/superpowers/plans/2026-09-17-pending-surface-mapping-state.md src/compositor/state/subsurfaces.rs src/compositor/state/surface_tree_readiness.rs
  git commit -m "fix(compositor): project pending surface mapping state"
  ```
