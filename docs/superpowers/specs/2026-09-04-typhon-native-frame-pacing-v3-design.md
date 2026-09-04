# Typhon Native Frame Pacing v3 Design

**Date:** 2026-09-04  
**Status:** Approved for implementation  
**Scope:** exact fast-client visual attribution and Predictive O1 deferred physical binding

## Problem

Typhon currently uses one `PresentationTarget` for both two different decisions:

1. an advisory decision to render a future frame; and
2. exact ownership of a future physical refresh opportunity.

`plan_render_ahead()` derives a successor `PrimaryRefreshClaim` from the predecessor's predicted phase. If the predecessor later presents one or more refresh intervals late, the already-ready O1 frame is correctly classified as an overtake and safely abandoned. That recovery is correct; the error is creating physical ownership before the predecessor's physical phase is known.

The fast-client oracle has a separate attribution error. `SurfaceDamagePresentation::is_exclusive_surface_id()` currently treats every sampled scene surface as competing even when its committed content did not advance. The oracle must compare each sampled surface with its exact last physically presented surface-local baseline.

The predictor, physical clock, XWayland deadline ownership, DMA-BUF ownership, ReactiveDouble, CommitTiming, callbacks, and `wp_presentation` semantics are not being redesigned.

## Invariants

- `PrimaryRefreshClaim` remains immutable after creation.
- A frame may be prepared and GPU-ready without owning a physical claim.
- Actual pageflip feedback remains the only physical-phase authority.
- A deferred O1 frame binds at most once, to the first feasible opportunity strictly after its validated predecessor claim.
- A bound frame follows the existing claim-ordering, overtake recovery, worker, KMS, and quarantine paths unchanged.
- An unbound frame cannot be worker-queued, TEST_ONLY checked, submitted, or marked kernel-in-flight.
- An unbound frame consumes one rendered-buffer/O1 credit, but contributes zero to bound physical future depth.
- Hardware-cursor movement is excluded from primary-content exclusivity. A software cursor composed into the primary scene remains a competing sampled visual surface and is documented as such.
- Unknown surface history is not treated as unchanged.

## Fast-client visual attribution

Add an explicit per-sample classification:

```text
Unchanged | Advanced | Unknown
```

For the same surface ID and presentation generation, compare the captured commit counter with the compositor's last physically presented baseline:

- equal baseline: `Unchanged`;
- newer commit: `Advanced`;
- missing baseline or generation mismatch: `Unknown`.

A callback candidate is exclusive only when the callback surface is `Advanced` and every other competing primary sampled surface is `Unchanged`. Any other advanced surface or unknown history rejects the candidate. Static wallpaper, panels, docks, and unchanged layer-shell surfaces therefore do not reject an otherwise attributable candidate.

The capture token carries the classification beside each sampled surface. The settle path continues to update the presented baseline only at existing physical/no-visual settlement points. No client heuristic or callback admission rule changes. Hardware cursor samples are ignored only for this primary-content oracle; software cursor composition is not silently ignored.

## Deferred O1 state model

Use a typed reservation boundary rather than a boolean alongside a target:

```text
FramePresentationReservation::Bound(PresentationTarget)
FramePresentationReservation::DeferredO1(O1PrepareIntent)
```

`O1PrepareIntent` contains only preparation identity and advisory telemetry: output generation, exact predecessor identity/token/transaction anchor, predicted timing, refresh interval, and O1 reason/admission identity. It contains no `PrimaryRefreshClaim`, physical sequence ownership, or KMS submission ownership.

The lifecycle is:

```text
O1PrepareIntent -> RenderingUnbound -> ReadyUnbound
                                  -> validate predecessor pageflip
                                  -> bind once to Bound(P + k)
                                  -> ReadyBound -> worker -> kernel -> Presented
```

The old O1 lease rule is superseded only before binding. Before binding, the preparation intent is immutable but has no physical claim. From the instant of binding onward, the existing immutable-claim rule remains authoritative; binding is not retargeting.

### Transaction and swapchain ownership

`OutputTransaction` stores the typed reservation. Its ledger gains an explicit `ReadyUnbound` state. `ReadyUnbound` may settle through the existing safe-abandonment/no-visual paths, but its transition table rejects queue and submit transitions. Binding replaces the deferred reservation with one exact `PresentationTarget` and promotes the ledger to ordinary `Ready`.

