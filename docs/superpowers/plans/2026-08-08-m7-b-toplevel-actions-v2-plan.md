# M7-B — Astrea Toplevel Actions v2 Implementation Plan

> **For agentic workers:** Execute this plan inline in the current session. Do not dispatch sub-agents, create branches, create worktrees, detach HEAD, reset, clean, amend, squash, or rewrite history. Track each step with the checkbox syntax below.

**Goal:** Add manager-owned, exact-WindowId Astrea toplevel actions v2 to Typhon while preserving v1 read-only compatibility and all frozen M7-A behavior.

**Architecture:** Extend the existing astrea_toplevel_manager_v1 protocol and publisher. Each authorized v2 request validates manager/handle/client identity, checks manager-scoped token availability, resolves the exact WindowId, reserves a token, invokes the existing central exact-window primitive synchronously, emits action_done on the manager, and releases the token. The 64-entry tracker is a direct manager action-state primitive; M7-B does not create an artificial asynchronous action queue.

**Tech Stack:** Rust 2024, wayland-server 0.31, wayland-scanner 0.31, generated private Wayland protocol bindings, existing Typhon compositor state and XWayland/XDG action primitives, Rust unit/integration tests, Cargo locked validation.

## Global Constraints

- Work only in /home/agony/GitHub/Typhon on the existing main branch.
- Preserve all unrelated uncommitted user changes; do not reset, clean, amend, squash, or rewrite history.
- Do not modify Eclipse until M7-B is committed and deterministic qualification passes.
- The existing protocol v1 remains semantically and wire compatible and read-only.
- Version 2 adds only exact activate, minimize, restore, and close requests plus manager-owned action_done.
- Token uniqueness and the maximum of 64 action-state entries are manager-scoped.
- Every two-uint token pair is syntactically valid; there is no special zero-token rejection.
- Use this validation order: manager/resource version and authorization → token duplicate/bound check → exact handle-to-WindowId resolution → token reservation → exact action primitive → manager action_done → token release.
- Authorized semantic rejection for an unavailable target, duplicate token, or full manager action-state bound emits action_done(..., unavailable) and leaves no new pending entry.
- Unauthorized clients and requests unavailable at the bound protocol version use the existing protocol/authentication rejection path, emit no action_done, and mutate no token state.
- M7-B actions complete synchronously from Typhon's perspective; do not add sleeps, artificial deferred production actions, unnecessary queues, or a test-only asynchronous state machine.
- close accepted acknowledges issuance of the existing graceful XDG or managed-X11 close request only; it does not wait for closed.
- Reuse existing central exact-window primitives, family-aware stacking, focus policy, minimize/restore policy, and XWM synchronization.
- Modify M7-A code only for a concrete deterministic regression, with a focused regression test and separate correction commit.
- Native Firefox/Kitty TTY/DRM qualification is DEFERRED; do not report it as passed.

---

## File map

- Modify: protocols/astrea-toplevel-management-v1.xml — v2 enums, requests, and manager completion event; preserve v1.
- Modify: src/compositor/protocols/versions.rs — advertise the existing global at version 2.
- Modify: src/compositor/protocols/toplevel_management.rs — dispatch manager destruction, handle destruction, and v2 requests.
- Modify: src/compositor/toplevel_publication.rs — manager-scoped action state, exact resource resolution, result emission, and cleanup.
- Modify only if focused tests prove a defect: src/compositor/state/surface_focus.rs and src/compositor/state/windows.rs — central exact outcomes.
- Test: src/compositor/state/desktop_window_tests.rs — XDG and managed-X11 exact action outcomes.
- Test: src/compositor/toplevel_publication_tests.rs — direct manager action-state primitive.
- Test: src/compositor/tests/toplevel_management.rs — v1/v2 behavior, completions, and lifecycle ordering.
- Test: src/compositor/tests/protocol_contract.rs — advertised v2 contract.
- Modify: docs/astrea-toplevel-protocol.md — precise v2 behavior.
- Modify: docs/M7_QUALIFICATION_STATUS.md — actual M7-B evidence.

