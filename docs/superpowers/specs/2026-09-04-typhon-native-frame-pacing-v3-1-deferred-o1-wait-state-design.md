# Typhon Native Frame Pacing v3.1 — Deferred O1 Wait-State Design

**Date:** 2026-09-04  
**Status:** Approved for implementation  
**Scope:** Correct Deferred O1 live-predecessor classification and qualify the exact release binary under shell-hover stress

## Goal

Close the source-proven Deferred O1 lifecycle defect in which a ready successor is marked stale solely because its exact live predecessor has not physically presented yet. Add deterministic race coverage, preserve the accepted v3 ownership architecture, rebuild with provenance, and qualify the current binary against the Astrea/Eclipse shell-hover workload without modifying Eclipse.

## Root cause

The existing `deferred_o1_binding_failure()` compares the last physically presented anchor with the Deferred O1 intent's expected predecessor. In the normal render-ahead order, frame A is the last presented frame, frame B is the exact predecessor still owned by `pending` or `worker_queued`, and frame C finishes rendering before B pageflips. Because A differs from B, the old classifier returns `IdentityMismatch` even though B remains a live physical owner and can still become the predecessor.

The invariant is explicit:

> A mismatch between the historical last-presented frame and the expected predecessor does not prove the expected predecessor stale while that exact predecessor remains a live physical owner.

`NotYetPresented` is not `Stale`.

## Architecture

`AtomicOutputSwapchain` owns one authoritative classifier:

```rust
enum DeferredO1BindingReadiness {
    NotDeferred,
    WaitingForPredecessor,
    Bindable {
        predecessor: O1PredecessorAnchor,
        actual_claim: PrimaryRefreshClaim,
    },
    Stale(DeferredO1BindingFailure),
}
```

The exact name may follow local conventions, but the behavior is fixed. The classifier validates output, pool, and clock generation first. It returns `Bindable` only when the expected predecessor equals the recorded last physically presented anchor. If the last-presented anchor differs but `deferred_o1_predecessor()` still returns the exact expected anchor, it returns `WaitingForPredecessor`. It returns `Stale(IdentityMismatch)` only when the expected predecessor is no longer live and the lifecycle proves it cannot become the predecessor. Proven generation mismatches remain terminal stale outcomes.

The render-completion and pageflip paths consume the same classifier. Waiting maps to the existing prepared-but-unbound behavior: the transaction stays `ReadyUnbound`, the rendered buffer and fence remain owned by the ready frame, and no `PrimaryRefreshClaim`, worker entry, TEST_ONLY validation, KMS submit, kernel-in-flight state, abandonment, quarantine, or stale counter is created. No timer, polling loop, retry storm, or new wake source is introduced. The existing predecessor pageflip event retries binding after recording the predecessor's actual physical claim. Render completion binds immediately when the predecessor pageflip occurred during GPU rendering.

When binding is possible, existing first-feasible-successor selection remains unchanged. The new claim is created strictly after the actual predecessor claim and becomes immutable. A second binding attempt remains rejected and cannot duplicate transaction or worker effects.

## Deterministic coverage

Tests use the production `AtomicOutputSwapchain` state and existing transaction helpers. They cover:

- render completion before a live pending predecessor pageflip;
- the equivalent live worker-queued predecessor state;
- predecessor pageflip followed by exactly-once binding;
- predecessor pageflip during O1 rendering followed by immediate render-completion binding;
- true stale predecessor identity and generation mismatch;
- repeated side-effect-free waiting;
- rejection of physical worker/TEST_ONLY/submit entry while unbound;
- terminal settlement while waiting, using the existing safe-abandonment/recovery mechanisms;
- preservation of immutable claim and first-feasible-successor behavior.

The waiting test must demonstrate that the old implementation fails for the intended false `IdentityMismatch` reason before the minimal classifier change is made, then pass after the change.

## Non-goals and preserved invariants

This closure does not tune predictor policy, change `PrimaryRefreshClaim`, alter fast-client attribution, change ReactiveDouble or CommitTiming, weaken physical overtake recovery, add timer ownership, or alter DMA-BUF/GPU release ownership. `OutputTransactionId`, `SafeAbandonment`, exact KMS worker ownership, current-token validation, swapchain quarantine, `wl_surface.frame`, `wp_presentation`, Native Wake Authority, XWayland deadline ownership, Direct Scanout safety, and all existing cadence/transaction/DMA-BUF invariants remain unchanged.

Eclipse is a stress workload only. No Eclipse file or behavior is modified. Native evidence must distinguish a Typhon presentation/KMS stall from an Eclipse client-side render/input stall; absent exact evidence, the result is inconclusive.

## Qualification and provenance

After deterministic and static verification, build `target/release/oblivion-one` in the checkout's normal target directory. Record source `HEAD`, dirty state, release binary SHA-256, file metadata, and build completion evidence. Run the specified 1920x1080@165 native command with the unchanged Astrea shell and manually exercise Dock, topbar, tray, tooltip, popup, menu, idle, resume, and normal-application interactions. Assess forward progress and the v3/v2.2/DMA-BUF/transaction/shutdown counters from fresh evidence.

The report must separately classify the source-proven bug, hardware-proven behavior, client-side behavior, any independent shutdown issue, and remaining hypotheses.
