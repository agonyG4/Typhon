# Typhon DMA-BUF GPU Release Authority v1.3 Plan

## Scope

Close the teardown-only exactly-once gap in DMA-BUF protocol completion. The
normal GPU-release coordinator, retry debt, Direct/KMS barriers, SHM lifetime,
O1 admission, regional damage, and KMS scheduling remain unchanged.

## Steps

1. Add RED state tests covering duplicate exact-token ownership across active,
   deferred, frame-batch, and GPU-lease containers; preserve distinct explicit
   release points; and verify repeated shutdown finalization is idempotent.
2. Introduce one shutdown-local exact-token collection and route all DMA-BUF
   shutdown sources, including cached and unmaterialized pending buffers,
   through it. Keep SHM shutdown release behavior direct and unchanged.
3. Run focused compositor frame/shutdown tests, then the required formatting,
   check, clippy, full-test, diff, and status verification.
4. Update the v1 report with source evidence, test results, and the explicit
   distinction between unit/integration/native qualification. Commit the narrow
   teardown closure.

## Verification record

- [x] RED tests fail against the pre-fix shutdown implementation: three
  duplicate-token tests observed two completions instead of one.
- [x] Focused tests pass after the fix.
- [x] Full repository verification is run; the aggregate suite is clean.
- [x] Native DRM/KMS qualification is not run for this teardown-only change.
