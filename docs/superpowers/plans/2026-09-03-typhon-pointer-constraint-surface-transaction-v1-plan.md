# Typhon Pointer Constraint Surface Transaction v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. This plan will be executed inline because the user explicitly prohibited subagents.

**Goal:** Make pointer-constraint state exact at `wl_surface.commit`, synchronize client lifecycle changes with published surface generations, and keep native transition timing associated with the transition that produced it.

**Architecture:** Add explicit pending/captured pointer-constraint deltas to the existing `CachedSubsurfaceCommit`, publish the captured payload for every surface-tree node, and keep protocol-resource lifecycle separate from current effective native routing. Native backend work remains queued and settled only at existing `NativeInputEpoch` boundaries; each selected routing transition carries its own action timing.

**Tech Stack:** Rust, Smithay Wayland protocol resources, existing Typhon compositor surface transactions, `NativeInputEpoch`, native input routing tests, and Cargo test/clippy on Windows through `rtk`.

## Global Constraints

- Preserve one `libinput.dispatch()` per `NativeInputEpoch`, bounded raw drain of 256 events, continuation debt, and no client read during continuation.
- Preserve exact relative deltas, locked absolute immutability, current acceleration policy, targeted nonblocking input readiness, the pre-read gate, the transition-latency guard, late-bound activation anchors, one-shot compatibility warp, and generation/token revalidation.
- Do not add an input thread, polling loop, timer, sleep, busy wait, mutex ingress queue, scheduler policy, or realtime-priority policy.
- Do not drop, clamp, reorder, or timestamp-reclassify physical motion.
- `set_region` and `set_cursor_position_hint` remain protocol-defined double-buffered surface state.
- Commit-synchronized creation/client-requested destruction is Typhon architectural policy chosen for coherent surface generations/KWin parity, not an explicit protocol requirement.
- The task does not claim every scheduler-induced physical-input backlog race is closed.
- Do not use synthetic Sober desktop automation as protocol proof.
- Use `rtk` commands and Windows PowerShell paths; do not use subagents.

## File map

- Modify `src/compositor/subsurface.rs` to define the captured pointer payload carried by `CachedSubsurfaceCommit`, its explicit delta types, and pure merge behavior.
- Modify `src/compositor/protocols/core.rs` to capture and clear the pending surface pointer payload at `wl_surface::Request::Commit`.
- Modify `src/compositor/protocols/advanced.rs` so create, region, hint, and destroy requests update protocol-resource state and pending surface deltas instead of publishing or deleting effective state immediately.
- Modify `src/compositor/state/pointer_constraints.rs` to separate pending/current surface state from resource/native metadata, stage lifecycle changes, validate `AlreadyConstrained`, publish captured state, and implement forced teardown.
- Modify `src/compositor/state/surface_transactions.rs` to apply captured pointer state after ordinary surface state for each published node.
- Modify `src/compositor/state/subsurfaces.rs` to publish captured pointer state for the root and every synchronized child; remove root-only pending-state publication.
- Modify `src/compositor/state/surface_focus.rs` so focus reevaluation never copies pending region or hint into current state.
- Modify `src/compositor/mod.rs` and any existing constructors/helpers needed for new state ownership and test commits.
- Modify `src/native_output/input/routing.rs` to return transition-local action timing and retain stale-request rejection.
- Modify `src/native_output/runtime/cycle_dispatch.rs` to consume selected transition evidence as one unit while preserving pre-read observations and epoch boundaries.
- Modify `src/native_output/runtime/pointer_timing.rs` only as needed to accept the transition-local timing without changing its bounded ring or disabled fast path.
- Extend the existing compositor tests under `src/compositor/tests/input_output/` and native timing tests under `src/native_output/runtime/`.
- Update manual `CachedSubsurfaceCommit` constructors in `src/compositor/state/shutdown.rs` and test helpers in `src/compositor/subsurface.rs`.
- Create `docs/superpowers/specs/2026-09-03-typhon-pointer-constraint-surface-transaction-v1-report.md` with evidence, verification, and limitations after implementation.

