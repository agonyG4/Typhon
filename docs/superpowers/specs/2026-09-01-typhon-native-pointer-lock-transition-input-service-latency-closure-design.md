# Typhon Native Pointer-Lock Transition Input-Service Latency Closure v1

## Status

Approved implementation design for the narrow native input-service latency
closure. The design preserves the accepted native input semantic epoch,
activation-time anchor, bounded continuation, pointer-routing, cursor, KMS,
presentation, and resource-ownership architectures.

## Evidence and scope

The latest Sober trace shows that the former activation-anchor causality issue
is closed: for representative lock activations, the compositor cursor, the
resolved activation anchor, the native backend anchor, and the active compositor
route anchor are equal. The remaining symptom is a transition-local batch of
real physical events, commonly 28 events spanning about 27 ms, followed by a
single coalesced relative motion. The batches are below the 256-event
continuation budget, so this design does not change motion arithmetic or
coalescing.

The existing `TYPHON_POINTER_DEBUG` trace is not a latency-neutral profiler.
Its shared pointer logger formats and writes synchronous `eprintln!` records,
while the native launcher copies compositor output through `tee`. The new
timing probe therefore uses compositor monotonic timestamps, bounded in-memory
state, and one compact transition summary. It never becomes scheduling
authority.

The source audit found that Typhon already registers each native input fd as a
first-class `NativeEventSource::Input` epoll source. The open question is the
authority after one readiness snapshot: `run_cycle()` can continue through
Wayland, pacing, control/cursor, XWayland, acquire/prepare, and presentation
work before the next reactor wait. The implementation therefore adds a narrow
transition outcome and barrier at this boundary instead of adding another input
backend or an input thread.

## Invariants retained

The implementation retains all of the following:

* native input/backlog is serviced before new client read-side input topology;
* one stable native input semantic epoch owns topology and interpretation;
* no client read-side dispatch occurs inside an active semantic epoch;
* `ActivateLocked` carries intent only and resolves its anchor from current
  compositor pointer state;
* the native backend and compositor active route store the same anchor;
* an already-serviceable input epoch is processed immediately after any
  pre-existing activation settles;
* one `libinput.dispatch()` is used per semantic epoch;
* a 256-event continuation retains the same semantic epoch and does not create
  a routing boundary;
* motion coalescing, exact relative sums, raw evdev bounded storage, and all
  previously closed pointer behavior remain unchanged.

## Observer-neutral timing architecture

Add a small `NativePointerTimingTrace` owned by `NativeRuntime`. Its enabled
state is cached from `TYPHON_POINTER_TIMING_TRACE`. Disabled operation is a
single boolean branch with no formatting, allocation, output, or scheduling
effect. Enabled operation stores `Copy` timestamp and counter fields in a
bounded transition-local record; it does not record individual motion events.

The record starts at a real routing transition and captures the compositor
monotonic timestamps needed to attribute the next input-service gap:

* reactor wake return and input-service start;
* libinput dispatch start/end and native batch materialization end where the
  backend exposes those boundaries;
* Wayland read-side dispatch start/end;
* activation resolution and native backend activation;
* cursor synchronization start/end;
* return from `dispatch_wayland_and_input()`;
* cycle-tail phase boundaries for pacing, control/cursor, XWayland,
  acquire/prepare, render, and present/KMS;
* cycle return, next reactor wake, and next input-service start;
* first post-transition batch raw/coalesced counts and oldest/newest hardware
  timestamps.

The summary is emitted once per completed observation and contains derived
intervals rather than per-event records. Hardware timestamps remain diagnostic
only and never determine ordering or scheduling.

## Structured routing-transition ownership

Native backend settlement returns:

```rust
struct NativePointerConstraintSettlementOutcome {
    redraw_requested: bool,
    routing_transition: Option<NativeInputRoutingTransition>,
}
```

The transition enum has explicit activation/deactivation variants for locked
and confined routing. The outcome is produced only from the backend action that
actually changed `NativePointerConstraintBackend.active`. A queued request,
stale-generation request, rejected activation, failed revalidation, region
update, warp, or cursor-visibility-only action produces no transition.

