# Implementation Plan: XWayland Popup Crossing

## Overview

Implement the approved XWayland popup-crossing design on `main`. The work is
ordered from XWM protocol authority, through compositor stack and pointer
state, to the native-cycle boundary and diagnostics. Each implementation
slice starts with a failing regression test, then the smallest production
change, then focused verification. Direct compositor test/API calls remain
immediate outside an explicit scene batch.

## Invariants

- QueryTree is asynchronous, nonblocking, one-request-in-flight, and scoped
  to the current XWayland generation.
- Only a reply whose request epoch equals the current dirty epoch can produce
  a snapshot. Superseded or incomplete replies do not mutate scene order.
- Root children are documented and represented bottom-to-top.
- Override-redirect relative order comes from the X root tree; managed order
  and compositor layer policy remain Typhon-owned.
- Observed override-redirect ConfigureNotify never emits RestackExact.
- Override-redirect windows do not participate in transient-family raising or
  automatic family normalization.
- Scene batch begin/commit is explicit, non-nestable, token-checked, and has
  no side effects from drop or abort cleanup.
- Batch commit occurs before backend-command collection and command execution.
- Pointer crossing is one final atomic operation, with leave before enter and
  no intermediate target exposure.
- Existing damage/publication journals remain the owners of damage; the batch
  coalesces only repaint scheduling.
- Keyboard focus remains on the managed application while pointer targeting
  uses the actual topmost popup surface.

## Phase 0: Baseline and Test Harness

### Task 1: Establish baseline

**Description:** Confirm the cleaned `main` worktree, record the existing
focused test commands, and identify fixture helpers without changing source.

**Acceptance criteria:**

- [ ] `git status --short` is empty.
- [ ] Existing XWM and compositor XWayland tests pass before changes.
- [ ] The baseline test commands and relevant fixture names are recorded in
      the implementation notes or commit message.

**Verification:**

- `cargo test --locked xwayland::xwm::`
- `cargo test --locked compositor::tests::xwayland::`
- `cargo test --locked native_output::runtime::xwayland_reactor_tests::`

**Dependencies:** None

**Files likely touched:** None

**Estimated scope:** Small

## Phase 1: Authoritative XWM Root Stacking

### Task 2: Add failing QueryTree state-machine tests

**Description:** Add tests for dirty epochs, one pending query, nonblocking
reply polling, superseded replies, incomplete replies, generation changes,
and bottom-to-top filtering. Tests use the existing XWM fake connection and
raw reply helpers rather than a second protocol mock.

**Acceptance criteria:**

- [ ] A dirty stack issues one QueryTree request and records its sequence and
      epoch.
- [ ] Repeated dirty events while pending issue no additional request.
- [ ] A reply from an older epoch is consumed, counted as superseded, emits no
      snapshot, and causes exactly one follow-up query.
- [ ] A reply missing a currently live mapped override-redirect record is
      incomplete, does not mutate order, and requests reconciliation again.
- [ ] Stale-generation replies do not emit snapshots or mutate current state.
- [ ] QueryTree child order is asserted as bottom-to-top and unknown XIDs are
      pruned without failing the complete snapshot.

**Verification:**

- Run the new focused XWM tests and confirm each new test fails before the
  implementation exists.

**Dependencies:** Task 1

**Files likely touched:**

- `src/xwayland/xwm/events_regression_tests.rs`
- `src/xwayland/xwm/api.rs`
- `src/xwayland/xwm/reactor.rs`

**Estimated scope:** Medium

### Task 3: Implement XWM root-stack reconciliation

**Description:** Add running-state QueryTree tracking separate from startup
adoption's `pending_tree`. Add the snapshot event and metrics, poll replies
with the existing reconstructed-cookie pattern, and mark dirty from relevant
X events, kind transitions, and startup adoption completion.

**Acceptance criteria:**

- [ ] At most one running QueryTree request is in flight.
- [ ] `WouldBlock` retains the request for a later reactor wakeup.
- [ ] Only the current dirty epoch emits a snapshot.
- [ ] Superseded and incomplete replies are retained only as diagnostics and
      never become compositor snapshots.
