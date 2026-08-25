# Typhon Resource Efficiency v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close source-proven input/runtime resource waste and make pure pointer input quiescent without weakening protocol, cursor, KMS, explicit-sync, XWayland, pointer-constraint, or Dwindle correctness.

**Architecture:** Add retained input ownership, a shared lazy pointer-debug seam, generation-stable XWayland state, and a focused `NativeRuntime` work-domain decision/metrics layer. Preserve the existing compositor scene/hit authority and output transaction pipeline; only add locality or recipient reuse where existing generations and lifecycle callbacks prove it safe.

**Tech Stack:** Rust, Cargo locked builds, smithay/Wayland protocol resources, libinput, epoll/native reactor, KMS/Atomic output pipeline, existing unit/integration tests, procfs/perf where available, and `rtk` command wrappers.

## Global Constraints

- The dirty working tree is authoritative; do not reset, restore, checkout over, stash, clean, delete unrelated files, create a worktree, or run `cargo clean`.
- Stage only files owned by the current task; preserve all pre-existing modified, deleted, and untracked files.
- Do not add the Animation Engine, Dwindle v1.2 features, new input threads, hidden polling loops, a second pointer/scene/XWayland authority, or a raw DRM cursor shortcut.
- Preserve typed output transactions, pageflip generation validation, framebuffer lifetime, explicit-sync ownership, KMS worker ownership, Atomic cursor ownership, session sequencing, XWayland generation ownership, ActiveScene/SceneWorkIndex authority, damage correctness, pointer constraints, popup grabs, frame callbacks, and bounded histories.
- Use TDD for every independently testable behavior: write the focused failing test, run it, implement the smallest change, rerun the focused test, then commit the task-owned slice.
- Use the existing incremental Cargo target and `rtk` where available; never invent CPU/perf numbers or claim native hardware qualification unless it actually runs.

---

### Task 1: Add bounded resource-efficiency metrics and typed work decisions

**Files:**
- Create: `src/native_output/runtime/resource_efficiency.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/control_snapshots.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `src/native_output/tests/mod.rs`
- Test: `src/native_output/runtime/resource_efficiency.rs` unit tests and the snapshot serialization test in `src/control_snapshots.rs`

**Interfaces:**
- `ResourceEfficiencyMetrics::default()` owns plain `u64` counters and has mutation methods that perform no formatting, allocation, environment lookup, or I/O.
- `ResourceEfficiencyPerformanceSnapshot` is the serializable control-snapshot projection with fields for native/input/scene/cursor/protocol/XWayland/pacing/acquire/presentation evidence.
- `NativeWorkClass` has `NoOutputWork`, `ProtocolOnly`, `CursorOnly`, and `PrimaryScene` variants.
- `NativeWorkDecision` records the selected class plus boolean service decisions for XWayland, pacing, explicit-sync/acquire, primary scene, control, children, session, and shutdown.

- [ ] **Step 1: Write failing metric and classifier tests.**

Add tests that assert:

```rust
assert_eq!(NativeWorkClass::from_flags(false, false, false), NativeWorkClass::NoOutputWork);
assert_eq!(NativeWorkClass::from_flags(true, false, false), NativeWorkClass::ProtocolOnly);
assert_eq!(NativeWorkClass::from_flags(false, true, false), NativeWorkClass::CursorOnly);
assert_eq!(NativeWorkClass::from_flags(false, true, true), NativeWorkClass::PrimaryScene);
```

Also assert that incrementing every required counter appears unchanged in the hot-path metric object and that `PerformanceSnapshot` round-trips with the new field.

- [ ] **Step 2: Run the focused tests to verify the new API is absent.**

Run:

```bash
rtk cargo test --locked resource_efficiency
```

Expected: compilation/test failure because the new metric, decision, and snapshot types do not yet exist.

- [ ] **Step 3: Implement the metric and decision types.**

Add the module declaration and fields to `NativeRuntime`, `PerformanceSnapshot`, and the runtime snapshot builder. Keep all fields numeric and bounded. Do not add `String`, `Vec`, `format!`, `println!`, or environment access to counter update methods. Use `#[serde(rename_all = "camelCase")]` consistently with existing control snapshots.

