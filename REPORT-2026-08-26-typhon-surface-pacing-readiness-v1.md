# Typhon Surface Pacing Readiness v1 — Closure Report

**Date:** 2026-08-26
**Scope:** Native surface-pacing readiness, ordered surface-tree work, commit-timing planning, and related input-frequency residuals
**Authoritative checkout:** Current dirty working tree; unrelated existing changes were preserved

## Outcome

Surface-pacing readiness is now separated from raw input frequency. Active pacing is diagnostic state, while pacing service is admitted only by a matured deadline or an explicit readiness transition. Unarmed Commit Timing is planning-only protocol work, blocked FIFO state no longer creates perpetual visible scene preparation, and pacing publication reports an active-scene visual handoff back to the native scheduler.

The implementation also closes the remaining residuals identified by the supplied Pointer Protocol Quiescence investigation: native input does not require a full Wayland read-side tick, clean Astrea publication is gated before reconciliation, native interaction flushes are batched, and compositor-owned interaction motion avoids generic decoration hover traversal.

Rendering topology, O1 buffering, KMS scheduling, Direct Scanout, Dwindle layout authority, and scheduler-admitted interaction geometry were not redesigned.

## A. Baseline findings

### Pacing readiness was conflated with pacing activity

Before this closure, `NativeRuntime::should_progress_surface_pacing()` admitted service whenever surface-pacing work existed, and `NativeWorkDomains::classify()` promoted active pacing for unrelated timer, control, explicit-sync, Wayland, or scene work. A pending FIFO or Commit Timing transaction could therefore make unrelated wakes enter `progress_surface_pacing()`.

The source baseline was revalidated in:

- `src/native_output/runtime/cycle.rs`
- `src/native_output/runtime/work_domains.rs`
- `src/compositor/state/surface_pacing.rs`

The first RED test run also confirmed that the required planning state/API was absent before implementation.

### Blocked pacing masqueraded as scene preparation

The dirty `SceneWorkIndex` implementation added prepare work for active FIFO barriers even after a matching frame batch owned the claim, and for future Commit Timing transactions before a target was armed. That made blocked or planning-only state look like visible work due now.

The affected source is `src/compositor/state/scene_work.rs`. The corrected index only exposes FIFO prepare work until frame ownership exists, and only exposes ordered transaction prepare work when the transaction can actually be prepared. Future root-head Commit Timing requests are represented by a separate planning obligation.

### Deadline discovery repeatedly scanned mutable pacing state

`next_surface_pacing_deadline_ns()` previously scanned active barriers and general Commit Timing state on every query. The native deadline arm path queries this during cycle rearming, so stable state could repeatedly pay the scan cost.

The relevant source is `src/compositor/state/surface_pacing.rs`. The deadline is now cached and invalidated by pacing mutations. The stable-query test observes one recomputation across repeated queries.

### Readiness transitions lacked an independent service latch

Explicit-sync acquire readiness and FIFO barrier presentation completion can unblock ordered work without mouse motion. The closure adds a stale-safe monotonic readiness generation, with the serviced generation advanced only to the generation observed at the beginning of a service pass. A transition occurring during service remains pending for the next pass.

### Native input still carried unrelated protocol work

The prior native path folded input readiness into `wayland_dispatch`, called the broad `server.tick()` before draining input, and let clean pointer motion enter Astrea reconciliation. The supplied Pointer Protocol Quiescence implementation already removed those input-path calls; this closure preserves that separation and extends it to pacing and Commit Timing planning.

## B. Final architecture

### Input readiness vs. Wayland readiness

`NativeWorkDomains::classify()` now derives `wayland_dispatch` only from Wayland listener/client readiness. Input readiness remains an independent service domain. A pure input wake therefore does not call the broad server tick. Native-only key/button input retains the existing narrow follow-up tick only where pointer-constraint state may change.

The reactor’s existing separate listener, client, and native-input sources remain the readiness authority.

### Read-side dispatch vs. write-side flush

`OwnCompositorServer::tick()` remains available for synthetic/headless and other complete progression callers. Native runtime dispatch uses `tick_with_outcome()` when actual Wayland read-side readiness exists, while input-generated protocol events continue to use the native input batch’s write-side flush latch.