The rendered frame and transaction retain their buffer, fence, frame-batch identity, and DMA-BUF lifecycle while unbound. `AtomicOutputSwapchain` retains the frame as ready, records the exact predecessor anchor when a pageflip completes, excludes the unbound frame from physical-claim ordering/revalidation, and exposes it as a prepared-but-unbound scheduler state. It still blocks another rendered ready slot, preserving bounded buffering.

If the predecessor pageflip occurs first, pageflip handling establishes and records the actual physical claim before binding. If rendering completes first while the exact predecessor remains live in `pending` or `worker_queued`, the frame remains `ReadyUnbound` and waits for the predecessor pageflip; the historical last-presented anchor differing from the expected predecessor is not stale evidence. If the predecessor presented during GPU rendering, render completion binds immediately against that already-recorded physical evidence. Both paths validate output generation and the exact predecessor transaction/frame/token identity. Only a predecessor proven no longer live and unable to present causes abandonment without creating a claim.

Binding chooses the first feasible claim strictly after the actual predecessor claim. The first candidate is normally `P + 1`; if the remaining KMS service window cannot meet it, later successors are tested using the existing dispatch and apply-guard evidence. Full render cost is not charged again. A successful bind updates the transaction/frame submission target and can then use the unchanged worker/KMS path.

No speculative unbound worker job is created. No unbound path can call TEST_ONLY or a submit ioctl.

## Physical frontier and scheduler

The pipeline reports two bounded concepts:

- prepared future frame depth: includes `ReadyUnbound` for buffer/credit limits;
- bound future primary depth: includes only exact claims in pending, worker, and ready-bound states.

The scheduler can observe `ReadyUnbound`, but its decision is to wait for physical binding. It cannot choose `SubmitReady` for that state. `latest_future_primary_target`, claim ordering, and bound overtake recovery continue to use only bound claims.

## Unchanged architectures

- Predictor policy, render-cost margins, EWMA/p90/p95 selection, and MissRecovery hysteresis are unchanged.
- ReactiveDouble remains advisory and low-latency.
- `CommitTiming` remains client lower-bound authority and never adopts deferred O1 semantics.
- Frame callbacks remain attached to exact transaction/frame-batch settlement; rendering early is not presentation.
- `wp_presentation` timestamps remain physical pageflip timestamps.
- Composited DMA-BUF release remains driven by GPU completion fences. Physical correlation remains observability only; direct scanout remains KMS-owned.
- Native wake and XWayland deadline ownership remain unchanged.

## Observability

Retain existing predictive-ready and overtake counters. Add bounded lifecycle counters for unbound creation/readiness, binding after predecessor pageflip, binding after render completion, skipped intervals during binding, identity/generation abandonment, and existing ready submission. The summary must reconcile preparation through binding, submission, abandonment, and shutdown without per-frame logging.

## Comparator principles

The design adopts lifecycle principles, not framework structures:

- [Hyprland `MonitorFrameScheduler.cpp`](https://github.com/Hyprland/Hyprland/blob/main/src/output/MonitorFrameScheduler.cpp): render readiness and physical commit are separable.
- [KWin `renderloop.cpp`](https://github.com/KDE/kwin/blob/master/src/core/renderloop.cpp): timing estimates guide scheduling while presentation feedback reanchors phase.
- [Mutter frame clock](https://gitlab.gnome.org/GNOME/mutter/-/blob/main/src/backends/meta-frame-clock.c): multiple future states do not turn uncertain prediction into physical truth.

Typhon keeps the stronger exact identities, immutable bound claims, physical pageflip authority, safe abandonment, worker ownership, and bounded swapchain already present in the repository.

## Acceptance boundary

Deterministic tests cover attribution A-F; main and multi-refresh O1 misses; pageflip/render completion races; unbound submission rejection; predecessor identity and generation mismatch; exactly-once and late binding; bound overtake regressions; buffer credit/depth; and lifecycle observability. Static verification is fresh. Native qualification is attempted only after deterministic/static GREEN and reports exact evidence, including legitimate environment blockers.