- [ ] Generation teardown clears pending reconciliation state.
- [ ] Dirty tracking covers OR map/unmap/destroy/configure/restack, kind
      changes, and startup adoption completion.

**Verification:**

- `cargo test --locked xwayland::xwm::events_regression_tests`
- `cargo test --locked xwayland::xwm::window`
- `cargo fmt --check`

**Dependencies:** Task 2

**Files likely touched:**

- `src/xwayland/xwm/mod.rs`
- `src/xwayland/xwm/api.rs`
- `src/xwayland/xwm/reactor.rs`
- `src/xwayland/xwm/events.rs`
- `src/xwayland/xwm/startup_adoption.rs`
- `src/xwayland/xwm/metrics.rs`

**Estimated scope:** Large, split if needed

### Checkpoint: XWM Authority

- [ ] QueryTree tests pass.
- [ ] No startup `pending_tree` behavior changed.
- [ ] No synchronous wait or unbounded query queue exists.
- [ ] XWM event drain remains bounded and nonblocking.

## Phase 2: Compositor Stack Authority

### Task 4: Add failing snapshot and feedback tests

**Description:** Add compositor tests for applying two OR snapshots, preserving
managed relative order, stale rejection, idempotence, kind migration, and
absence of ConfigureNotify restack feedback. Add transient-family tests that
allow a parent/submenu chain while preventing OR family promotion.

**Acceptance criteria:**

- [ ] Snapshot `[A, B]` produces A below B and `[B, A]` produces B below A.
- [ ] Managed X11 ordering and non-X11 layer ordering remain unchanged.
- [ ] Applying an identical snapshot twice does not reorder or refresh.
- [ ] Older epochs and generations do not mutate scene state or refresh
      pointer focus.
- [ ] OR ConfigureNotify requests reconciliation but produces no RestackExact.
- [ ] Transient metadata remains available while OR windows are excluded from
      family raising, subtree ordering, and automatic parent-above-child
      correction.

**Verification:**

- Run focused compositor tests and confirm the new assertions fail against
  current behavior.

**Dependencies:** Task 3

**Files likely touched:**

- `src/compositor/tests/xwayland.rs`
- `src/compositor/state/desktop_window_tests.rs`
- `src/compositor/tests/xwayland_geometry_ordering.rs`

**Estimated scope:** Medium

### Task 5: Implement snapshot application and command audit

**Description:** Apply snapshots only to live OR members, exclude OR windows
from transient-family ordering and family raising, and update ConfigureNotify
handling so observed X order is never written back. Keep deliberate managed
and client-request stack commands intact and document each caller.

**Acceptance criteria:**

- [ ] Snapshot application changes only the OR subsequence of compositor
      stacking.
- [ ] Renderable surfaces follow the resulting scene order at most once per
      application.
- [ ] Managed, XDG, layer-shell, and non-X11 order is preserved.
- [ ] No observation-driven OR RestackExact command is emitted.
- [ ] Deliberate managed ConfigureRequest and compositor stack operations still
      acknowledge/execute as before.

**Verification:**

- `cargo test --locked compositor::tests::xwayland::`
- `cargo test --locked compositor::state::desktop_window_tests`
- Search all `RestackExact`, `Raise`, `RaiseFamily`, `Stack`, and `StackFamily`
  callers and review their role classification.

**Dependencies:** Task 4

**Files likely touched:**

- `src/compositor/state/desktop_windows.rs`
- `src/compositor/state/subsurfaces.rs`
- `src/compositor/server.rs`
- `src/compositor/server_xwayland.rs`
- `src/compositor/window_backend.rs`
- `src/compositor/tests/xwayland.rs`

**Estimated scope:** Large, split if needed

## Phase 3: Explicit Scene Batch and Pointer Crossing

### Task 6: Add failing batch-token and pointer tests

**Description:** Add tests before changing batching. Cover exact token
ownership, nested begin rejection, mismatched and double commit, abort/drop
cleanup without side effects, same-batch temporary popup suppression,
association replacement, map/unmap cancellation, and atomic leave/enter
ordering under pointer constraints and implicit grabs.

**Acceptance criteria:**

- [ ] Batch begin returns a token/epoch and rejects nesting.
- [ ] Mismatched and double commits are rejected without scene side effects.
- [ ] Abort/drop cleanup clears active bookkeeping but leaves dirty work for a
      later commit.
