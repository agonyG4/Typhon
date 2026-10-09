# Typhon VRR Phase 2.2B: Domain-Neutral KMS Presentation Foundation

## Goal

Represent fixed, variable, and deferred presentation ownership explicitly through the KMS submission boundary while keeping every current production frame on its existing fixed-refresh path.

## Design

`FramePresentationReservation` will expose domain-neutral metadata (`reason`, `opportunity`, `fixed_target`, `variable`, and explicit deferred-O1 state). Fixed-target compatibility accessors may remain only at call sites whose operations are intrinsically fixed-refresh. Runtime code must match the reservation or ask whether it is explicitly deferred; absence of a fixed target does not imply deferred O1.

Rendered frames and KMS jobs will carry both the exact reservation and a `KmsPresentationTiming`. The timing enum will preserve `KmsSubmitWindow` unchanged for fixed refresh and add a variable form containing an earliest-submit lower bound and preferred-present telemetry only. KMS payload validation will compare exact reservation values for the transaction and every owner, reject deferred O1 jobs, and reject reservation/timing domain mismatches.

The KMS worker will keep its fixed wake, wait, deadline, fairness, apply-guard, telemetry, TEST_ONLY, and ioctl behavior unchanged. Variable work may wait until its earliest-submit time, but will not use preferred-present time as a deadline or contribute deadline misses to fixed deadline-tail learning. Generic execution telemetry remains available.

`PresentationDeadlinePlanner` will gain a separate phase-free preparation method that uses the planner's existing generation and sequence authority and advances one sequence per actual variable presentation. The method will not be wired into pageflip runtime in this phase. O1 remains fixed-only and runtime presentation remains unchanged.

## Validation

Regressions will cover reservation classification and exact ownership, fixed KMS timing equivalence, variable lower-bound/no-hard-deadline behavior, one-step variable pageflip accounting at 165 Hz and after a 100 ms idle, unchanged fixed accounting, and zero production construction of variable reservations or timings. Validation will run the requested focused Cargo tests, the full Cargo test suite, clippy, formatting, and diff checks with build output directed to `/mnt/Aether/Desktop/GitHub/Typhon-target`.

## Scope limits

No production caller will construct `FramePresentationReservation::Variable` or `KmsPresentationTiming::Variable`. No native runtime will call `VrrWindowPlanner` or the variable pageflip preparer. Physical presentation scheduling and O1 policy will not change.
