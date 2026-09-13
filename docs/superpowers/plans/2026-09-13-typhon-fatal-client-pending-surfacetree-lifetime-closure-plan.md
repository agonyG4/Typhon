# Typhon Fatal-Client and Pending SurfaceTree Lifetime Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. This session executes the plan inline because subagents are explicitly disallowed.

**Goal:** Close fatal-client, pacing-only SurfaceTree, terminal callback, and pointer-replacement lifetime holes without changing downstream rendering or output systems.

**Architecture:** Keep `CompositorState` as the immediate/deferred cleanup authority, but route every fatal wire error through a reusable split-state primitive in `client_lifecycle.rs` that marks the owning client terminal before posting. Add an independently captured, node-aligned SurfaceTree publication-lifetime vector; acquire dependencies remain solely for acquire completion. Add a no-event callback discard path selected only for terminal-client publication rejection.

**Tech Stack:** Rust, `wayland-server`, existing compositor unit/integration tests, Cargo locked toolchain, `rtk` command proxy.

## Global Constraints

- Preserve pointer-constraint request/protocol-object lifetime separation, replacement merge semantics, explicit-sync publication guards, and `terminal_client_ids` behavior already present.
- Compile in the repository's existing `target` directory.
- Use `rtk` for repository commands.
- Do not use subagents.
- Preserve unrelated dirty-worktree changes.
- Do not change KMS commit worker, atomic output, renderer, buffer age, adaptive buffering, Direct Scanout, VRR, NVIDIA, Proton, or scene-wide repaint behavior.
- Production SurfaceTree admission must fail closed if a node lifetime cannot be captured.
- Every new behavior change follows a failing-test-first red/green cycle.

---

### Task 1: Centralize fatal Wayland error emission

**Files:**
- Modify: `src/compositor/state/client_lifecycle.rs`
- Modify: `src/compositor/explicit_sync.rs`
- Modify: `src/compositor/dmabuf.rs`
- Modify: `src/compositor/protocols/buffers.rs`
- Modify: `src/compositor/protocols/effects_control.rs`
- Modify: `src/compositor/protocols/commit_timing.rs`
- Modify: `src/compositor/protocols/fifo.rs`
- Modify: `src/compositor/protocols/syncobj.rs`
- Modify: `src/compositor/protocols/presentation_modes.rs`
- Modify: `src/compositor/protocols/background_effect.rs`
- Modify: `src/compositor/protocols/keyboard_shortcuts_inhibit.rs`
- Modify: `src/compositor/layer_shell.rs`
- Test: `src/compositor/protocol_error_trace.rs` or a focused lifecycle test module

**Interfaces:**
- Produce `post_fatal_protocol_error(...)` in the central lifecycle module for split-state callers. It must resolve `resource.client()`, insert the `ClientId` into `terminal_client_ids`, record the supplied category/error metadata once, and call `resource.post_error(...)` only after insertion and recording.
- Produce detailed `CompositorState` wrappers for immediate and deferred cleanup, preserving `post_protocol_error()` and `post_protocol_error_deferred()` semantics.
- Update `SyncobjSurfaceState::post_error_with_metrics`, dmabuf validation, and wl_drm validation to call the same primitive; remove their duplicate metric/trace bookkeeping.

- [ ] **Step 1: Add the source-contract test first.** Recursively scan `.rs` files below `src/compositor`, collect lines containing the dynamically constructed needle `.{post_error}(`, allow only `src/compositor/state/client_lifecycle.rs`, and assert no other path is reported. Keep the test string construction split so the guard does not match itself.

- [ ] **Step 2: Run the guard to verify it fails.**

Run: `rtk cargo test --locked protocol_error_trace -- --nocapture`

Expected: FAIL listing the current raw emitters outside `client_lifecycle.rs` (baseline count: 39).

- [ ] **Step 3: Implement the low-level primitive and detailed wrappers.** Centralize metric/trace/terminal insertion and posting in `client_lifecycle.rs`. Make the no-client case record an unavailable diagnostic and skip wire emission. Keep cleanup as a separate boolean/wrapper decision so deferred pointer-constraint errors do not trigger immediate teardown.

