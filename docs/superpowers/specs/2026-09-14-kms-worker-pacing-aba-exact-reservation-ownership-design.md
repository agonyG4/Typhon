# KMS Worker Pacing ABA: Exact Reservation Ownership

## Goal

Prevent asynchronous KMS worker success and terminal events from mutating a
new pacing lifecycle that reuses the predecessor's `NativeOutputFrameId`.

## Context and root cause

`NativeOutputFrameId` is a scheduler identity and can be reused by a new
Predictive O1 render attempt while an older worker job is still outstanding.
The current worker reservation and its settlement paths compare that logical
ID, so an old predecessor result can clear the newer ready successor. The
successor's physical Predictive O1 lifecycle then leaks until the bounded
four-entry ledger rejects a later admission.

The existing `PredictiveO1AttemptId` and immutable `OutputFrameKey` separation
is correct and remains unchanged. `OutputFrameKey` is an additional physical
identity when available, not a generic worker capability.

## Design

`NativeFramePacing` owns a dedicated `WorkerPacingReservationId` sequence and
returns an immutable `WorkerPacingTicket` when a worker reservation is made.
The ticket captures:

- exact reservation ID;
- logical `NativeOutputFrameId` for telemetry only;
- source `ready_submit` semantics;
- `PredictiveO1AttemptId`, when present;
- `OutputFrameIdentitySnapshot`, when present; and
- `OutputFrameKey`, when present.

The pacing state stores the same ticket as its single outstanding reservation,
plus `active_worker_reservation_id` and `ready_worker_reservation_id` lane
affiliations. Reservation settlement removes the matching ticket by exact ID.
It clears only the lane explicitly affiliated with that ID and builds pending
state from the ticket snapshot. It never decides which lane to mutate from a
logical frame-ID comparison.

When active work migrates to ready, its affiliation moves with it. When a new
render attempt begins on an already active logical frame, the active
affiliation is detached before the new attempt receives predictive metadata;
the old ticket remains valid until its own terminal event arrives.

## Worker transport and terminal paths

`KmsCommitJob` carries `Option<WorkerPacingTicket>` instead of split pacing
frame and predictive physical fields. The explicit composited, compatibility,
and direct worker admission paths capture the ticket once. The direct rollback
guard retains that ticket. Worker success, known rejection, queued-job drop,
invalidated-job replan, direct rejection, quiesce, fatal handling, and shutdown
retention all use the ticket carried by the job. Jobs without pacing ownership
continue to carry `None`.

Pacing trace events expose reservation ID alongside logical frame ID,
`ready_submit`, predictive attempt ID, and physical output frame ID where
applicable. A stale returned ticket reports its ID and the currently
outstanding ID without changing newer state.

## Verification strategy

Tests are added before the implementation and first run RED. The regression
suite models a normal predecessor reservation, a same-logical-ID Predictive O1
successor becoming ready, and the predecessor's success or cancellation. It
also retains same-work active→ready settlement, rejects delayed stale results
after replacement, protects timing ownership, and proves ticket preservation
through a real `KmsCommitJob` boundary and a real rejection path. A
deterministic 10,000-iteration overlap stress test requires exact Predictive
O1 terminal accounting, no invalid stage transitions, no reservation leaks,
and no lifecycle-capacity growth.

No lifecycle capacity, pageflip recovery, fullscreen/CU/effects subsystem, or
physical-key validation behavior is changed.
