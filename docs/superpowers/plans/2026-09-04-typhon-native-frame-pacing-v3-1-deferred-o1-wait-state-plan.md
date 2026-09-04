# Typhon Native Frame Pacing v3.1 Deferred O1 Wait-State Implementation Plan

> **For agentic workers:** This plan is executed inline in the current task. The user explicitly requested no sub-agents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Correct false-stale Deferred O1 classification for live predecessors, add deterministic race coverage, and qualify a freshly built exact release binary under shell-hover stress.

**Architecture:** `AtomicOutputSwapchain` will expose one authoritative `DeferredO1BindingReadiness` classifier. `WaitingForPredecessor` preserves the existing `ReadyUnbound` transaction/frame ownership and is consumed by both render-completion and pageflip paths without creating physical ownership. Existing binding, first-feasible-successor, worker, KMS, overtake, quarantine, and terminal paths remain unchanged.

**Tech Stack:** Rust, existing Typhon native-output swapchain and transaction pipeline, deterministic Cargo tests, Cargo release build, SHA-256/stat provenance, and the repository's `/home/agony/.local/bin/rtk` command wrapper.

## Global Constraints

- Do not use sub-agents.
- Run shell commands through `/home/agony/.local/bin/rtk` and compile in the checkout's existing `target/` directory.
- Preserve unrelated working-tree changes and stage only task files.
- Do not modify Eclipse or tune predictor policy.
- Do not redesign `PrimaryRefreshClaim`, `OutputTransactionId`, `SafeAbandonment`, Native Wake Authority, DMA-BUF release ownership, XWayland ownership, ReactiveDouble, CommitTiming, or fast-client attribution.
- Do not add timers, timerfds, eventfds, polling, sleeps, busy retries, blocking GPU waits, unbounded ledgers, or per-frame stdout logging.
- A waiting Deferred O1 frame remains `ReadyUnbound`; it cannot create a claim, enter the worker, run TEST_ONLY, submit to KMS, become kernel-in-flight, trigger stale accounting, be abandoned, or be quarantined merely because the predecessor has not presented.
- A bound `PrimaryRefreshClaim` is immutable and binding occurs at most once.
- Native qualification uses the exact freshly built `target/release/oblivion-one`, the unchanged Eclipse build, no ydotool, no screenshots, and no machine reconfiguration.

## File map

- Modify `src/native_output/scanout/output_swapchain.rs`: define the authoritative readiness classification, route candidate selection through its bindable evidence, and add production-state race/side-effect tests.
- Modify `src/native_output/scanout/atomic_egl_gbm.rs`: consume the readiness classifier and carry waiting through render outcomes without invoking abandonment or physical entry.
- Modify `src/native_output/runtime/presentation_cycle.rs` only if the exact render-outcome integration requires a distinct waiting result; preserve ready-unbound handling and terminal settlement.
- Modify `src/native_output/runtime/cycle/pageflip.rs` only if the exact pageflip integration requires matching the new waiting result; preserve predecessor completion-before-binding ordering.
- Modify `src/native_output/presentation/ledger.rs` only if tests expose a transaction transition gap; do not add a `WaitingForPredecessor` transaction state.
- Create/update `docs/superpowers/specs/2026-09-04-typhon-native-frame-pacing-v3-1-deferred-o1-wait-state-design.md`, the v3 design amendment, this plan, and `REPORT-2026-09-04-typhon-native-frame-pacing-v3-1.md`.

## Task 1: Establish exact baseline and classifier boundary

**Files:**
- Read: `src/native_output/scanout/output_swapchain.rs`, `src/native_output/scanout/atomic_egl_gbm.rs`, `src/native_output/runtime/cycle/pageflip.rs`, `src/native_output/runtime/presentation_cycle.rs`, `src/native_output/presentation/ledger.rs`
- Read: v3 design, plan, report, and tests
- Test: existing `deferred_o1_*` tests in `src/native_output/scanout/output_swapchain.rs`

- [ ] **Step 1: Capture baseline status and provenance**

Run:

```bash
rtk git status --short
rtk git rev-parse HEAD
rtk cargo test deferred_o1 -- --nocapture
```

Expected: clean status, current `fed66fe` baseline, and existing Deferred O1 tests pass before the new regression test is added.

- [ ] **Step 2: Confirm the single classifier seam**

The current seam is `AtomicOutputSwapchain::deferred_o1_binding_failure()` plus `deferred_o1_binding_candidate()`. The new classifier must inspect the ready `DeferredO1` intent, output/pool/clock generations, `last_presented_primary_anchor`, and `deferred_o1_predecessor()`; no second ownership registry may be introduced.

- [ ] **Step 3: Commit the approved design documents**

```bash
rtk git add docs/superpowers/specs/2026-09-04-typhon-native-frame-pacing-v3-1-deferred-o1-wait-state-design.md docs/superpowers/specs/2026-09-04-typhon-native-frame-pacing-v3-design.md docs/superpowers/plans/2026-09-04-typhon-native-frame-pacing-v3-1-deferred-o1-wait-state-plan.md
rtk git commit -m "docs: design Deferred O1 predecessor wait closure"
```

## Task 2: Add the failing live-predecessor regression tests

**Files:**
- Modify: `src/native_output/scanout/output_swapchain.rs` test module
- Test: focused swapchain tests in the same file

- [ ] **Step 1: Add the pending-predecessor RED test**

Construct the existing submitted predecessor, retain its exact `O1PredecessorAnchor`, render a Deferred O1 successor, and call the production readiness/binding seam before the predecessor pageflip. Assert the result is expected to be waiting and that the ready frame remains deferred with the same identity and no claim.

- [ ] **Step 2: Run the new test and verify the intended RED**

