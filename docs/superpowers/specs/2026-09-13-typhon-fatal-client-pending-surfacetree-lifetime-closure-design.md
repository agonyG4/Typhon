# Typhon Fatal-Client and Pending SurfaceTree Lifetime Closure

## Goal

Make every fatal Wayland protocol error poison its owning client before the wire
error is posted, make every queued SurfaceTree transaction carry independent
publication-lifetime proof for every node, discard terminal-owner frame
callbacks without sending `wl_callback.done`, and add the missing production
wiring regression for pointer-constraint replacement.

## Design

### Fatal protocol errors

`src/compositor/state/client_lifecycle.rs` will own a low-level fatal-emission
primitive that receives the compliance metrics, protocol trace, terminal-client
set, a `Resource`, error metadata, and the fatal message. It resolves the
resource's owning `ClientId`, inserts that ID into `terminal_client_ids`, records
the diagnostic exactly once, and only then calls `Resource::post_error`. If a
resource no longer has an owning client, the primitive records an unavailable
diagnostic and does not emit a wire error; this preserves the fail-closed
ordering invariant.

`CompositorState` wrappers will provide the existing immediate and deferred
cleanup semantics, including a detailed deferred form for protocol handlers
that currently record a non-wire category before posting. Explicit-sync,
dmabuf, and wl_drm validation will use the same primitive through adapters that
already receive split metric/trace state. Existing callers will not double-count
metrics.

All direct compositor `.post_error(` calls will be removed except the one
central implementation point. A source-contract test will recursively scan
`src/compositor` and allow only `client_lifecycle.rs`, so future protocol code
cannot reintroduce an unpoisoned fatal emitter.

### SurfaceTree publication lifetime

Add `SurfaceTreeNodeLifetime` containing `surface_id`, `owner_client_id`, and
`surface_presentation_generation`. `PendingSurfaceTreeTransaction` will carry a
bounded vector of these records aligned with its node vector. Production
admission captures one record for every node through the existing
`capture_surface_publication_lifetime`; failure to capture any record releases
the unpublished tree and does not queue it. Synthetic unit fixtures may use an
explicit test-only lifetime marker where no live Wayland surface exists.

Acquire dependencies retain their owner and generation fields. They validate
late acquire completion; node lifetimes validate eventual visual publication.
`surface_tree_async_publication_rejection` will validate every captured node,
including trees with no acquire dependencies: terminal owner, current surface
ownership, live resource, and unchanged presentation generation. Normal commit
sequence and publication-order checks remain after this lifetime gate. Mixed
trees therefore validate nodes without acquire dependencies even while another
node is waiting on a fence.

All transaction creation, merge, queue, and release paths will preserve the
aligned lifetime vector. Discard continues through the existing ownership
pipeline: watcher cancellation when applicable, pending buffer release,
presentation-feedback discard, resize-capture release, and callback settlement.

### Terminal callback disposal

Add an explicit callback-discard operation that removes callback bookkeeping,
removes callbacks from pending visible/unvisible queues and uncompleted frame
batches, and marks callback settlement ownership cancelled exactly once. It will
not send a protocol event. SurfaceTree and direct explicit-sync rejection paths
will select this operation when the rejection decision is `TerminalClient`;
live-client and unrelated rejection semantics remain unchanged.

### Pointer replacement regression

Extend the existing real Wayland pointer-constraint transaction test suite with
the production sequence `A commit/activate -> destroy A -> create B -> one
surface commit`. The test will observe retirement and installation backend
requests, settle both sides, verify B owns routing while A is retired, and feed
a stale A generation completion to prove it cannot change B. A second sequence
will destroy B before the commit and verify the transaction retires A and
installs C, with no effective B state.

## Testing strategy

Use test-first regressions for the no-dependency commit-timing queue, no-
dependency FIFO queue, mixed dependency tree, fatal-before-teardown explicit
sync and SurfaceTree paths, terminal callback disposal, and pointer replacement.
Retain existing acquire identity tests and add lifetime assertions beside them.
Run focused compositor tests first, then formatting, locked check, locked
clippy with warnings denied, the locked full test suite, and any repository
source-layout check. Compile artifacts in the repository's existing `target`
directory as required by the workspace instructions.

## Non-goals

Do not change KMS commit processing, atomic output, renderer, buffer age,
adaptive buffering, Direct Scanout, VRR, NVIDIA paths, Proton configuration,
scene-wide repaint policy, explicit sync/FIFO/commit-timing enablement, or
pointer-constraint behavior beyond the requested lifetime regression.

Native-hardware Wayland game-session behavior remains an explicit follow-up
validation boundary; repository tests cannot prove that affected session.
