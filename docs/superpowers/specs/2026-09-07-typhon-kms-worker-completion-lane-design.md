# Typhon KMS Worker Completion-Lane Hardening Design

## Goal

Make KMS worker completion publication lossless and independent of main-thread
consumption so the worker can always reach quiescence, fatal termination, and
join without weakening the existing ownership-oriented presentation pipeline.

## Current architecture and confirmed defect

The worker admits at most one queued job (`QUEUED_JOB_CAPACITY = 1`) and tracks
the exact job state through predecessor validation, cursor-sidecar collection,
TEST_ONLY, real Atomic submit, kernel in-flight ownership, exact pageflip
acknowledgement, and settlement. The lifecycle remains `Running`, `Quiescing`,
`ShutdownQuiescing`, `ShutdownAbandoning`, `Stopped`, or `Fatal`.

The result queue is an internal compositor-owned `VecDeque<KmsWorkerEvent>`
signalled by an eventfd. It is currently capped at eight entries, and both
`publish_event()` and `mark_fatal()` wait on `result_space` when that cap is
reached. Teardown joins the worker before its final result drain. This creates a
worker-forward-progress dependency on the main thread and can strand submitted
ownership or the fatal marker behind earlier undrained results.

## Design

Keep bounded admission and completion delivery separate:

```text
main admission: one reserved/queued job plus the existing active slot
worker result:  mutex-protected lossless internal completion queue
reactor wake:   eventfd, used only as a best-effort notification
```

Remove the result-capacity constant and `result_space` condition variable.
`publish_event()` pushes every event into the internal queue without waiting,
then attempts the eventfd notification. If notification fails, it records the
existing metric and marks the worker fatal; the just-published event remains in
the authoritative queue and the fatal marker is appended there as well.

`mark_fatal()` retains queued/fatal job ownership exactly as before, updates the
authoritative fatal reason atomically, and appends one `Fatal` event without
waiting. Its eventfd write remains best effort and cannot recursively publish a
second fatal event. `drain_events()` only drains events; it no longer wakes a
producer because no producer waits for result space.

All current event variants remain lossless, including `BusyDeferred`; no event
coalescing or queue-depth expansion is introduced. Event order remains the
mutex-protected enqueue order, while the atomic fatal reason and explicit fatal
job list remain independent recovery fallbacks.

## Lock ordering and ownership

The worker state lock is released before taking `fatal_jobs` or the result
queue. Result publication holds only the result-queue mutex while appending.
The eventfd write occurs after releasing it. Fatal handling follows the same
order and never waits for a consumer. The main thread may join first and drain
afterward; the worker has no path that waits for that drain.

Successful submit ownership is still represented by `Submitted` and remains
kernel-in-flight until the exact generation/token/bundle/CRTC pageflip is
acknowledged. If an eventfd write fails after submit, the queued `Submitted`
event, `Fatal { uncertain_submit: true }`, and any explicit fatal-job record are
all retained for health-check or teardown recovery. Quiesce still returns the
exact queued job and pending cursor sidecar in one `Quiesced` event.

## Verification strategy

Deterministic worker tests will cover:

1. nine undrained successful completions followed by quiesce and join;
2. a fatal path after prior undrained completions;
3. eventfd failure after a rejected job and after successful submission;
4. quiesce with an active submit, a queued next job, a pending sidecar, and
   earlier undrained results;
5. one-shot draining and exact event counts for completion settlement.

Existing pageflip deferral/arbitration, timeout, EBUSY retry, input-fence,
cursor-sidecar, session, shutdown, explicit-sync, and Direct Scanout ownership
tests remain unchanged and are included in final verification.

## Qualification boundary

This change provides deterministic source-level hardening only. It does not
change `OBLIVION_ONE_KMS_COMMIT_WORKER` policy, enable Direct Scanout, alter
retry counts, or claim native DRM, VT, 165 Hz, suspend/resume, or workload
qualification. The default remains `off` until the existing physical
qualification matrix is completed.
