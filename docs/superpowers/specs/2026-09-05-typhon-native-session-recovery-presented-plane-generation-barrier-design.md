# Typhon Native Session Recovery v1 — Presented Plane Physical-Provenance Generation Barrier

## Status

Approved design for the current Typhon checkout. This is a narrow session-recovery correctness change; Native Frame Pacing v3.x remains unchanged.

## Problem and source diagnosis

The native recovery path already quiesces the KMS worker, quarantines the old pageflip, performs a synchronous KMS recovery modeset, settles suspended transactions, allocates a new DRM-file generation, rebinds timing/swapchain/explicit-sync state, restarts the worker, restores the cursor, and only then rearms the scheduler. The missing state transition is in the physical presentation ledger.

Before recovery, `PresentedPlaneSnapshot.primary` may contain a `PresentedPrimaryState::Composed` or `Direct` whose immutable `PlanePageflipIdentity` belongs to generation G1. The synchronous recovery modeset establishes the framebuffer physically scanning out after reacquisition, but it is not a pageflip and creates no `PageFlipToken`, transaction, presentation serial, or physical timestamp. The current code leaves the G1 primary in the snapshot and later changes `NativeRuntime::drm_file_generation` and the associated pipeline state to G2. The first scheduler wake reaches `current_scheduler_wake_deadline() -> validate_output_pipeline() -> build_output_pipeline_snapshot_with_presented() -> validate_presented_primary()`, correctly rejects `pageflip.output_generation == G1` against `output_generation == G2`, and reports `wake pipeline validation failed`.

The XWayland `SIGKILL` observed after that error is an unwind consequence of `NativeRuntime::Drop`, not the cause. The failing path does not show the normal native-input shutdown request or a `SafeDisable` teardown. This design therefore contains no XWayland or shutdown workaround.

## Invariants

1. A session/output generation transition is a physical-provenance barrier. No pageflip identity created in generation G remains authoritative as presented physical provenance after generation G+1 becomes active.
2. `PresentedPlaneSnapshot.primary` means the exact primary-plane state last promoted by a matching physical pageflip. It never means “the framebuffer the kernel currently scans out.”
3. A synchronous recovery modeset establishes a physical baseline but is never represented as a pageflip or a client presentation.
4. Before the recovery modeset succeeds, G1 remains the authoritative runtime generation. A prepared G2 token is local pending state only. After physical recovery bookkeeping succeeds, the exact prepared generation is consumed by timing, swapchain, explicit-sync watches, worker, cursor, and runtime state.
5. A late G1 pageflip is handled by the existing stale-generation/quarantine logic and cannot promote, resurrect, or overwrite G2 state.

## Chosen architecture

### One prepared recovery generation

Allocate the replacement DRM-file generation once while preparing session recovery. Carry that exact value in a private pending recovery record. Pass it to `NativeAtomicCursor::prepare_for_recovery`, then consume it in `rearm_explicit_sync` for presentation timing, scanout generation, acquire watches, worker validation, and `NativeRuntime::drm_file_generation`.

This removes the current split between cursor preparation using `self.drm_file_generation.saturating_add(1)` and runtime rearm using a second allocator call. Allocation can reserve a number even if a later modeset fails, but the reserved value is not authoritative and is never installed in runtime state on that failure path.

### Physical baseline commit

After the synchronous recovery modeset succeeds, and after the exact cursor request has been promoted into the cursor's `current`/`submitted` bookkeeping, commit a narrow snapshot-owned barrier operation:

```text
PresentedPlaneSnapshot::rebase_after_session_recovery(exact_cursor_state)
    primary = None
    cursor = exact physical cursor baseline
    revision = revision.next()
```

The pending record is retained through scanout retirement. The barrier commits during `retire_quarantined_pageflip`, before the exact prepared generation is rebound, so the snapshot is already truthful when G2 becomes authoritative. `primary = None` is intentional: no recovery pageflip exists to support a primary provenance record. The next genuine G2 pageflip promotes a new exact `PresentedPrimaryState`.

The cursor side uses the existing initial-modeset semantic pattern. Extract or reuse a clearly named synchronous-modeset promotion helper so `NativeAtomicCursor::current`, `submitted`, and the presented cursor snapshot all describe the exact cursor state passed to the successful recovery modeset. Hidden and software-fallback modes use a hidden hardware-plane baseline; the existing desired/submitted/current distinctions remain intact.

