# Presentation Engine v2 Geometry Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden the implemented Geometry v2 presentation stage without changing its SceneNode-owned, transaction-based, immutable-frame architecture.

**Architecture:** Pending layout transactions will merge repeated Geometry mutations by preserving the first start and replacing only the final target and curve. Physical ACKs will be immutable evidence from the promoted frame, qualified against the compositor’s allocated logical output, while engine retirement will use exact node/property/revision identity. Inline native frame tests will move to a child test module so the production module returns below the source-layout limit.

**Tech Stack:** Rust, Cargo, repository `rtk` command wrappers, Codebase Memory MCP, native Wayland compositor test harnesses.

## Global Constraints

- Preserve SceneNode ownership, root-surface adapter semantics, analytical absolute-time curves, velocity-preserving active-track retargeting, scheduled sampling, physical settlement, input, XWayland continuity, Direct Scanout conservatism, and cache signatures.
- Do not add Opacity, Clip, Lamp migration, workspace transitions, or new visual effects.
- Do not use subagents; compile and test in `/home/agony/GitHub/Typhon`.
- Preserve unrelated dirty work and stage only files belonging to this closure.
- Run source-layout through `rtk run ./bin/check-source-layout`; do not raise configured limits or fix unrelated debt.

---

### Task 1: Pending Geometry Mutation Merge

**Files:**
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/presentation_animation/transaction.rs` if the helper belongs with mutation semantics
- Test: `src/compositor/state/tiled_layout_tests.rs` and/or `src/presentation_animation/transactions_tests.rs`

**Interfaces:**
- Consumes: `PendingPresentationGeometryTransaction`, `PresentationGeometryMutation`, `begin_layout_reflow_batch`, `finish_layout_reflow_batch`, and `PresentationEngine::commit`.
- Produces: `PendingPresentationGeometryTransaction::upsert_geometry_mutation`, preserving the first `start`, replacing `target` and `curve`, and retaining the common `started_at`.

- [ ] **Step 1: Add a failing direct merge test** asserting `A -> B` followed by `B -> C` stores one member with `A -> C`.
- [ ] **Step 2: Add a failing nested-batch compositor test** asserting inner finish does not commit and outer finish commits one member/revision with the final target.
- [ ] **Step 3: Add a failing active-track regression** asserting final commit samples the old track at the common batch time, preserves sampled velocity, and uses the final pending target.
- [ ] **Step 4:** Run the focused tests and confirm they fail for the old replacement behavior.
- [ ] **Step 5:** Implement `upsert_geometry_mutation` and route every pending same-node insertion through it without duplicating retarget math.
- [ ] **Step 6:** Run the focused tests and the existing presentation transaction/retargeting tests.
- [ ] **Step 7:** Commit only the pending-merge implementation and tests with a focused message.

### Task 2: Immutable Output-Qualified Physical ACK

**Files:**
- Modify: `src/presentation_animation/engine.rs`
- Modify: `src/presentation_animation/transaction.rs` or `src/presentation_animation/frame.rs` for the ACK value
- Modify: `src/compositor/state/active_scene.rs`
- Test: `src/presentation_animation/transactions_tests.rs` and compositor/native publication tests as needed

**Interfaces:**
- Consumes: `PresentationFrameSnapshot.output_id`, `PresentationGroupTransform`, `publish_presented_presentation`, and `NativeSceneHistory` promotion output.
- Produces: an immutable `PresentedGeometryAck` carrying output, node, property, transaction, revision, and presented rectangle; `PresentationEngine::acknowledge_presented_geometry(expected_output_id, ack) -> bool` with no sampled-output state.

- [ ] **Step 1:** Add a failing pure-engine cross-output test that samples A, samples B, and accepts a frozen A ACK when expected output is A.
- [ ] **Step 2:** Add a failing wrong-output test that rejects B against expected A without changing the active track, transaction, revision, or settlement state, then accepts the later correct A ACK.
- [ ] **Step 3:** Run the focused tests and confirm the first test fails because `sampled_output` was overwritten by B.
- [ ] **Step 4:** Remove `sampled_output` and make the ACK API validate only the supplied expected output and immutable ACK evidence.
- [ ] **Step 5:** Make compositor publication qualify the snapshot output against the allocated presentation output and construct exact ACK values only from mathematically settled transforms.
- [ ] **Step 6:** Run physical ACK, scene-history, immediate-promotion, pageflip, input, and Direct Scanout focused tests.
- [ ] **Step 7:** Commit the ACK redesign and its tests with focused messages.

### Task 3: Exact Transaction-Member Retirement

**Files:**
- Modify: `src/presentation_animation/transaction.rs`
- Modify: `src/presentation_animation/engine.rs`
- Test: `src/presentation_animation/transactions_tests.rs`

**Interfaces:**
- Consumes: `PresentationTransactionMember`’s node/property/transaction/revision identity and all geometry retirement paths.
- Produces: `remove_member_exact(scene_node_id, property, revision_id) -> bool`, used by retarget, cancel, exact physical ACK, and cleanup paths.

- [ ] **Step 1:** Add failing tests for wrong revision retention and exact revision removal.
- [ ] **Step 2:** Add a failing test that a transaction disappears only after its final exact member is retired.
- [ ] **Step 3:** Run the focused transaction tests and confirm the stale-removal test fails against node-only removal.
- [ ] **Step 4:** Implement exact matching and thread the old track’s property/revision through every retirement call.
- [ ] **Step 5:** Run transaction, retarget, stale ACK, cancel, and backing-surface continuity tests.
- [ ] **Step 6:** Commit the exact retirement change and tests.

### Task 4: Production Output Identity Invariant

**Files:**
- Modify: `src/compositor/state/active_scene.rs`
- Modify: presentation-specific test setup files only where default fixtures call production presentation APIs
- Test: focused compositor presentation tests

**Interfaces:**
- Consumes: `native_output_id` and `ensure_native_output_id`.
- Produces: `presentation_output_id` as an explicit invariant that never fabricates `OutputId(1)` in production.

- [ ] **Step 1:** Add or adapt a failing test proving a default fixture cannot use presentation publication without establishing an output identity.
- [ ] **Step 2:** Run the focused test and confirm the old fallback masks the missing identity.
- [ ] **Step 3:** Replace the fallback with the repository’s invariant mechanism and explicitly allocate output IDs in presentation-specific fixtures.
- [ ] **Step 4:** Run focused compositor sampling/publication tests and search production presentation code for `OutputId(1)` fallbacks.
- [ ] **Step 5:** Commit the output-identity invariant and fixture setup changes.

### Task 5: Native Frame Test Extraction

**Files:**
- Modify: `src/native_output/runtime/frame.rs`
- Create: `src/native_output/runtime/frame_tests.rs`
- Preserve: `src/native_output/runtime/frame_scene_identity_tests.rs`

**Interfaces:**
- Consumes: the existing inline `#[cfg(test)] mod tests` in `frame.rs`, including the pre-existing dirty test insertion.
- Produces: `#[cfg(test)] #[path = "frame_tests.rs"] mod tests;` with the inline tests moved verbatim and private access preserved through the child module relationship.