```bash
rtk cargo test deferred_o1_waits_for_live_pending_predecessor -- --nocapture
```

Expected: failure caused by the old `A != B` identity-mismatch classification, not a test construction or compile error.

- [ ] **Step 3: Add worker-queued and pageflip-during-render RED coverage**

Cover the equivalent `worker_queued` live owner if the existing swapchain test helpers expose that transition, and cover the existing pageflip-before-render-completion sequence. The first case must wait; the second must bind immediately after render completion.

- [ ] **Step 4: Add stale/generation/side-effect RED assertions**

Retain true stale identity and generation mismatch tests. Add repeated waiting calls, no worker/TEST_ONLY/submit entry while waiting, exactly-once binding after predecessor presentation, and terminal settlement coverage using existing mechanisms. Assertions must prove no claim, quarantine, stale counter, duplicate transaction transition, or ownership leak is created by waiting.

## Task 3: Implement the authoritative classifier minimally

**Files:**
- Modify: `src/native_output/scanout/output_swapchain.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/runtime/presentation_cycle.rs` only if required by the result shape
- Modify: `src/native_output/runtime/cycle/pageflip.rs` only if required by the result shape

- [ ] **Step 1: Define one readiness result**

Use a copyable enum with `NotDeferred`, `WaitingForPredecessor`, `Bindable { predecessor, actual_claim }`, and `Stale(DeferredO1BindingFailure)` variants, following local visibility and naming conventions.

- [ ] **Step 2: Implement ordered classification**

Validate output/pool/clock generations first. Return `Bindable` for an exact last-presented predecessor. If that identity differs but `deferred_o1_predecessor()` exactly equals the intent predecessor, return `WaitingForPredecessor`. Return identity stale only when no exact live predecessor remains. Keep generation mismatch terminal.

- [ ] **Step 3: Make candidate selection consume bindable evidence**

Remove the independent identity interpretation from `deferred_o1_binding_candidate()`. It may calculate only the first feasible successor from the classifier's actual claim and existing `KmsSubmitWindow::rebind()` behavior.

- [ ] **Step 4: Map waiting without side effects**

Update `DeferredO1BindingResult` and the render/pageflip matches so waiting is the same prepared-unbound path as `NotReady`, while stale alone enters existing exact safe abandonment. Do not introduce a timer, transaction state, physical claim, or worker action for waiting.

- [ ] **Step 5: Run focused tests GREEN**

```bash
rtk cargo test deferred_o1 -- --nocapture
rtk cargo test deferred_o1_binding -- --nocapture
```

Expected: all focused Deferred O1, claim, and unbound-entry tests pass, including the new race cases.

- [ ] **Step 6: Commit the implementation and deterministic tests**

```bash
rtk git add src/native_output/scanout/output_swapchain.rs src/native_output/scanout/atomic_egl_gbm.rs src/native_output/runtime/presentation_cycle.rs src/native_output/runtime/cycle/pageflip.rs
rtk git commit -m "fix: wait for live Deferred O1 predecessor"
```

## Task 4: Static verification and release provenance

**Files:**
- Modify: `REPORT-2026-09-04-typhon-native-frame-pacing-v3-1.md`

- [ ] **Step 1: Run all required static checks**

```bash
rtk cargo fmt --check
rtk cargo check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk cargo test
rtk git diff --check
```

Expected: each command exits zero with no weakened tests or warnings promoted to errors.

- [ ] **Step 2: Build release in the checkout target directory**

```bash
rtk git rev-parse HEAD
rtk git status --short
rtk cargo build --release
rtk run "sha256sum target/release/oblivion-one"
rtk run "stat target/release/oblivion-one"
```

Record exact output in the report. The runtime executable must be the just-built binary, not a copied historical artifact.

## Task 5: Native qualification and decision gate

**Files:**
- Create: `REPORT-2026-09-04-typhon-native-frame-pacing-v3-1.md`

- [ ] **Step 1: Run the approved exact-binary launch command**

Use the command from the spec with the freshly built checkout binary and unchanged Eclipse shell build. Do not use ydotool, screenshots, Eclipse edits, or machine configuration changes.

- [ ] **Step 2: Exercise the manual hover workload**

Exercise Dock enter/leave and magnification, rapid icon movement, topbar/tray hover, tooltips, popups, menu hover, idle/resume, and a normal application between shell segments. Record whether input, cursor, client commits, render cycles, KMS submissions/pageflips, and other clients continue making progress.

- [ ] **Step 3: Capture fresh Typhon and client evidence**

Record O1 creation/readiness/binding/submission and abandonment counters, physical cadence, Native Wake Authority counters, DMA-BUF reconciliation, OutputTransaction reconciliation, shutdown result, and any exact `current_composed/output_generation` failure. Classify the freeze as Typhon, Eclipse, or inconclusive only from fresh evidence.

- [ ] **Step 4: Write the final report and adversarial answers**

Include the source root cause, corrected state machine, all RED/GREEN tests, provenance, native result, preserved invariants, decision gate, and one evidence-backed answer for every adversarial question in the spec. State unavailable native evidence as unavailable; do not infer hardware proof from historical logs.

- [ ] **Step 5: Commit the report**

```bash
rtk git add REPORT-2026-09-04-typhon-native-frame-pacing-v3-1.md
rtk git commit -m "docs: report Deferred O1 wait-state qualification"
```

## Final verification checklist

- [ ] Re-read this plan and the v3.1 design.
- [ ] Run `rtk git diff --check` and `rtk git status --short`.
- [ ] Confirm only task commits changed task files and unrelated files remain untouched.
- [ ] Report exact command results, including any native environment blocker or inconclusive decision.
