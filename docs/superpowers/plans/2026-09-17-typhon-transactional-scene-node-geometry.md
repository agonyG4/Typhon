# Typhon Transactional SceneNode Geometry Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Migrate Typhon’s production geometry presentation state from transient root-surface ownership to stable WindowGroup `SceneNodeId` ownership with atomic presentation transactions and exact physical revision acknowledgement.

**Architecture:** Split the existing presentation module into focused time, curve, geometry, transaction, engine, and frame modules while preserving the `presentation_animation` public import path. The compositor owns one sparse SceneNode-keyed geometry engine; frame resolution adds the current root surface as a render/input adapter, freezes output-qualified evidence, and lets `NativeSceneHistory` promote that exact evidence before ACKing settled revisions.

**Tech Stack:** Rust, existing Typhon compositor/native-output pipeline, `SceneNodeId`, `OutputId`, analytical spring/easing sampling, focused unit/integration tests, Cargo, and the repository `rtk` wrapper.

## Global Constraints

- Compile and test in `/home/agony/GitHub/Typhon` so build artifacts stay in the checkout.
- Do not use sub-agents.
- Preserve the existing absolute-time analytical animation mathematics and visual behavior.
- Do not create a second production geometry engine or physical presentation ledger.
- Geometry presentation ownership is exclusively `WindowGroup SceneNodeId`; root IDs remain frame/render/input adapters.
- Every semantic geometry operation uses a presentation transaction, including one-member operations.
- Transaction identity and exact per-track revision identity remain distinct.
- NativeSceneHistory remains the only physical frame promotion authority.
- Input continues to use the last physically promoted geometry; future samples never become input truth.
- Lamp/lifecycle ownership, opacity, clip, workspace transitions, and new visible effects remain deferred.
- Preserve unrelated dirty changes, stage only task files, and never reset, stash, amend, or discard concurrent work.
- Keep new production modules below the configured source-layout limits and do not increase the final diagnostic count.
- Use `rtk` for reads, searches, Git, formatting, Cargo checks/tests, and source-layout verification.

---

### Task 1: Establish the design and predecessor-contract baseline

**Files:**
- Modify: `docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md`
- Test: existing Phase 2A SceneNode/history test modules selected after inspection

**Interfaces:**
- Consumes: current SceneNode registry, ActiveScene projection, immutable native frame snapshot, and NativeSceneHistory.
- Produces: a recorded baseline and predecessor status; any missing Phase 2A regressions are added before geometry migration.

- [ ] **Step 1: Re-check current HEAD, branch, dirty paths, graph generation, source-layout diagnostics, and focused presentation suites.**
- [ ] **Step 2: Run the existing Phase 2A SceneNode regressions, including XWayland replacement, canonical deletion after submit, immediate promotion, and Direct Scanout promotion.**
- [ ] **Step 3: Add only any missing failing predecessor regression, then implement the minimum test-only/source restriction needed to make it pass.**
- [ ] **Step 4: Record the verified predecessor status in the generalized presentation design document without claiming v2 completion yet.**
- [ ] **Step 5: Commit only predecessor/documentation changes with `test(frame): close Phase 2A SceneNode promotion contracts`.**

### Task 2: Split `presentation_animation.rs` without changing behavior

**Files:**
- Create: `src/presentation_animation/mod.rs`
- Create: `src/presentation_animation/ids.rs`
- Create: `src/presentation_animation/time.rs`
- Create: `src/presentation_animation/curve.rs`
- Create: `src/presentation_animation/geometry.rs`
- Create: `src/presentation_animation/transaction.rs`
- Create: `src/presentation_animation/engine.rs`
- Create: `src/presentation_animation/frame.rs`
- Create: `src/presentation_animation/tests.rs` and focused test children as required by source layout
- Delete: `src/presentation_animation.rs`
- Modify: `src/lib.rs`
- Modify: `src/compositor/mod.rs`

**Interfaces:**
- Consumes: the exact existing public types and tests in `src/presentation_animation.rs`.
- Produces: the same public API and numerical outputs, re-exported from `presentation_animation::mod`, with focused responsibilities and no monolithic implementation file.

- [ ] **Step 1: Add a failing module-layout/API smoke test that imports `AnimationTime`, `AnimationCurve`, `PresentationRect`, and `PresentationAnimator` through `crate::presentation_animation`.**
- [ ] **Step 2: Run the focused presentation tests and confirm the folder module/API is not yet available.**
- [ ] **Step 3: Move time, curve, geometry, frame, and existing engine implementation into the focused modules, preserving signatures, field semantics, and analytical math exactly.**
- [ ] **Step 4: Re-home the existing unit tests into focused test modules without adding a giant test file.**
- [ ] **Step 5: Re-export all compatibility names from `mod.rs`, including the temporary `TransitionId` alias.**
- [ ] **Step 6: Run the full existing presentation policy and animation tests; any numerical mismatch is fixed by restoring the original implementation, not by changing expected values.**
- [ ] **Step 7: Commit with `refactor(animation): split presentation animation modules`.**

