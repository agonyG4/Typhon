# SurfaceTree Cumulative Mapping Preflight Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make SurfaceTree admission validate and prepare repeated same-surface Content Updates using the same cumulative mapping state that sequential publication observes.

**Architecture:** Keep exact candidate order through a side-effect-free per-surface scratch preflight. The scratch state carries viewport, scale, transform, effective retained/attached/empty content, and source error ownership. After validation succeeds, apply prepared attachment mappings; only then run the existing destructive canonicalizer for coalescible candidates and re-prepare the canonical nodes. Frozen and pacing-protected candidates remain exact. Publication consumes the prepared mapping and treats reconstruction failure as an internal invariant failure with cleanup, not as a client-triggerable panic.

**Tech Stack:** Rust, Cargo, Wayland protocol test harness, Codebase Memory, `rtk` command proxy.

## Global Constraints

- Work in the current Typhon checkout on the current `main` branch; do not create or switch branches.
- Preserve unrelated dirty changes; stage only files/hunks belonging to this blocker and its focused docs.
- Do not continue F07, redo F08, or redo existing F09 coordinate/damage work.
- Keep `PendingViewportChange::merge`, CU-DAG represented ranges, lineage contiguity, external dependency normalization, pacing boundaries, and explicit-sync ownership semantics unchanged.
- Do not mutate live `SurfaceData` during preflight.
- Use the current checkout as authoritative when Codebase Memory line ranges differ; re-index before final graph audit.
- Run builds in the checkout's existing target directory and invoke terminal commands through `rtk`.
- Do not use subagents.

---

### Task 1: Establish focused RED regressions

**Files:**
- Modify: `src/compositor/state/subsurfaces.rs` test module for unit-level exact-node regressions.
- Modify: `src/compositor/subsurface_cache_tests.rs` for production cache-freeze lifecycle coverage if the existing test does not expose both frozen nodes.
- Modify: `src/compositor/tests/protocol_error.rs` only for an end-to-end source-owner regression if unit-level resource construction cannot observe the protocol object identity.

**Interfaces:**
- Consume the existing `CachedSubsurfaceCommit`, `PendingSurfaceBuffer`, `capture_direct_child_dependencies`, `extract_content_update_candidate`, and `submit_surface_tree_nodes_with_kind` APIs.
- Produce named failing tests that exercise exact repeated same-surface candidates and record the pre-fix behavior before production changes.

- [ ] **Step 1: Add the minimal final-invalid regression.** Build two child commits in predecessor order. Cache A, call `capture_direct_child_dependencies(parent_id)` to freeze A, cache B, capture B through the same lifecycle, then extract and submit both. A sets fractional source `2.5 x 2.0` with destination `4 x 4`; B resets only destination. Assert admission rejects the candidate and that committed `SurfaceData`, `current_surface_buffers`, and renderable content remain unchanged.

- [ ] **Step 2: Run the regression and verify the expected RED.**

Run: `rtk cargo test --locked --lib final_invalid_merge_frozen_surface_tree_mapping -- --exact --nocapture`

Expected: FAIL because the stale per-node preflight accepts A and B independently or otherwise fails to reject the final fractional-source-without-destination state.

- [ ] **Step 3: Add the final-valid, attachment, scale, transform, NULL, and pacing regressions.** Use the same real freeze path for repeated child candidates. Cover source reset followed by destination reset from a committed fractional source; destination `50x50` followed by a new asymmetric `100x80` buffer; scale `2` followed by attachment; transform `90` followed by asymmetric attachment; NULL removal followed by a viewport-dependent update; and a pacing boundary followed by an attachment. Assert exact node count at protected boundaries and inspect `PendingSurfaceBuffer.surface_size`, `buffer_scale`, and `buffer_transform`.

- [ ] **Step 4: Add ownership and release assertions.** Ensure a destination-only failure after a source update reports ownership from the source update. Add a rejection case with a pending buffer and callback and assert the buffer release/callback completion cardinality is one after admission rejection. Use the existing client/server test state counters where unit ownership cannot observe Wayland object identity.

- [ ] **Step 5: Run the new focused tests and record all RED failures.**

Run: `rtk cargo test --locked --lib merge_frozen -- --nocapture`; `rtk cargo test --locked --lib pacing_protected_surface_mapping -- --nocapture`; `rtk cargo test --locked --test compositor protocol_error -- --nocapture`.

Expected: the newly added blocker tests fail for the missing cumulative behavior, while unrelated existing tests are not changed to manufacture failures.

### Task 2: Implement side-effect-free cumulative state derivation

**Files:**
- Modify: `src/compositor/state/subsurfaces.rs` near `prepare_surface_tree_surface_state` and `submit_surface_tree_nodes_with_kind`.
- Modify: `src/compositor/state_data.rs` near `SurfaceViewportCommit`, `SurfaceData`, and `PendingSurfaceBuffer` mapping helpers.
- Modify: `src/compositor/subsurface.rs` only if a small ownership/cached-state helper is required; do not alter cache merge policy.

**Interfaces:**
- Consume exact nodes in dependency order.
- Produce `EffectiveSurfaceMappingState` with `viewport`, `buffer_scale`, `buffer_transform`, `EffectiveContentState`, and `viewport_error_owner`.
- Produce side-effect-free `SurfaceContentMapping` values for each attached pending buffer, then apply those values in a second phase.

- [ ] **Step 1: Add the effective content representation and committed source-owner storage.** Represent retained current content, a newly attached buffer, and no content separately while storing only buffer dimensions needed for validation. Preserve the source owner when a destination-only change is applied; replace it only when a source delta, including an explicit reset, is applied.