- [ ] M -> A -> removal -> B in one batch produces only M -> B.
- [ ] Map and unmap in one batch never exposes A as a pointer target.
- [ ] Attachment replacement does not enter the retired surface.
- [ ] Final crossing queues leave before enter and frames only after all events
      are queued.
- [ ] Active grabs and constraints retain ownership throughout the commit.

**Verification:**

- Run the new tests and confirm they fail before production batching code.

**Dependencies:** Task 5

**Files likely touched:**

- `src/compositor/tests/xwayland.rs`
- `src/compositor/tests/xwayland_resize_visual.rs`
- `src/compositor/tests/support/registry_state.rs`
- `src/compositor/state/hit_testing.rs`

**Estimated scope:** Large, split if needed

### Task 7: Implement explicit compositor batch and atomic crossing

**Description:** Add non-nestable batch state and token APIs to
`CompositorState`/`OwnCompositorServer`. Gate only XWayland scene side effects;
leave normal pointer motion and Wayland-native popup behavior immediate.
Refactor pointer event helpers into an atomic final crossing primitive that
queues events and groups final frames without sending to dead resources.

**Acceptance criteria:**

- [ ] Direct non-batch event application remains immediate.
- [ ] Active batches defer pointer refresh, render-stack reorder, client-list
      publication, and repaint scheduling.
- [ ] Commit applies the newest available snapshot, reorders once, syncs lists
      once, performs a conditional final hit-test, crosses atomically, and
      schedules one repaint signal.
- [ ] Existing damage/publication journals remain unchanged as owners of
      accumulated damage.
- [ ] Focused keyboard identity is not changed by popup admission or crossing.

**Verification:**

- `cargo test --locked compositor::tests::xwayland::`
- `cargo test --locked compositor::tests::xdg::`
- `cargo test --locked compositor::state::hit_testing`
- `cargo test --locked compositor::state::pointer_constraints`

**Dependencies:** Task 6

**Files likely touched:**

- `src/compositor/mod.rs`
- `src/compositor/server.rs`
- `src/compositor/state/hit_testing.rs`
- `src/compositor/state/subsurfaces.rs`
- `src/compositor/state/xwayland_windows.rs`
- `src/compositor/state/surface_commits.rs`
- `src/compositor/state/windows.rs`

**Estimated scope:** Large, split if needed

### Checkpoint: Compositor Commit

- [ ] Batch-token tests pass, including failure cleanup.
- [ ] Pointer events show one final crossing and no temporary target.
- [ ] Existing Wayland popup and pointer-constraint tests pass.
- [ ] No second damage accumulator exists.

## Phase 4: Native-Cycle Integration

### Task 8: Add failing native-cycle ordering tests

**Description:** Extend the native XWayland reactor fixture to assert the
complete production sequence: associations, buffers, XWM events, scene
commit, backend collection, terminal command normalization, execution, and
flush. Include a test proving commands emitted by commit execute in the same
cycle.

**Acceptance criteria:**

- [ ] One native-cycle mutation batch yields one final pointer refresh.
- [ ] Commit-generated client-list/focus/managed commands are collected and
      executed before cycle completion.
- [ ] Destroyed-window normalization still prunes commands after commit.
- [ ] Required lifecycle, buffer-release, protocol replies, and managed
      ConfigureRequest acknowledgements are not coalesced away.

**Verification:**

- Run focused `xwayland_reactor_tests` and confirm ordering assertions fail
  against the current dispatch split.

**Dependencies:** Task 7

**Files likely touched:**

- `src/native_output/runtime/cycle.rs`
- `src/native_output/runtime/xwayland.rs`
- `src/native_output/runtime/xwayland_reactor_tests.rs`

**Estimated scope:** Medium

### Task 9: Refactor dispatch around commit boundary

**Description:** Begin the batch before association dispatch, apply association
and buffer events, apply all XWM events, explicitly commit, then collect backend
commands, normalize terminal handles, execute commands, and flush. Preserve
focus deadline handling and existing command coalescing after commit.

**Acceptance criteria:**

- [ ] Commit occurs before `take_xwayland_backend_commands()`.
- [ ] Commit-produced commands are present in the same command collection and
      execution pass.