- [ ] **Step 4: Migrate split-state callers.** Pass `terminal_client_ids` through dmabuf and wl_drm helpers; replace `note_*_protocol_error` plus raw post pairs with the central primitive. Make `SyncobjSurfaceState` use the central primitive for stored resources. Update effects helpers to receive `&mut CompositorState` and the dispatch `&Client` where necessary.

- [ ] **Step 5: Migrate stateful raw emitters.** Replace direct posts in layer shell, commit timing, FIFO, presentation modes, background effects, keyboard-shortcuts inhibit, and any remaining protocol file with the appropriate state wrapper and category. Preserve current immediate/deferred cleanup behavior and existing diagnostic categories.

- [ ] **Step 6: Run the guard and focused protocol tests.**

Run: `rtk cargo test --locked protocol_error_trace -- --nocapture`

Expected: PASS with zero raw emitters outside the allowlist; protocol-error classification tests remain green and metrics are counted once.

- [ ] **Step 7: Commit the centralized migration.**

Run:

```text
rtk git add src/compositor docs/superpowers/plans/2026-09-13-typhon-fatal-client-pending-surfacetree-lifetime-closure-plan.md
rtk git commit -m "fix: centralize fatal protocol client poisoning"
```

---

### Task 2: Capture independent SurfaceTree node lifetimes

**Files:**
- Modify: `src/compositor/state/surface_transactions.rs`
- Modify: `src/compositor/state/subsurfaces.rs`
- Modify: `src/compositor/state/mod.rs`
- Modify: `src/compositor/state/surface_tree_readiness.rs`
- Modify: `src/compositor/state/frame_tests.rs`
- Modify: `src/compositor/state/surface_pacing.rs`
- Modify: `src/compositor/state/xdg_lifecycle.rs`

**Interfaces:**
- Add `SurfaceTreeNodeLifetime { surface_id: u32, owner_client_id: ClientId, surface_presentation_generation: u64 }`.
- Add a production `Captured(Vec<SurfaceTreeNodeLifetime>)` representation to `PendingSurfaceTreeTransaction`; provide an explicit `#[cfg(test)]` synthetic fixture representation for existing tests that intentionally use fake surface IDs.
- Add `CompositorState::capture_surface_tree_node_lifetimes(&self, nodes: &[(u32, CachedSubsurfaceCommit)]) -> Option<...>` using `capture_surface_publication_lifetime` once per node.
- Change transaction build/queue/merge functions to carry and update the lifetime vector aligned with `nodes`.

- [ ] **Step 1: Add the failing pacing-only lifetime regressions.** In `src/compositor/state/frame_tests.rs`, add a live-surface commit-timing test and a live-surface FIFO test that queue a transaction with `dependencies.is_empty()`, mark the owner terminal without teardown, make the pacing gate eligible, call `commit_ready_surface_tree_transactions()`, and assert no pending transaction, renderable surface, publication, presentation feedback, or callback completion.

- [ ] **Step 2: Add the failing mixed-tree regression.** Build a root/child tree through normal SurfaceTree admission, make A wait on a scripted unsignaled acquire while B has an attachment without explicit sync, invalidate B's captured generation/owner before signaling A, signal A, and assert the whole transaction is discarded with B never published.

- [ ] **Step 3: Run the new tests to verify they fail for the dependency-coupled reason.**

Run: `rtk cargo test --locked compositor::state::frame_tests -- --nocapture`

Expected: the no-dependency pacing cases demonstrate that the old rejection helper does not inspect the transaction lifetime (or otherwise fail at the new assertion), and the mixed tree demonstrates stale B can currently escape dependency validation.

- [ ] **Step 4: Add lifetime capture at production admission.** Capture every node before queuing/merging. If any surface lifetime is unavailable, release the unpublished nodes through the existing release path and return without queueing. Do not reconstruct lifetimes in readiness processing.

- [ ] **Step 5: Preserve lifetimes through merge and queue paths.** When a node is added or replaced, update its aligned lifetime from the incoming capture; when a bufferless update preserves an existing attachment, retain the existing dependency and the transaction's effective node lifetime consistently. Carry the vector through pacing readiness and transaction removal without dropping it.

- [ ] **Step 6: Replace dependency-derived rejection with per-node validation.** Validate every captured node for terminal owner, owner map equality, live resource and client, and current presentation generation. Require a captured production vector; only explicit synthetic test fixtures may bypass the live-resource gate. Then leave normal `surface_publication_decision` and commit-order validation in place.