### Task 3: Add typed IDs, transaction requests, and the SceneNode-keyed engine

**Files:**
- Modify: `src/presentation_animation/ids.rs`
- Modify: `src/presentation_animation/transaction.rs`
- Modify: `src/presentation_animation/engine.rs`
- Modify: focused presentation transaction/revision tests

**Interfaces:**
- Consumes: `SceneNodeId`, `OutputId`, `PresentationRect`, `PresentationVelocity`, and the preserved curve sampler.
- Produces: `PresentationTransactionId`, `PresentationRevisionId`, `TransitionId` compatibility alias, `PresentationPropertyKind::Geometry`, `PresentationTransactionRequest`, `PresentationTransactionRecord`, `GeometryTrack`, and `PresentationEngine`.

- [ ] **Step 1: Write failing pure-engine tests for monotonic nonzero transaction/revision allocation, one-member transactions, multi-member shared transaction IDs, distinct revisions, and duplicate member rejection.**
- [ ] **Step 2: Run those tests and confirm the typed IDs/transaction API is absent.**
- [ ] **Step 3: Implement checked monotonic allocators with test-configurable exhaustion and no recycling.**
- [ ] **Step 4: Implement prepare-then-commit validation: reject duplicate `(SceneNodeId, Geometry)` members, invalid geometry, missing owners, and ID exhaustion before mutating active tracks or transaction records.**
- [ ] **Step 5: Implement SceneNode-keyed start, retarget, cancel, sparse sampling, transaction membership, and bounded record cleanup while preserving curve-selection semantics.**
- [ ] **Step 6: Add failing tests for position/velocity-preserving retarget, KDE/macOS curve switching, `None` cancellation, mathematical settlement retention, matching physical ACK retirement, stale revision rejection, wrong node/output rejection, and final transaction-record removal.**
- [ ] **Step 7: Implement exact ACK qualification by `OutputId`, `SceneNodeId`, `PresentationRevisionId`, and final geometry; keep stale/wrong-output diagnostics bounded.**
- [ ] **Step 8: Run the pure engine tests and commit with `feat(animation): add transactional SceneNode geometry engine`.**

### Task 4: Make frame targets and evidence SceneNode-aware

**Files:**
- Modify: `src/presentation_animation/frame.rs`
- Modify: `src/compositor/state/scene.rs`
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/scene_history.rs` only where an accessor is required
- Modify: focused frame evidence/history tests

**Interfaces:**
- Consumes: stable WindowGroup registry mapping, current backing root surface, `OutputId`, and the sparse engine sample.
- Produces: SceneNode-aware `PresentationWindowTarget`, `PresentationGroupTransform`, `PresentedWindowGeometry`, and `FramePresentationSample`/`PresentationFrameSnapshot` with explicit sample-time source and output identity.

- [ ] **Step 1: Add failing tests for target uniqueness by WindowGroup, both node/root identities in evidence, output-qualified sampling, scheduled/fallback time-source recording, and immutable clone behavior.**
- [ ] **Step 2: Change target construction to resolve WindowGroup IDs before constructing targets, deduplicate by node ID, and retain the current root ID only as the adapter.**
- [ ] **Step 3: Add `PresentationSampleTimeSource` and output-qualified sampling APIs; preserve scheduled target timestamps in the normal native path and use fallback metadata only when no target exists.**
- [ ] **Step 4: Add node/revision/transaction fields to frame transforms and both node/root fields to presented window geometry; retain the existing revision-based visual signature behavior without adding transaction/node identity to pixel signatures.**
- [ ] **Step 5: Freeze the one v2 frame sample inside `NativeFrameSceneSnapshot` and keep all history publication paths immutable and registry-independent.**
- [ ] **Step 6: Run frame, scene-history, fullscreen, cursor, and existing Phase 2A tests; commit with `refactor(animation): publish SceneNode frame evidence`.**

### Task 5: Migrate all production geometry producers to WindowGroup transactions

**Files:**
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/scene.rs`
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/state/window_resize.rs`
- Modify: `src/compositor/state/xwayland_mode.rs`
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/desktop_windows.rs`
- Modify: `src/compositor/state/window_interaction.rs`
- Modify: `src/compositor/state/lifecycle_animation.rs` only for Lamp takeover cancellation
- Modify: `src/compositor/state/surfaces.rs`
- Modify: `src/compositor/state/tiled_layout.rs`
- Modify: `src/compositor/state/workspaces.rs` where reflow batching is called
- Modify: focused move/resize/maximize/fullscreen/XDG/XWayland tests

