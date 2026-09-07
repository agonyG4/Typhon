# Typhon KMS Commit Worker Production Hardening and Auto-Readiness v1

Date: 2026-09-07

Status: completion-lane hardening implemented and committed; all 42 KMS-worker
tests pass. Repository-wide format, clippy, and all-target test gates remain
affected by unrelated unstaged edits in the shared checkout; physical DRM
qualification remains pending. The worker default remains `off`.

## Outcome

The confirmed completion-publication defect was fixed without redesigning the
native presentation architecture. The KMS worker still has bounded admission
with `QUEUED_JOB_CAPACITY = 1`, but its internal completion queue is now
lossless and never waits for the compositor to consume an earlier result.

Implementation commits:

- `220fc98` — completion-lane design and implementation plan;
- `2add594` — lossless KMS worker completion publication and deterministic
  regression tests.
- `05d2116` — chain the fatal regression job from the preceding successful
  submission so the scripted panic is exercised rather than validation-
  invalidated.

The accepted baseline remains present and was not modified:

- `2a37b40` — XWayland backend continuation;
- `a1693f3` — render-fence timing evidence retention;
- `67d2c3e` — complete KMS dispatch-duration budgeting;
- `3fe2704` — submission-anchored pageflip timeout.

## Initial architecture

The worker follows the existing ownership state machine:

```text
Built/Ready
  -> reservation
  -> one queued job
  -> dequeue
  -> predecessor validation/wait
  -> cursor-sidecar collection
  -> frozen immutable payload
  -> TEST_ONLY when required
  -> real Atomic submit
  -> KernelInFlight
  -> Submitted ownership event
  -> exact pageflip acknowledgement
  -> settlement
```

Worker lifecycle is monotonic across:

```text
Running -> Quiescing -> Stopped
Running -> ShutdownQuiescing -> ShutdownAbandoning/Stopped
Running -> Fatal
```

Execution phases remain `Idle`, `DequeuedWaitingPredecessor`,
`CollectingSidecar`, `FrozenForValidation`, `TestOnly`, `SubmitIoctl`, and
`KernelInFlight`. The existing `submit_gate` remains the one concurrent kernel
submission boundary used by submission and session pre-revoke quiesce.

## Confirmed defect

`WorkerShared` held a result `VecDeque`, a `RESULT_EVENT_CAPACITY` of eight,
and a `result_space` condition variable. Both `publish_event()` and
`mark_fatal()` waited when the result queue was full. Teardown joins the worker
before its final result drain, so an undrained completion queue could prevent
worker progress and termination. A fatal marker could likewise wait behind
older results, leaving submitted ownership or fatal state inaccessible through
the normal completion lane.

This was a real synchronization defect, not a queue-throughput problem. The
correct boundary is bounded admission plus lossless compositor-owned completion
delivery.

## Rejected hypotheses and out-of-scope changes

The implementation deliberately does not:

- increase admission capacity or add multi-CRTC queues;
- drop or coalesce `Submitted`, terminal, quiesce, fatal, or sidecar events;
- add another submission mutex, polling loop, busy wait, or worker thread;
- change EBUSY retry count or approximately 100/400 microsecond backoffs;
- change pageflip timeout anchoring or dispatch-duration measurement;
- add KWin-style commit merging, reordering, or realtime scheduling;
- enable Direct Scanout or change explicit-sync/session-recovery ownership;
- change `OBLIVION_ONE_KMS_COMMIT_WORKER` default from `off` to `auto`.

`BusyDeferred` is also preserved losslessly because the current state and
diagnostic behavior do not justify a coalescing policy.

## Implementation changes

`src/native_output/kms_worker/queue.rs`:

- removed `RESULT_EVENT_CAPACITY`;
- removed `WorkerShared::result_space`;
- retained the existing admission queue capacity of one;
- initialized the internal completion queue without a producer-facing cap.

`src/native_output/kms_worker/thread.rs`:

- `publish_event()` appends the event and releases the result mutex before the
  best-effort eventfd write;