Existing committed design:

~~~text
docs/superpowers/specs/2026-08-08-m7-b-toplevel-actions-v2-design.md
~~~

---

### Task 1: Capture the implementation baseline and classify the dirty Typhon tree

**Files:**

- Read only: the Typhon working tree, current main history, and the file map above.

**Interfaces:**

- Consumes: the committed M7-B design.
- Produces: a recorded starting HEAD and a file-by-file classification of existing user changes into M7-A, M7-B, documentation, or unrelated work.

- [ ] Step 1: Record the exact starting state.

Run:

~~~bash
git rev-parse HEAD
git status --short
git diff --name-status
git log -8 --oneline --decorate
~~~

Expected: record the current HEAD and dirty paths; do not discard any file.

- [ ] Step 2: Inspect only the action-related dirty diff.

Run:

~~~bash
git diff -- protocols/astrea-toplevel-management-v1.xml \
  src/compositor/protocols/versions.rs \
  src/compositor/state/surface_focus.rs \
  src/compositor/state/windows.rs \
  src/compositor/toplevel_publication.rs \
  src/compositor/toplevel_publication_tests.rs \
  src/compositor/state/desktop_window_tests.rs
~~~

Expected: reuse existing M7-B scaffolding that matches the approved design; leave unrelated edits unstaged.

- [ ] Step 3: Confirm Eclipse is untouched.

Run:

~~~bash
git -C /home/agony/GitHub/Eclipse status --short
git -C /home/agony/GitHub/Eclipse rev-parse --abbrev-ref HEAD
~~~

Expected: Eclipse remains on its existing main and is not changed by M7-B.

- [ ] Step 4: Keep baseline evidence in the final M7-B report and ledger.

Record the starting Typhon HEAD and dirty-path classification. Never stage unrelated user changes.

---

### Task 2: Lock the v2 protocol surface and generated dispatch contract

**Files:**

- Modify: protocols/astrea-toplevel-management-v1.xml
- Modify: src/compositor/protocols/versions.rs
- Modify: src/compositor/protocols/toplevel_management.rs
- Test: src/compositor/tests/protocol_contract.rs

**Interfaces:**

- Consumes: generated server bindings from src/astrea_toplevel_management.rs.
- Produces: generated handle requests Activate, Minimize, Restore, Close and manager event ActionDone, with stable Action and ActionResult values.

- [ ] Step 1: Add/verify contract assertions.

Extend src/compositor/tests/protocol_contract.rs with assertions equivalent to:

~~~rust
assert_eq!(
    versions::all_globals()
        .into_iter()
        .find(|global| global.interface == "astrea_toplevel_manager_v1")
        .map(|global| global.version),
    Some(2)
);
~~~

Assert stable values:

~~~text
activate=0, minimize=1, restore=2, close=3
accepted=0, no_change=1, unavailable=2
~~~

- [ ] Step 2: Run the focused contract test before implementation.

Run:

~~~bash
cargo test --locked compositor::tests::protocol_contract -- --test-threads=1
~~~

Expected: capture any failure caused by incomplete generated dispatch or contract assertions.

- [ ] Step 3: Update only the protocol source and advertised version.

Keep interface names, v1 requests/events, enum values, argument order, object lifecycle, snapshot semantics, and revision semantics unchanged. Ensure action_done is manager-owned and since=2, and each handle action is since=2 with two uint token arguments. Describe semantic unavailable, synchronous completion, and close issuance precisely.

Set ASTREA_TOPLEVEL_MANAGER_V1 to 2 in src/compositor/protocols/versions.rs.

- [ ] Step 4: Make request matching exhaustive.

Update src/compositor/protocols/toplevel_management.rs so generated v2 requests route to the action dispatch boundary from Task 5. Do not treat a v2 request as destruction or a focused-window helper.

- [ ] Step 5: Rerun the contract test.

Run:

~~~bash
cargo test --locked compositor::tests::protocol_contract -- --test-threads=1
~~~

Expected: PASS for advertised version and stable protocol values.

- [ ] Step 6: Commit an independently correct protocol boundary.

~~~bash
git add protocols/astrea-toplevel-management-v1.xml \
  src/compositor/protocols/versions.rs \
  src/compositor/protocols/toplevel_management.rs \
  src/compositor/tests/protocol_contract.rs
git diff --cached --check
git commit -m "feat(protocol): add Astrea toplevel actions v2"
~~~

Stage no unrelated dirty path.

---

### Task 3: Make manager action state bounded, synchronous, and directly testable

**Files:**

- Modify: src/compositor/toplevel_publication.rs
- Test: src/compositor/toplevel_publication_tests.rs

**Interfaces:**

- Consumes: WindowId and AstreaToplevelAction.
- Produces:
  AstreaActionToken::new(high: u32, low: u32) -> AstreaActionToken,
  AstreaActionTracker::can_reserve(token),
  AstreaActionTracker::reserve(token, action, window_id),
  AstreaActionTracker::release(token),
  AstreaActionTracker::clear(), and pending_len().

- [ ] Step 1: Add direct failing tests for manager scope and reuse.

Use a test equivalent to:

~~~rust
let token = AstreaActionToken::new(7, 11);
tracker.reserve(token, AstreaToplevelAction::Activate, id(1)).unwrap();
assert_eq!(tracker.can_reserve(token), Err(AstreaActionBeginError::Duplicate));
assert_eq!(
    tracker.reserve(token, AstreaToplevelAction::Close, id(2)),
    Err(AstreaActionBeginError::Duplicate)
);
assert_eq!(tracker.release(token).unwrap().window_id, id(1));
tracker.reserve(token, AstreaToplevelAction::Close, id(2)).unwrap();
~~~

The same token on another WindowId is rejected while reserved and accepted after release.

- [ ] Step 2: Add the direct capacity test.

Reserve exactly MAX_ASTREA_PENDING_ACTIONS distinct tokens, assert the next can_reserve and reserve return Limit, release one token, and assert the capacity is reusable. Do not create delayed compositor actions or sleep.

- [ ] Step 3: Run the direct tracker test to verify the new API is missing.

Run:

~~~bash
cargo test --locked astrea_action_tracker -- --test-threads=1
~~~

Expected: fail until the tracker API and manager field are implemented.

- [ ] Step 4: Implement the minimal tracker API.

Keep the tracker as a BTreeMap<AstreaActionToken, PendingAstreaAction> owned by one AstreaToplevelManagerBinding. Use can_reserve for the pre-resolution duplicate/capacity check, reserve only after exact handle-to-WindowId resolution, and release immediately after manager completion. Do not retain completed-token history, target-based completion queues, or artificial deferred states.

- [ ] Step 5: Add the tracker to manager binding and cleanup.

Initialize one tracker during manager admission and clear it during manager destruction/disconnect. Never put the token table on a toplevel handle or the global publisher.

- [ ] Step 6: Rerun direct tracker tests.

Run:

~~~bash
cargo test --locked astrea_action_tracker -- --test-threads=1
~~~

Expected: PASS for manager scope, 64-entry capacity, release, reuse, and clear.

- [ ] Step 7: Commit the independently tested action-state primitive.

~~~bash
git add src/compositor/toplevel_publication.rs \
  src/compositor/toplevel_publication_tests.rs
git diff --cached --check
git commit -m "feat(protocol): bound manager action state"
~~~

If a file also contains pre-existing M7-A publication edits, inspect the staged diff carefully and stage only complete in-scope changes.

---

### Task 4: Qualify and, only if needed, complete central exact-window primitives

**Files:**

- Test: src/compositor/state/desktop_window_tests.rs
- Modify only for a focused defect: src/compositor/state/surface_focus.rs
- Modify only for a focused defect: src/compositor/state/windows.rs

**Interfaces:**