- [ ] Early-return/error cleanup aborts bookkeeping without side effects and
      leaves dirty work available.
- [ ] No KMS, scanout, pageflip, output transaction, or explicit-sync code is
      changed.

**Verification:**

- `cargo test --locked native_output::runtime::xwayland_reactor_tests::`
- `cargo test --locked compositor::tests::xwayland::`
- Review `git diff -- src/native_output` for scope containment.

**Dependencies:** Task 8

**Files likely touched:**

- `src/native_output/runtime/cycle.rs`
- `src/native_output/runtime/xwayland.rs`

**Estimated scope:** Medium

## Phase 5: Lifecycle, Focus, Trace, and Metrics

### Task 10: Add failing lifecycle/focus/trace tests

**Description:** Add tests for pre-admission cancellation, first versus
redundant cleanup, generation teardown, focus preservation across grab/ungrab
events, bounded recent lifecycle retention, and flush-storm resistance.

**Acceptance criteria:**

- [ ] A canceled association never produces WindowReady admission, renderable
      publication, client-list membership, or pointer enter.
- [ ] Repeated teardown is a no-op with a distinct redundant-cleanup record.
- [ ] Popup roles never replace the active managed X11 focus.
- [ ] Lifecycle records remain available after a flush storm.
- [ ] Flush calls do not produce individual high-value trace records.

**Verification:**

- Run focused XWM focus, lifecycle, compositor association, and trace tests.
- Confirm each new regression test fails before implementation changes.

**Dependencies:** Task 9

**Files likely touched:**

- `src/xwayland/trace.rs`
- `src/xwayland/xwm/focus.rs`
- `src/xwayland/xwm/commands.rs`
- `src/xwayland/xwm/events.rs`
- `src/compositor/server_xwayland.rs`
- `src/compositor/state/windows.rs`
- `src/compositor/tests/xwayland.rs`

**Estimated scope:** Large, split if needed

### Task 11: Implement lifecycle, focus policy, trace retention, and metrics

**Description:** Implement exact terminal-reason settlement and idempotent
cleanup, preserve managed keyboard focus, remove flush trace pollution, add
bounded category-aware lifecycle retention, and expose all requested counters
through the existing XWayland metrics paths.

**Acceptance criteria:**

- [ ] All requested lifecycle fields are present on relevant trace records.
- [ ] All requested counters are incremented at the owning boundary and
      remain bounded in normal logging paths.
- [ ] Focus events caused by grabs/ungrabs cannot replace managed focus.
- [ ] Pre-admission cancellation and attachment replacement settle once.
- [ ] No application, Steam, Wine, Proton, title, PID, executable, or class
      heuristic is introduced.

**Verification:**

- `cargo test --locked xwayland::xwm::focus`
- `cargo test --locked xwayland::trace`
- `cargo test --locked compositor::tests::xwayland::`
- Audit for forbidden heuristics and `x11_resize_command_order` flush emits.

**Dependencies:** Task 10

**Files likely touched:**

- `src/xwayland/trace.rs`
- `src/xwayland/metrics.rs`
- `src/xwayland/service.rs`
- `src/xwayland/xwm/commands.rs`
- `src/xwayland/xwm/events.rs`
- `src/xwayland/xwm/focus.rs`
- `src/compositor/mod.rs`
- `src/native_output/runtime/metrics.rs`

**Estimated scope:** Large, split if needed

## Phase 6: Stress and Real-Path Coverage

### Task 12: Add deterministic stress and performance assertions

**Description:** Add at least 1000 deterministic popup mutation cycles,
including map, association, buffer-ready, metadata, configure, snapshot,
unmap, destroy, and attachment replacement combinations. Assert bounded query,
pointer, reorder, client-list, repaint, and memory/FD behavior.

**Acceptance criteria:**

- [ ] No stale popup, stale snapshot, duplicate client-list entry, or feedback
      RestackExact remains after the run.
- [ ] Pointer refreshes scale with completed batches rather than raw events.
- [ ] Root queries remain at most one active plus bounded follow-ups.
- [ ] Work is linear in ingested mutations with O(1) final pointer and stack
      work per batch.

**Verification:**

