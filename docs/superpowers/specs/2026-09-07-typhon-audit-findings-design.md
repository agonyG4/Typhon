# Typhon Confirmed Audit Fixes Design

## Goal

Implement the four confirmed correctness findings from the Typhon performance/correctness audit without changing ownership, scheduling, buffering, synchronization, or presentation policy.

## Scope and constraints

The work is deliberately limited to XWayland backend-command delivery, render-fence timing evidence, KMS worker dispatch-budget training, and the worker-owned pageflip watchdog. Existing retry counts/backoffs, queue capacities, guards, buffering heuristics, Direct Scanout policy, shutdown ownership, and explicit-sync ownership remain unchanged. The existing checkout and target directory are reused.

## Architecture

### 1. XWayland command continuation

`CompositorState` will expose a non-destructive predicate that recognizes only commands consumable by the managed XWayland runtime. Window-scoped commands count only when their window is X11-backed; workspace publication counts as XWayland work because it is translated into an XWM command. The predicate will be surfaced through `OwnCompositorServer` and included in `NativeRuntimeState`/`NativeWorkDomains`.

The normal runtime wake plan will carry the predicate into `XwaylandContinuation`. At the cycle tail, after presentation/render admission, the runtime will request one coalesced continuation when a live managed XWayland generation owns pending consumable commands. The next cycle will use the existing scene-batch boundary and `take_xwayland_backend_commands` path. No command will execute synchronously from render admission, and no continuation will be requested for an absent generation or a non-running shutdown lifecycle.

### 2. Durable render-fence timing evidence

`RenderedOutputFrame` will own a small optional timing-evidence value containing the physical signal timestamp, timestamp quality, and whether the early fence reaction already accounted for the physical observation. Fence-first sampling stores the evidence before taking the timing FD; pageflip completion consumes the stored evidence instead of sampling a closed FD. Pageflip-first completion samples once, stores the evidence locally for the completion, and retains existing resource-release timing.

`PresentedOutputFrame` will carry both the physical fence signal and its accounting provenance. Pageflip classification and frame-service pairing always use the physical timestamp when present. Render-sample and fence-quality estimator updates occur only when the evidence was not already accounted for by the early fence path.

### 3. Complete KMS worker dispatch budget

`KmsWorkerDispatchModel::record` will accept the already measured `dispatch_duration_ns` from worker wake return through successful submission. It will continue recording wake lateness, pre-submit duration, and successful ioctl duration in separate histories. The dispatch budget will use the p95 complete dispatch history plus the existing p95 wake-lateness and fixed guard.

### 4. Absolute pageflip watchdog

The worker will derive one absolute deadline from `WorkerInFlight.submit_returned_at_ns + 1s`. Each condition-variable wake will re-check lifecycle and token identity, compute only the remaining duration, and emit at most one timeout event. After reporting the event, the worker will wait without periodic timeout polling until acknowledgement or quiesce/shutdown.

## Testing strategy

Each finding gets a failing regression test before production changes and a focused commit after targeted and neighboring tests pass.

- XWayland tests cover mixed-queue filtering, work-domain ownership, continuation wake planning, post-dispatch resize delivery, and final resize delivery where the current fixtures permit it.
- Timing tests cover fence-first/pageflip-first equivalence, evidence surviving timing-FD consumption, one-shot estimator accounting, and bounded FD lifetime using deterministic test seams already present in the scanout fixtures.
- Worker timing tests cover synthetic complete-dispatch samples and scripted success/Busy retry metrics without asserting scheduler-exact nanoseconds.
- Watchdog tests cover repeated legitimate test notifications, exactly-once timeout, acknowledgement before the deadline, and existing quiesce/shutdown paths.

The lower-priority audit findings about presentation-trace export, disabled logging overhead, and XWayland association allocation churn are explicitly out of scope.