- [ ] **Step 1:** Record the exact inline test-module boundaries and current line counts, including the dirty test insertion.
- [ ] **Step 2:** Move the inline module verbatim into `frame_tests.rs` and replace it with the explicit child-module path declaration.
- [ ] **Step 3:** Run frame tests and frame-scene-identity tests before any behavioral cleanup.
- [ ] **Step 4:** Run `rtk run ./bin/check-source-layout` and confirm `frame.rs` and `frame_tests.rs` comply without increasing diagnostics.
- [ ] **Step 5:** Commit only the test extraction.

### Task 6: Documentation and Full Verification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md`
- Do not modify unrelated dirty files.

- [ ] **Step 1: Run focused presentation, Dwindle, nested batching, XWayland, ACK, input, Direct Scanout, scene-history, native-frame, and frame identity suites.
- [ ] **Step 2: Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk cargo clippy --locked --all-targets -- -D warnings`, `rtk cargo test --locked`, and `rtk run ./bin/check-source-layout`.
- [ ] **Step 3: Update the approved design status to distinguish hardened/closed Geometry v2 from pending Opacity, Clip, retained sources, Lamp, and new effects.
- [ ] **Step 4: Review the final diff, status, line counts, diagnostic count, and invariant checklist.
- [ ] **Step 5: Commit documentation and any final scoped adjustments.
