# Typhon Pointer Constraint Surface Transaction v1.1 Implementation Plan

> **For agentic workers:** This plan is executed inline in the current checkout because the user explicitly prohibited subagents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close resource-lifetime, captured-ownership, and constraint-identity defects while preserving the accepted pointer-constraint surface transaction and native epoch timing architecture.

**Architecture:** Keep protocol requests flowing into pending surface state, capture that state at the exact `wl_surface.commit`, carry it through `CachedSubsurfaceCommit`, and publish it before queuing native work. Add explicit protocol-resource liveness and surface-slot ownership state to `PointerConstraint`; make every non-no-op captured pointer record carry its constraint id; cancel uncommitted installs without leaving their region or hint payloads.

**Tech Stack:** Rust, Smithay Wayland server/client test resources, Typhon surface-tree transactions, synchronized subsurfaces, existing native pointer backend request seams, and Cargo verification through `rtk`.

## Global Constraints

- Preserve the causal transition timing architecture from `23a2d3e`.
- Preserve exact `wl_surface.commit` capture from `45e9371` and native settlement boundaries.
- Keep `set_region` and `set_cursor_position_hint` protocol-defined double-buffered state.
- Keep commit-synchronized client lifecycle changes as Typhon policy inspired by current KWin behavior, not an explicit protocol statement.
- Protocol resource lifetime, committed surface constraint lifetime, and native effective routing lifetime remain separate ownership domains.
- Do not add an input thread, scheduler changes, extra readiness probes, sleeps, timers, motion dropping, motion clamping, timestamp filtering, application detection, or acceleration changes.
- Do not change Sober-specific behavior or claim the Sober/Roblox pointer jump is fixed.
- Do not use surface iteration or `ids.first()` to attribute region or hint state.
- Run repository commands through `rtk`; commit each coherent stage.

## File map

- Modify `src/compositor/subsurface.rs` for the identity-bearing captured pointer record, lifecycle delta reducer, and reducer tests.
- Modify `src/compositor/state/pointer_constraints.rs` for protocol-resource liveness, surface ownership, registration, cancellation, publication, and backend guards.
- Modify `src/compositor/mod.rs` for the additional `PointerConstraint` ownership fields.
- Modify `src/compositor/tests/support/server_runtime.rs` and `src/compositor/tests/support/input_client.rs` for test-only state capture needed by real-resource regressions.
- Modify `src/compositor/tests/input_output/mod.rs` to register the new transaction regression module.
- Create `src/compositor/tests/input_output/pointer_constraint_transaction.rs` with real Wayland locked/confined lifecycle, delayed publication, identity, and cancellation tests.
- Modify `src/compositor/tests/input_output/relative_and_constraints.rs` only if an existing timing/relative assertion needs a narrow ownership assertion; do not rewrite motion behavior.
- Modify `docs/superpowers/specs/2026-09-03-typhon-pointer-constraint-surface-transaction-v1.1-design.md` only if implementation evidence requires a design correction.
- Modify `docs/superpowers/specs/2026-09-03-typhon-pointer-constraint-surface-transaction-v1-report.md` with exact defects, lifecycle/ownership model, RED/GREEN evidence, blockers, and non-claims.

---

### Task 1: Add RED reducer and real-resource regressions

**Files:**
- Modify: `src/compositor/subsurface.rs` unit tests near the existing pointer-state merge tests.
- Modify: `src/compositor/tests/support/server_runtime.rs` for a test-only pointer-constraint snapshot command.
- Modify: `src/compositor/tests/support/input_client.rs` for the snapshot helper.
- Modify: `src/compositor/tests/input_output/mod.rs` to load the new module.
- Create: `src/compositor/tests/input_output/pointer_constraint_transaction.rs`.

**Interfaces:**
- The new integration module consumes `OwnCompositorServer`, `ServerCommand`, `RegistryTestState`, the existing client setup helpers, and the existing backend activation/deactivation command seam.
- The test-only snapshot helper reports the selected constraint's current flag, pending-removal flag, committed region, and committed cursor hint without changing production behavior.
- Production code is not changed in this task.

- [ ] **Step 1: Write the reducer RED assertion.**

Extend `install_then_remove_before_publication_collapses_without_activation` so its captured install includes a non-default region and hint, its remove record has the same explicit owner, and the assertions require the collapsed result to contain no region or hint. Keep the lifecycle assertion. The current reducer must fail because it preserves A-owned payloads.

- [ ] **Step 2: Add the test-only state capture seam.**