- [ ] **Step 4: Run the focused tests to verify the slice passes.**

Run:

```bash
rtk cargo test --locked resource_efficiency
rtk cargo test --locked snapshot_objects_are_deserializable_and_require_all_fields
```

Expected: PASS.

- [ ] **Step 5: Commit the scoped slice.**

```bash
rtk git add src/native_output/runtime/resource_efficiency.rs src/native_output/runtime/mod.rs src/native_output/runtime/metrics.rs src/control_snapshots.rs src/native_output/tests/mod.rs
rtk git commit -m "feat(native): add resource efficiency metrics"
```

### Task 2: Close disabled pointer-diagnostic and libinput device-key allocations

**Files:**
- Create: `src/pointer_debug.rs`
- Modify: `src/lib.rs`
- Modify: `src/compositor/state/support_types.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/input/state.rs`
- Modify: `src/native_output/input/routing.rs`
- Modify: `src/native_output/input/mod.rs`
- Test: `src/pointer_debug.rs`, `src/native_output/input/routing.rs`, and existing native input tests

**Interfaces:**
- `crate::pointer_debug::enabled() -> bool` is the single cached production enablement check.
- `crate::pointer_debug::log_lazy(message: impl FnOnce() -> String)` evaluates the closure only when enabled.
- `hardware_input_event_from_libinput(event, output_width, output_height, scroll_v120_remainders)` no longer receives an always-owned device key; only wheel/v120 branches borrow `event.device().sysname()` and insert a key when state is first created.

- [ ] **Step 1: Write failing laziness and device-identity tests.**

Add a pure formatter-seam test that sets a `Cell<bool>` inside a disabled closure and asserts it remains false. Add a device-state test that repeatedly looks up an existing v120 remainder without changing the map length and keeps separate horizontal/vertical remainders per device. Add a conversion-level test seam proving motion conversion has no device-key argument.

- [ ] **Step 2: Run focused tests and capture the expected failure.**

```bash
rtk cargo test --locked pointer_debug
rtk cargo test --locked wheel_remainders_are_independent_per_axis
```

Expected: the new formatter seam/signature tests fail before implementation.

- [ ] **Step 3: Implement the shared debug seam.**

Move cached `TYPHON_POINTER_DEBUG` enablement into `src/pointer_debug.rs`. Route compositor `pointer_debug_enabled`, `pointer_debug_log`, and `pointer_debug_log_lazy` through it. Change native pointer/relative/constraint callers, including `NativeInputState::handle_pointer_motion`, to pass closures so disabled paths do not execute `format!`. Preserve enabled output text and the existing test seam.

- [ ] **Step 4: Implement borrowed libinput identity.**

Remove the `device_key: &str` parameter from `hardware_input_event_from_libinput`. Keep motion, absolute motion, button, finger, and continuous events independent of device identity. In the wheel branch, borrow the sysname only long enough to call the per-device remainder entry. Keep device add/remove and suspend/resume cleanup unchanged.

- [ ] **Step 5: Run the focused input suite.**

```bash
rtk cargo test --locked pointer_debug
rtk cargo test --locked native_input
rtk cargo test --locked wheel_remainders
```

Expected: PASS, with exact pointer/relative behavior unchanged.

- [ ] **Step 6: Commit the scoped slice.**

```bash
rtk git add src/pointer_debug.rs src/lib.rs src/compositor/state/support_types.rs src/native_output/runtime/frame.rs src/native_output/runtime/mod.rs src/native_output/input/state.rs src/native_output/input/routing.rs src/native_output/input/mod.rs
rtk git commit -m "perf(input): make disabled pointer diagnostics allocation-free"
```

### Task 3: Retain input batch storage without changing event ordering

**Files:**
- Create: `src/native_output/input/batch.rs`
- Modify: `src/native_output/input/mod.rs`
- Modify: `src/native_output/input/routing.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/native_output/tests/frame.rs`
- Modify: `src/native_output/tests/input.rs`
- Test: `src/native_output/input/batch.rs` and existing coalescing tests