- Consumes: existing exact methods activate_desktop_window, minimize_desktop_window_outcome, restore_minimized_desktop_window_outcome, and close_desktop_window_outcome.
- Produces: stable WindowActionOutcome mappings for protocol dispatch without protocol-specific policy.

- [ ] Step 1: Add deterministic exact-action outcome tests.

Cover:

~~~text
missing WindowId -> Unavailable for all four operations
active and family-topmost -> Activate NoChange
active but not topmost -> Activate Changed and one family-aware raise
minimized -> Activate Changed with restore/focus/raise
already minimized -> Minimize NoChange
not minimized -> Restore NoChange
XDG close -> Changed when xdg_toplevel.close is issuable
managed X11 close -> Changed and one existing XWM close command
unsupported or auxiliary target -> Unavailable
~~~

Assert exact activation does not create duplicate focus serial churn or duplicate restacks when the target is already focused and topmost.

- [ ] Step 2: Run focused state tests.

Run:

~~~bash
cargo test --locked desktop_window_tests -- --test-threads=1
~~~

Expected: identify only a missing outcome mapping or a concrete regression; do not refactor M7-A behavior.

- [ ] Step 3: Make the smallest central fix only if a test proves it is needed.

Keep policy in the existing central methods. Do not add protocol-only focus, stacking, minimize, restore, or close helpers. If this exposes a real M7-A regression, add the narrow regression test and create a separate correction commit.

- [ ] Step 4: Rerun focused state and affected M7-A tests.

~~~bash
cargo test --locked desktop_window_tests -- --test-threads=1
cargo test --locked compositor::tests::xwayland_focus -- --test-threads=1
~~~

Expected: PASS with the frozen M7-A invariants unchanged.

---

### Task 5: Implement manager-owned exact action dispatch

**Files:**

- Modify: src/compositor/protocols/toplevel_management.rs
- Modify: src/compositor/toplevel_publication.rs
- Test: src/compositor/tests/toplevel_management.rs

**Interfaces:**

- Consumes: AstreaToplevelResourceData { manager_id, window_id, client_id }, manager-scoped AstreaActionTracker, and central exact primitives.
- Produces:

~~~text
authorized v2 + duplicate/full/unavailable
    -> manager action_done(unavailable)
    -> no new reserved token

authorized v2 + exact handle + primitive result
    -> reserve token
    -> execute exact WindowId primitive
    -> manager action_done(accepted|no_change|unavailable)
    -> release token

unauthorized or unsupported version
    -> existing protocol/authentication rejection
    -> no action_done
    -> no token mutation
~~~

- [ ] Step 1: Add failing manager-completion tests.

Extend the client test state in src/compositor/tests/toplevel_management.rs to record token, action, and result from manager ActionDone. Bind an authorized v2 manager, obtain an exact handle, issue one action, roundtrip deterministically, and assert one matching manager completion with no handle callback.

- [ ] Step 2: Add semantic rejection tests.

Cover:

~~~text
live manager + unavailable WindowId -> action_done(unavailable)
manager action-state duplicate -> action_done(unavailable)
manager action-state full -> action_done(unavailable)
~~~

Assert pending_len() == 0 after each synchronous rejection and that a later valid token is accepted.

- [ ] Step 3: Add protocol/authentication rejection tests.

Cover an unauthorized bind/request and a request sent through a v1-bound object. Assert protocol/authentication error, no action_done, and unchanged token count. Use existing Astrea auth support; do not replace the capability gate with UID, app-id, PID, title, or environment checks.

- [ ] Step 4: Implement exact resource preparation.

Add a publisher boundary that receives exact client ID, manager resource ID, handle resource ID, token, and action. It must confirm manager and handle ownership, confirm the handle is live, check duplicate/capacity without mutating the tracker, resolve the immutable WindowId and actionable target, then reserve only after exact resolution.

Resource mismatch or unsupported request version uses the existing protocol error path. A valid authorized request whose target is unavailable, token duplicate, or tracker full emits unavailable and does not reserve a new entry.