`tick_with_outcome()` reports whether its pacing pass changed the active scene, so a readable Wayland cycle cannot publish visible pacing state and lose the corresponding render admission.

### Dirty-driven Astrea publication

Astrea publication has a cheap pre-reconcile gate. Clean pointer samples return before collection construction, dead-resource pruning, metrics refresh, or publisher reconciliation. Focus and other real toplevel changes still mark publisher state dirty, and the pending publication is independently serviced on its timer/domain path rather than being advanced by every input sample.

### Surface-pacing domains

The native runtime now distinguishes:

- active pacing, used for diagnostics/existence only;
- pacing service due, admitted by a matured fallback/release deadline or readiness generation;
- Commit Timing planning due, admitted by its own immediate planning deadline;
- scene preparation due, admitted only when visible work is actually owned and actionable.

Commit Timing planning is `ProtocolOnly` and does not request primary rendering. Once a target is armed, the planning obligation disappears and the existing release/recheck deadline owns later pacing service.

### Interaction-local pointer update

An active compositor-owned move/resize uses exact pointer/cursor state updates without generic decoration hover discovery. Grabbed-surface targeting and pointer delivery remain intact. Normal hover/focus is restored at the existing interaction-terminal refresh boundary.

### Visual handoff after pacing

`CompositorState::progress_surface_pacing()` compares the active-scene render generation before and after the pacing pass. A visible publication returns `true`; hidden/protocol-only progress returns `false`. Native runtime ORs this result into `cycle.redraw_requested`, allowing the normal scheduler admission path to prepare and render newly visible state without retaining a perpetual blocked scene-work entry.

## C. Deterministic counters and operation evidence

The focused deterministic evidence is:

| Scenario | Result |
| --- | ---: |
| Independent input wakes in the domain seam | 1,000 input services, 0 Wayland read-dispatch services |
| Future pacing input wakes | 1,000 classifications, 0 pacing service calls |
| Clean Astrea publication gate | 0 reconcile calls, 0 prune passes |
| Stable cached pacing deadline | 1 deadline recomputation across two queries |
| Active interaction hover path | 0 generic scene-hit calls for local interaction updates |
| Native input batch flush ownership | state-only mutation does not flush; internal flush requests coalesce |
| Future Commit Timing scene work | 0 scene prepare work; independent planning deadline present |
| Pacing service result seam | visible-work handoff is preserved as `true` |

Existing aggregate telemetry was reused where available: input-only cycles, Wayland read-dispatch cycles, full server-tick calls, client flushes, pointer hit-test locality, Astrea gate/reconcile/prune metrics, and interaction-local hover avoidance.

The 1,000-wake proof is a deterministic native work-domain/service seam, not a real DRM/KMS measurement. No host CPU or GPU improvement is claimed from it.

## D. Correctness tests

Passing focused suites and tests include:

- `rtk cargo test --locked --lib surface_pacing -- --nocapture` — 24 passed
- `rtk cargo test --locked --lib frame_consumption_tests -- --nocapture` — 33 passed
- `rtk cargo test --locked --bin oblivion-one work_domains -- --nocapture` — 16 passed
- `rtk cargo test --locked --bin oblivion-one input -- --nocapture` — 120 passed
- `rtk cargo test --locked --lib shortcut_inhibition -- --nocapture` — 1 passed
- `rtk cargo test --locked --bin oblivion-one input_interaction_liveness -- --nocapture` — 6 passed
- `rtk cargo test --locked --lib window_interaction_tests -- --nocapture` — 65 passed
- `rtk cargo test --locked --lib xwayland_pointer_batch -- --nocapture` — 15 passed
- `rtk cargo test --locked --lib clean_publication_gate_skips_reconcile_and_pruning -- --nocapture` — 1 passed
- `rtk cargo test --locked --bin oblivion-one active_window_interaction_motion_updates_geometry_before_exact_client_dispatch -- --nocapture` — 1 passed
- `rtk cargo test --locked --bin oblivion-one native_input_batch_defers_and_coalesces_write_side_flushes -- --nocapture` — 1 passed
- `rtk cargo test --locked --lib interaction_pointer_motion_updates_local_state_without_generic_hover_hit_testing -- --nocapture` — 1 passed

The tests cover stable owners, future pacing, focus/toplevel gate behavior, active interaction ownership, cached deadlines, input/readiness domain separation, planning-only Commit Timing, and write-side flush batching.

