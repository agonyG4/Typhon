# Typhon DMA-BUF GPU Release Authority v1.2 Plan

## Goal

Close the current-token eligibility and retry-quiescence regressions while preserving the v1.1 GPU-release coordinator, the worker Direct/KMS barrier, O1 callback admission, SHM materialization release, regional damage, and KMS scheduling.

## Steps

- [x] Add RED tests for exact-token revalidation at frame-batch terminals, distinct explicit-sync tokens, and current-token retry suppression.
- [x] Centralize exact active-token eligibility in compositor state and use it at every non-shutdown DMA-BUF completion terminal.
- [x] Partition deferred obligations by exact-token eligibility so current-token-blocked obligations are event-driven and inactive obligations retain bounded retry progress.
- [x] Run focused GREEN tests, audit all callers and pointer-reposition overlap, and update the v1 report with v1.2 evidence.
- [x] Run repository verification, record the unrelated flaky full-suite results, and commit the narrow closure.

## Non-goals

Do not change O1 pacing, SHM or DMA-BUF ownership domains beyond this eligibility fix, regional damage, DirectReleaseProof/DirectPrimaryLease semantics, KMS worker scheduling, resize behavior, or concurrent pointer-reposition work.