**Interfaces:**
- `NativeInputBatch` owns retained `raw` and `coalesced` vectors, a maximum drain budget, and capacity inspection used only by tests/control metrics.
- `NativeInputBatch::clear()` resets logical lengths without releasing capacity.
- `NativeInputBackend::drain_events_into(&mut self, batch: &mut NativeInputBatch)` fills retained storage and enforces the existing hard drain budget.
- `coalesce_pointer_motion_events_into(batch: &mut NativeInputBatch)` preserves the current motion flush rules and does not allocate in steady state after warm-up.

- [ ] **Step 1: Write failing reuse/order tests.**

Test repeated batches for stable capacity, absolute-motion latest-value behavior, legal relative coalescing, button/axis/key boundaries, non-motion flushes, and the existing maximum drain budget. Assert that the second warm batch has the same capacities as the first warm batch.

- [ ] **Step 2: Run the focused tests to establish red.**

```bash
rtk cargo test --locked native_input_coalesces
rtk cargo test --locked retained_input_batch
```

Expected: retained-batch tests fail before the new type/API exists.

- [ ] **Step 3: Implement retained storage.**

Move the current coalescing state machine into `NativeInputBatch`, reserve only up to the existing bounded drain budget, and replace `Vec` transfer/`Vec::with_capacity` churn in `dispatch_wayland_and_input` with `drain_events_into` plus retained coalescing. Do not drop motion samples across a non-motion boundary and do not alter relative/absolute semantics.

- [ ] **Step 4: Handle session transitions.**

Clear logical batch lengths when input is suspended, resumed, or discarded so stale events cannot cross a session boundary. Keep the retained allocation bounded and reusable after resume.

- [ ] **Step 5: Run the focused batch and existing input tests.**

```bash
rtk cargo test --locked retained_input_batch
rtk cargo test --locked native_input_coalesces
rtk cargo test --locked native_input_coalescing_preserves_button_boundaries
rtk cargo test --locked input
```

Expected: PASS.

- [ ] **Step 6: Commit the scoped slice.**

```bash
rtk git add src/native_output/input/batch.rs src/native_output/input/mod.rs src/native_output/input/routing.rs src/native_output/runtime/mod.rs src/native_output/runtime/cycle_dispatch.rs src/native_output/tests/frame.rs src/native_output/tests/input.rs
rtk git commit -m "perf(input): retain native event batch storage"
```

### Task 4: Make XWayland environment and reactor synchronization generation-stable

**Files:**
- Modify: `src/xwayland/mod.rs`
- Modify: `src/xwayland/service.rs`
- Modify: `src/xwayland/service_state.rs`
- Modify: `src/xwayland/service_support.rs`
- Modify: `src/xwayland/metrics.rs`
- Modify: `src/xwayland/tests.rs`
- Modify: `src/native_output/runtime/xwayland_reactor.rs`
- Modify: `src/native_output/runtime/xwayland_reactor_tests.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/native_output/launch.rs`
- Modify: `src/native_output/input/routing.rs`
- Test: XWayland service/reactor tests and launch-environment tests

**Interfaces:**
- `XwaylandService::app_environment(&self) -> Option<&XwaylandAppEnvironment>` and `normal_app_environment(&self) -> Option<&XwaylandAppEnvironment>` borrow a generation-stable cached value.
- `XwaylandService::reactor_registration_generation(&self) -> u64` returns the monotonic interest generation.
- `sync_xwayland_reactor_sources` accepts the last-synced generation and returns whether actual registration reconciliation occurred; unchanged generation returns before desired-set construction.
- Spawn APIs clone `XwaylandAppEnvironment` only when a launch is actually being created.

- [ ] **Step 1: Write failing environment and generation tests.**

Add tests showing two reads within one lease/generation point to the same cached state without rebuilding it, a generation/lease change publishes new display/auth data, and no pending launch requests or clones an environment. Add reactor tests for unchanged generation no-op, add/remove, writable-interest change, restart, and teardown.

