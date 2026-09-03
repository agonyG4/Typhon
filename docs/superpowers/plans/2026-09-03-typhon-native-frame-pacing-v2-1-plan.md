# Typhon Native Frame Pacing v2.1 Plan

## 1. Establish regression tests

- Add a deterministic five-refresh fast-client compositor-stall test using exact same-surface callback/commit evidence.
- Correct the idle fixture so its delayed commit has the original callback admission and a slow reaction, then assert the gap is excluded.
- Keep or extend normal cadence, slow-client, and ambiguous multi-surface tests.
- Add output-transaction physical-terminal classification tests and centralized runtime-terminal draining tests.
- Add shutdown queued-frame, later-GPU-signal, normal both-order, and overtake non-regression coverage.

## 2. Implement demand-causal qualification

- Remove the four-refresh elapsed cutoff only from the fast-client continuity predicate.
- Preserve the cutoff for global active-pageflip cadence and its idle-gap metric.
- Keep exact callback surface, exclusive damage, callback admission, commit, reaction, and handoff evidence as the qualification gate.
- Do not add predictor or scheduling behavior.

## 3. Centralize physical terminal notification

- Add typed output physical-terminal and settled-terminal event values to the presentation ledger.
- Queue exact no-pageflip events at terminal finalization, keyed by the settled transaction ID.
- Add a runtime drain that maps those events into the existing DMA-BUF correlation observer.
- Remove caller-specific overtake retirement and invoke the central drain at pageflip, worker, shutdown, session, and output teardown boundaries.
- Leave ordinary no-visual-change, uncertain failure, GPU leases, buffer releases, retry state, and pageflip validation untouched.

## 4. Verify and report

- Run focused tests for all qualification and terminal cases.
- Run fresh `rtk cargo fmt --check`, `rtk cargo check`, `rtk cargo clippy --all-targets --all-features -- -D warnings`, `rtk cargo test`, `rtk git diff --check`, and `rtk git status --short`.
- Attempt the existing 1920x1080@165 Hz qualification without ydotool or desktop screenshots; report environment limitations explicitly.
- Create `docs/superpowers/specs/REPORT-2026-09-03-typhon-native-frame-pacing-v2-1.md` with exact results and adversarial answers.
- Commit the implementation, tests, and documentation.
