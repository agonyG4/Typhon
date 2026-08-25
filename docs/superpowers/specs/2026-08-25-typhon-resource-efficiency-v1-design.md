# Typhon Resource Efficiency v1 Design

## Goal

Reduce Typhon-owned cost for high-rate pointer input by making runtime work proportional to semantic readiness and visual work, while preserving input, Wayland, XWayland, explicit-sync, KMS/Atomic, cursor, pointer-constraint, and Dwindle correctness.

This milestone does not add the Animation Engine or Dwindle v1.2 behavior. The current dirty checkout is authoritative; existing user changes remain untouched and task commits stage only task-owned files.

## Current evidence

The current native path has several source-proven costs:

- `NativeInputState::handle_pointer_motion` eagerly formats a diagnostic before `native_pointer_debug_log` checks `TYPHON_POINTER_DEBUG`.
- Libinput conversion receives a device key and calls `to_owned()` for wheel state even for events that do not need per-device state.
- `NativeInputBackend::drain_events` transfers a fresh raw vector and `coalesce_pointer_motion_events` allocates a second vector.
- `XwaylandService::app_environment` reconstructs owned `DISPLAY` and `XAUTHORITY` values on every read.
- `sync_xwayland_reactor_sources` rebuilds desired/retained vectors and performs linear membership scans on every call.
- `NativeRuntime::run_cycle` dispatches unrelated XWayland, pacing, acquire/prepare, and presentation work around input processing without a single readiness/dirty-domain decision.
- Relative locked-pointer routing clones resource vectors and builds recipient/frame vectors per sample.
- `PointerSceneHitCache` is generation-bound but exact-coordinate-only; it will not be replaced with an unsafe focus shortcut.

The codebase already has a cached/lazy compositor pointer-debug seam, generation-bound pointer-hit state, typed output transactions, Atomic/KMS ownership, Dwindle layout authority, and extensive locked-pointer and resize regression tests. Those existing authorities remain the source of truth.

## Architecture

### 1. Resource-efficiency metrics and work classification

Add a focused native runtime metrics/work-domain module. It will own plain integer counters for native cycles, input/raw/coalesced events, pointer samples, primary attempts/renders/submits, cursor-only opportunities/submits, protocol-only and pure-input completions, hit-test locality/full scans, XWayland sync requests/reconciliations/unchanged skips, environment materializations, pacing progression, acquire/prepare runs/skips, and presentation planning runs/skips.

The runtime will classify each wake using reactor readiness reasons, explicit dirty generations, due deadlines, pending launches, interaction state, session/shutdown state, and output ownership. The decision must preserve independent domains such as Input, WaylandProtocol, Scene, Cursor, Presentation, ExplicitSync, SurfacePacing, XWayland, Control, Children, Session, and Shutdown.

The common hardware-cursor input-only case will therefore be:

```text
wait -> service ready input/protocol work -> update latest cursor -> prove all other domains quiescent -> return
```

The classifier will never skip a domain that is actually ready or due. A pure-input completion is valid only after explicit-sync, pacing, XWayland, control, child, session, interaction, scene, and output conditions have been checked.

### 2. Input fast path

Introduce retained batch/scratch ownership at the native input boundary. Logical lengths are cleared between wakes while capacity is retained, a hard drain budget remains enforced, and motion is coalesced in place or into a second retained buffer. Non-motion boundaries flush pending motion exactly as current semantics require. Capacity is bounded by a tested high-water policy.

Split libinput device identity from ordinary motion conversion. Relative/absolute motion and buttons do not own a sysname. Wheel/v120 conversion borrows the sysname and allocates a map key only on first insertion for a device-specific remainder state. Horizontal/vertical remainder independence, device add/remove, and suspend/resume semantics remain unchanged.

Unify native and compositor pointer diagnostics around cached enablement and lazy formatting. Disabled diagnostics must evaluate neither their message closure nor any formatting/allocation work.

### 3. Stable XWayland state

`XwaylandService` will own one generation-stable application environment. Read-only runtime paths borrow it, and the process-spawn boundary clones only when the spawn API requires ownership. The environment is published/replaced with the XWayland lease/generation and is not requested when no launch is pending.

The service will also own a monotonic registration-interest generation. Any registration-set or writable-interest mutation advances it, including startup, running, restart, teardown, and writable transitions. `NativeRuntime` remembers the last synchronized generation; an unchanged generation is an O(1) no-op, while a changed generation reconciles registrations and cleans stale tokens before finishing teardown. Existing listener, displayfd, XWM, stderr, restart, backoff, and failure ownership remains intact.

