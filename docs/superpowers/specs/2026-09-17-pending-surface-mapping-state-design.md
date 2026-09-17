# F09 Pending Surface Mapping State Design

## Goal

Prepare each SurfaceTree transaction against the exact mapping state produced by the ordered pending transactions that are guaranteed to publish before it, while preserving separate transaction boundaries and the existing cumulative candidate preflight.

## Architecture

At SurfaceTree admission, derive an immutable projection of the effective mapping state. The projection starts with the live committed `SurfaceData` and retained `CurrentSurfaceBuffer`, then replays metadata from the pending transaction prefix selected by scheduler ordering: earlier transactions for the incoming root and transactions that cover the incoming candidate's external Content Update dependencies, including their transitive ordering prerequisites. Projection uses `EffectiveSurfaceMappingState::apply_commit` only; it does not run ownership, explicit-sync, callbacks, publication, or error side effects.

The projected per-surface state is passed into the existing cumulative validation/preparation path. Canonicalization and merged-transaction re-preparation remain unchanged. The publication scheduler and root-head FIFO rules remain unchanged, so pacing-protected, commit-timing, FIFO, merge-frozen, and ready-transaction boundaries remain exact.

## Ordering and lifecycle

The prefix is selected from scheduler guarantees rather than raw global vector position. Same-root transactions precede a new transaction for that root. A pending transaction covering a required external Content Update precedes the incoming transaction, and its own pending external dependencies and same-root predecessors are recursively included. Transactions outside this dependency closure are not replayed.

Pending work that affects a surface is retired by the existing surface/root lifecycle cancellation paths before that surface can publish new state. A focused regression will verify that discarding a pending predecessor cannot leave a later transaction publishing with predecessor-derived metadata. Explicit-sync remains owned by the transaction: projection reads only the already-admitted commit metadata and never signals, consumes, cancels, or materializes an acquire.

## Validation and ownership

The projected state handles retained content, new attachment, `RemoveContent`, scale, transform, source/destination changes, and resets. Mapping errors continue to return the historical `viewport_error_owner` carried by the effective source state, including when a later destination-only operation becomes invalid.

## Testing

Regression coverage will demonstrate separate pending destination, scale, and transform propagation; valid and invalid cross-transaction viewport compositions; historical viewport error ownership; cancellation/discard safety; and at least one real PacingProtected boundary. Existing within-candidate cumulative, canonicalization, merge re-preparation, coordinate, damage, and explicit-sync tests remain in place and are run in the focused and full verification suites.