- [ ] **Step 7: Run the SurfaceTree focused tests.**

Run: `rtk cargo test --locked compositor::state::frame_tests -- --nocapture`

Expected: existing and new SurfaceTree readiness, explicit-sync, FIFO, commit-timing, mixed-node, and publication-rejection tests pass.

- [ ] **Step 8: Commit the SurfaceTree lifetime model.**

Run: `rtk git add src/compositor/state && rtk git commit -m "fix: capture SurfaceTree publication lifetimes"`

---

### Task 3: Discard callbacks for terminal publication rejection

**Files:**
- Modify: `src/compositor/state/frame_callbacks.rs`
- Modify: `src/compositor/state/frames.rs`
- Modify: `src/compositor/state/subsurfaces.rs`
- Modify: `src/compositor/state/surface_tree_readiness.rs`
- Modify: `src/compositor/state/frame_tests.rs`

**Interfaces:**
- Produce `CompositorState::discard_frame_callbacks(&mut self, callbacks: Vec<wl_callback::WlCallback>)`, which removes callback bookkeeping from all pending collections and uncompleted frame batches, increments cancellation settlement exactly once, drops resources, and never calls `send_event(Done)`.
- Keep `complete_frame_callbacks()` unchanged for live owners and non-terminal outcomes.
- Route `TerminalClient` direct explicit-sync cancellation and SurfaceTree discard through the new operation; retain existing completion for other reasons.

- [ ] **Step 1: Add failing callback tests.** Extend the direct explicit-sync and SurfaceTree terminal-client regressions with a real callback resource and assert callback bookkeeping is removed, settlement is reconciled, and the client event queue contains no `wl_callback.done`.

- [ ] **Step 2: Run the callback tests to verify the old behavior sends Done.**

Run: `rtk cargo test --locked terminal_callback -- --nocapture`

Expected: FAIL because the current discard path calls `complete_frame_callbacks()` and sends Done while the callback resource is still alive.

- [ ] **Step 3: Implement explicit discard accounting.** Centralize removal from `pending_frame_callbacks`, `visible_pending_frame_callbacks`, `pending_frame_callback_surfaces`, `pending_frame_callback_timing`, and uncompleted `frame_batches`, using `FrameCallbackSettlement::cancel` for exactly the callbacks removed from batches. Do not touch completed/terminal batches twice.

- [ ] **Step 4: Route only terminal publication rejection to discard.** Pass the rejection decision into SurfaceTree and direct explicit-sync cleanup so `TerminalClient` uses `discard_frame_callbacks`, while live-client completion and unrelated paths preserve their current behavior.

- [ ] **Step 5: Run focused callback and ownership tests.**

Run: `rtk cargo test --locked compositor::state::frame_tests compositor::tests::surface_frames -- --nocapture`

Expected: terminal callbacks produce no Done event; pending buffers, release points, feedbacks, resize captures, and acquire watchers still settle exactly once.

- [ ] **Step 6: Commit callback disposal.**

Run: `rtk git add src/compositor/state && rtk git commit -m "fix: discard terminal client frame callbacks"`

---

### Task 4: Add fatal-before-teardown end-to-end regressions

**Files:**
- Modify: `src/compositor/state/frame_tests.rs`
- Modify: `src/compositor/tests/support/input_client.rs` only if an existing support fixture is required
- Modify: relevant explicit-sync test module discovered during focused test selection

**Interfaces:**
- Exercise the interval where `terminal_client_ids` contains the client but normal `teardown_client_resources` has not yet run.
- Cover one direct `PendingExplicitSyncCommit` and one `PendingSurfaceTreeTransaction` path, including late/idempotent acquire completion and no publication/presentation/frame callback.

- [ ] **Step 1: Add a failing direct explicit-sync fatal-race test.** Queue unsignaled work with captured owner/generation, invoke the central/deferred fatal primitive, assert terminal membership before teardown, signal the acquire, process ready work, and assert no renderable/dead buffer, presentation success, or callback Done; verify release ownership settles once and late completion is harmless.

- [ ] **Step 2: Add a failing SurfaceTree fatal-race test.** Use the same sequence through normal tree admission, with a dependency present, and assert the tree is discarded before surface teardown.

