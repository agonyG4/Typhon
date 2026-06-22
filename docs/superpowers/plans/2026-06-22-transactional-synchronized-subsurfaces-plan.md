# Implementation Plan: Transactional Synchronized Subsurfaces

## Architecture Decisions

- Put role, synchronization, cached commit, and parent-latched state behind one
  subsurface transaction API.
- Capture protocol deltas before changing current state.
- Publish a collected synchronized subtree under one generation.
- Extend existing event-driven explicit-sync identities to whole-tree waits.

## Task List

1. Add failing integration tests proving default-synchronized child buffers and
   position remain invisible before parent commit.
2. Add pure role-state tests for requested/effective modes, ancestry, lifecycle,
   recursion, and transition behavior.
3. Introduce surface commit deltas and cached-commit merge/release semantics.
4. Route synchronized commits into the role cache and desynchronized commits to
   immediate publication.
5. Collect and atomically publish cached descendants, placement, and stacking
   on parent commit using one generation.
6. Integrate root resize snapshots and preview completion with tree publication.
7. Extend acquire waiting, supersession, callbacks, releases, feedback,
   diagnostics, metrics, and cleanup to tree transactions.
8. Add recursive, resize, damage, explicit-sync, and lifecycle regression tests.
9. Run focused and workspace validation, reproduce the clipboard baseline, and
   document hardware/manual limitations.

## Checkpoints

- After tasks 1-3: role/cache unit tests compile and demonstrate red/green.
- After tasks 4-6: real-client buffer, placement, stacking, and resize tests pass.
- After tasks 7-8: explicit-sync and lifecycle suites pass without leaks.
- After task 9: formatting, checking, linting, tests, release build, and diff
  checks are recorded.

## Risks

- Existing commit helpers mutate generation and renderer state incrementally.
  Publication must reserve a generation and prevent observation between nodes.
- Explicit-sync currently queues individual surface commits. Tree ownership must
  prevent a child fence from promoting independently.
- Cached callback and buffer supersession must preserve exactly-once lifecycle.
