# Dead-owner async publication implementation plan

## Goal

Prevent terminal or stale owners from creating visual scene work while
settling all explicit-sync, buffer, release, callback, feedback, resize, and
SurfaceTree transaction obligations exactly once.

## Red tests first

1. Add a lifecycle eligibility unit test covering live, terminal-owner,
   owner-mismatch, missing-surface, and stale-presentation-generation results.
2. Add deterministic direct explicit-sync coverage for an unsignaled acquire,
   terminal owner before cleanup, readiness, and repeated readiness. Assert no
   visual generation/active renderable/publication/frame callback/feedback is
   created and ownership settles once.
3. Add deferred-fatal-protocol-error integration coverage so the terminal
   marker is observed before `teardown_disconnected_clients` drains the client.
4. Add SurfaceTree coverage for dead root, dead dependency node, client
   disconnect, stale generation, and a later transaction. Assert the dead
   transaction is discarded in queue order, later work is not reordered, and
   all node obligations are released once.

Run focused tests against the current code and confirm the dead-owner
publication assertions fail before production changes.

## Implementation

1. Add a state-owned `terminal_client_ids` set. Mark it before posting any
   fatal protocol error, including deferred errors; clear it at terminal client
   teardown. Ensure resource-exhaustion client kills use the same marker if
   they can precede normal cleanup.
2. Add a small `AsyncSurfacePublicationIdentity` containing surface ID, owner
   `ClientId`, and the existing presentation generation. Capture it in direct
   pending explicit-sync commits, acquire dependencies, and the queued
   SurfaceTree root.
3. Implement an O(1) lifecycle eligibility result that verifies terminal-owner
   state, current surface ownership/resource, captured presentation generation,
   and existing publication ordering. Keep immediate publication's existing
   missing-publication-state behavior unless an audit proves all immediate
   paths establish state.
4. Make acquire readiness idempotently advance synchronization state but record
   an explicit discarded outcome when the captured owner is invalid. Do not
   enqueue visual work from the readiness callback.
5. Apply the eligibility proof before direct explicit-sync publication. Route
   invalid work through existing release/discard helpers so watcher
   cancellation, buffer/release ownership, resize capture, callbacks, and
   feedback are settled exactly once.
6. Apply the same proof to SurfaceTree transactions and dependencies. A
   transaction with an invalid root or dependency is discarded as a unit in
   queue order; its resources are released before later ordered work is
   considered. Do not erase a transaction prematurely in a way that lets a
   later transaction leapfrog it.
7. Extend the existing surface-pipeline trace with concise discard reasons
   (`terminal_client`, `owner_gone`, `surface_gone`,
   `stale_surface_generation`) for acquire readiness and publication
   rejection. Keep normal tracing disabled/no-op and avoid per-frame scans or
   allocations.

## Verification

Run explicit-sync, SurfaceTree, surface/client teardown, frame/presentation,
and frame-buffer ownership tests. Then run:

```text
cargo fmt --check
cargo check --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Use the repository-local target directory and project-native `rtk` wrappers.
Inspect the final diff for changes outside lifecycle, pointer constraints,
tracing, and their tests.

## Commit

Commit the async changes separately after the pointer commit and before the
final verification pass.