## Task 1: Associate native transition timing with the selected action

**Files:**
- Modify: `src/native_output/input/routing.rs:1436-1630`
- Modify: `src/native_output/runtime/cycle_dispatch.rs:64-1400`
- Modify: `src/native_output/runtime/pointer_timing.rs:726-1050`
- Test: existing native timing tests in `src/native_output/runtime/pointer_timing.rs` and settlement-focused tests in `src/native_output/input/routing.rs`

**Interfaces:**
- Produce `NativePointerConstraintActionTiming` containing the existing activation/flush timing points for one backend action.
- Produce `NativePointerTransitionEvidence { transition: NativeInputRoutingTransition, action_timing: NativePointerConstraintActionTiming }`.
- Replace the independent `routing_transition` and action-timing selection in `NativePointerConstraintSettlementOutcome` with `selected_transition: Option<NativePointerTransitionEvidence>` and a `settlement_timing` field for aggregate settlement data.
- Keep `pointer_timing` records and `NativePointerTimingTransition` unchanged except for receiving the selected evidence's matching timing.

- [ ] **Step 1: Add RED tests for transition-local evidence.**

Add tests covering these exact invariants:

```rust
#[test]
fn deactivation_a_does_not_receive_activation_b_timing() { /* selected A timing is unknown */ }

#[test]
fn activation_a_timing_stays_with_activation_a_when_later_action_deactivates_b() { /* selected activation A */ }

#[test]
fn selected_deactivation_has_unknown_activation_phase_even_if_neighbor_activates() { /* no borrowed timing */ }

#[test]
fn selected_action_wall_and_thread_cpu_measurements_have_one_constraint_identity() { /* same id */ }
```

Use the existing backend request/test construction patterns and assert the selected evidence, rather than only asserting a summary containing non-`None` timestamps.

- [ ] **Step 2: Run the focused tests and observe failure.**

Run:

```text
rtk cargo test --locked native_output::input::routing::tests::deactivation_a_does_not_receive_activation_b_timing
rtk cargo test --locked native_output::runtime::pointer_timing::tests::selected_action_wall_and_thread_cpu_measurements_have_one_constraint_identity
```

Expected: compilation or assertion failure because the current settlement result keeps transition identity and first activation timing in separate fields.

- [ ] **Step 3: Implement per-action timing.**

In `process_native_pointer_constraint_backend_requests`, allocate no metadata when timing is disabled. When timing is enabled, capture the timestamps for the current activation action into a local `NativePointerConstraintActionTiming`, create `NativePointerTransitionEvidence` from the same action transition, and set `selected_transition` only with that evidence. Do not copy timing from a neighboring request. Preserve the existing aggregate timing values for callers that need settlement-wide diagnostics, but stop using them as the identity-bearing timing in `cycle_dispatch`.

Update initial and final settlement handling in `dispatch_wayland_and_input` to call `timing_transition(evidence.transition)` and use `evidence.action_timing` for `NativePointerPreReadObservation`. Preserve all existing `constraint_settlement_start/end`, targeted readiness, and epoch timing behavior.

- [ ] **Step 4: Run the focused timing tests.**

Run:

```text
rtk cargo test --locked native_output::input::routing
rtk cargo test --locked native_output::runtime::pointer_timing
rtk cargo test --locked native_output::runtime::cycle_dispatch
```

Expected: PASS, including existing disabled-fast-path, ring, pre-read, CPU, and empty-input tests.

- [ ] **Step 5: Commit the timing fix.**

```text
rtk git add src/native_output/input/routing.rs src/native_output/runtime/cycle_dispatch.rs src/native_output/runtime/pointer_timing.rs
rtk git commit -m "fix: associate pointer transition timing causally"
```

## Task 2: Model and merge commit-exact pointer state