The dispatch seam propagates the outcome together with its existing pacing
result. The runtime records the transition once and owns its one-shot scheduler
intervention. The compositor remains the authority for request eligibility and
anchor resolution; the backend remains the authority for the completed native
routing action.

## Transition barrier and cycle ordering

After a semantic input epoch has ended and any newly readable Wayland request
has been processed, the real settlement seam applies the requested routing
transition. Immediately after that settlement, before latency-heavy tail work,
the runtime consumes a one-shot transition barrier.

The barrier performs a bounded nonblocking check of the registered input fds.
It does not read or consume unrelated readiness. If a genuinely fresh native
input fd is serviceable, the runtime runs one real input microturn through the
existing `dispatch_wayland_and_input()` path with read-side Wayland dispatch
disabled. If no input is serviceable, the current cycle continues with its
already-owned non-input work; no readiness is discarded and no arbitrary
reactor round trip is inserted.

The barrier is armed only by a real locked/confined activation or deactivation
outcome and is consumed at most once for that transition. A microturn cannot
recursively yield forever. Ordinary motion, cursor visibility, and all other
non-transition paths retain the input-only fast path.

The ordering is therefore:

```text
reactor wake snapshot
  -> settle older constraint work
  -> service existing native epoch, if serviceable
  -> end epoch and flush its write batch
  -> perform allowed Wayland read-side dispatch
  -> resolve/apply the new native routing transition
  -> one-shot nonblocking input checkpoint
  -> at most one fresh native input microturn
  -> existing pacing/control/cursor/XWayland/acquire/prepare/render/present work
  -> return to reactor
```

An activation that was already pending before the wake still settles before
the current serviceable input epoch, so that input is processed as locked in
the same cycle. Existing input readiness is never moved behind the Wayland
request. A >256 continuation keeps the current epoch active and cannot arm a
new routing boundary in the middle of that epoch.

## Deterministic evidence

The regression suite uses the production settlement outcome and production
barrier decision. The real compositor-server/resource tests cover the
Wayland-created persistent lock sequence and the old-input-plus-Wayland
readiness sequence. The scheduler test records the actual production ordering
of transition observation, checkpoint, optional input microturn, and tail-work
admission; it does not duplicate `run_cycle()` in a fake implementation or
encode a wall-time threshold.

Coverage includes:

* exactly one scheduler-visible outcome for each real activation/deactivation;
* no outcome for queued, stale, rejected, or visibility-only actions;
* fresh post-transition input using locked/confined semantics;
* already-ready input without an unnecessary yield;
* combined old-input and Wayland readiness ordering;
* no barrier on ordinary motion;
* no boundary during a >256 continuation;
* repeated lock/unlock liveness across rendering, pacing, control, XWayland,
  and presentation;
* equivalent high-level ordering for raw evdev;
* bounded, no-hot-path-allocation timing observation with deterministic
  replacement/wraparound policy.

## Reference comparison

Current upstream references reinforce the boundary without prescribing Typhon's
pointer semantics:

* KWin separates libinput fd acquisition from queued event processing and
  coalesces consecutive motions while preserving non-motion ordering.
* Hyprland with Aquamarine keeps the visible libinput fd callback short:
  dispatch, drain, pointer callback, and relative-pointer delivery.
* wlroots registers `libinput_get_fd()` directly with the Wayland event loop;
  readability dispatches libinput and drains its available events.
* Weston follows the same direct event-loop-source pattern.

Typhon already has first-class epoll input readiness. The v1 change is about
post-snapshot latency authority and typed routing-transition ownership, not a
mechanical port of another compositor's backend or pointer-constraint rules.

## Qualification boundary

The implementation report must distinguish source/deterministic evidence from
runtime evidence. The primary qualification command enables only
`TYPHON_POINTER_TIMING_TRACE=1` and leaves `TYPHON_POINTER_DEBUG` unset. The
optional semantic-debug command enables the existing high-volume logger only
when semantic inspection is needed. Real Sober/Roblox interaction remains a
user qualification step; unit tests do not close that application claim.
