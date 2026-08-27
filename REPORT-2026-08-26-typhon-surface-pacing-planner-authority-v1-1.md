# Typhon Surface Pacing Planner Authority v1.1

**Date:** 2026-08-26
**Scope:** Surface Pacing Planner Authority v1.1
**Checkout policy:** Current dirty working tree was authoritative; unrelated changes were preserved.

## A. Baseline findings

The source audit found two hidden authorities in the generic presentation path:

- `src/native_output/runtime/planner.rs::prepare_presentation_target()` called Commit Timing planning and unconditionally progressed surface pacing. A generic presentation opportunity could therefore mutate compositor pacing state even when no pacing work domain was due.
- `src/native_output/runtime/presentation_cycle.rs` invoked that helper before refreshed publication state and final output admission. A pacing mutation could publish scene work after the repaint decision, with its visual handoff discarded.

The native Wayland path also used `OwnCompositorServer::tick_with_outcome()` for readable-client dispatch. That broad operation included pacing progression, so read-side dispatch and pacing ownership were coupled.

The deadline cache audit found `rebuild_scene_work_index()` invalidating the pacing deadline cache. Scene-index rebuilds are common and do not themselves change pacing deadlines. The RED test `unrelated_scene_work_rebuild_does_not_invalidate_pacing_deadline_cache` caught this coupling. A second RED test caught stale cached FIFO deadlines after barrier removal.

## B. Final architecture

### Planner authority

`prepare_presentation_target()` now only returns the already-owned target. It does not enumerate Commit Timing candidates, arm targets, progress surface pacing, or mutate compositor state.

Commit Timing target planning remains in the explicit native `plan_pending_commit_timing()` path. A cheap pending-state recheck after `process_acquire_and_prepare()` plans newly-created Commit Timing work before final presentation admission.

### Wayland read-side dispatch and pacing

`OwnCompositorServer::dispatch_wayland_with_outcome()` performs readable-client acceptance, dispatch, lifecycle cleanup, Astrea publication, clipboard maintenance, and protocol flushing. It does not progress surface pacing.

It returns a readiness-generation transition. The native cycle services pacing when its deadline is due or when readable Wayland dispatch created new pacing readiness. An unrelated Wayland read does not clock an older pacing transaction.

`tick_with_outcome()` remains as the complete progression API for legacy/synthetic callers and explicitly performs pacing after read-side dispatch.

### Deadline cache ownership

The generic scene-work rebuild no longer invalidates the pacing deadline cache. Invalidation is now attached to pacing mutations, including:

- FIFO barrier activation and clearing;
- Commit Timing target arm, invalidation, and backward-clock replan;
- removal/cancellation/supersession of armed surface-tree transactions;
- output refresh-rate changes.

## C. Deterministic evidence

The native operation-plan seam was exercised for 1,000 independent input-only wakes:

| Operation | Count |
| --- | ---: |
| Input services | 1,000 |
| Wayland read-side dispatches | 0 |
| Full server progression calls | 0 |

Focused results:

- 18 native runtime work-domain tests passed;
- 1 pure presentation-target preparation test passed;
- 26 surface-pacing tests passed, including cache mutation tests;
- full `rtk cargo test --locked`: 2,986 passed, 5 ignored.

The task’s code paths do not claim real-host CPU/GPU improvement from these deterministic counts.

## D. Correctness tests

Relevant tests include:

- `one_thousand_independent_input_wakes_do_not_count_as_wayland_ticks`;
- `input_only_readiness_does_not_request_wayland_read_dispatch`;
- `combined_input_and_wayland_readiness_keeps_both_domains_once`;
- `wayland_dispatch_services_only_new_pacing_readiness`;
- `astrea_publication_deadline_is_protocol_only`;
- `generic_presentation_target_preparation_only_returns_owned_target`;
- `unrelated_scene_work_rebuild_does_not_invalidate_pacing_deadline_cache`;
- `clearing_a_fifo_barrier_invalidates_the_pacing_deadline_cache`;
- `selected_timing_target_releases_before_presentation_time_and_remains_owned`.

The existing full suite also passed the surface-tree, Commit Timing, frame-consumption, input, explicit-sync, cursor, XWayland, and interaction regressions.

## E. Full verification

Passed:

```text
rtk cargo fmt --all -- --check
rtk cargo check --locked
rtk cargo test --locked
rtk git diff --check
```

`cargo check` reports 7 existing dead-code warnings and no errors.

`rtk cargo clippy --all-targets --all-features --locked -- -D warnings` remains blocked by 22 diagnostics in unrelated dirty workspace/layout/protocol paths, including `src/compositor/state/tiled_layout.rs`, `src/compositor/state/tiled_resize.rs`, `src/wm/layout/constraints.rs`, and other pre-existing dirty files. No task-specific planner, pacing, or native dispatch diagnostic was reported.

## F. Real-host qualification status

- **Verified in deterministic tests:** planner purity, explicit Commit Timing planning ownership, read-side/pacing separation, transition-aware pacing service, operation-domain classification, cache invalidation ownership, and full regression suite.
- **Verified on real KMS hardware:** not run in this environment.
- **Requires user qualification:** 1000 Hz hardware-cursor motion over light/native and Chromium/Electron clients, heavy multi-window workspaces, floating/tiled move and resize, relative pointer lock, software cursor, and XWayland eager mode on the NVIDIA/165 Hz machine.

No numeric CPU/GPU improvement is claimed.

## G. Remaining risks

- The native key/button pointer-constraint follow-up still uses the complete `tick_with_outcome()` path because it is an explicit constraint-sensitive case; ordinary pointer motion does not use it.
- Session transition code retains the legacy complete tick API for its own Wayland progression.
- Real-host qualification is still required to determine how much Chromium/Electron client behavior contributes to residual pointer latency.

## Review pass results

### Correctness / ownership

The implementation was reviewed for pure input, combined readiness, prompt write-side flushing, pending transaction liveness, lifecycle cleanup, output-admission ordering, and preservation of scheduler-admitted geometry. The native input-only path no longer calls the full server tick. Publication and pacing are serviced through their own explicit boundaries.

### Adversarial performance / regression

The implementation was checked against stale cached deadlines, planner-side mutation, discarded visual handoffs, unrelated Wayland reads clocking old pacing work, and input-only domain classification. The 1,000-wake operation-plan test distinguishes input service from read-side dispatch and full progression.