- [ ] **Step 2: Run focused XWayland tests to establish red.**

```bash
rtk cargo test --locked xwayland::tests
rtk cargo test --locked xwayland_reactor
```

Expected: new borrow/generation assertions fail against the current eager materialization/reconciliation.

- [ ] **Step 3: Add cached app-environment ownership.**

Store the environment with its lease/generation boundary in `XwaylandService`, refresh it exactly when a new lease is published, and clear it when the generation becomes unavailable. Change input/runtime contexts to borrow the environment; clone only inside the actual spawn path. Guard the normal pending-launch drain so stable pointer cycles with an empty launch queue do not request the environment.

- [ ] **Step 4: Add registration-interest generation and counters.**

Advance the service generation for every semantic registration/writable-interest mutation. Preserve token ownership and `finish_reactor_teardown`. Add XWayland sync-request, actual-reconcile, unchanged-skip, and environment-materialization counters to the resource snapshot.

- [ ] **Step 5: Implement O(1) unchanged-generation sync.**

Store the last synced service generation in `NativeRuntime`. Return before `reactor_registrations().collect()`, `tokens.drain(..)`, `contains`, or `any` work when unchanged. On change, reconcile exact registrations, unregister stale tokens, register additions, preserve writable flags, and finish teardown once.

- [ ] **Step 6: Run the focused service, reactor, and launch tests.**

```bash
rtk cargo test --locked xwayland
rtk cargo test --locked xwayland_reactor
rtk cargo test --locked binding_launch
```

Expected: PASS, including stale-generation and teardown coverage.

- [ ] **Step 7: Commit the scoped slice.**

```bash
rtk git add src/xwayland/mod.rs src/xwayland/service.rs src/xwayland/service_state.rs src/xwayland/service_support.rs src/xwayland/metrics.rs src/xwayland/tests.rs src/native_output/runtime/xwayland_reactor.rs src/native_output/runtime/xwayland_reactor_tests.rs src/native_output/runtime/mod.rs src/native_output/runtime/cycle.rs src/native_output/runtime/cycle_dispatch.rs src/native_output/launch.rs src/native_output/input/routing.rs
rtk git commit -m "perf(xwayland): reuse stable generation state"
```

### Task 5: Gate native runtime domains and prove pure-input quiescence