Add a `ServerCommand` variant carrying a constraint id and a reply channel whose value includes `committed`, `lifecycle_removal_pending`, `committed_region`, and `committed_cursor_position_hint`. Handle it beside `CapturePointerConstraintIds`, and add a helper that sends the command and waits for the reply. The command must only read compositor state.

- [ ] **Step 3: Add the active locked destroy-without-commit RED test.**

Create a real client surface, pointer, and persistent locked constraint. Commit and settle the activation using the existing backend command helper. Clear event/request observations, destroy the locked resource, flush without a surface commit, and assert that no `Unlocked` event or backend `Deactivate` request appears, the constraint remains in the server snapshot as current, and a relative motion sample still reaches the locked path. Commit the surface afterward and assert that `Deactivate` is now queued. Do not settle that request until the test has checked the queue.

- [ ] **Step 4: Add the equivalent active confined destroy-without-commit RED test.**

Use a full-surface region and a persistent confined constraint. Commit and settle activation, destroy the confined resource without committing the surface, assert no `Unconfined` event or `Deactivate` request and current confined ownership, then commit and assert the queued deactivation.

- [ ] **Step 5: Add the stale backend activation and create/destroy RED tests.**

For a newly committed locked constraint whose activation request is captured but not acknowledged, destroy the protocol object, flush without a surface commit, assert that activation bookkeeping is empty, inject the stale backend activation acknowledgement, and assert no locked event or active routing appears. Separately create and destroy a constraint before its first effective commit, commit the surface, and assert no activation request, event, or surviving constraint id.

- [ ] **Step 6: Add the captured-but-unpublished `AlreadyConstrained` RED test.**

Create a parent toplevel and synchronized child, request lock A for the child, commit the child without committing the parent so A leaves the mutable pending map but remains cached, then request lock B on the same child and pointer. Flush and assert the real protocol error is `zwp_pointer_constraints_v1` / `AlreadyConstrained`; do not depend on native activation.

- [ ] **Step 7: Add delayed hint and region identity RED tests.**

Use a synchronized child so A's install, A-owned hint or region, destroy, and removal commit remain cached. Request B after the removal commit is captured, publish the cached removal before committing B, and use the snapshot seam to assert B has neither A's hint nor A's region. Commit B with its own hint/region, publish it, and assert the snapshot contains only B-owned values. Include the lock hint restore assertion and the confine activation-region assertion where the existing test seam exposes them.

- [ ] **Step 8: Run the new tests and confirm expected RED failures.**

Run:

```text
rtk cargo test --locked compositor::subsurface::window_geometry_tests::install_then_remove_before_publication_collapses_without_activation
rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction
```

Expected: the reducer test fails on preserved region/hint, the active destroy tests observe premature routing loss or missing commit-gated removal, the captured install test accepts B, and the delayed identity tests observe A-owned state on B. If a test has a setup or compilation error instead of a behavioral failure, fix only the test setup and rerun before changing production code.

- [ ] **Step 9: Commit the RED test stage.**

```text
rtk git add src/compositor/subsurface.rs src/compositor/tests/support/server_runtime.rs src/compositor/tests/support/input_client.rs src/compositor/tests/input_output/mod.rs src/compositor/tests/input_output/pointer_constraint_transaction.rs
rtk git commit -m "test: reproduce pointer constraint identity regressions"
```

### Task 2: Make captured pointer state identity-safe

**Files:**
- Modify: `src/compositor/subsurface.rs` pointer captured-state definitions, merge implementation, constructors, and tests.
- Modify: `src/compositor/state/pointer_constraints.rs` pending-state staging helpers and test-visible callers.
- Modify: `src/compositor/state/shutdown.rs` empty cached-commit construction.
- Modify: `src/compositor/protocols/core.rs` only as needed for the new default/no-op representation in the captured commit literal.

**Interfaces:**
- `CapturedPointerConstraintSurfaceState` has an unowned `NoChange` value and an identity-bearing mutation value.
- The mutation value contains `constraint_id`, lifecycle delta, region delta, and cursor-hint delta.
- The lifecycle delta distinguishes `NoChange`, `Install`, `Remove`, and tagged `Cancel`; `Cancel` is used only when an install never became current and contains no region or hint payload.
- `stage_pointer_constraint_install`, `stage_pointer_constraint_removal`, region staging, and hint staging all create records for an explicit constraint id.

- [ ] **Step 1: Change the types without changing publication behavior.**