## E. Full verification

Passed:

```text
rtk cargo fmt --all -- --check
rtk git diff --check
rtk cargo check --locked
```

`rtk cargo check --locked` completed with 0 errors and 7 existing dead-code warnings in unrelated input/runtime/debug helpers.

`rtk cargo clippy --all-targets --all-features -- -D warnings` remains blocked by 22 existing lint errors and 1 warning in unrelated dirty workspace/layout/compositor code. The reported diagnostics are outside the surface-pacing closure, including tiled layout, workspace protocol, fullscreen, and pre-existing test code.

The full `rtk cargo test --locked` run completed with 1,876 passed, 2 ignored, and 1 failure. The failure was:

```text
native::kms::tests::explicit_atomic_flip_adopts_out_fence_and_closes_input_after_success
```

It is an unrelated KMS test in `src/native/kms/tests.rs` and passed when rerun in isolation with:

```text
rtk cargo test --locked --lib explicit_atomic_flip_adopts_out_fence_and_closes_input_after_success -- --nocapture
```

No source changes were made to that KMS test or its implementation.

## F. Mandatory review pass 1 — correctness and ownership

1. Pure input does not set `wayland_dispatch`; native pointer motion therefore does not enter the broad tick.
2. Pointer events still flush promptly through the native input batch boundary.
3. Focus/toplevel changes still mark Astrea state dirty; independent publication service remains available.
4. Pending Astrea transactions are timer-serviced and are not advanced by raw input merely because they remain pending.
5. Astrea manager/handle/client lifecycle cleanup remains at explicit destroy and disconnect ownership boundaries.
6. Combined Wayland/input readiness performs one full readable-client tick and one input drain; the input branch does not add a duplicate tick.
7. Locked, confined, relative-pointer, button, and key paths retain their existing routing and constraint checks.
8. Interaction release continues to use the terminal pointer-focus/hover refresh path.
9. Scheduler-admitted interaction geometry remains outside `prepare_frame()` and raw input.
10. Direct Scanout, O1, KMS worker, explicit-sync ownership, and ready-frame lineage were not moved into input.
11. `OwnCompositorServer::tick()` remains available to direct synthetic/headless callers.
12. Pacing publication returns an active-scene-aware visual handoff rather than relying on removed blocked scene-work entries.

## G. Mandatory review pass 2 — adversarial performance/regression

The implementation was checked for the known false closures:

- no pointer wrapper or pure-input branch reintroduces `server.tick()`;
- the Astrea gate is before reconciliation/pruning;
- active transactions are not progressed from the input branch;
- state-only interaction geometry does not create a pre-pointer flush;
- the interaction-local branch does not enter generic decoration hit testing;
- clipboard, surface pacing, and publication are not copied into an input-only helper;
- Commit Timing planning is timer-owned and `ProtocolOnly`;
- pointer protocol flush remains independent of presentation;
- the 1,000-wake test exercises independent domain classifications rather than one batched state call;
- FIFO/frame ownership, restoration, fallback, and terminal teardown rebuild scene-work state;
- current dirty-tree Dwindle/workspace work and prior v1.1.1 scheduler/occlusion behavior were preserved.

## H. Real-host qualification status

Verified in deterministic tests:

- domain separation and input-only quiescence;
- pacing readiness/deadline ownership;
- planning-only Commit Timing classification;
- active-scene visual handoff seam;
- Astrea clean gating;
- interaction-local hover behavior;
- flush coalescing.

Verified on real KMS hardware: **not run in this session**.

The target host still needs qualification across stationary idle, 1000 Hz motion over light and Chromium/Electron clients, heavy multi-window workspaces, floating move/resize, Dwindle/tiled resize, pointer lock, software cursor, and XWayland. No CPU/GPU/frame-time improvement numbers are claimed.

## Remaining risks

- Native-only key/button input can still use the narrow full tick when pointer constraints may change; pointer motion does not use it.
- The full test suite contains an unrelated KMS assertion that is timing-sensitive in the aggregate run and passes in isolation.
- Real KMS/NVIDIA behavior and the residual contribution of Chromium/Electron client activity remain unqualified here.
- Existing dirty-tree clippy failures remain outside this closure.