**Files:**
- Modify: `src/compositor/subsurface.rs:1-120, test helpers`
- Modify: `src/compositor/mod.rs` state declarations and constructors
- Modify: `src/compositor/state/pointer_constraints.rs` pending/current representations
- Modify: `src/compositor/state/shutdown.rs` empty commit constructor
- Test: `src/compositor/subsurface.rs` unit tests or the existing compositor transaction test module

**Interfaces:**
- Add explicit pending delta types with `NoChange` variants for lifecycle, region, and cursor hint.
- Add `CapturedPointerConstraintSurfaceState` as an immutable `CachedSubsurfaceCommit` field.
- Add a state method equivalent to `take_pending_pointer_constraint_surface_state(surface_id, commit_sequence) -> CapturedPointerConstraintSurfaceState`.
- Add a state method equivalent to `merge_captured_pointer_constraint_surface_state(existing, incoming) -> CapturedPointerConstraintSurfaceState` through `CachedSubsurfaceCommit::merge`.

- [ ] **Step 1: Add RED tests for pure merge semantics.**

Write tests with explicit values and identities for:

```rust
#[test]
fn commit_without_pointer_state_then_pointer_state_keeps_new_state() { /* A/no pointer -> B */ }

#[test]
fn newer_region_replaces_older_region_but_no_change_preserves_it() { /* R1 -> R2 */ }

#[test]
fn explicit_default_region_is_not_treated_as_no_change() { /* null/default is a value */ }

#[test]
fn install_then_remove_before_publication_collapses_without_activation() { /* no ghost */ }

#[test]
fn current_constraint_removal_survives_cached_commit_merge() { /* effective removal */ }
```

Update every manual `CachedSubsurfaceCommit` fixture to construct the new payload with an explicit default, but do not implement merge behavior yet.

- [ ] **Step 2: Run the merge tests and observe failure.**

Run:

```text
rtk cargo test --locked compositor::subsurface
```

Expected: compile or assertion failures because `CachedSubsurfaceCommit` has no pointer payload and no pointer-specific merge reducer.

- [ ] **Step 3: Implement the data model and reducer.**

Define the pending and captured enums so `NoChange`, explicit empty/default region, concrete region, and concrete finite hint are distinct. Store the captured value in `CachedSubsurfaceCommit`. Extend its existing merge destructuring and update logic so the newer explicit pointer delta wins and `NoChange` preserves the earlier captured value. Ensure merging never borrows or reads live `PointerConstraint` pending fields.

Keep the model allocation-bounded: one payload per cached surface commit, no event history and no per-input-event metadata.

- [ ] **Step 4: Run the merge tests and formatting.**

Run:

```text
rtk cargo fmt --check
rtk cargo test --locked compositor::subsurface
```

Expected: PASS for pure merge tests and all existing commit fixture tests.

- [ ] **Step 5: Commit the commit-state model.**

```text
rtk git add src/compositor/subsurface.rs src/compositor/mod.rs src/compositor/state/pointer_constraints.rs src/compositor/state/shutdown.rs
rtk git commit -m "fix: capture pointer constraint state in surface commits"
```

## Task 3: Capture and publish committed surface pointer state

**Files:**
- Modify: `src/compositor/protocols/core.rs` `wl_surface::Request::Commit`
- Modify: `src/compositor/state/surface_transactions.rs:162-313`
- Modify: `src/compositor/state/subsurfaces.rs:70-441, 728-775, 1029-1072`
- Modify: `src/compositor/state/surface_focus.rs:49-111`
- Test: existing compositor input/output transaction tests

**Interfaces:**
- `wl_surface::Commit` consumes pending pointer deltas before constructing `CachedSubsurfaceCommit`.
- `apply_cached_subsurface_commit` applies only the commit's captured pointer payload.
- `publish_surface_tree` applies pointer state for the root and every child after each node's ordinary state is current.
- Focus reevaluation observes current state but never calls a pending-to-current publisher.

- [ ] **Step 1: Add RED tests for commit capture and publication.**

Add protocol-level tests for:

