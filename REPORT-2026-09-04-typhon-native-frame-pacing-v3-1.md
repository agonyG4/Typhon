# Typhon Native Frame Pacing v3.1 — Deferred O1 Qualification Report

**Date:** 2026-09-04  
**Implementation commits:** `f8beb63`, `bcd40f2`, `6398dba`  
**Native qualification status:** blocked before first rendered frame by environment permissions

## Executive result

The source-proven Deferred O1 lifecycle defect is fixed. A single authoritative classifier now distinguishes `NotDeferred`, `WaitingForPredecessor`, `Bindable`, and `Stale`. A ready Deferred O1 successor waits when its exact predecessor remains live in `pending` or `worker_queued`, even when the historical last-presented anchor is an earlier frame. It binds only after the predecessor's physical pageflip evidence is recorded, or immediately at render completion when that pageflip happened during GPU rendering.

The deterministic source suite is green. The approved native launch was attempted against the exact final release binary, reached the requested `1920x1080@165Hz` DRM output, and failed before rendering because the pre-render atomic `TEST_ONLY` ioctl returned `Permission denied (os error 13)`. The user lacks `/dev/input/event*` group access, and libseat activation also reported `Function not implemented`. No shell-hover workload ran, so the original freeze is not classified as Typhon or Eclipse from this run.

## Source-proven root cause

The old sequence was:

```text
A physically presented
B exact current predecessor, still pending or worker queued
C Predictive O1 anchored to B, render completes
C ReadyUnbound
last_presented_primary_anchor = A
intent.predecessor = B
A != B -> IdentityMismatch -> SafeAbandon
```

That comparison confused historical physical progress with predecessor liveness. `deferred_o1_predecessor()` already exposes the exact live `pending`/`worker_queued` anchor, but the old `deferred_o1_binding_failure()` ignored it.

The corrected invariant is:

> `last_presented != expected_predecessor` does not prove stale while the exact expected predecessor remains a live physical owner.

`NotYetPresented != Stale`.

## Corrected classifier

`AtomicOutputSwapchain::deferred_o1_binding_readiness()` in `src/native_output/scanout/output_swapchain.rs:1317` is the sole interpretation point:

```text
NotDeferred
WaitingForPredecessor
Bindable { predecessor, actual_claim }
Stale(GenerationMismatch | IdentityMismatch)
```

Classification order:

1. Non-Deferred frames return `NotDeferred`.
2. Output, pool, and clock generation mismatches return terminal `Stale(GenerationMismatch)`.
3. An exact match with the recorded last physical anchor returns `Bindable`.
4. A mismatch with that historical anchor returns `WaitingForPredecessor` when `deferred_o1_predecessor()` exactly matches the intent anchor.
5. Identity becomes `Stale(IdentityMismatch)` only after no exact live predecessor remains.

The candidate path consumes the classifier's `Bindable` evidence and no longer performs an independent predecessor interpretation. `AtomicEglGbmScanout::bind_ready_deferred_o1()` propagates waiting separately from stale; the render-completion and pageflip paths leave waiting in the existing prepared-unbound path.

## Waiting semantics

While waiting, the frame remains `ReadyUnbound` with its rendered buffer, render fence, transaction identity, frame batch, O1 credit, and immutable preparation intent. It has no `PrimaryRefreshClaim` and cannot enter the worker, run TEST_ONLY, submit to KMS, or become kernel-in-flight. Waiting does not invoke stale accounting, SafeAbandonment, quarantine, or a new timer/polling wake source.

The existing pageflip order remains physical-first:

```text
complete predecessor ownership
record actual physical claim and anchor
retry Deferred O1 binding
```

Binding still creates the first feasible successor strictly after the actual predecessor claim, exactly once, and leaves the resulting claim immutable.

## Deterministic RED/GREEN evidence

The new RED test was added before the classifier change:

```text
rtk cargo test deferred_o1_waits_for_live_pending_predecessor -- --nocapture
```

It failed for the intended reason:

```text
left: Some(IdentityMismatch)
right: None
```

