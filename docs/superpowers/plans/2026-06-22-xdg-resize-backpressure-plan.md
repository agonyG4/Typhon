# Implementation Plan: XDG Resize Backpressure And Transactions

## Phase 1: Flow Model

1. Add failing tests for one in-flight configure, 1,000 coalesced updates,
   delayed ACK retention, duplicate geometry, independent surfaces, and clear.
2. Replace fixed resize history with per-root bounded flow state and metrics.
3. Add final-pending behavior and transition cleanup.

## Phase 2: Transactional Commits

4. Add failing tests for buffer, bufferless, geometry-only, pre-ACK, duplicate,
   child, and anchored actual-size commits.
5. Capture acknowledged resize state once at root commit receipt and apply the
   immutable snapshot from both commit paths.
6. Clear preview and advance flow only after successful application.

## Phase 3: Explicit Sync

7. Add delayed-fence and starvation tests.
8. Store commit snapshots in pending acquire records and implement the bounded
   oldest-candidate/newest-successor policy.
9. Add diagnostics and pending-acquire metrics.

## Phase 4: Regression And Verification

10. Add the serial-308/300-update/final-configure regression.
11. Update architecture documentation.
12. Run focused suites and the full validation matrix; reproduce the known
    clipboard failure unchanged on the task base commit if it remains.