Replace the loose surface-wide struct with an enum/value arrangement in which a default commit has no owner and every mutation carries exactly one `constraint_id`. Keep explicit default regions and concrete hints distinct from `NoChange`. Update every `CachedSubsurfaceCommit` constructor and destructuring site to compile with the new value.

- [ ] **Step 2: Implement same-owner merge semantics.**

For two records with the same id, newer non-`NoChange` region/hint values replace older values and `NoChange` preserves them. `Install + Remove` and `Install + Cancel` become tagged `Cancel` and normalize region/hint to `NoChange`. A `Remove` of a current constraint remains `Remove`. If records carry different ids, select the newer explicit record without borrowing any older payload; do not infer ownership from surface contents.

- [ ] **Step 3: Update staging to preserve identity.**

Have install, remove, region, and hint staging construct an explicit record for the originating constraint. Region and hint staging must reject a missing or dead protocol resource and must never create an unowned surface delta. Keep the exact `wl_surface.commit` capture path unchanged.

- [ ] **Step 4: Make the reducer tests GREEN.**

Run:

```text
rtk cargo test --locked compositor::subsurface::window_geometry_tests
```

Expected: all existing merge tests plus the cancellation payload assertion pass. Refactor only after the focused reducer suite is green.

- [ ] **Step 5: Commit the captured-state stage.**

```text
rtk git add src/compositor/subsurface.rs src/compositor/state/pointer_constraints.rs src/compositor/state/shutdown.rs src/compositor/protocols/core.rs
rtk git commit -m "fix: make captured pointer state constraint scoped"
```

### Task 3: Separate resource liveness from surface ownership

**Files:**
- Modify: `src/compositor/mod.rs` `PointerConstraint` fields and initialization.
- Modify: `src/compositor/state/pointer_constraints.rs` registration, removal, backend validation, publication, and forced teardown.
- Modify: `src/compositor/protocols/advanced.rs` only if request handlers need an explicit liveness guard; preserve protocol event suppression.

**Interfaces:**
- A constraint records protocol-resource liveness separately from surface ownership and semantic `defunct` state.
- Surface ownership distinguishes pending/captured install, current committed state, and removal pending; current ownership remains true through client destroy until removal publication.
- Registration derives `AlreadyConstrained` from the constraint record's ownership state, not from `pending_pointer_constraint_surface_states`.
- A dead resource cannot pass backend activation-current validation or receive `locked`, `confined`, `unlocked`, or `unconfined` events.

- [ ] **Step 1: Add explicit ownership state and initialize it.**

Add fields or an equivalent enum for `protocol_resource_alive`, current surface ownership, captured/pending install ownership, and `lifecycle_removal_pending`. Initialize a new registration as protocol-alive with pending install ownership; leave semantic `defunct` false.

- [ ] **Step 2: Change registration ownership checks.**

Reject a new constraint when any same-surface record owns a pending install, captured install, current committed state, or pending removal. A record is released from `AlreadyConstrained` only after its removal/cancellation has been published; backend deactivation may still be pending after that point. Preserve per-client/per-pointer surface matching behavior already used by the compositor.

- [ ] **Step 3: Change client destruction semantics.**

On protocol destroy, mark the protocol resource dead and clear its resource references. Cancel all backend-pending activation/update bookkeeping and pending reveal state. For a current constraint, keep surface ownership/current routing and do not set semantic `defunct`; stage tagged removal. For a pending or captured install, stage tagged cancellation and remove the non-current record without leaving an activation-capable entry. Do not emit an event to the destroyed resource.

- [ ] **Step 4: Add backend stale-work guards.**

Require `protocol_resource_alive`, generation, ownership, committed state, backend-pending state, and pending backend id to match before resolving or completing activation. Keep late-bound locked activation anchors and generation validation unchanged. Ensure cancellation clears the queue, pending backend id, `backend_pending`, and pending reveal records together.

- [ ] **Step 5: Remove publication owner discovery by iteration.**

Apply only the explicit identity-bearing captured record. For `Install`, mark the identified record current and apply only its payload. For `Remove`, apply any same-commit region/hint to that same record, clear current/removal-pending ownership, mark semantic defunct, and call the existing deactivation path so native work remains queued at the same epoch boundary. For `Cancel`, clear pending work and remove only the identified non-current record without applying region/hint. For unowned `NoChange`, do nothing.

- [ ] **Step 6: Preserve one-shot and forced teardown behavior.**