- `mark_fatal()` appends exactly one fatal marker without waiting for result
  space;
- `drain_events()` only drains and no longer wakes a producer;
- eventfd remains a reactor wake mechanism, not the authoritative state store.

If eventfd notification fails, the event already in the queue remains
authoritative, the existing fatal reason is stored atomically, and the fatal
marker is appended to the same queue. Existing fatal-job retention remains
separate for jobs whose submit ownership is uncertain or that were still
queued.

## New deterministic tests

Added to `src/native_output/kms_worker/tests.rs`:

- `undrained_completion_results_do_not_block_quiesce_or_join` leaves nine
  successful `Submitted` results undrained, quiesces, joins, and checks one-shot
  recovery of all results;
- `fatal_publication_does_not_wait_for_undrained_completion_results` forces a
  fatal submit after nine prior completions and checks fatal-job identity and
  `uncertain_submit`;
- `eventfd_failure_preserves_submitted_ownership_and_uncertain_state` forces
  eventfd saturation after a successful submit and checks `Submitted`, fatal
  reason, in-flight ownership, and uncertain state;
- existing eventfd failure tests now join before draining and assert rejection,
  fatal, and queued-job results are not lost;
- `quiesce_returns_queued_job_and_sidecar_once_with_undrained_results` checks
  active submit, queued successor, pending sidecar, prior undrained result,
  exact quiesce ownership, and no pending duplicate;
- `completion_drain_is_one_shot_and_does_not_duplicate_settlement` checks exact
  `Submitted`/`Quiesced` counts and an empty second drain.

The requested RED run was attempted before the production queue change. The
initial repository-wide compile was blocked by unrelated unstaged effect/frame
edits in the shared checkout. The follow-up fatal-test failure was then traced
to the test itself leaving job 10 on the helper's default `Presented` base after
job 9 had established a bundle; job 10 was correctly invalidated and never
reached the scripted panic. Commit `05d2116` makes job 10 explicitly depend on
job 9. The focused fatal test and the complete KMS-worker test suite now pass.

## Lock-order and ownership review

The relevant intentional ordering is:

```text
submit_gate -> state
state released -> fatal_jobs when fatal queued ownership is extracted
state released -> results when a completion is published
results released -> eventfd notification
```

No path holds the completion mutex while waiting for another completion or for
the main thread. No path holds the result mutex while taking `state` or
`fatal_jobs`. `join()` has no dependency on result consumption, and teardown
can join first and drain afterward.

Successful submit still transfers physical ownership into `KernelInFlight`;
`Submitted` is never dropped. Exact generation, token, bundle, transaction,
and CRTC checks remain in the pageflip acknowledgement path. Quiesce returns
queued jobs and pending cursor sidecars through the existing `Quiesced` event.
The existing deferred-pageflip path remains responsible for pageflip readiness
observed before the main thread consumes `Submitted`; no epoll ordering
assumption was introduced.

## KWin and Aquamarine comparison