**Files:**
- Create: `src/native_output/runtime/work_domains.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/native_output/runtime/cycle/pageflip.rs`
- Modify: `src/native_output/runtime/xwayland.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `src/compositor/server_frames.rs`
- Modify: `src/compositor/state/surface_pacing.rs`
- Modify: `src/native_output/tests/frame.rs`
- Test: `src/native_output/runtime/work_domains.rs` and native runtime tests

**Interfaces:**
- `NativeWorkDomains::from_wakeup(&NativeWakeup, &NativeRuntimeState) -> NativeWorkDecision` maps reactor readiness/deadlines and state generations to independently serviceable domains.
- `NativeRuntime::should_progress_surface_pacing(now_ns) -> bool` is true only for active pacing/transaction state or a due pacing deadline.
- `NativeRuntime::should_process_acquire_and_prepare(&NativeCycleState) -> bool` is true for actual output/scene work, explicit-sync readiness, required recovery, or a due presentation deadline.
- `NativeCycleState` records the selected `NativeWorkClass` and fast-path completion for metrics.

- [ ] **Step 1: Write failing domain-combination tests.**

Cover these exact cases:

```text
input + stable hardware cursor -> NoOutputWork / pure-input completion
input + explicit-sync readiness -> acquire service required
input + pacing deadline -> pacing service required
input + XWM readiness -> XWayland service required
input + control/child/session readiness -> those domains serviced
input + cursor-only due -> CursorOnly
input + scene dirty or interaction -> PrimaryScene
```

Pair every class assertion with an exact event/cursor/protocol correctness assertion in the existing test harness.

- [ ] **Step 2: Run focused work-domain tests to establish red.**

```bash
rtk cargo test --locked work_domains
rtk cargo test --locked input_readiness
```

Expected: new decision tests fail because the runtime has no domain classifier.

- [ ] **Step 3: Implement readiness/dirty-domain classification.**

Create the focused classifier from `WakeReasons`, explicit-sync token lists, timer/deadline facts, pending launch/interaction/session/shutdown state, cursor mode, and existing redraw/scene state. Keep it pure where possible so combinations are deterministic and allocation-free.

- [ ] **Step 4: Gate unrelated cycle work.**

In `run_cycle`, service children only for child readiness, XWayland events only for XWayland readiness or a relevant due/generation transition, control only for control readiness, and pacing only when `should_progress_surface_pacing` is true. Keep the input/protocol dispatch path active for input readiness and preserve required Wayland flush/progress.

- [ ] **Step 5: Gate acquire/prepare and presentation.**

Run acquire/prepare only for the classifier’s primary/cursor/explicit-sync/recovery decisions. Return after protocol-only or no-output work once all due domains are serviced. Do not bypass pageflip, Atomic, explicit-sync, worker, frame-callback, or shutdown validation.

- [ ] **Step 6: Add aggregate counter updates.**

Increment native-cycle, input-ready, pure-input, protocol-only, primary-attempt, cursor-only, pacing, acquire/prepare, and presentation-planning counters at domain boundaries. Counter updates must remain numeric and non-allocating.

- [ ] **Step 7: Run focused quiescence and regression tests.**

```bash
rtk cargo test --locked work_domains
rtk cargo test --locked native_input
rtk cargo test --locked pointer_scene_hit
rtk cargo test --locked tiled_resize
rtk cargo test --locked frame
```

Expected: PASS, with ordinary pointer motion producing no Dwindle solve, configure, primary scene render, pacing scan, acquire/prepare, or stable XWayland reconcile.

- [ ] **Step 8: Commit the scoped slice.**

```bash
rtk git add src/native_output/runtime/work_domains.rs src/native_output/runtime/mod.rs src/native_output/runtime/cycle.rs src/native_output/runtime/cycle_dispatch.rs src/native_output/runtime/cycle/pageflip.rs src/native_output/runtime/xwayland.rs src/native_output/runtime/metrics.rs src/compositor/server_frames.rs src/compositor/state/surface_pacing.rs src/native_output/tests/frame.rs
rtk git commit -m "perf(native): quiesce unrelated runtime domains"
```

### Task 6: Keep pointer hit testing local and generation-correct

**Files:**
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/compositor/state/input_dispatch.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/state/scene_work.rs`
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/pointer_constraints.rs`
- Modify: `src/compositor/state/window_decoration_tests.rs`
- Modify: `src/compositor/state/task_05_8_tests.rs`
- Test: existing hit-testing and pointer invalidation suites

**Interfaces:**
- `PointerInputMetrics` gains `full_scene_hit_scans`, `owner_locality_fast_hits`, and invalidation counters keyed by explicit causes.
- Any owner fast path stores the current scene render generation, pointer-hit generation, and authoritative owner/index identity; it returns to `pointer_scene_hit_uncached` unless containment and all generations match.
- No owner cache survives map/unmap/destroy, input-region, stack/popup/grab, workspace/Special, Dwindle geometry, transform/scale, or pointer-constraint changes.

- [ ] **Step 1: Write failing metric/invalidation tests.**

Extend the current pointer-scene tests to assert full-scan/locality counters and cover overlap crossing, input-region mutation, map/unmap/destroy, popup/grab changes, workspace/Special changes, Dwindle geometry changes, and lock/confine transitions.

- [ ] **Step 2: Run focused hit tests to establish red.**

```bash
rtk cargo test --locked pointer_scene_hit
rtk cargo test --locked pointer_constraint
```

Expected: the new counters/invalidation assertions fail before implementation.

- [ ] **Step 3: Use existing scene/index authority only.**

Remove any avoidable `surface_id` linear lookup if `ActiveScene`/`SceneWorkIndex` already exposes the exact current index. If a containment proof is available without duplicating input regions, add the generation-bound fast path; otherwise keep the exact-coordinate cache and add instrumentation only.

- [ ] **Step 4: Verify every invalidation boundary.**

Route all relevant existing generation bumps through the cache invalidation helper. Assert that a fast hit preserves exact local coordinates, target surface, decoration result, focus recipient, and popup/constraint semantics.

- [ ] **Step 5: Run pointer, decoration, popup, workspace, and Dwindle tests.**

```bash
rtk cargo test --locked pointer_scene_hit
rtk cargo test --locked window_decoration
rtk cargo test --locked popup
rtk cargo test --locked special_workspace
rtk cargo test --locked tiled_layout
```

Expected: PASS with no unsafe same-focus shortcut.

- [ ] **Step 6: Commit the scoped slice.**

```bash
rtk git add src/compositor/state/hit_testing.rs src/compositor/state/input_dispatch.rs src/compositor/state/input_resources.rs src/compositor/state/active_scene.rs src/compositor/state/scene_work.rs src/compositor/state/windows.rs src/compositor/state/pointer_constraints.rs src/compositor/state/window_decoration_tests.rs src/compositor/state/task_05_8_tests.rs
rtk git commit -m "perf(pointer): bound hit-test locality by generations"
```

### Task 7: Remove locked-relative per-sample recipient churn

**Files:**
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/input_dispatch.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/compositor/state/pointer_constraints.rs`
- Modify: `src/compositor/state/client_lifecycle.rs`
- Modify: `src/compositor/state/support_types.rs`
- Modify: `src/compositor/tests/support/locked_relative.rs`
- Modify: `src/compositor/tests/relative_pointer.rs`
- Test: locked-relative synthetic stream tests