- [ ] **Step 2: Add pure mapping derivation for pending buffers.** Factor buffer-size lookup and mapping construction so validation can derive `SurfaceContentMapping` without writing `PendingSurfaceBuffer` fields. Add an infallible application helper for a previously derived mapping and keep `apply_committed_surface_state` compatible with existing non-tree callers.

- [ ] **Step 3: Implement the cumulative validation pass.** Initialize one scratch state per surface from committed `SurfaceData` and `CurrentSurfaceBuffer`. For each node, apply viewport/scale/transform deltas, select effective content after attach/remove/no-change, validate without a buffer when empty, validate against the effective buffer otherwise, retain the correct owner, and advance the scratch state. Return the owner carried by the state that produced an error.

- [ ] **Step 4: Make attachment preparation two-phase.** Derive every mapping for the whole node slice first. If any derivation fails, return before mutating any pending buffer. After the pass succeeds, write mappings into the corresponding pending attachments.

- [ ] **Step 5: Place canonicalization after exact validation.** Validate exact nodes before any destructive ownership work. For coalescible nodes, canonicalize with existing `CachedSubsurfaceCommit::merge` machinery, then run the same derive/apply preparation against the canonical nodes so a surviving attachment uses the final merged viewport/scale/transform. Preserve exact nodes for pacing and `merge_frozen` boundaries.

- [ ] **Step 6: Run the final-invalid test to GREEN.**

Run: `rtk cargo test --locked --lib final_invalid_merge_frozen_surface_tree_mapping -- --exact --nocapture`

Expected: PASS, with no invalid live-state mutation and the source-owned viewport error selected.

### Task 3: Harden queued publication and transition behavior

**Files:**
- Modify: `src/compositor/state/surface_transactions.rs` at `apply_cached_subsurface_commit`.
- Modify: `src/compositor/state/subsurfaces.rs` at merge/reprepare call sites.
- Modify: `src/compositor/state_data.rs` only for the committed mapping snapshot API used by publication.

**Interfaces:**
- Consume prepared exact/canonical node mappings and existing explicit-sync/transaction ownership machinery.
- Produce publication that does not reconstruct ordinary invalid client mappings by silently converting errors to `None`.

- [ ] **Step 1: Reprepare merged transactions after their destructive merge.** Keep the existing transaction rejection cleanup, but invoke the cumulative derive/apply preparation after nodes are merged so pending transaction state matches its final publication state.

- [ ] **Step 2: Harden retained mapping reconstruction.** Replace the direct `.content_mapping_for_state(...).ok()` admission/publication behavior with explicit invariant handling: use the proven mapping when present, record a debug assertion or internal trace on an impossible failure, and release/abort conservatively without a production panic reachable from malformed client input.

- [ ] **Step 3: Verify content transitions.** Run the final-valid, attachment, scale, transform, NULL, and pacing tests and inspect pending metadata plus unchanged current/renderable state on rejection.

Run: `rtk cargo test --locked --lib valid_final_composed_viewport_is_not_rejected_by_an_independent_delta -- --exact --nocapture`; `rtk cargo test --locked --lib canonical_ -- --nocapture`; `rtk cargo test --locked --lib pacing_protected_surface_mapping -- --nocapture`; `rtk cargo test --locked --lib merge_frozen_surface_mapping -- --nocapture`.

Expected: all focused regressions pass; protected candidates remain exact; canonical candidates still merge only where legal.

### Task 4: Audit, verify, and commit

**Files:**
- Verify: all modified source/test files and current unrelated dirty worktree state.

**Interfaces:**
- Consume current source, Codebase Memory graph, focused test results, and full Cargo checks.
- Produce one focused code commit with no unrelated dirty file staged.

- [ ] **Step 1: Re-index and perform the Codebase Memory final audit.** Trace candidate extraction → cumulative preflight → mapping derivation → pending-buffer preparation → legal canonicalization → explicit-sync preparation → queue/publication. Search every caller of `prepare_surface_tree_surface_state`, `canonicalize_coalescible_surface_tree_nodes`, `PendingSurfaceBuffer::apply_committed_surface_state`, `SurfaceBufferMapping::new`, `viewport_for_change`, `buffer_scale_for_change`, and `buffer_transform_for_change`; check coverage for every relied-on path.

- [ ] **Step 2: Run the relevant suites.**

Run: `rtk cargo test --locked --lib subsurface`; `rtk cargo test --locked --lib surface_frames`; `rtk cargo test --locked --test compositor protocol_error`; `rtk cargo test --locked --lib explicit_sync`; `rtk cargo test --locked --lib f07`; `rtk cargo test --locked --lib f08`; `rtk cargo test --locked --lib f09`.

- [ ] **Step 3: Run final verification commands.**

Run: `rtk cargo fmt --check`; `rtk cargo check --locked --all-targets`; `rtk cargo clippy --locked --all-targets -- -D warnings`; `rtk cargo test --locked`; `rtk git diff --check`.

- [ ] **Step 4: Review the diff and stage only blocker changes.** Confirm unrelated dirty files remain unstaged, review resource-release paths, and verify no F07/F08/F09 coordinate/damage hunks were added.

- [ ] **Step 5: Create the focused commit after fresh verification.**

Run: `rtk git add src/compositor/state/subsurfaces.rs src/compositor/state_data.rs src/compositor/state/surface_transactions.rs src/compositor/subsurface.rs src/compositor/subsurface_cache_tests.rs src/compositor/tests/protocol_error.rs src/compositor/tests/surface_frames.rs`; `rtk git commit -m "fix(compositor): preflight cumulative surface mapping state"`.

- [ ] **Step 6: Report evidence without extrapolation.** Include baseline HEAD before code changes, Codebase Memory generation/status, RED/GREEN evidence for each required scenario, chosen architecture, focused/full test counts, fmt/check/clippy status, source-layout status, final commit hash, and remaining risks.