Current upstream KWin's DRM commit thread keeps one committed item, waits for
its pageflip before allowing another physical submit, and pings the main side
to distinguish a stalled main thread from a real pageflip timeout. It also has
separate commit optimization and safety-margin machinery. Typhon retains the
narrow single-output ownership rule and its accepted absolute watchdog, without
adding KWin commit merging, reordering, or realtime scheduling. Reference:
[KWin `drm_commit_thread.cpp`](https://raw.githubusercontent.com/KDE/kwin/master/src/backends/drm/drm_commit_thread.cpp)
and [header](https://raw.githubusercontent.com/KDE/kwin/master/src/backends/drm/drm_commit_thread.h).

Current Aquamarine exposes request-owned Atomic commit data and explicit
connector/CRTC pending-flip identity in its DRM interfaces. Those are useful
architectural parallels for immutable request ownership and exact release,
but Typhon does not import Aquamarine's multi-output or multi-queue direction.
Reference:
[Aquamarine `Atomic.hpp`](https://raw.githubusercontent.com/hyprwm/aquamarine/main/include/aquamarine/backend/drm/Atomic.hpp)
and [DRM interfaces](https://raw.githubusercontent.com/hyprwm/aquamarine/main/include/aquamarine/backend/DRM.hpp).

## Deterministic verification status

Commands were run in `/home/agony/GitHub/Typhon` through the existing `rtk`
wrapper and existing Cargo build state. The worker-specific files pass direct
rustfmt checking:

```text
rtk rustfmt --edition 2024 --check \
  src/native_output/kms_worker/queue.rs \
  src/native_output/kms_worker/thread.rs \
  src/native_output/kms_worker/tests.rs       PASS
rtk git diff --check                           PASS
```

The required repository gates were run after the final worker change. Current
results are:

| Command | Result | Blocking evidence |
|---|---|---|
| `rtk cargo fmt --check` | BLOCKED | rustfmt differences in concurrently edited `src/compositor/tests/xwayland.rs` |
| `rtk cargo check --locked --all-targets` | PASS | no compile errors |
| `rtk cargo clippy --locked --all-targets -- -D warnings` | BLOCKED | unrelated `clippy::too_many_arguments` at `src/effects/render_graph.rs:277` |
| `rtk cargo test --locked --all-targets` | BLOCKED | 2 unrelated compositor test failures; 2153 passed and 2 ignored |
| `rtk cargo test --locked --bin oblivion-one native_output::kms_worker::tests` | PASS | 42 passed, 1227 filtered |

The dirty files were not staged, modified, reset, or discarded. They include
unstaged compositor state/tests and support, effects/render graph, EGL effect
tests, and native repaint tests. This report therefore does not represent the
repository as fully green. The two all-target failures were:

- `compositor::state::desktop_window_tests::moving_between_regular_and_special_marks_presence_dirty`;
- `compositor::tests::workspace::wire_visibility_is_atomic_and_independent_of_astrea_manager`.

They are outside the KMS worker change and were already part of the shared
checkout's concurrent dirty work.

## Physical qualification and readiness

No native TTY/DRM session, real pageflip, VT switch, suspend/resume cycle,
1920x1080@165 Hz run, worker-off comparison, worker-auto comparison, or
application workload was available in this agent environment. Observed
hardware qualification is therefore:

```text
worker-off evidence: 0 runs observed / not evaluated
worker-auto evidence: 0 runs observed / not evaluated
VT away/back cycles: 0 observed / not evaluated
165 Hz qualification: 0 observed / not evaluated
```

The existing approved isolated command for a physical operator is:

```bash
TYPHON_FRAME_PACING_DEBUG=1 \
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon \
TYPHON_XWAYLAND=eager \
OBLIVION_ONE_MODE=1920x1080@165 \
OBLIVION_ONE_KMS_MODE=atomic \
OBLIVION_ONE_SCANOUT_BACKEND=native-egl-gbm \
OBLIVION_ONE_CURSOR=auto \
OBLIVION_ONE_KMS_COMMIT_WORKER=auto \
OBLIVION_ONE_TRIPLE_BUFFERING=auto \
OBLIVION_ONE_DIRECT_SCANOUT=off \
./bin/start-oblivion-one-tty
```

The physical matrix must compare equivalent worker `off` and `auto` runs,
exercise idle/pointer/Dock/window/browser/XWayland/high-refresh workloads,
perform three complete suspend/resume cycles where supported, and inspect
fatal events, result mismatches, eventfd failures, driver timeout suspicions,
EBUSY exhaustion, stale/duplicate pageflip rejection, ownership ledgers,
cursor sidecars, input fences, pacing, shutdown, and `SafeDisable` evidence.

## Default-policy decision

Keep `OBLIVION_ONE_KMS_COMMIT_WORKER` default `off`. Deterministic source-level
hardening is not physical qualification, and the repository-wide test gates are
currently blocked by unrelated unstaged code. The worker must not be described
as qualified, hardware-validated, or production-ready until the clean
deterministic gates and the physical worker-off/worker-auto matrix pass with no
unexplained ownership or timeout anomalies.