**Interfaces:**
- `relative_pointer_resources_generation: u64` advances on add/remove/death/client cleanup.
- `LockedRelativeRecipientCache` is keyed by resource generation, active constraint generation, surface identity, and source-pointer identity; cached recipients are rebuilt only on those lifecycle changes.
- `send_relative_pointer_motion` preserves exact `timestamp_usec`, accelerated/unaccelerated deltas, event order, and one frame per source pointer while avoiding per-sample resource-vector/recipient/frame-vector allocation.

- [ ] **Step 1: Write failing high-rate semantic tests.**

Extend `locked_relative.rs` with deterministic streams of 500, 1000, 2000, 4000, and 8000 samples. Assert delivered count/order/timestamps/delta tracks, bounded recipient-cache capacity, and stable lock/focus ownership. Add tests for resource destruction and lock/confinement transitions invalidating the cache.

- [ ] **Step 2: Run the focused locked-relative tests to establish red.**

```bash
rtk cargo test --locked locked_relative
rtk cargo test --locked relative_pointer
```

Expected: new bounded-cache assertions fail against per-sample clone/vector construction.

- [ ] **Step 3: Add explicit lifecycle generations.**

Increment the resource generation in all existing resource registration/removal/client-lifecycle paths. Include active constraint generation and surface identity in the cache key. Do not infer validity from focus alone.

- [ ] **Step 4: Implement cached recipient dispatch.**

Build recipient/source-pointer sets only after invalidation, then borrow the cached set for delivery. Preserve same-client filtering, source-pointer protocol identity, deduplication, frame emission, debug-disabled behavior, and drop reasons. Keep stale-resource cleanup bounded and lifecycle-driven.

- [ ] **Step 5: Run semantic and existing pointer-constraint tests.**

```bash
rtk cargo test --locked locked_relative
rtk cargo test --locked relative_pointer
rtk cargo test --locked pointer_constraint
rtk cargo test --locked window_interaction
```

Expected: PASS with no dropped/reordered relative samples.

- [ ] **Step 6: Commit the scoped slice.**

```bash
rtk git add src/compositor/mod.rs src/compositor/state/input_dispatch.rs src/compositor/state/input_resources.rs src/compositor/state/pointer_constraints.rs src/compositor/state/client_lifecycle.rs src/compositor/state/support_types.rs src/compositor/tests/support/locked_relative.rs src/compositor/tests/relative_pointer.rs
rtk git commit -m "perf(pointer): reuse locked relative recipients"
```