- [ ] Step 5: Map actions only to central exact methods.

~~~rust
Activate -> activate_desktop_window(window_id, WindowFocusReason::ShellActivation)
Minimize -> minimize_desktop_window_outcome(window_id)
Restore  -> restore_minimized_desktop_window_outcome(window_id)
Close    -> close_desktop_window_outcome(window_id)
~~~

Map Changed to accepted, NoChange to no_change, and Unavailable to unavailable. Never call a focused-window helper or create a second XWM/focus/stacking path.

- [ ] Step 6: Emit manager completion and release synchronously.

Send astrea_toplevel_manager_v1::Event::ActionDone from the manager resource with the original token/action/result, then release(token). If the exact primitive becomes unavailable during dispatch, emit one unavailable and release immediately.

For close, emit accepted as soon as the existing graceful request is successfully issued. Do not wait for closed, inspect client disappearance, or dereference the handle while emitting the manager event.

- [ ] Step 7: Clear manager action state on teardown.

Clear the manager tracker in the same publisher cleanup paths that remove a manager. Ensure stale resource/client IDs cannot release or complete a token in a new manager generation.

- [ ] Step 8: Run focused protocol/action tests.

~~~bash
cargo test --locked toplevel_management -- --test-threads=1
cargo test --locked astrea_action_tracker -- --test-threads=1
~~~

Expected: PASS for exact routing, manager completion, semantic unavailable, protocol/auth rejection, and synchronous release.

- [ ] Step 9: Commit the production action boundary.

~~~bash
git add src/compositor/protocols/toplevel_management.rs \
  src/compositor/toplevel_publication.rs \
  src/compositor/tests/toplevel_management.rs
git diff --cached --check
git commit -m "feat(protocol): dispatch exact Astrea window actions"
~~~

---

### Task 6: Qualify v1 compatibility and close/lifecycle ordering

**Files:**

- Modify: src/compositor/tests/toplevel_management.rs
- Modify: src/compositor/tests/protocol_contract.rs

**Interfaces:**

- Consumes: committed v2 server action dispatch and existing publication lifecycle.
- Produces: deterministic proof that v1 remains read-only and close completion is independent of handle lifecycle.

- [ ] Step 1: Add v1 read-only assertions.

Bind a manager at version 1 and assert existing publication events, ordering, enum values, and manager done behavior. Ensure no v2 request is available through the v1-bound handle.

- [ ] Step 2: Add close ordering tests.

Drive both observer orderings without sleeps:

~~~text
graceful close issued -> manager action_done(close, accepted) -> handle closed
graceful close issued -> handle closed -> manager action_done(close, accepted)
~~~

Both must produce one manager completion, never a handle-owned completion, and never wait for client disappearance.

- [ ] Step 3: Add exact identity and stale-resource tests.

Issue actions through two handles owned by one manager and assert each affects only its own WindowId. Destroy one handle, then exercise a stale object/resource path and assert no action reaches a new handle with a reused protocol object slot.

- [ ] Step 4: Run focused compatibility tests.

~~~bash
cargo test --locked toplevel_management -- --test-threads=1
cargo test --locked protocol_contract -- --test-threads=1
~~~

Expected: PASS with v1 unchanged and close/lifecycle ordering safe.

- [ ] Step 5: Commit compatibility qualification.

~~~bash
git add src/compositor/tests/toplevel_management.rs \
  src/compositor/tests/protocol_contract.rs
git diff --cached --check
git commit -m "test(protocol): qualify exact Astrea toplevel actions"
~~~

---

### Task 7: Update protocol documentation and the M7 ledger

**Files:**

- Modify: docs/astrea-toplevel-protocol.md
- Modify: docs/M7_QUALIFICATION_STATUS.md

**Interfaces:**

- Consumes: actual implementation commits, test output, starting/final HEADs, and the committed protocol XML.
- Produces: English documentation that never overstates native qualification.

- [ ] Step 1: Document semantic versus protocol rejection.

Add:

~~~text
authorized valid-v2 + unavailable target/duplicate/full bound
    -> manager action_done(..., unavailable)
    -> no new pending entry

unauthorized or unsupported bound version
    -> existing protocol/authentication rejection
    -> no action_done
    -> no token mutation
~~~

Document synchronous reserve → exact primitive → manager completion → release, manager-scoped 64-entry state, token reuse, and close issuance semantics.

- [ ] Step 2: Compute the committed XML hash.

~~~bash
sha256sum protocols/astrea-toplevel-management-v1.xml
git rev-parse HEAD
~~~

Record source path, hash, and final Typhon HEAD in the ledger.

- [ ] Step 3: Update only the M7-B ledger row.

Prepare the M7-B evidence fields but leave the row as NOT RUN until Task 8 has passed. Keep M7-B Native exactly DEFERRED; do not change M7-C or M7-D.

- [ ] Step 4: Commit documentation.

~~~bash
git add docs/astrea-toplevel-protocol.md docs/M7_QUALIFICATION_STATUS.md
git diff --cached --check
git commit -m "docs(protocol): document Astrea toplevel actions v2"
~~~

---

### Task 8: Run full deterministic qualification and finalize M7-B evidence

**Files:**

- Read/modify only as required for actual evidence: docs/M7_QUALIFICATION_STATUS.md
- Read only: all source and tests changed by Tasks 1–7.

**Interfaces:**

- Consumes: focused M7-B tests and committed source/documentation.
- Produces: reproducible deterministic evidence and a committed-XML handoff to Eclipse.

- [ ] Step 1: Run formatting.

~~~bash
cargo fmt --check
~~~

Expected: PASS. If formatting is required, run cargo fmt, inspect the diff, and commit only the focused formatting correction.

- [ ] Step 2: Run locked compilation.

~~~bash
cargo check --locked --all-targets
~~~

Expected: PASS with generated v2 bindings and all test targets compiling.

- [ ] Step 3: Run locked Clippy.

~~~bash
cargo clippy --locked --all-targets -- -D warnings
~~~

Expected: PASS with no warnings promoted to errors.

- [ ] Step 4: Run the serial full Rust suite.

~~~bash
cargo test --locked -- --test-threads=1
~~~

Expected: all relevant M7-B tests pass. Any unchanged host-environment failure is recorded exactly and not relabeled as a product pass.

- [ ] Step 5: Run source-layout and diff checks.

~~~bash
./bin/check-source-layout
git diff --check
git status --short
~~~

Expected: source layout and diff checks pass; remaining dirty paths are unrelated pre-existing user changes or explicitly recorded evidence changes.

- [ ] Step 6: Verify commit and protocol evidence.

~~~bash
git log --oneline --decorate -12
sha256sum protocols/astrea-toplevel-management-v1.xml
git rev-parse HEAD
~~~

Expected: focused M7-B commits are present, the XML hash is recorded, and no previous milestone was amended or rewritten.

- [ ] Step 7: Mark deterministic M7-B status.

Set the ledger row to:

~~~text
Implementation: PASS
Deterministic: PASS
Native: DEFERRED
~~~

Do not modify Eclipse or claim integrated M7 completion. The handoff is the committed XML path/hash and deterministic Typhon evidence.

---

## Final M7-B handoff

Report:

~~~text
M7-A
    implementation: PASS
    deterministic: PASS
    native: DEFERRED
    starting/current Typhon HEAD: record the actual values captured in Task 1 and Task 8

M7-B
    implementation: PASS
    deterministic: PASS
    native: DEFERRED
    commits: record the actual focused commit IDs from Task 8
    protocol version: 2
    XML path: protocols/astrea-toplevel-management-v1.xml
    XML SHA-256: record the actual sha256sum output from Task 7
    test results: record each validation command and its actual result from Task 8
~~~

Do not begin M7-C until the M7-B deterministic row is PASS and the v2 XML has been copied byte-for-byte into Eclipse as the next separately planned milestone.