### 4. Cursor, scene, and protocol behavior

Hardware cursor movement remains a latest-state cursor output class governed by Atomic/KMS ownership and existing transaction arbitration. It may piggyback on an already-needed primary commit, but it cannot bypass pageflip generation, framebuffer lifetime, explicit-sync, or worker ownership. Software cursor movement keeps old/new cursor damage local and collapses positions to useful presentation opportunities.

Pointer hit testing keeps precise coordinates and current scene/input generations. The existing exact cache remains valid. A locality/index optimization is allowed only where the current ActiveScene/SceneWorkIndex and input-region/stack authorities provide a containment and invalidation proof. Required invalidations include overlap/stack, input-region, map/unmap/destroy, popup/grab, decorations, transforms/scales, workspace/Special, Dwindle geometry, pointer constraints, and compositor interactions. If no safe owner region exists, the implementation will instrument and retain the current full scan rather than create a parallel region engine.

Locked relative-pointer delivery will use explicit resource/constraint generations and stable recipient ownership where the current lifecycle permits. It will remove per-sample vector cloning, recipient reconstruction, and duplicate searches without merging timestamps or accelerated/unaccelerated deltas, reordering events, dropping meaningful samples, or weakening lock/confinement and interaction suppression semantics.

### 5. Stable-frame and memory audit

After the input/runtime path is controlled, audit stable scene preparation, decoration/popup/surface snapshots, damage, presentation planning, ActiveScene/SceneWorkIndex projections, and hidden workspace/Special state. Non-visual input must not rebuild stable scene data. Any retained storage has an explicit bounded ownership or generation boundary.

When procfs is available, qualification records PSS, private clean/dirty, and anonymous RSS. Tests and counters cover input scratch, cursor ownership, scene/presentation histories, XWayland retired generations, explicit-sync watches, and telemetry bounds. No monotonic task-owned growth is accepted after warm-up.

## Data flow and correctness gates

```text
reactor wake
  -> collect readiness/deadline facts
  -> service only ready/due non-input domains
  -> drain retained input batch
  -> deliver exact pointer/relative/protocol events
  -> update latest cursor state
  -> classify output work
       NoOutputWork      -> return to reactor
       ProtocolOnly      -> preserve flush/progress, then return
       CursorOnly        -> bounded cursor output through existing ownership
       PrimaryScene      -> existing acquire/prepare/render/present path
```

Every fast-path test pairs efficiency with correctness: exact coordinates/timestamps, protocol recipients and focus, cursor state, and required domain service are asserted alongside no-render/no-reconcile/no-scan counters. Tests explicitly cover input plus due explicit-sync, pacing, XWM, control, child, session, cursor-only, and primary-scene work.

## Testing and validation

Implementation proceeds in red/green slices:

1. formatter laziness and cached enablement;
2. borrowed device identity and wheel-state reuse;
3. retained batch ordering, coalescing, capacity, and budget;
4. stable XWayland environment and registration-generation transitions;
5. work-domain classifier and pure-input completion proof;
6. pointer locality/index boundaries and invalidations;
7. locked-relative synthetic 500/1000/2000/4000/8000 Hz semantic streams;
8. Dwindle/layout and software-cursor regressions;
9. stable-frame allocation and bounded-memory evidence.

Focused tests run after each slice. Final validation uses the existing incremental target via `rtk` where available:

```bash
rtk cargo fmt --check
rtk cargo check --locked
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

The source-layout gate and any available native/perf/procfs qualification are also run. CPU parity, 165 Hz hardware behavior, and Hyprland comparisons are reported only when actually executed under matched conditions.

## Review and delivery

Before finalizing, perform three independent passes:

1. correctness/ownership/protocol safety;
2. hot-path/allocation/165 Hz/high-poll efficiency;
3. root-cause challenge against counters and executed profiles.

Create `REPORT-2026-08-24-typhon-resource-efficiency-v1.md` in English with baseline, dirty-tree boundary, source findings, implementation evidence, focused/full validation, exact commands, unavailable environment qualifications, all review findings/fixes, remaining bottlenecks, and an evidence-backed Animation Engine readiness recommendation.

The design commit contains only this spec. The implementation commit contains only task-owned source/tests/report changes; existing dirty files remain unstaged.