**Interfaces:**
- Consumes: the SceneNode-keyed engine and resolved current root adapter.
- Produces: one-member transactions for move/resize/maximize/fullscreen/XDG/XWayland operations and one atomic transaction for each outer Dwindle reflow batch.

- [ ] **Step 1: Add failing integration tests proving single-window operations install transaction/revision metadata and stable WindowGroup ownership.**
- [ ] **Step 2: Resolve the WindowGroup node from `WindowId` at each geometry mutation seam and route immediate/animated/cancel behavior through one transaction API.**
- [ ] **Step 3: Replace `layout_animation_epoch`’s grouping role with nested pending transaction context and common start time; finish commits only at outermost depth.**
- [ ] **Step 4: Add the three-window Dwindle atomicity/nested-batch regression and repeated-sampling counters proving no layout solve, XDG configure, X11 configure, or canonical geometry mutation occurs during sampling.**
- [ ] **Step 5: Make logical WindowGroup removal cancel its track, while root-surface teardown/replacement leaves the node track alive; preserve Lamp’s specialized cancellation handoff.**
- [ ] **Step 6: Run focused compositor geometry suites and commit with `refactor(animation): migrate geometry to WindowGroup ownership`.**

### Task 6: Promote exact revisions and preserve XWayland/input compatibility

**Files:**
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/presentation_direct.rs`
- Modify: `src/native_output/runtime/cycle_direct.rs`
- Modify: `src/native_output/runtime/cycle/pageflip.rs` only if publication qualification requires it
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/compositor/state/direct_scanout.rs`
- Modify: `src/compositor/state/desktop_windows.rs`
- Modify: focused XWayland, input, ACK, and promotion tests

**Interfaces:**
- Consumes: immutable output-qualified frame sample and NativeSceneHistory promotion callbacks.
- Produces: exact physical ACK behavior, stable A→B backing replacement continuity, conservative physically presented input, and SceneNode-aware active/physical Direct Scanout blockers.

- [ ] **Step 1: Add failing physical ACK tests for stale R1/R2 frames, mathematical settlement without physical presentation, dropped/rejected/ready/submitted-only frames, immediate promotion, and wrong-output evidence.**
- [ ] **Step 2: Publish ACKs only from a snapshot returned after matching NativeSceneHistory promotion, qualifying `(OutputId, SceneNodeId, RevisionId)` and final geometry.**
- [ ] **Step 3: Add the mandatory XWayland A→B replacement test proving one WindowGroup and revision survive, position/velocity remain continuous, B is the next adapter, and a submitted A frame remains A evidence.**
- [ ] **Step 4: Preserve root-surface physical-presence suppression and inverse physically presented hit mapping; prove G/A evidence never routes input to current B early.**
- [ ] **Step 5: Change active scanout blockers to visible WindowGroup track lookup and preserve the physically promoted identity blocker until an identity frame is promoted; hidden unrelated tracks do not block globally.**
- [ ] **Step 6: Verify root client/subsurface/SSD/effect transforms remain applied exactly once and run focused suites.**
- [ ] **Step 7: Commit with `fix(animation): preserve physical revisions across backing replacement`.**

### Task 7: Cleanup, observability, documentation, and final verification

**Files:**
- Modify: `src/presentation_animation/engine.rs`
- Modify: `src/presentation_animation/frame.rs`
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/state/direct_scanout.rs`
- Modify: `src/compositor/state/surfaces.rs`
- Modify: `docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md`
- Modify: only task-scoped focused test modules

**Interfaces:**
- Consumes: completed geometry migration and fresh focused-test evidence.
- Produces: bounded metrics, no root-keyed production authority, accurate documentation, and a verified handoff with Lamp/new effects explicitly deferred.

- [ ] **Step 1: Add bounded metrics for commits/members/starts/retargets/cancels/active tracks/math settlements/physical and stale/wrong-output ACKs/frame samples/time sources.**
- [ ] **Step 2: Remove obsolete root-keyed start/retarget/cancel authority and narrow/remove `layout_animation_epoch` only after transaction parity is proven.**
- [ ] **Step 3: Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk cargo clippy --locked --all-targets -- -D warnings`, `rtk cargo test --locked`, and `rtk run ./bin/check-source-layout`.**
- [ ] **Step 4: Re-check the 24 architectural invariants against the diff and focused/full test output, including no extra GPU pass/thread/framebuffer/traversal/layout solve.**
- [ ] **Step 5: Update the generalized design document to mark OutputId, SceneNodeId, topology, immutable frame evidence, physical promotion, transaction IDs, SceneNode geometry ownership, exact revision ACK, and Dwindle transactions implemented; keep opacity/clip/Lamp/lifecycle/new effects pending.**
- [ ] **Step 6: Commit only task-scoped files with `docs(animation): mark geometry Presentation Engine v2 implemented`.**

