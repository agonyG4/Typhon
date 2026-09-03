# Typhon Native Frame Pacing v2.1 Design

## Scope

Native Frame Pacing v2 is accepted. This closure makes two narrow qualification-integrity corrections without changing presentation policy, prediction policy, ownership semantics, or Astrea.

The corrections are independent:

1. Fast-client tail qualification becomes demand-causal instead of being censored by elapsed wall-clock time.
2. Exact no-pageflip DMA-BUF correlation terminals are produced centrally by output-transaction settlement instead of being retired only in selected callers.

## A. Demand-causal fast-client tail qualification

The existing callback timing evidence already identifies the exact surface, callback admission, and next callback-requesting visual commit. A fast-client candidate therefore proves useful next content by requiring:

- one exact callback surface;
- exclusive surface damage for that surface;
- callback admission and the next visual commit;
- callback reaction within the existing fast-client threshold; and
- no callback-handoff limitation.

The dedicated fast-client interval must not additionally require `elapsed <= 4 * refresh`. A long interval is a compositor tail when the next callback-requesting visual commit is already known and remains exact frame/presentation content. The existing time cutoff remains valid for global active-pageflip cadence and idle-gap accounting; it is removed only from the dedicated fast-client continuity decision.

This preserves the distinctions below:

- normal same-refresh cadence remains a continuous fast-client sample;
- slow callback reaction remains client-limited and is excluded;
- a long gap with no next visual commit remains idle and is excluded;
- an exact fast callback reaction plus an early next visual commit remains a continuous sample regardless of presentation delay.

No synthetic demand state, predictor policy, target policy, or global cadence definition is added.

## B. Central exact no-pageflip correlation terminals

`OutputTransactionLedger` remains responsible only for output-transaction ownership and protocol settlement. It does not receive GPU-release or sync-file state.

When a transaction terminal is finalized, the ledger emits a bounded-scope runtime terminal event containing the exact transaction ID and physical classification. The classification is conservative:

- `Presented` is the normal physical terminal;
- `NoPageflip::SafeAbandonment` is emitted for the existing typed safe-abandonment paths, including physical-claim overtake and proven shutdown abandonment;
- `NoPageflip::Superseded` is emitted only for supersession accepted before the submitted state;
- `NoPageflip::SubmissionRejected` is emitted for failure terminals proven before physical submission;
- output/session teardown terminals are emitted only for their proven teardown reasons;
- no event is emitted for ordinary no-visual-change or failure terminals whose physical outcome is not proven, including uncertain submitted-state failures.

The runtime drains these terminal events and asks the DMA-BUF observability registry to retire only the correlation keyed by that exact transaction ID. Retirement is idempotent and affects only GPU-vs-pageflip observability. It never completes a GPU release lease, releases a client buffer, closes client synchronization ownership, changes retry debt, or changes token validation.

The drain occurs before pageflip validation after worker events, immediately after physical-claim recovery, and at shutdown/session/output teardown boundaries. Existing direct correlation retirement calls are removed so there is one canonical observer path.

Normal pageflip/GPU completion ordering remains unchanged. A later GPU signal completes its independent release lease after a no-pageflip correlation has retired, without recreating the correlation or fabricating a pair.

## Non-goals

- Do not enable the paired predictor.
- Do not change `prediction.total_cost_ns`, render-start policy, target selection, O1 admission, or ReactiveDouble semantics.
- Do not redesign READY, worker predecessor, KMS, wake authority, frame callback, presentation feedback, SHM, input, pointer-constraint, XWayland, Direct Scanout, or Astrea behavior.

## Evidence required

Deterministic tests must prove the five-refresh outstanding-demand tail, true idle exclusion, slow-client exclusion, exact-surface exclusion, centralized queued shutdown retirement, GPU completion after retirement, normal pairing in both event orders, physical overtake preservation, terminal classification, and predictor telemetry non-regression. Static checks and a fresh native qualification attempt are reported separately.
