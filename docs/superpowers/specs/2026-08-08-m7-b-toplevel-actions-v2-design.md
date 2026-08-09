# M7-B — Astrea Toplevel Actions v2 Design

## Status and scope

M7-B extends Typhon's existing `astrea_toplevel_manager_v1` protocol from
version 1 to version 2. The protocol remains one read-only publication
protocol for v1 clients and gains exact window actions for authorized v2
clients. This design covers Typhon only; Eclipse remains unchanged until
M7-B is committed and its deterministic qualification passes.

Native Firefox/Kitty TTY/DRM qualification remains deferred. M7-B is ready
to advance only after deterministic compilation, tests, stress, static,
protocol, sanitizer-where-applicable, source-layout, and diff checks pass.

The current working tree contains uncommitted M7-A-related changes. Those
changes are user-owned. Implementation will classify them before editing,
preserve unrelated edits, and make any required cross-milestone correction in
a separate focused commit rather than redesigning M7-A.

## Goals and non-goals

M7-B will provide:

- `activate`, `minimize`, `restore`, and graceful `close` requests on the
  existing toplevel handle, each with `since="2"`;
- manager-owned `action_done(token_hi, token_lo, action, result)` completion;
- exact Typhon `WindowId` targeting with no secondary identity arguments;
- authorization through the existing Astrea session-capability mechanism;
- bounded, manager-scoped pending action state;
- deterministic behavior across target destruction, manager destruction,
  disconnect, duplicate tokens, and stale generations;
- v1 semantic and wire compatibility, with v1 remaining read-only.

M7-B does not add maximize, fullscreen, workspace, output, icon, thumbnail,
PID, XID, title, app-id, or helper-process action paths. It does not perform
real native qualification and does not change Eclipse.

## Protocol contract

The existing protocol XML remains the source of truth:

```text
protocols/astrea-toplevel-management-v1.xml
```

The global and handle interfaces advertise version 2. Version 1 retains its
existing requests, events, enum values, argument ordering, object lifecycle,
snapshot semantics, and manager revision semantics. The only v1 request on
the manager and handle remains destruction; mutation requests are introduced
with `since="2"`.

The stable action values are:

```text
activate = 0
minimize = 1
restore = 2
close = 3
```

The stable results are:

```text
accepted   = the requested state-changing operation was issued successfully
no_change  = the exact target already satisfies the requested state
unavailable = the request cannot be acted on by Typhon
```

`unavailable` is terminal for that request. It covers an invalid or
non-actionable target, a duplicate token that is already pending on this
manager, and rejection because this manager has reached its pending-action
bound. These cases do not create pending state and do not replace or cancel
an existing pending action. No other result values are introduced.

The wire representation is two `uint` values, so every 64-bit token pair is
syntactically valid; there is no special zero-token rejection. Token
uniqueness is the only token-specific semantic check.

For `close`, `accepted` acknowledges only that Typhon issued the existing
graceful close request: `xdg_toplevel.close` for XDG windows or the existing
XWM close request for managed X11. It does not acknowledge client exit and
does not wait for the existing `closed` lifecycle event. Client disappearance
continues to be represented by `closed`.

## Server ownership and action flow

The manager binding owns the pending-action tracker. The token namespace and
the capacity bound are manager-scoped: one `(token_hi, token_lo)` may not be
pending on two handles owned by the same manager, while the same token may be
reused after completion or independently by another manager.

The tracker is a bounded manager action-state primitive with a hard maximum of
64 reserved entries. Normal M7-B actions complete synchronously from the
compositor's perspective: they reserve a token, execute the exact primitive,
emit the manager result, and release the token. There is no artificial
asynchronous state machine, delayed completion, or test-only queue. A
longer-lived entry may be retained only if an existing production action
genuinely requires deferred completion. Manager destruction or client
disconnect clears any such real entries. Completed tokens are immediately
reusable and no completed-token history is retained.

Every v2 request follows this order:

```text
manager/resource version and authorization
    → token validity, duplicate check, and pending-bound check
    → exact handle-to-WindowId resolution
    → pending admission
    → exact action primitive
    → manager action_done
```

The first stage rejects an unauthorized client or an object that cannot use
the v2 request surface through the existing authentication/protocol error
path. It must not mutate token state and emits no `action_done`. The second
stage checks the manager-local duplicate and capacity conditions without
mutating the tracker. A duplicate or full tracker is a semantic action
rejection: the manager emits `action_done(..., unavailable)` and leaves no
new pending entry. The third stage verifies that the exact handle is live,
belongs to the request's manager and client, and still maps to its immutable
`WindowId`; an unavailable target is also a semantic action rejection that
emits `unavailable` without reserving a token. Only then is the token
reserved.

After reservation, the request invokes one central exact-window primitive and
immediately emits the manager-owned result, then releases the token. A
primitive may complete with `no_change`, `unavailable`, or `accepted`.
For `close`, `accepted` means the graceful close request was issued; it
does not wait for `closed`. The manager completion does not dereference the
handle, so a handle becoming terminal before or after the manager event is
safe. If a future existing production primitive genuinely becomes deferred,
its real completion may retain the reserved entry until the same manager
completion path releases it; M7-B does not manufacture such a path.