- [ ] **Step 3: Run the tests to verify the pre-fix race is observable.**

Run: `rtk cargo test --locked compositor::state::frame_tests -- --nocapture`

Expected: the old direct/tree path can complete callbacks or publish before teardown; the tests fail on those assertions.

- [ ] **Step 4: Integrate the terminal decision with cleanup.** Ensure direct explicit-sync ready processing and tree discard use terminal-aware callback disposal but still call existing pending-buffer release, presentation feedback discard, resize capture release, watcher cancellation, and release-point settlement functions.

- [ ] **Step 5: Run focused fatal-race and ownership tests.**

Run: `rtk cargo test --locked compositor::state::frame_tests compositor::tests::surface_frames -- --nocapture`

Expected: both direct and tree races discard without visual publication, and late completion is idempotent.

- [ ] **Step 6: Commit fatal-race coverage.**

Run:

```text
rtk git add src/compositor/state src/compositor/tests
rtk git commit -m "test: cover fatal client before teardown"
```

---

### Task 5: Add production pointer replacement wiring regression

**Files:**
- Modify: `src/compositor/tests/input_output/pointer_constraint_transaction.rs`

**Interfaces:**
- Use existing real Wayland fixture, `capture_pointer_constraint_backend_requests`, pointer snapshots, `ServerCommand::PointerConstraintBackendActivated`, and `ServerCommand::PointerConstraintBackendDeactivated` helpers.
- Do not change pointer-constraint merge or backend implementation unless the new regression exposes a real failing implementation defect.

- [ ] **Step 1: Add the failing A -> destroy -> B -> one commit test.** Create/commit/activate A; destroy A; create B without an intermediate `wl_surface.commit`; commit once; capture requests and assert both A deactivation and B activation are issued. Settle them, then assert no client protocol error, A is no longer effective/current, B is current, and a stale A-generation activation/deactivation cannot affect B.

- [ ] **Step 2: Add the failing A -> destroy -> B -> destroy -> C -> commit test.** Assert the commit emits A retirement and C installation, B never becomes effective, and C remains current.

- [ ] **Step 3: Run the pointer transaction tests to verify the new production sequence.**

Run: `rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction -- --nocapture`

Expected: the tests fail only if request-to-commit-to-backend wiring does not apply both retirement and replacement; existing merge-algebra tests remain unchanged.

- [ ] **Step 4: Keep production code unchanged if both tests pass; otherwise make the smallest test-proven wiring correction.** Re-run the exact focused test after any correction and check stale generation handling explicitly.

- [ ] **Step 5: Commit the integration coverage.**

Run:

```text
rtk git add src/compositor/tests/input_output/pointer_constraint_transaction.rs
rtk git commit -m "test: cover pointer constraint replacement wiring"
```

---

### Task 6: Full verification and repository audit

**Files:**
- Inspect all changed files and existing source-layout/check scripts
- Modify only if a verification-discovered issue is directly caused by this work

- [ ] **Step 1: Re-audit fatal emitters.**

Run: `rtk rg -n '\.post_error\(' src/compositor --glob '*.rs'`

Expected: exactly one intentional raw call in `src/compositor/state/client_lifecycle.rs`; record its location and why it is the central primitive.

- [ ] **Step 2: Run focused tests.** Cover pointer constraint transaction, protocol-error trace/classification, fatal poisoning, direct explicit sync, SurfaceTree explicit sync, commit timing, FIFO, mixed-node lifetime, frame callbacks, surface/client teardown, and release/presentation ownership. Use exact test filters discovered during implementation and record pass counts.

- [ ] **Step 3: Run required full verification.**

Run, in order:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
```

Also run the repository source-layout check if `Cargo.toml`, Makefile, scripts, or docs identify one.

- [ ] **Step 4: Inspect final diff and ownership accounting.** Use `rtk git diff --check`, `rtk git diff --stat`, and targeted diffs to confirm no unrelated subsystem changed and that every discard path still releases buffers/fences, feedbacks, resize captures, callbacks, and watchers exactly once.

- [ ] **Step 5: Commit any verification-only repair, then make the final commit if needed.** Do not weaken unrelated tests or claim native-game-session closure without testing the corrected build in the affected native Wayland session.
