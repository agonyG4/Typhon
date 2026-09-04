# Typhon Native Frame Pacing v3 Implementation Plan

> **For agentic workers:** This plan is executed inline in the current task. The user explicitly requested no sub-agents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Correct surface-local fast-client visual attribution and make Predictive O1 render future content before, rather than during, exact physical claim binding.
**Architecture:** Add explicit `Unchanged`/`Advanced`/`Unknown` attribution and a typed `Bound`/`DeferredO1` reservation. Preserve exact pageflip authority and route only bound frames through existing worker/KMS ownership.
**Tech Stack:** Rust, existing Typhon compositor/native-output pipeline, deterministic unit tests, Cargo verification, and the repository's `rtk` command wrapper.

## Global Constraints

- Do not use sub-agents.
- Use `/home/agony/.local/bin/rtk` for all shell commands.
- Preserve unrelated working-tree changes and stage only task files.
- Do not tune the predictor or reopen XWayland, DMA-BUF, physical-clock, ReactiveDouble, CommitTiming, callback, or `wp_presentation` architectures.
- Do not add threads, timers, polling, blocking GPU waits, hot-path mutexes, unbounded per-frame state, or per-event logging.
- Keep `PrimaryRefreshClaim` immutable and make unbound physical entry impossible by type/state checks.

---

## 1. Establish design and baseline evidence

- [x] Read the v3 attachment, current source, prior v1/v2/v2.1/v2.2 design-plan-report documents, and upstream comparator sources.
- [x] Confirm graph project generation and coverage for all planned source paths; read the reported partial `presentation_cycle.rs:153` range directly.
- [ ] Capture fresh baseline status, focused tests, and static command results before implementation where useful.
- [x] Commit this design and plan before production changes.

## 2. Fast-client attribution — tests first

- [ ] Add deterministic tests for: only callback surface advanced; two advanced surfaces; missing baseline; stale generation; unchanged callback; and multiple unchanged shell surfaces.
- [ ] Run the focused attribution tests and confirm the pre-change semantic failure is represented by RED tests.
- [ ] Add `SurfacePresentationChange` and per-sample classification using exact presented commit/generation baselines.
- [ ] Make exclusivity require exactly one advanced callback surface and unchanged competing primary surfaces; reject unknown; ignore only hardware-cursor samples.
- [ ] Update baseline maintenance/removal paths and focused tests without changing callback admission or settlement semantics.
- [ ] Run the focused attribution tests GREEN before changing O1.

## 3. Deferred O1 reservation — tests first

- [ ] Add typed reservation/intent and deterministic unit tests for the current main failure: predicted `N`, rendered successor, predecessor physically reaches `N+1`, and the successor remains unbound rather than being overtaken.
- [ ] Add RED coverage for multi-refresh predecessor movement, pageflip-before-render-completion, unbound queue/TEST_ONLY/submit rejection, identity mismatch, generation change, exactly-once binding, late binding to `P+k`, buffer credit, and observability reconciliation.
- [ ] Introduce `Bound(PresentationTarget)` and `DeferredO1(O1PrepareIntent)` at the frame/transaction boundary.
- [ ] Add `ReadyUnbound` to the transaction ledger and reject physical queue/submit transitions until binding promotes it to ordinary `Ready`.
- [ ] Preserve buffer ownership, fence ownership, transaction identity, and safe-abandonment handling for unbound frames.

## 4. Bind only from physical evidence

- [ ] Record the exact presented predecessor anchor and output generation at pageflip completion.
- [ ] Validate that anchor before binding on either completion order; abandon on mismatch without creating a claim.
- [ ] Select the first feasible exact claim strictly after actual `P`, normally `P+1`, using only remaining KMS service evidence and no second render charge.
- [ ] Update the bound transaction/frame target and submit window once; reject a second bind.
- [ ] Ensure no unbound frame reaches worker queue, TEST_ONLY, submit ioctl, kernel-in-flight, bound depth, or overtake recovery.
- [ ] Keep bound `OvertakesReady`/`OvertakesWorkerQueued`, quarantine, and SafeAbandonment behavior unchanged and green.

## 5. Integrate scheduler and observability

- [ ] Represent prepared-unbound separately from ready-bound in the pipeline/scheduler view.
- [ ] Count unbound frames for bounded prepared-buffer credit but not bound physical future depth.
- [ ] Keep render-ahead planning/admission policy unchanged; use predicted timing only as advisory scheduling telemetry until binding.
- [ ] Add minimum bounded unbound lifecycle counters and include them in the existing summary/reconciliation output.
- [ ] Add focused tests for shutdown, output destruction, and abandoned unbound DMA-BUF/GPU-release settlement.

## 6. Verification and qualification

- [ ] Run focused compositor, attribution, transaction, swapchain, pipeline, scheduler, O1, worker, and physical-claim tests.
- [ ] Run fresh `rtk cargo fmt --check`.
- [ ] Run fresh `rtk cargo check`.
- [ ] Run fresh `rtk cargo clippy --all-targets --all-features -- -D warnings`.
- [ ] Run fresh `rtk cargo test`.
- [ ] Run fresh `rtk git diff --check` and `rtk git status --short`.
- [ ] Attempt the approved 1920x1080@165 native qualification command only after deterministic/static GREEN; do not use ydotool, screenshots, machine reconfiguration, or Eclipse/Astrea changes.
- [ ] Write the final report with exact commands/results, deterministic evidence, native evidence or blocker, unchanged architectures, and remaining risks.
- [ ] Commit implementation and report, preserving unrelated edits unstaged.