### Ordering

The existing recovery order remains intact, with one explicit physical-baseline commit after cursor recovery and before DRM source registration:

```text
KMS recovery modeset and exact cursor-state promotion
  -> retire quarantined pageflip and suspended transactions
  -> commit primary=None, exact cursor baseline, revision increment
  -> rebind the single prepared generation G2
  -> re-arm cursor resources for G2
  -> register DRM source
  -> rearm scheduler
  -> resume input
```

No scheduler wake is admitted between generation rebind and baseline reconciliation. A failed modeset returns before the pending recovery record is installed and leaves G1 authoritative. A failed later recovery leaves the session inactive through the existing resume-failure teardown; it cannot expose a stale G1 primary as current G2 provenance.

## Slot and ownership reasoning

`AtomicOutputSwapchain::current()` remains the physical scanout slot. Its current slot is not a free render slot, and `free_slot_count`, `render_target_available_for_limit`, and `validate_invariants_for_limit` continue to enforce that ownership independently of `PresentedPlaneSnapshot.primary`. Clearing primary therefore does not make the current framebuffer renderable and does not require a `RecoveredBaseline` enum.

The same barrier clears both composed and direct presented-primary provenance. Existing `DirectPrimaryOwnership`, safe-abandonment, callback-owner leak accounting, frame-batch completion, and `OutputTransactionDropReason::SessionSuspended` settlement remain the authority for suspended direct work. No recovery modeset creates a presented transaction or DMA-BUF release correlation.

## Validation-base semantics

The KMS worker already combines presented snapshot revision, output generation, and CRTC identity in its validation base, and the worker is quiesced/restarted during recovery. Incrementing the snapshot revision at the physical baseline commit makes the discontinuity explicit: a pre-recovery `KmsValidationBase::Presented` cannot be mistaken for the post-recovery baseline. The validator remains strict; it is not changed to accept stale generations.

## Observability

Add one recovery-only event, following existing native perf/session logging conventions, containing old generation, new generation, whether a presented primary existed, old primary kind, whether primary provenance was retired, cursor baseline delivery, and snapshot revision before/after. This is not per-frame logging. Existing frame-pacing, O1, Wake Authority, transaction, DMA-BUF, predictor, and worker counters remain untouched.

## Test architecture

Tests must exercise the physical ownership layer, not only the `NativeSessionIo` recorder. The deterministic suite will cover:

- strict G1 presented-primary validation, the stale G1/G2 RED, and the post-rebase G2 GREEN;
- immediate scheduler/pipeline validation with no intervening pageflip;
- exact first G2 composited promotion and immutable G1 identity preservation;
- late G1 rejection, repeated G1→G2→G3 recovery, no prior primary, and direct primary retirement;
- visible, hidden, disappeared-plane, and software cursor baselines with exact `current`/`submitted`/presented agreement;
- snapshot revision advancement and validation-base generation/revision separation;
- current-slot non-renderability after `primary = None`;
- existing late-pageflip, direct ownership, transaction settlement, DMA-BUF, O1, and Wake Authority non-regression tests.

The existing abstract session ordering tests remain and gain the new barrier point, but they are not the sole proof.

## Upstream comparison

Current Aquamarine `restoreAfterVT()` explicitly clears stale page-flip bookkeeping before restoring outputs; current KWin treats paused/resumed DRM devices and output/layer state as an ownership transition. Typhon adopts the principle—invalidate stale physical history at the resume boundary—using its typed generation, snapshot, transaction, direct-ownership, and swapchain model rather than copying either implementation:

- [Aquamarine DRM session restore](https://github.com/hyprwm/aquamarine/blob/main/src/backend/drm/DRM.cpp)
- [KWin DRM backend](https://github.com/KDE/kwin/blob/master/src/backends/drm/drm_backend.cpp)

## Out of scope

No predictor tuning, frame-pacing redesign, Wake Authority changes, KMS scheduling-policy changes, PrimaryRefreshClaim changes, DMA-BUF protocol changes, transaction-terminal redesign, XWayland changes, Eclipse changes, synthetic VT control, or shutdown-only workaround.