After implementation, focused coverage is green:

```text
rtk cargo test deferred_o1 -- --nocapture
cargo test: 9 passed, 3343 filtered out
```

The focused tests cover:

- live `pending` predecessor waiting;
- live `worker_queued` predecessor waiting;
- repeated waiting with unchanged frame identity and no candidate;
- no ready worker entry or submission while unbound;
- explicit terminal settlement through the existing suspend quarantine/recovery path;
- pageflip-before-render-completion binding readiness;
- pageflip during rendering followed by immediate render-completion bindability;
- true stale identity rejection;
- generation mismatch rejection;
- first-feasible successor and exactly-once binding behavior.

Fresh full-suite result after the final source commit:

```text
rtk cargo test
cargo test: 3357 passed, 5 ignored, 40 filtered out (30 suites, 45.05s)
```

Static verification:

```text
rtk cargo fmt --check                         PASS
rtk cargo check                               PASS
rtk cargo clippy --all-targets --all-features -- -D warnings  PASS
rtk git diff --check                          PASS
```

No predictor, fast-client attribution, ReactiveDouble, CommitTiming, physical overtake recovery, Native Wake Authority, XWayland, DMA-BUF release ownership, or transaction architecture was changed.

## Exact release provenance

The final release build was performed in the checkout's normal `target/` directory:

```text
source HEAD: 6398dba582147baabbf1b20b4ebe2b88d60e774d
dirty state before build: clean
build: rtk cargo build --release -> PASS, 10.82s
binary: /home/agony/GitHub/Typhon/target/release/oblivion-one
sha256: d84a667ebf7b8501f336e2f550d5d578cd06e4d432e858ef1a62c5304921e0c5
size: 19,243,144 bytes
mode: 0755
```

The launcher log confirms the exact path used:

```text
/home/agony/GitHub/Typhon/target/release/oblivion-one compositor --socket oblivion-one-tty
```

## Native qualification

The exact approved command was run against the final binary with the unchanged Eclipse shell path:

```text
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell
ASTREA_COMPOSITOR_BACKEND=typhon
TYPHON_XWAYLAND=eager
OBLIVION_ONE_MODE=1920x1080@165
OBLIVION_ONE_KMS_MODE=atomic
OBLIVION_ONE_SCANOUT_BACKEND=native-egl-gbm
OBLIVION_ONE_CURSOR=auto
OBLIVION_ONE_KMS_COMMIT_WORKER=auto
OBLIVION_ONE_TRIPLE_BUFFERING=auto
OBLIVION_ONE_DIRECT_SCANOUT=off
OBLIVION_ONE_PERF_LOG=0
./bin/start-oblivion-one-tty
```

Fresh evidence reached:

```text
kms device: /dev/dri/card1
connected output: card1-DP-1
native scanout target: connector 826, crtc 200, 1920x1080@165Hz (exact)
native scanout backend target: explicit Atomic EGL/GLES GBM
```

The run then stopped at:

```text
pre-render atomic TEST_ONLY commit failed: Permission denied (os error 13)
```

The launcher also reported:

```text
user 'agony' is not in group 'input'
native seat: libseat activation failed; using direct fallbacks: Function not implemented (os error 38)
```

No ydotool, screenshots, machine configuration changes, or Eclipse modifications were used.

Because the failure occurred before the first rendered frame:

- the manual Dock/topbar/tray/tooltip/popup/menu hover workload was not executable;
- no native O1 creation/readiness/binding/submission counters were emitted;
- no native physical cadence, Native Wake Authority, DMA-BUF, or OutputTransaction summary was emitted for this run;
- clean-shutdown `SafeDisable` and `current_composed/output_generation` behavior were not reached on the final binary.

The original hover freeze is therefore **not reproduced and not disproven**. The Typhon-vs-Eclipse decision is **inconclusive due to the pre-render environment blocker**, not evidence of an Eclipse-side issue.

## Accounting and preserved invariants