```rust
#[test]
fn lock_without_surface_commit_does_not_activate_or_send_locked() { /* pending only */ }

#[test]
fn empty_surface_commit_publishes_initial_lock_state() { /* matching commit */ }

#[test]
fn focus_change_does_not_publish_pending_region_or_hint() { /* focus is not commit */ }

#[test]
fn delayed_commit_does_not_absorb_region_or_hint_from_a_later_request() { /* R2 belongs to later commit */ }

#[test]
fn synchronized_child_commit_publishes_child_pointer_state() { /* not root-only */ }

#[test]
fn region_and_input_region_in_one_commit_use_new_input_region() { /* R2 ∩ I2 */ }
```

- [ ] **Step 2: Run the focused tests and observe failure.**

Run:

```text
rtk cargo test --locked compositor::tests::input_output
```

Expected: failures showing lock activation before commit, focus publishing pending fields, delayed requests leaking backward, or child state not being applied.

- [ ] **Step 3: Capture pending state in `wl_surface::Commit`.**

Take the surface's pending pointer delta alongside the existing pending window geometry, viewport, scale, transform, input region, opaque region, pacing, presentation, and attachment fields. Put the taken value into `CachedSubsurfaceCommit`; reset pending state to `NoChange` so later protocol requests can only affect a later commit.

- [ ] **Step 4: Apply captured state in transaction order.**

In `apply_cached_subsurface_commit`, apply ordinary surface data first and then reduce the commit payload into current pointer-constraint surface state. Derive confinement regions only after current input region and renderable placement are updated. Queue reevaluation from current state; do not copy pending region or hint.

- [ ] **Step 5: Publish every tree node's payload.**

Reorder `publish_surface_tree` so each node's parent/placement and ordinary cached state are applied before that node's captured pointer payload. Apply the root payload and every synchronized child payload. Remove the root-only call to `apply_pending_pointer_constraint_state_for_surface`.

- [ ] **Step 6: Remove focus-time pending publication.**

Change `set_desktop_focus` to reevaluate already-current constraints only. A focus change may affect activation eligibility and queue a native request, but cannot promote a pending region, hint, install, or removal.

- [ ] **Step 7: Run the focused tests and inspect the transaction diff.**

Run:

```text
rtk cargo fmt --check
rtk cargo test --locked compositor::tests::input_output
rtk git diff --check
```

Expected: PASS, with no whitespace errors and no references from focus/publication to mutable pending pointer state.

- [ ] **Step 8: Commit commit-synchronized publication.**

```text
rtk git add src/compositor/protocols/core.rs src/compositor/state/surface_transactions.rs src/compositor/state/subsurfaces.rs src/compositor/state/surface_focus.rs
rtk git commit -m "fix: publish pointer constraints from committed surface state"
```

## Task 4: Synchronize protocol lifecycle and forced teardown

**Files:**
- Modify: `src/compositor/protocols/advanced.rs` lock/confine create, set-region, hint, and destroy handlers
- Modify: `src/compositor/state/pointer_constraints.rs` registration, removal, activation, deactivation, and teardown paths
- Modify: `src/compositor/state/surfaces.rs` and `src/compositor/state/client_lifecycle.rs` only where teardown needs the new forced-removal entry point
- Test: `src/compositor/tests/input_output/relative_and_constraints.rs`, `pointer_lock_warp.rs`, `pointer_warp_serial.rs`, and real-resource protocol tests

**Interfaces:**
- Protocol create registers a live resource and pending install but does not request native activation until publication.
- Protocol destroy calls a client-lifecycle staging method; forced teardown calls a separate immediate invalidation method.
- `AlreadyConstrained` checks current effective constraints and pending installs.
- Backend requests carry only validated current identities; stale requests fail without events or routing changes.

- [ ] **Step 1: Add RED lifecycle tests.**

Add tests for:

```rust
#[test]
fn destroy_without_commit_keeps_no_effective_constraint() { /* install never published */ }

#[test]
fn destroy_after_install_requires_removal_commit() { /* current remains until commit */ }

#[test]
fn create_then_destroy_before_first_commit_has_no_locked_event_or_backend_activation() { /* no ghost */ }

#[test]
fn pending_install_is_already_constrained_for_the_same_surface_identity() { /* request rejected */ }

#[test]
fn install_and_remove_before_backend_settlement_rejects_stale_activation() { /* generation/token */ }

#[test]
fn forced_teardown_cancels_pending_and_active_routing_without_dead_resource_events() { /* immediate */ }

#[test]
fn same_commit_hint_and_removal_uses_captured_hint_for_restore_without_relative_motion() { /* exact commit */ }
```

- [ ] **Step 2: Run lifecycle tests and observe failure.**

Run:

```text
rtk cargo test --locked compositor::tests::input_output::relative_and_constraints
rtk cargo test --locked compositor::tests::input_output::pointer_lock_warp
rtk cargo test --locked compositor::tests::input_output::pointer_warp_serial
```

Expected: failures because create activates immediately, destroy removes immediately, pending installs are absent from `AlreadyConstrained`, or stale backend work can outlive the resource state.

- [ ] **Step 3: Stage installs at resource creation.**

Change `register_pointer_constraint` to create the resource metadata and pending install identity, including the initial requested region. Do not populate current effective state or call native activation. Treat an empty commit as the commit that publishes the initial state.

- [ ] **Step 4: Stage client-requested removal.**

Change protocol `Destroy` handling to mark the resource defunct, clear its resource handles, and record a pending removal delta. Keep current committed routing metadata alive until the removal commit. If no install has ever been published, collapse the staged install/removal to no effective state. Do not send events through defunct resources.

- [ ] **Step 5: Split forced teardown from client removal.**

Update surface/client/pointer/seat/shutdown teardown callers to use an immediate forced invalidation path. Cancel pending backend activation, current backend state, active routing, reveal/cursor-visibility ownership, and restore work with generation/token checks before releasing metadata. Preserve one-shot compositor-driven deactivation as its existing separate policy.

- [ ] **Step 6: Validate current-state publication and stale requests.**

Advance the effective generation when a captured install or removal becomes current. Make all activation/deactivation/update paths validate internal identity, generation, liveness, surface/pointer/seat ownership, and current focus before changing native routing. Keep `ActivateLocked { id }` late-bound with no request-time coordinate; compute the anchor only after validation from the current native compositor pointer position.

- [ ] **Step 7: Run lifecycle tests and existing warp/constraint tests.**

Run:

```text
rtk cargo test --locked compositor::tests::input_output::relative_and_constraints
rtk cargo test --locked compositor::tests::input_output::pointer_lock_warp
rtk cargo test --locked compositor::tests::input_output::pointer_warp_serial
rtk cargo test --locked compositor::tests::input_output
```

Expected: PASS, including one-shot hint warp, compositor-driven one-shot deactivation, relative-resource compatibility, stale activation rejection, and forced teardown.

## Task 5: Integrate committed topology with native epochs and finish regressions

**Files:**
- Modify: `src/compositor/state/pointer_constraints.rs` native-request queueing helpers if required by staged current state
- Modify: `src/native_output/input/epoch.rs` only if an invariant assertion needs to cover the unchanged settlement boundary
- Modify: `src/native_output/runtime/cycle_dispatch.rs` existing pre-read/settlement call sites
- Test: `src/native_output/runtime/cycle_dispatch.rs`, `src/native_output/tests/input.rs`, and compositor input/output tests

**Interfaces:**
- Current committed surface state queues native work only through the existing settlement request queue.
- `NativeInputEpoch::constraint_settlement_allowed()` remains the only permission boundary for native constraint settlement.
- `NativePointerTransitionEvidence` is the only source used to associate a transition with action timing.

- [ ] **Step 1: Add RED epoch-boundary regressions.**

Extend existing tests with:

```rust
#[test]
fn committed_install_waits_until_the_current_input_epoch_ends() { /* no mid-epoch activation */ }

#[test]
fn committed_removal_waits_until_the_current_input_epoch_ends() { /* no mid-epoch deactivation */ }

#[test]
fn continuation_debt_over_256_events_preserves_constraint_settlement_boundary() { /* no read mid-epoch */ }

#[test]
fn pre_existing_activation_is_effective_before_a_new_input_epoch() { /* existing contract */ }
```

- [ ] **Step 2: Run focused epoch tests and observe any regression.**

Run:

```text
rtk cargo test --locked native_output::runtime::cycle_dispatch
rtk cargo test --locked native_output::tests::input
```

Expected: new tests fail until all current-state queueing and evidence consumers use the existing epoch settlement gate.

- [ ] **Step 3: Route committed topology through the existing gate.**

Ensure commit publication only changes compositor current state and enqueues a backend request. Let the existing initial settlement, targeted pre-read promotion, input epoch, and final settlement flow process it. Do not call backend operations directly from commit or focus code.

- [ ] **Step 4: Verify stale and continuation behavior.**

Use the existing `pointer_constraint_activation_request_id`, `pointer_constraint_backend_activation_current`, and generation checks for every request variant that can mutate native routing. Confirm `NativeInputEpoch::begin_dispatch`, bounded drain, continuation debt, and no-client-read continuation paths remain unchanged except for the new tests.

- [ ] **Step 5: Run all focused regressions.**

Run:

```text
rtk cargo test --locked native_output::runtime::cycle_dispatch
rtk cargo test --locked native_output::runtime::pointer_timing
rtk cargo test --locked compositor::tests::input_output
rtk cargo test --locked native_output::tests::input
```

Expected: PASS.

- [ ] **Step 6: Commit native integration.**

```text
rtk git add src/compositor/state/pointer_constraints.rs src/native_output/input/epoch.rs src/native_output/runtime/cycle_dispatch.rs src/native_output/tests/input.rs src/compositor/tests/input_output
rtk git commit -m "fix: settle committed pointer topology at native epochs"
```

## Task 6: Full verification and evidence report

**Files:**
- Create: `docs/superpowers/specs/2026-09-03-typhon-pointer-constraint-surface-transaction-v1-report.md`
- Inspect: all changed files and the four implementation commits

- [ ] **Step 1: Run formatting and static checks.**

```text
rtk cargo fmt --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
```

Expected: all commands exit successfully with no warnings promoted to errors.

- [ ] **Step 2: Run the locked full test suite.**

```text
rtk cargo test --locked
```

Expected: all tests pass. Record exact output summary and any test targets not exercised by the command.

- [ ] **Step 3: Check the final diff.**

```text
rtk git diff --check
rtk git status --short
rtk git log --oneline --decorate -8
```

Expected: no whitespace errors, only intended commits/files, and a clean working tree after the report commit.

- [ ] **Step 4: Write the evidence report.**

The report must state the starting and ending HEADs; list inspected and changed files; explain protocol requirements versus Typhon policy; cite the KWin commit principle supplied in the brief; document the old mixed model, focus-time pending publication defect, delayed backward leak, and root-only synchronized-child gap; describe pending/captured/current/native layers, merge rules, lifecycle cases, `AlreadyConstrained`, one-shot behavior, forced teardown, publication ordering, native epoch/generation/current-token validation, and timing association.

It must include these exact statements:

```text
`set_region` and `set_cursor_position_hint` are protocol-defined double-buffered state and are now owned by the exact `wl_surface.commit` that captured them.

Commit-synchronized creation/client-requested destruction is Typhon architectural policy chosen for coherent surface generations/KWin parity, not an explicit protocol requirement.

The task does not claim every scheduler-induced physical-input backlog race is closed.
```

Record RED/GREEN tests, focused verification, full verification, files changed, commands not run, and any remaining limitation.

- [ ] **Step 5: Commit the report.**

```text
rtk git add docs/superpowers/specs/2026-09-03-typhon-pointer-constraint-surface-transaction-v1-report.md
rtk git commit -m "docs: report pointer constraint surface transaction closure"
```

The final handoff will include the complete commit list and only verification commands that actually ran.