Keep compositor-driven one-shot `defunct` transitions and their compatibility warp policy intact. Keep surface/pointer/client/shutdown forced teardown immediate, canceling cached/pending work and removing resources without protocol events. Do not use the new client-destroy path for forced teardown.

- [ ] **Step 7: Run the focused integration suite GREEN.**

Run:

```text
rtk cargo test --locked compositor::tests::input_output::pointer_constraint_transaction
rtk cargo test --locked compositor::tests::input_output::pointer_cursor
rtk cargo test --locked compositor::tests::input_output::relative_and_constraints
rtk cargo test --locked compositor::lifecycle::disconnect_removes_pointer_constraints_for_destroyed_surfaces
```

Expected: the new destroy, cancellation, captured-ownership, delayed identity, dead-resource, and stale-backend tests pass, and existing lock/confine/relative/wrap/disconnect behavior remains green.

- [ ] **Step 8: Commit lifecycle and publication closure.**

```text
rtk git add src/compositor/mod.rs src/compositor/state/pointer_constraints.rs src/compositor/protocols/advanced.rs src/compositor/subsurface.rs src/compositor/tests/support/server_runtime.rs src/compositor/tests/support/input_client.rs src/compositor/tests/input_output/pointer_constraint_transaction.rs
rtk git commit -m "fix: close pointer constraint lifecycle ownership"
```

### Task 4: Re-run timing and architecture-preservation coverage

**Files:**
- Read and test: `src/native_output/input/routing.rs`, `src/native_output/runtime/cycle_dispatch.rs`, `src/native_output/runtime/pointer_timing.rs`, and their existing tests.
- Modify: none unless a pointer-constraint ownership change exposes a narrow compile/test adaptation that does not alter timing semantics.

**Interfaces:**
- `NativePointerTransitionEvidence` and `NativeInputEpoch::constraint_settlement_allowed()` remain unchanged.
- Deactivation-A/activation-B selection and wall/thread-CPU association remain covered by the existing tests.

- [ ] **Step 1: Run causal timing tests.**

```text
rtk cargo test --locked native_output::input::routing
rtk cargo test --locked native_output::runtime::pointer_timing
rtk cargo test --locked native_output::runtime::cycle_dispatch
```

- [ ] **Step 2: Run the existing pointer transition tests.**

```text
rtk cargo test --locked pointer_constraint
rtk cargo test --locked relative_and_constraints
rtk cargo test --locked pointer_warp_serial
```

- [ ] **Step 3: Keep timing changes out of this closure.**

Do not modify or commit the native timing files. If an ownership change exposes
a compile-only adaptation in an existing timing test, include that exact test
change in the lifecycle/publication commit after confirming that the timing
assertions and `NativePointerTransitionEvidence` behavior are unchanged. Do
not create an empty commit.

### Task 5: Update the English closure report and verify the checkout

**Files:**
- Modify: `docs/superpowers/specs/2026-09-03-typhon-pointer-constraint-surface-transaction-v1-report.md`.

**Interfaces:**
- The report names the exact previous defects, documents the new lifecycle and captured ownership model, records RED/GREEN results and anything not run, and distinguishes Windows/environment blockers from Linux verification.

- [ ] **Step 1: Update the report with exact defect statements.**

Include these exact defects: `resource death prematurely invalidated current effective routing`; `captured-but-unpublished installs escaped AlreadyConstrained ownership`; `region/hint payloads lacked stable constraint identity`.

- [ ] **Step 2: Document the invariants and policy boundary.**

State exactly: `Protocol resource lifetime, committed surface constraint lifetime, and native effective routing lifetime are separate ownership domains.` State exactly: `Captured pointer-constraint region and cursor-hint state can never be attributed to a different constraint identity.` Continue documenting protocol-defined double buffering for region/hint and Typhon/KWin-inspired commit-synchronized lifecycle policy.

- [ ] **Step 3: Record verification evidence and non-claims.**

List every focused command run, the full verification commands run, passes/failures, platform/library blockers, and commands not run. Explicitly state that the Sober/Roblox pointer jump is not claimed fixed pending native Linux qualification.

- [ ] **Step 4: Run final verification before claiming completion.**

Run each supported command, recording the exit code and full result:

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

Also inspect:

```text
rtk git status --short --branch
rtk git log --oneline -8
rtk git diff --stat fddb4fd..HEAD
```

- [ ] **Step 5: Commit the report.**

```text
rtk git add docs/superpowers/specs/2026-09-03-typhon-pointer-constraint-surface-transaction-v1-report.md
rtk git commit -m "docs: report pointer constraint identity closure"
```