- Run the focused stress test repeatedly with `--test-threads=1`.
- Check process/resource behavior using the existing deterministic fixture,
  not sleeps or arbitrary event cutoffs.

**Dependencies:** Task 11

**Files likely touched:**

- `src/compositor/tests/xwayland.rs`
- `src/xwayland/xwm/events_regression_tests.rs`
- `src/native_output/runtime/xwayland_reactor_tests.rs`

**Estimated scope:** Medium

### Task 13: Exercise the real XWM path

**Description:** Extend the installed-XWayland fixture as practical to cover
X event drain, dirty tracking, asynchronous QueryTree, snapshot event, scene
batch commit, and final crossing. Skip honestly when required runtime assets
are unavailable.

**Acceptance criteria:**

- [ ] The test observes the actual XWM event drain and QueryTree request/reply
      path when the environment supports it.
- [ ] The compositor receives a snapshot event and commits one final target.
- [ ] No test claims hardware or Steam qualification.

**Verification:**

- `TYPHON_XWAYLAND_NATIVE_TESTS=1 cargo test --locked --test xwayland_native_regression -- --nocapture`

**Dependencies:** Task 12

**Files likely touched:**

- `src/native_output/runtime/xwayland_reactor_tests.rs`
- `tests/xwayland_native_regression.rs`

**Estimated scope:** Medium

## Checkpoint: Reviews and Validation

### Task 14: Run two fresh reviews

**Review A: X11 authority and lifecycle**

- [ ] Root-tree order is used only for live override-redirect windows.
- [ ] Managed order remains compositor-owned.
- [ ] ConfigureNotify writeback loops are absent.
- [ ] Stale queries cannot mutate a new generation.
- [ ] Unmap/destroy and pre-admission cancellation settle immediately and once.
- [ ] Transient metadata cannot reorder OR families.

**Review B: Pointer and focus semantics**

- [ ] One final pointer refresh occurs only for pointer-affecting batches.
- [ ] Temporary popup targets are unobservable.
- [ ] Leave precedes enter and frames are grouped correctly.
- [ ] Parent/submenu chains coexist.
- [ ] Keyboard focus remains managed while actual popup surfaces receive input.
- [ ] No focused-window hit-test substitution or application heuristic exists.

**Dependencies:** Tasks 1-13

### Task 15: Full validation

**Verification:**

- `cargo fmt --check`
- `cargo check --locked --all-targets`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo test --locked`
- `cargo test --locked -- --test-threads=1`
- `cargo build --locked --release`
- `./bin/check-source-layout`
- `git diff --check`

Report unavailable environment-dependent checks explicitly. Do not claim
real TTY/DRM or Steam qualification unless run.

## Commit Boundaries

Use focused commits where each slice is green:

1. `test(xwayland): cover popup stack authority and query coalescing`
2. `fix(xwayland): reconcile override-redirect root stacking`
3. `test(compositor): cover atomic xwayland scene commits`
4. `fix(compositor): batch xwayland scene mutations`
5. `fix(xwayland): settle popup lifecycle and preserve focus`
6. `test(xwayland): cover popup crossings and stress`

Do not mix formatting-only changes, KMS/presentation changes, or unrelated
refactors into these commits.

## Risks and Mitigations

| Risk | Impact | Mitigation |
| --- | --- | --- |
| QueryTree state is confused with startup adoption | High | Separate fields and tests for both paths |
| A stale snapshot exposes an intermediate popup | Critical | Require reply epoch == current dirty epoch and reject incomplete replies |
| Batch abort loses repaint or damage | High | Keep existing journals immediate; defer only repaint scheduling |
| Pointer helpers emit intermediate frames | Critical | Use one queue-then-send crossing primitive |
| Commit-generated commands execute next cycle | High | Commit before backend collection and test same-cycle execution |
| Nested or dropped batch leaves state stuck | High | Exact token, explicit commit, no-side-effect cleanup tests |
| Auxiliary popup steals focus | High | Focus tests around role policy and X11 grab/ungrab events |
| Trace budget is exhausted again | Medium | Remove flush records and retain bounded lifecycle category coverage |
| Real fixture is unavailable | Low | Keep deterministic coverage primary and report skips honestly |