### Task 8: Audit stable-frame allocations, bounded ownership, and Dwindle/software-cursor regressions

**Files:**
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Modify: `src/native_output/runtime/presentation_cursor.rs`
- Modify: `src/native_output/runtime/presentation_ready.rs`
- Modify: `src/native_output/runtime/presentation_pipeline.rs`
- Modify: `src/native_output/runtime/cursor_cycle.rs`
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/state/scene_work.rs`
- Modify: `src/compositor/state/tiled_layout.rs`
- Modify: `src/native_output/tests/frame.rs`
- Modify: `src/native_output/tests/special_workspace.rs`
- Modify: `src/native_output/tests/mod.rs`
- Test: stable-frame, software-cursor, Dwindle, hidden-workspace/Special, and bounded-state tests

**Interfaces:**
- Stable scene/presentation data is rebuilt only for its owning generation or a semantic visual dirty mark.
- Cursor-only planning remains a `PlaneDelta`/cursor-only decision where the existing planner proves it safe; software cursor damage is old-rect union new-rect.
- Input-only counters show zero primary scene snapshot/prepare/render and zero Dwindle solves/configures.

- [ ] **Step 1: Write failing audit/regression tests.**

Assert ordinary pointer motion does not rebuild `ActiveScene`/`SceneWorkIndex`, solve Dwindle, configure windows, or build a primary frame. Force software cursor and assert old/new cursor damage only. Activate hidden Regular/Special locations and assert no hidden tree work occurs without a semantic dirty generation. Add bounded-capacity assertions for retained scratch and existing histories.

- [ ] **Step 2: Run focused tests to establish red.**

```bash
rtk cargo test --locked frame
rtk cargo test --locked tiled_layout
rtk cargo test --locked special_workspace
```

Expected: new audit counters/assertions fail until the audited gates are complete.

- [ ] **Step 3: Remove stable-frame rebuilds caused only by input.**

Use existing scene/work/presentation generations and dirty flags. Do not add a permanent unbounded cache. Preserve the parse-partial `presentation_cycle.rs` line by reading the source directly before editing and verifying its surrounding ownership manually.

- [ ] **Step 4: Verify cursor and Dwindle contracts.**

Run the existing planner, cursor-cycle, software-cursor, Dwindle resize/coalescing, workspace/Special, and window configure suites. Fix only task-owned regressions.

- [ ] **Step 5: Commit the scoped slice.**

```bash
rtk git add src/native_output/runtime/frame.rs src/native_output/runtime/presentation_cycle.rs src/native_output/runtime/presentation_cursor.rs src/native_output/runtime/presentation_ready.rs src/native_output/runtime/presentation_pipeline.rs src/native_output/runtime/cursor_cycle.rs src/compositor/state/active_scene.rs src/compositor/state/scene_work.rs src/compositor/state/tiled_layout.rs src/native_output/tests/frame.rs src/native_output/tests/special_workspace.rs src/native_output/tests/mod.rs
rtk git commit -m "perf(native): preserve stable frame ownership"
```

### Task 9: Run validation, independent reviews, profiling/memory checks, and write the final report

**Files:**
- Create: `REPORT-2026-08-24-typhon-resource-efficiency-v1.md`
- Modify: task-owned files only if review finds a defect
- Test: all focused suites and repository validation commands

**Interfaces:**
- The report records actual baseline HEAD, initial dirty-tree boundary, source findings, pre-existing fixes, counters, before/after evidence, exact commands, tests, review findings, unavailable environment failures, remaining bottlenecks, and Animation Engine readiness.

- [ ] **Step 1: Record the final task boundary before validation.**

Run with `rtk`:

```bash
rtk git status --short --branch
rtk git log -12 --oneline
rtk git diff --stat
```

Compare against the initial baseline `9d3fb34b45f6ce4ffc4582c3231e220b3643e959` and record only task-owned commits/files in the report.

- [ ] **Step 2: Run the full required validation.**

```bash
rtk cargo fmt --check
rtk cargo check --locked
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
rtk run "bin/check-source-layout"
```

Record exact failures. Keep known pre-existing SUN_LEN/PoisonError/missing-library environment failures documented rather than weakening tests.

- [ ] **Step 3: Run available runtime evidence without verbose hot-path logs.**

If native Linux/TTY execution is available, collect the specified idle, stable-window, XWayland-off/eager, boundary-crossing, software-cursor, locked-relative, floating-move, and tiled-resize cases. Use the existing control snapshot and the running Typhon process's `/proc/$PID/smaps_rollup`. If `perf` is available, run the requested `perf stat` counters and only use `perf record` for attribution. If unavailable, state that CPU/165 Hz/hardware qualification was not executed.

- [ ] **Step 4: Perform Review Pass 1 — correctness/ownership/protocol safety.**

Check explicit-sync and pacing due work, Wayland flush/progress, XWayland readiness/writable transitions/stale tokens/generation reuse, wheel remainder isolation, suspend/resume batch clearing, hit-cache invalidations, pointer lock/confinement, relative sample ordering/timestamps, cursor Atomic ownership, software damage, Dwindle coalescing, and bounded counters. Fix every task-owned issue and rerun focused tests.

- [ ] **Step 5: Perform Review Pass 2 — hot path/165 Hz/high-poll efficiency.**

Search task-owned ordinary motion paths with `rtk rg` for `format!`, `to_string`, `to_owned`, fresh `Vec`, `collect`, `std::env`, XWayland reconciliation, pacing scans, acquire/prepare, primary scene snapshot work, recipient reconstruction, and retained capacity growth. Fix every task-owned issue and rerun focused performance/coalescing tests.

- [ ] **Step 6: Perform Review Pass 3 — root-cause challenge.**

Answer from counters/profile evidence whether Phase 1 waste closed, broad cycle cost remains, work moved elsewhere, hit testing dominates, XWayland is quiet, cursor submissions are bounded, relative input is exact, remaining CPU is Typhon-owned, latency/deadlines changed, and benchmark claims are executed. Update conclusions when evidence contradicts the initial hypothesis.

- [ ] **Step 7: Write and self-review the final report.**

Create `REPORT-2026-08-24-typhon-resource-efficiency-v1.md` in English. Include all 39 requested report sections, exact commands, focused/full results, source-layout result, environment blockers, three review outcomes, final status, measured bottlenecks, and either `Animation Engine foundation is resource-ready` or a specific measured blocker.

- [ ] **Step 8: Commit the report and final task-owned fixes.**

Run `rtk git status --short` and inspect `rtk git diff --unified=0`. Add the new report and new task-owned files explicitly, then use `rtk git add -p` on modified files to select only hunks introduced by this task. Do not add an entire already-dirty path when it contains pre-existing user work. After `rtk git diff --cached --check` and a cached diff review pass, commit with:

```bash
rtk git commit -m "feat: close Typhon resource efficiency v1"
```

Never stage pre-existing dirty files or unrelated hunks.

## Plan self-review

- Spec coverage: Tasks 1–3 cover Phase 0 and Phase 1A–1C; Task 4 covers Phase 1D–1E; Task 5 covers Phase 3 and work-class semantics; Task 6 covers Phase 2; Task 7 covers Phase 4; Task 8 covers Phase 5, cursor/software/Dwindle interactions, and bounded state; Task 9 covers Phase 6, profiling, validation, all three review passes, and the required report.
- Placeholder scan: the plan contains no `TBD`, `TODO`, `FIXME`, or implementation hand-waves. The final staging command explicitly requires a verified, enumerated path list from the final status check.
- Type consistency: `ResourceEfficiencyMetrics`, `ResourceEfficiencyPerformanceSnapshot`, `NativeWorkClass`, `NativeWorkDecision`, `NativeInputBatch`, `app_environment` borrowed accessors, and registration-generation accessors are introduced before dependent tasks use them. The exact existing output and protocol types remain unchanged.
- Scope check: all files listed are directly connected to the approved resource-efficiency design; no animation, Dwindle v1.2, new thread, or unrelated cleanup is included.
