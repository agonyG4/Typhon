# Typhon Native Content Frame Clock v1

## Status

Approved design for the 165 Hz native content-cadence attribution and wake-ownership closure.

## Problem

The native Atomic EGL/GBM + KMS-worker qualification is physically refreshing at 165 Hz, but primary content frequently presents at approximately 12.12 ms intervals. Scheduler wake lateness is already small, O1/render-ahead is active, and the v1/v1.1/v1.2 wake loops are closed. The remaining question is whether a client, compositor service stage, KMS admission, or presentation-target ownership is responsible for the skipped refresh.

The current checkout contains a specific ownership hazard. `PresentationDeadlinePlanner::reactive_target()` intentionally returns a non-gating ReactiveDouble target and does not store it as the planner's scheduled target. The target is nevertheless copied into a rendered output frame. `AtomicOutputSwapchain::latest_future_primary_target()` returns that target from pending or worker ownership, and `PresentationDeadlinePlanner::plan_render_ahead()` currently derives the O1 successor timestamp and submit boundary from it. Thus advisory prediction metadata can become a downstream physical reservation even though ReactiveDouble itself is supposed to be immediate.

The current primary cadence evidence is relevant to content: cursor-only `AtomicCommitKind::PlaneDelta` completions are classified as stale for the primary frame scheduler, do not call `NativeFramePacing::note_pageflip()`, and do not advance the presentation deadline phase. Only primary pageflip completion enters the active primary cadence path.

## Goals

* Attribute the next-primary opportunity to client handoff, client reaction, target selection, render readiness, submit timing, or KMS presentation.
* Preserve the existing physical presentation phase anchor.
* Make advisory ReactiveDouble predictions diagnostically useful without allowing them to reserve future refreshes.
* Preserve explicit CommitTiming lower bounds and binding predictive READY ownership.
* Ensure O1 successors follow genuine primary reservations, not advisory timestamps.
* Keep immediate work on the coalesced continuation eventfd and real temporal work on the absolute timerfd.
* Attribute residual stale/past deadline arms to a fixed deadline owner.
* Keep the compositor idle when no useful visual work exists.

## Non-goals

This change does not redesign DMA-BUF release ownership, output transactions, cursor liveness, input epochs, pointer behavior, Direct Scanout, SHM, callback admission, KMS worker timeout ownership, or shutdown. It does not tune prediction constants without evidence, force repainting, or change frame-callback protocol timing.

## Evidence-first decision gate

The implementation starts with a production-boundary RED test using the existing planning path:

```text
reactive_or_commit_timing_target()
    -> pending target from the real Atomic swapchain path
    -> plan_visual_target_for_budget()
    -> PresentationDeadlinePlanner::plan_render_ahead()
```

The test uses a virtual 165 Hz clock and a real pending output-frame target. It must demonstrate that a conservative ReactiveDouble target can cause an O1 successor to inherit an unnecessary later refresh reservation. If it does not fail, no frontier or authority policy is introduced; attribution telemetry and the virtual callback oracle remain the next diagnostic boundary.

## Proposed authority model

If the RED test fails, `PresentationTarget` gains explicit authority semantics rather than inferring ownership from timestamps:

```rust
enum PresentationTargetAuthority {
    Advisory,
    Reserved,
}
```

ReactiveDouble targets are advisory. Their target time remains available for feasibility, miss classification, and diagnostics, but it cannot create an O1 physical reservation.

CommitTiming targets remain binding because they carry an explicit client lower bound. Predictive/O1 READY targets are binding once accepted by the output pipeline. Direct behavior remains unchanged.

The physical primary refresh frontier is derived from existing authoritative pipeline state and target identities. It represents the last physically presented primary plus genuinely reserved future primary opportunities. It is not a second ownership queue and does not move for cursor PlaneDelta, timers, input, protocol-only work, NoVisual work, or advisory predictions.

O1 selects the earliest phase-aligned physically feasible opportunity strictly after the real binding frontier. It does not universally select the next vblank and does not blindly add one refresh to an advisory predecessor timestamp.

READY identity remains immutable. If a binding target becomes invalid, existing generation/identity invalidation and explicit replan paths are used.

## Attribution telemetry

Telemetry is bounded and diagnostic-only. It reuses `BoundedSamples` and does not participate in scheduling.

The content summary records:

* primary presentation intervals, excluding idle periods and cursor-only PlaneDelta;
* callback admission to next visual commit;
* client commit to render start;
* render start to READY;
* READY to submit;
* submit to pageflip;
* selected target distance and actual primary distance;
* ReactiveDouble and Predictive/O1 target early/late classes;
* diagnostic attribution classes: callback handoff, client, target, render, submit, KMS, or target hit;
* fast-client samples using `reaction <= min(refresh_interval / 2, 2 ms)` as telemetry only;
* predictor component values, including render risk, wake guard, worker stages, KMS dispatch budget, apply guard, total cost, and paired end-to-end service if later proven necessary.

No per-frame or per-event stdout is added. A bounded shutdown event is emitted as `native_content_frame_clock_summary`.

The wake summary also gets fixed per-owner stale and past-arm counters. These preserve the existing aggregate metrics while identifying any residual owner without changing the metrics' semantics.

SurfacePacing is changed only if owner-attributed RED evidence demonstrates that a matured release boundary is being reinstalled while another dependency owns progress.

## Virtual 165 Hz oracle

The deterministic oracle uses a 6,060,606 ns refresh, KMS-worker mode, O1 enabled, and current slot/depth limits. A callback-driven client reacts within 0–500 us, commits new content, and requests the next callback. Service costs are selected to fit one refresh.

The oracle requires bounded future-primary depth, active O1, one-refresh median primary cadence, no callback duplication or bursts, and no target identity mutation. A separate slow-render case must legitimately target a later opportunity. No empty frames are generated to manufacture cadence.

## Before/after wake ownership

```text
Before:
  physical primary phase
        -> advisory Reactive target
        -> pending/worker target lookup
        -> O1 successor based on advisory timestamp
        -> later submit boundary can reserve an unnecessary refresh

After:
  physical primary phase
        -> genuine binding frontier
        -> earliest feasible phase-aligned target
        -> advisory prediction used only for diagnostics
        -> binding READY target owns its exact opportunity
        -> physical primary pageflip advances phase
```

The already accepted native wake contract remains:

```text
bounded immediate work -> coalesced RuntimeContinuation eventfd
future temporal work   -> one absolute CLOCK_MONOTONIC timerfd deadline
external ownership     -> that owner's readiness
```

## Comparator principles

The design follows the relevant principles from the studied implementations: Wayland frame callbacks are throttling hints; KWin keeps each output's future presentation phase aligned with physical vblank; Mutter distinguishes immediate and later frame-clock states; Hyprland keeps pending/deferred frame progression separate from render execution; Chromium may wait for a previous frame callback before playing a pending frame. Typhon keeps its own typed pipeline and ownership model.

## Safety and performance

No new thread, mutex, sleep, polling loop, forced repaint, GPU blocking wait, unbounded history, or per-frame allocation is introduced. Existing DMA-BUF identities and release timing are untouched. All target changes are generation/identity validated.