| Area | Result |
| --- | --- |
| O1 unbound/bound/submit native accounting | Unavailable; native run stopped before rendering |
| Physical cadence at 165 Hz | Unavailable; no pageflip |
| Native Wake Authority | Unavailable natively; no source changes |
| DMA-BUF accounting | Unavailable for final run; no ownership code changes |
| OutputTransaction accounting | Unavailable for final run; deterministic transaction suite remains green |
| Shutdown | Final run did not reach shutdown; no claim made |
| Eclipse | Unmodified |

The source-level waiting tests prove that a live predecessor does not cause stale identity abandonment, a new physical claim, worker entry, or submission. They do not substitute for the blocked native qualification.

## Adversarial review

| Question | Evidence-backed answer |
| --- | --- |
| Can a live pending predecessor be mistaken for stale? | No in the corrected classifier; deterministic pending test returns `WaitingForPredecessor`. |
| Can a live worker-queued predecessor be mistaken for stale? | No; deterministic worker-queued test returns `WaitingForPredecessor`. |
| Does `last_presented != expected` automatically imply stale? | No; exact live ownership is checked first. |
| Can `ReadyUnbound` wait without mutating ownership? | Yes; repeated calls preserve identity, deferred reservation, and no target. |
| Can waiting create a `PrimaryRefreshClaim`? | No; only `Bindable` reaches candidate selection. |
| Can waiting enter the KMS worker? | No; unbound worker entry rejects it. |
| Can waiting perform TEST_ONLY or submit to KMS? | No; the unbound submission boundary rejects it; no waiting branch invokes KMS. |
| Can waiting trigger SafeAbandonment or quarantine merely by waiting? | No. An explicit terminal suspend path may safely quarantine/settle it, covered separately. |
| Can repeated waiting attempts accumulate side effects? | No; repeated classifier/candidate calls remain side-effect free. |
| Can pageflip bind exactly once? | Yes at the swapchain boundary; the existing second-bind rejection remains green. |
| Can render completion bind after predecessor presentation during GPU rendering? | Yes; the existing race test sees `Bindable` and the first feasible successor. |
| Can a genuinely stale predecessor still be rejected? | Yes; stale identity coverage remains green. |
| Can generation mismatch still be rejected? | Yes; generation mismatch coverage remains green. |
| Can a bound claim mutate? | No source path was changed; existing immutable-claim tests remain in the full green suite. |
| Can Deferred O1 survive output destruction incorrectly? | Not natively qualified here; explicit terminal ready settlement is covered, but final native destruction ordering was blocked before rendering. |
| Can it leak an OutputTransaction, DMA-BUF obligation, or correlation? | No new waiting ownership path exists; deterministic settlement/transaction tests pass, but final native counters were unavailable. |
| Did predictor policy change? | No; no predictor files or policy constants changed. |
| Did fast-client attribution semantics change? | No. |
| Did ReactiveDouble or CommitTiming change? | No. |
| Did physical overtake recovery weaken? | No; no overtake logic changed and the full suite is green. |
| Did Native Wake Authority regress? | No source change; native runtime result unavailable. |
| Did median 165 Hz cadence regress? | Unavailable because no pageflip occurred. |
| Was the native test executed from the exact freshly built binary? | Yes; the final launcher log names the final checkout binary path and the matching final build is recorded above. |
| Was the original hover freeze reproduced? | No; the run stopped before the shell-hover workload. |
| If reproduced, was forward-progress loss assigned to Typhon or Eclipse? | Not applicable; no hover frames were rendered and the result is inconclusive. |
| Did shutdown remain `SafeDisable`? | Not established on the final run because pre-render TEST_ONLY failed. |
| Did `current_composed/output_generation` `IdentityMismatch` reproduce? | Not evaluated on the final run; teardown was never reached. |

## Classification

```text
source-proven bug: fixed and deterministically covered
hardware-proven bug: not established; native launch blocked before rendering
client-side behavior: not evaluated; Eclipse was unchanged
unrelated shutdown issue: not evaluated on final binary
remaining hypothesis: hover freeze requires a permitted native run and fresh evidence
```
