# Typhon Surface Pacing Authority v1.1.1 — Closure Report

**Date:** 2026-08-26  
**Scope:** explicit-sync service ownership, acquire/prepare admission, Surface Pacing readiness, Commit Timing causality, and deterministic native-cycle evidence.

## A. Baseline findings

The current working tree was treated as authoritative. It contained unrelated dirty and untracked compositor, workspace, Dwindle, XWayland, input, and report work; none of that work was reset or discarded.

The source-level baseline reproduced these issues:

| Finding | Previous behavior | Closure evidence |
| --- | --- | --- |
| F1 — `src/compositor/state/frames.rs` and `src/native_output/runtime/cycle.rs` | Any pending surface-tree transaction could promote an unrelated wake into explicit-sync service. | `has_pending_acquire_watch_changes()` is now limited to runtime watch mutations; notifier tokens and fallback deadlines remain separate readiness causes. Future-timing, FIFO-only, and unreadable external-acquire tests pass. |
| F2 — `src/native_output/runtime/cycle.rs` and `work_domains.rs` | Acquire/prepare service admission was reused as presentation admission; CursorOnly also entered acquire/prepare. | `AcquirePrepareOutcome` separates acquire service, frame prepare, and visual-work creation. CursorOnly reaches presentation without acquire/prepare. |
| F3 — `src/compositor/state/frames.rs` | Every ordinary acquire-ready transition also advanced Surface Pacing readiness. | The ordinary acquire-ready test proves the pacing generation is unchanged. FIFO-owned transitions retain their pacing transition. |
| F4 — `src/compositor/state/scene_work.rs`, `commit_timing_runtime.rs` | Post-prepare planning only detected global `false → true`, missing `true → true` candidate-set changes. | A planning generation changes when the unarmed root-head candidate signature changes and remains stable across unchanged rebuilds. Native post-prepare recheck uses that generation. |
| F5 — `src/native_output/runtime/work_domains.rs` and native cycle gates | The former 1,000-wake evidence exercised classification only. | The new 1,000-cycle test exercises `NativeCycleOperationPlan`, the same operation-plan seam used by production `run_cycle()`. |
| F6 — `src/compositor/server_frames.rs` | Pacing could publish callback/protocol output after the preceding dispatch flush. | `OwnCompositorServer::progress_surface_pacing()` owns a write-side flush before returning; the service-boundary flush test passes. |

## B. Final architecture

### Explicit-sync service debt

Native explicit-sync service is now admitted only by explicit causes:

- explicit-sync reactor readiness or tokens;
- pending acquire-watch registration/cancellation mutations;
- a due acquire fallback deadline;
- existing session recovery/rearm ownership.

The classifier no longer treats the existence of FIFO, Commit Timing, or other pending surface transactions as explicit-sync service debt. Registered unreadable acquire fences remain passive.

### Acquire/prepare versus presentation

`NativeCycleOperationPlan` now distinguishes:

- input service;
- Wayland read-side dispatch;
- acquire/prepare service;
- explicit-sync service within that path;
- output presentation admission.

The typed `AcquirePrepareOutcome` records whether acquire service ran, whether frame preparation ran, and whether preparation created visual work. Presentation is admitted from actual output facts, completed frame work, redraw requests, or visual work—not solely because acquire maintenance ran.

CursorOnly output remains directly presentation-admitted and does not borrow acquire/prepare service.

### Surface Pacing readiness ownership

Ordinary acquire completion rebuilds scene work but does not create Surface Pacing readiness debt. FIFO barrier clear and pacing-owned transitions retain their existing readiness ownership. Pacing progress is serviced through `progress_surface_pacing()` and flushes outgoing Wayland events at that service boundary.

The native Astrea service entrypoint also performs the pending-state check before calling reconciliation, so clean service calls cannot reach resource pruning or metrics refresh.

### Commit Timing planning causality

`CompositorState` tracks a nonzero planning generation derived from the identity and eligibility of unarmed root-head Commit Timing candidates. The generation changes when the candidate set changes and stays stable for unrelated scene-index rebuilds. The native cycle compares this scalar before and after preparation, catching `true → true` candidate changes without scanning transactions on input-only service admission.

## C. Deterministic evidence

The production-used operation-plan test `one_thousand_independent_input_cycles_use_production_service_gates` asserts, for 1,000 independent input-only iterations with no due transaction/output domain:

```text
input service calls                 1000
Wayland read-side dispatches           0
explicit-sync service calls            0
acquire/prepare service calls          0
Surface Pacing service calls           0
Commit Timing planning services        0
presentation planning admissions       0
```

