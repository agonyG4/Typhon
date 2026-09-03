# Typhon Native Frame Pacing v2 — Implementation Plan

> **Execution note:** This plan is executed inline in the current checkout. No
> sub-agents are used.

## Scope

Implement the approved design in
`docs/superpowers/specs/2026-09-03-typhon-native-frame-pacing-v2-design.md`.
Preserve the established physical 165 Hz clock, pageflip authority, protocol
ownership, DMA-BUF lease ownership, and direct-scanout boundaries.

## Task 1 — Baseline and evidence

* Confirm the clean starting worktree and current test baseline with `rtk`.
* Use the codebase-memory evidence for `NativeFramePacing`,
  `AdaptiveRenderJournal`, callback timing, pageflip recovery, transaction
  settlement, and DMA-BUF observability. Fall back to `rtk rg` only for literals
  and the graph's two known partial parse ranges.
* Record the exact native qualification command, if one is documented; otherwise
  record the environment blocker in the final report.

## Task 2 — Phase A RED/GREEN: exact fast-client population

Tests first in `src/native_output/pacing.rs` and the callback/output-frame seams:

* add the deterministic virtual-165-Hz continuous same-surface test;
* prove the first frame only establishes the baseline;
* prove slow reaction, ambiguous/multi-surface damage, idle intervals, and direct
  observations are excluded; and
* prove target, render, submit, KMS, and target-hit attribution remains separate.

Then implement the smallest data path:

* add an exclusivity query to `SurfaceDamagePresentation` in
  `src/compositor/mod.rs`;
* carry callback surface identity through `RenderedOutputFrame`,
  `PresentedOutputFrame`, and `ExplicitPresentationObservation`;
* add bounded fast-client intervals, distances, miss buckets, attribution
  counters, and summary fields in `src/native_output/pacing.rs`; and
* keep the existing global callback and physical-clock metrics unchanged.

Run the focused pacing tests after RED and after GREEN, then commit:
`feat: qualify fast-client native cadence`.

## Task 3 — Phase B RED/GREEN: no-pageflip correlation retirement

Add observability-only tests in `src/native_output/runtime/dmabuf_release.rs`
before implementation:

* GPU signal before retirement, pageflip before retirement, and retirement before
  either signal;
* duplicate retirement idempotence;
* a later GPU signal completes the real lease while not recreating a correlation;
* only an armed correlation increments the unpairable timestamp counter; and
* the armed/paired/abandoned/unpairable/pending accounting remains exact.

Implement `DmabufCorrelationNoPageflipReason`, the registry API, summary counter,
and exact retirement wiring in the SafeAbandonment paths in
`src/native_output/runtime/cycle/pageflip.rs`. Do not touch lease completion,
retry debt, buffer ownership, current tokens, or KMS ownership. Run focused
DMA-BUF tests and commit:
`feat: retire abandoned dmabuf correlations`.

## Task 4 — Paired predictor evidence RED/GREEN

In `src/native/adaptive_buffering.rs`, add production-API tests for:

* independent unrelated tails not inflating paired service;
* a real same-frame tail being included;
* miss recovery escalating and decaying within the 120-sample bound; and
* client think time, READY/binding waits, and predecessor physical waits being
  excluded.

Add paired service observations and bounded estimator-mode telemetry. Keep the
legacy prediction policy unless the RED tests demonstrate material
overestimation. If the evidence does prove it, implement only the bounded warm
paired policy and miss-recovery fallback described in the design; otherwise leave
the physical target and advisory deadline behavior unchanged. Add the fast-vs-global
cadence comparison to pacing tests if policy changes.

Run the focused adaptive/pacing tests and commit either:

* `feat: use paired tails for native prediction`, when policy changes; or
* `feat: record paired native prediction evidence`, when measurement-only is the
  evidence-backed result.

## Task 5 — Integration and report

* Run the full static verification set from the design with `rtk`.
* Inspect the final diff and ensure no Astrea/config/input-tool changes exist.
* Attempt the documented native command when available; record skipped status and
  the exact reason otherwise.
* Write `docs/superpowers/specs/REPORT-2026-09-03-typhon-native-frame-pacing-v2.md`
  with changed files, tests, metric semantics, DMA-BUF invariants, predictor
  evidence, native status, and explicit answers to the adversarial questions.
* Commit the report and any final test-only/doc-only adjustments:
  `docs: report native frame pacing v2 verification`.

## Verification checkpoints

After each implementation commit, run the relevant focused tests and
`rtk git diff --check`. Before the final report commit, run:

```text
rtk cargo fmt --check
rtk cargo check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk cargo test
rtk git diff --check
rtk git status --short
```