The server must not focus a window temporarily and then invoke a generic
focused-window helper. All actions resolve and operate on the exact
`WindowId`.

## Exact action primitives

Protocol dispatch will reuse or complete the existing central exact-window
primitives rather than introduce protocol-specific policy:

- activation restores an exact minimized target when needed, focuses it,
  raises it through the existing family-aware stacking policy, and performs
  existing X11 synchronization where applicable;
- minimize and restore operate on the exact target and distinguish `no_change`
  from `unavailable`;
- close issues only the existing graceful XDG or managed-X11 close request;
- all target-kind checks, normal-role checks, focus serial behavior, family
  stacking, XWM synchronization, and publication marking remain centralized.

The primitives must preserve the frozen M7-A invariants: hover focus never
raises, click activation uses one original hit-test and the exact captured
target, a managed `WindowId` does not cause focus-serial churn when already
focused, captured move/resize targets remain authoritative, generic focus loss
does not terminate interaction, terminal cleanup performs one normal pointer
refresh, resize preview extents remain conservative, normal application
borders remain compositor-invisible, and managed X11 effective border width
remains zero.

If deterministic M7-B tests expose a concrete M7-A regression, the regression
will be reproduced with a focused test, fixed narrowly, committed separately,
and followed by all affected M7-A tests. Opportunistic refactoring is out of
scope.

## Authorization and lifecycle safety

The existing Astrea shell session-capability authentication remains the only
mutation authorization mechanism. UID is checked as part of that mechanism
but is not sufficient by itself. PID ancestry, process name, app ID, title,
environment strings, and claimed metadata are not substitutes for the
capability.

Manager and handle resource data will continue to carry exact client,
manager-resource, handle-resource, and `WindowId` identity. Destroyed or
stale resources cannot act on a new handle that happens to reuse a protocol
object slot. Manager teardown releases action state before the manager is no
longer reachable, and client disconnect cleanup is idempotent.

The action completion path is manager-owned because a close request can make
the toplevel handle terminal. No completion callback or state is stored only
on the handle. Manager failure or disconnect cannot deliver a completion to a
new manager generation.

## Deterministic tests

Tests will be added at the narrowest existing Typhon test layers and then
covered by the full suite.

### Contract and authorization

- v1 clients bind and receive the existing read-only publication semantics;
- v1 clients cannot issue mutation requests;
- authorized v2 clients can request all four actions;
- unauthorized clients cannot use the private mutation surface and cannot
  mutate token state;
- invalid object versions are rejected through the protocol path before token
  state changes and without `action_done`;
- authorized v2 semantic rejection emits `unavailable` for an unavailable
  target, duplicate token, or full manager action-state bound, without leaving
  pending state behind.

### Exact action behavior

For both XDG and managed X11 targets:

- inactive activation produces `accepted` and converges to restore/focus/
  raise as applicable;
- already active/topmost activation produces `no_change` without duplicate
  focus serials or restacks;
- minimized activation restores, focuses, and raises the exact target;
- minimize and restore affect only the exact requested `WindowId`;
- close issues the graceful request and reports `accepted` without waiting for
  `closed`.

### Completion and race behavior

- close followed by `action_done` then `closed`;
- close followed by `closed` then `action_done`;
- target destruction while an action is pending;
- manager destruction and client disconnect;
- direct manager action-state tests for a duplicate pending token on a
  different handle of the same manager;
- reuse of a completed token;
- direct manager action-state tests for the 64-entry pending bound;
- stale manager-generation completion rejection;
- XWayland generation restart;
- no duplicate completion for any token/action pair.

Tests will use deterministic event-driving and explicit synchronization. They
will not rely on arbitrary sleeps, artificial delayed production actions,
unnecessary completion queues, or native Firefox/Kitty sessions.

## Validation and qualification ledger

Before the M7-B commit, run focused protocol/action tests independently, then:

```text
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -- --test-threads=1
./bin/check-source-layout
git diff --check
```

Any unavailable toolchain or validator will be reported and installed only
with the user's approval. A failed or unavailable host-native test remains
explicitly recorded; it is not converted into a pass.

Update `docs/M7_QUALIFICATION_STATUS.md` with:

```text
M7-B
    Implementation: PASS only after the implementation commit exists
    Deterministic: PASS only after every required gate passes
    Native: DEFERRED
```

Record the actual starting and final Typhon HEADs, the committed protocol XML
path, and its SHA-256. Do not claim M7 complete or native qualification.

## Commit boundary

The M7-B work remains easy to bisect. The intended focused boundaries are:

```text
feat(protocol): add Astrea toplevel actions v2
test(protocol): qualify exact toplevel actions
docs(protocol): document toplevel actions v2
```

The exact commit count may follow the existing working-tree classification,
but no previous milestone is amended, squashed, reset, or rewritten.