This is distinct from the older classifier-only 1,000-wake test. Both pass.

Additional deterministic results:

- future Commit Timing and FIFO-only transactions do not become explicit-sync service work;
- an unreadable external acquire remains passive;
- acquire-watch mutation is serviceable;
- explicit-sync readiness is serviceable;
- CursorOnly is presentation-admitted without acquire/prepare;
- an ordinary acquire-ready transition leaves Surface Pacing readiness generation unchanged;
- Commit Timing generation changes for a new independent root-head candidate and remains stable for an unchanged rebuild;
- pacing service invokes its write-side flush boundary exactly once in the boundary test.

Telemetry now exposes separate counters for `explicitSyncServiceRuns`, `framePrepareRuns`, `surfacePacingServiceRuns`, and `commitTimingPlanningReplans`, in addition to the existing acquire/prepare, read-dispatch, flush, and presentation counters.

## D. Correctness tests

Focused tests passed:

```text
rtk cargo test --locked surface_pacing -- --test-threads=1                         28 passed
rtk cargo test --locked commit_timing -- --test-threads=1                          10 passed
rtk cargo test --locked native_output::runtime::work_domains -- --test-threads=1   23 passed
rtk cargo test --locked native_input_batch_defers_and_coalesces_write_side_flushes -- --test-threads=1  1 passed
rtk cargo test --locked resource_efficiency -- --test-threads=1                    4 passed
```

The new state and service-gate tests also passed individually, including the 1,000-cycle test and the pacing boundary flush test.

The existing input batch ownership test continues to prove that multiple internal flush requests collapse to one useful native-batch flush boundary.

## E. Verification

Passed:

```text
rtk cargo fmt --all -- --check
rtk cargo check --locked
rtk cargo test --locked --test sigchld one_child_exit_wakes_the_sigchld_signalfd_once -- --test-threads=1
rtk git diff --check
```

The serialized full suite command was run:

```text
rtk cargo test --locked -- --test-threads=1
```

It executed 1,886 passing tests and 2 ignored tests, but the aggregate command exited 101 because one `oblivion_one` SIGCHLD test (`one_child_exit_wakes_the_sigchld_signalfd_once`) failed once. The exact test passed on the immediate standalone retry. The failure was outside the Typhon task paths and did not reproduce in the focused retry.

Clippy was also run with `-D warnings`. It was blocked by 22 diagnostics and one warning in unrelated pre-existing dirty/untracked paths, including workspace protocol, fullscreen, tiled layout/resize, XWayland mode, and workspace layout files. No diagnostic was reported for the task’s runtime service-gate, pacing, Commit Timing, or resource-efficiency changes.

## F. Real-host qualification

Not run in this environment. No CPU, GPU, latency, KMS, NVIDIA, or 165 Hz improvement is claimed.

The remaining qualification should use the user’s real KMS/NVIDIA host with the existing Typhon counters and the prescribed idle, 1000 Hz pointer, Chromium/Electron, move/resize, pointer-lock, software-cursor, and XWayland scenarios.

## G. Remaining risks

- The full suite has a pre-existing/flaky external SIGCHLD failure that needs independent stabilization if it recurs.
- Clippy cleanliness remains blocked by unrelated dirty-tree diagnostics.
- Real-host contribution to the remaining Chromium/Electron pointer-lag symptom is unmeasured.
- `tick_with_outcome()` remains a broad compatibility API. Native input uses the narrower dispatch/flush boundaries except for the existing constrained key/button follow-up, which remains intentionally narrow.

## H. Self-review results

### Review pass 1 — correctness and ownership

Verified in source and tests that notifier/fallback acquire liveness remains active, session rearm remains owned by recovery, FIFO and Commit Timing deadlines remain independently armed, ordinary acquire completion is Scene Prepare-owned, CursorOnly remains output-owned, pointer/input batching still flushes promptly, and the existing scheduler-admitted interaction geometry path was not moved into raw input.

### Review pass 2 — adversarial performance and regression

Searched for the old broad explicit-sync predicate, overloaded `prepare_work`, hidden input-driven pacing, duplicate input flushes, and classifier-only evidence. The final code has no remaining `has_pending_explicit_sync_work()` or `should_process_acquire_and_prepare()` call. The 1,000-cycle test uses the production operation-plan gate, and the final runtime recomputes presentation admission from output facts after acquire/prepare service.

The task leaves blocked state passive: readiness transitions, deadlines, and owned output work—not unrelated pointer frequency—drive service.
