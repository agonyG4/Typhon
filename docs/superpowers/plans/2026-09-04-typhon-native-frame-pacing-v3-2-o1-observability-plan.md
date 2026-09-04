# Typhon Native Frame Pacing v3.2 O1 Observability Implementation Plan

> **For agentic workers:** This plan is being executed inline with `superpowers:executing-plans` because the user explicitly prohibited sub-agents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reconcile Predictive O1 observability by carrying exact frame-local origin through a bounded lifecycle ledger and separating it from generic READY waiting.

**Architecture:** Keep the accepted v3/v3.1 physical architecture unchanged. Add a fixed four-entry exact-ID lifecycle ledger to `NativeFramePacing`; mark stages at existing render, bind, worker, kernel-submit, and physical-pageflip boundaries, and terminalize by exact ID. Preserve legacy counters as compatibility views with corrected semantics and expose an explicit shutdown reconciliation equation.

**Tech Stack:** Rust, Cargo, existing native output pacing/runtime, fixed-size arrays, `rtk` command wrapper, unit and integration tests.

## Global Constraints

- Do not redesign O1, tune the predictor, or alter physical claim semantics.
- Do not change O1 admission, Deferred O1 binding, `PrimaryRefreshClaim`, KMS submit timing, worker policy, ReactiveDouble, predictor, scheduler, DMA-BUF ownership, OutputTransaction semantics, Native Wake Authority, or fast-client attribution.
- Do not add an unbounded `HashMap`, `Vec`, per-frame retained history, collector, thread, timer, or hot-path logging.
- Use the smallest bounded representation consistent with the existing pipeline; the implementation bound is four exact pacing ownership slots.
- Compile and build in `/home/agony/GitHub/Typhon` so Cargo uses the existing local `target/` directory.
- Preserve the unrelated dirty changes in `src/compositor/keyboard.rs`, `src/compositor/server_toplevel.rs`, `src/compositor/state/input_resources.rs`, `src/native_output/input/events.rs`, and `src/native_output/input/routing.rs`.
- Run commands through `rtk`; make focused commits after independently testable work.

## File map

- Modify `src/native_output/pacing.rs`: exact origin, fixed lifecycle ledger, stage/terminal counters, compatibility mappings, summary fields, and deterministic tests.
- Modify `src/native_output/runtime/presentation_cycle.rs`: pass exact render-ready/failure boundaries and classify READY by frame origin.
- Modify `src/native_output/runtime/cycle/pageflip.rs`: record exact pageflip presentation and preserve v3.1 stale/bind behavior.
- Modify `src/native_output/runtime/kms_worker/rejection.rs`: retain existing worker cancellation while using exact terminal identity.
- Modify `src/native_output/runtime/mod.rs`: drain remaining exact lifecycle entries into `current_at_shutdown` before summary.
- Create `docs/superpowers/specs/2026-09-04-typhon-native-frame-pacing-v3-2-o1-observability-design.md` and this plan.
- Create `docs/superpowers/specs/REPORT-2026-09-04-typhon-native-frame-pacing-v3-2-o1-observability.md` after implementation and qualification.

### Task 1: Record the approved v3.2 design

**Files:**

- Create `docs/superpowers/specs/2026-09-04-typhon-native-frame-pacing-v3-2-o1-observability-design.md`
- Create `docs/superpowers/plans/2026-09-04-typhon-native-frame-pacing-v3-2-o1-observability-plan.md`

**Interfaces:**

- Produces the approved design, boundedness decision, event boundaries, terminal equation, and execution order used by later tasks.

- [x] **Step 1: Write the design and plan documents**

  Include the root causes, exact origin, four-entry ledger, stage transitions, terminal equation, legacy compatibility, unchanged subsystems, and verification gates.

- [x] **Step 2: Self-review the documents**

  Check for placeholders, contradictory stage/terminal semantics, unbounded state, and any accidental policy change. The documents contain no unresolved placeholder.

- [x] **Step 3: Commit the documentation**

```bash
rtk git add docs/superpowers/specs/2026-09-04-typhon-native-frame-pacing-v3-2-o1-observability-design.md docs/superpowers/plans/2026-09-04-typhon-native-frame-pacing-v3-2-o1-observability-plan.md
rtk git commit -m "docs: design Predictive O1 observability reconciliation"
```

### Task 2: Add RED tests for exact origin and overlap

**Files:**

- Modify `src/native_output/pacing.rs` in the existing `#[cfg(test)] mod tests`.

**Interfaces:**

- Consumes the current `NativeFramePacing` test helpers and existing worker reservation APIs.
- Produces failing behavioral tests for normal waiting, N-plus-M reconciliation, overlapping exact IDs, reversed terminal order, duplicate submit, abandonment, shutdown, stage progression, and non-Predictive isolation.

- [ ] **Step 1: Write the normal waiting RED test**

  Model `render_ahead == false` with the existing predictive pacing mode and `waits_for_target == true`; assert `normal_ready_wait_count == 1`, `predictive_render_ahead_ready == 0`, and `predictive_ready_created == 0`.

- [ ] **Step 2: Write the N-plus-M reconciliation RED test**

  Complete two exact Predictive O1 frames and queue three normal waiting READY frames; assert Predictive O1 ready/render-ready counts remain two and the normal wait count is three.

- [ ] **Step 3: Write the overlapping and reversed-order RED tests**

  Keep P1 worker-reserved while P2 reaches READY, submit P1 first, then P2; separately abandon a newer READY identity before presenting an older pending identity; assert both exact lifecycles reconcile once.

- [ ] **Step 4: Write duplicate, abandonment, shutdown, progression, and isolation RED tests**

  Assert duplicate submit does not increment twice; generation and identity abandonment close without submit; shutdown reports each remaining exact identity; submitted-then-presented records every applicable stage; ReactiveDouble, normal, and direct paths leave Predictive O1 counts unchanged.

- [ ] **Step 5: Run the new tests and verify the intended RED failures**

```bash
rtk cargo test native_output::pacing -- --nocapture
```

Expected: the new assertions fail because generic `waits_for_target` and `predictive_ready_frame_id: Option<_>` still misclassify or lose exact lifecycle identities.

### Task 3: Implement the bounded exact-ID ledger

**Files:**

- Modify `src/native_output/pacing.rs` around `PredictiveReadyTerminal`, `WorkerPacingReservation`, `NativeFramePacing`, lifecycle methods, and summary output.

**Interfaces:**

- Consumes `NativeOutputFrameId`, existing active/ready/pending/worker reservation ownership, and existing legacy counters.
- Produces `PreparedFrameOrigin`, a fixed `[Option<PredictiveO1LifecycleEntry>; 4]`, exact stage methods, exact terminal methods, new stage/terminal summary fields, and no single-frame predictive terminal tracker.

- [ ] **Step 1: Add the origin and ledger types**

```rust
enum PreparedFrameOrigin {
    Normal,
    ReactiveDouble,
    PredictiveO1,
}

const PREDICTIVE_O1_LIFECYCLE_CAPACITY: usize = 4;
```

  Store the active origin next to `active` and use an exact-ID entry with a current stage plus observed-stage bits.

- [ ] **Step 2: Insert Predictive O1 at render start**

  Make `note_render_started` classify the exact active frame, insert only `(PredictiveTriple, true)` frames, increment `predictive_o1_created` and `predictive_render_ahead_attempts`, and return an error on the proven fixed-capacity violation.

- [ ] **Step 3: Record render-ready, ready-unbound, bound, and worker-queued stages**

  Add exact methods that increment each stage once. `note_ready_frame` must use the stored active origin and must count normal waiting independently of `waits_for_target`. `reserve_worker_submission` must record the exact reserved ID without consulting a global predictive pointer.

- [ ] **Step 4: Record exact submitted/presented stages and terminals**

  Make `note_submit_frame` classify the passed exact ID, increment the authoritative submit count only at the current worker/immediate submission boundary, and make `note_pageflip` terminalize the exact pending ID only after physical pageflip. Route safe, failed, identity, generation, duplicate, and shutdown paths through exact-ID terminalization.

- [ ] **Step 5: Remove the obsolete single-Option assumption**

  Delete `predictive_ready_frame_id` and update compatibility counters so `predictive_ready_submitted` is driven by the exact submitted READY ID. Preserve existing v3/v3.1 counter names unless a new exact replacement is emitted beside them.

- [ ] **Step 6: Add explicit reconciliation and stage fields to the summary**

  Emit the new stage counters, mutually exclusive terminal counters, active/peak ledger populations, terminal total, remainder, and exact reconciliation boolean. Keep progressive stages separate from terminal accounting.

- [ ] **Step 7: Run the RED tests as the GREEN target**

```bash
rtk cargo test native_output::pacing -- --nocapture
```

Expected: all pacing tests pass, including the new exact-origin and overlap assertions.

### Task 4: Wire exact event boundaries without policy changes

**Files:**

- Modify `src/native_output/runtime/presentation_cycle.rs`.
- Modify `src/native_output/runtime/cycle/pageflip.rs`.
- Modify `src/native_output/runtime/kms_worker/rejection.rs`.
- Modify `src/native_output/runtime/mod.rs`.

**Interfaces:**

- Consumes the ledger methods from Task 3.
- Produces exact render-ready, stale, worker-cancellation, physical-pageflip, render-failure, and shutdown evidence while preserving existing O1 decisions and ownership behavior.

- [ ] **Step 1: Wire render-ready and failure boundaries**

  Call the exact render-ready method only after `AtomicFrameRenderOutcome::Rendered`; call the failure or safe-abandonment terminal method on the existing skipped/error paths; call ready-unbound only when the exact deferred frame remains unbound.

- [ ] **Step 2: Separate generic waiting from Predictive O1**

  Call `note_ready_frame` without passing `waits_for_target` as identity. Keep `waits_for_target` only for scheduling/branch behavior and preserve `normal_ready_wait_count` for non-Predictive READY waits.

- [ ] **Step 3: Preserve pageflip and worker terminal authority**

  Keep `bind_ready_deferred_o1`, physical claim revalidation, worker cancellation, and existing recovery unchanged; add only exact accounting calls at the already authoritative boundaries.

- [ ] **Step 4: Drain all remaining entries at shutdown**

  Replace the single READY tracker shutdown call with an exact ledger drain so every live identity becomes `current_at_shutdown` and the reconciliation equation closes.

- [ ] **Step 5: Run focused runtime tests**

```bash
rtk cargo test deferred_o1 -- --nocapture
rtk cargo test native_output::runtime -- --nocapture
```

Expected: v3/v3.1 Deferred O1 tests and runtime tests pass with no changes to physical recovery or policy.

### Task 5: Write the v3.2 verification report

**Files:**

- Create `docs/superpowers/specs/REPORT-2026-09-04-typhon-native-frame-pacing-v3-2-o1-observability.md`.

**Interfaces:**

- Consumes test, static-verification, release-build, native-launch, and source-review evidence.
- Produces an English report that distinguishes source-proven behavior from hardware qualification and records all unavailable evidence honestly.

- [ ] **Step 1: Record source root causes and exact tracking model**

  Include the 130/138 normal-wait anomaly, 130/100 tracker divergence, frame-local origin, bounded ledger, stages, terminal categories, legacy mappings, and unchanged policy surfaces.

- [ ] **Step 2: Record adversarial answers with evidence**

  Answer normal, ReactiveDouble, Direct Scanout, overlap, duplicate terminal, submit ownership, physical presentation, equation closure, boundedness, and non-regression questions using source/test references.

- [ ] **Step 3: Record fresh verification commands and native result**

  Include exact outputs for format, check, clippy, full tests, diff check, release build/hash, approved launcher/workload, Predictive O1 counters, terminal equation, fast-client, cadence, wake, DMA-BUF, OutputTransaction, protocol, and SafeDisable. Mark any environment-blocked item inconclusive.

- [ ] **Step 4: Commit the report**

```bash
rtk git add docs/superpowers/specs/REPORT-2026-09-04-typhon-native-frame-pacing-v3-2-o1-observability.md
rtk git commit -m "docs: report Predictive O1 observability reconciliation"
```

### Task 6: Fresh final verification and clean handoff

**Files:**

- No source changes expected; inspect all changed files and preserve unrelated user edits.

**Interfaces:**

- Consumes the completed implementation and report.
- Produces fresh verification evidence, a clean scoped commit history, and an explicit native qualification status.

- [ ] **Step 1: Run static verification in the same checkout**

```bash
rtk cargo fmt --check
rtk cargo check
rtk cargo clippy --all-targets --all-features -- -D warnings
rtk cargo test
rtk git diff --check
```

- [ ] **Step 2: Build the release binary in the same checkout**

```bash
rtk cargo build --release
```

  Record the exact binary path and SHA-256.

- [ ] **Step 3: Run the approved native qualification without ydotool, screenshots, or Eclipse changes**

  Use the v3.2 command and workload from the request. Record actual counters and classify permission/session blockers without modifying machine configuration.

- [ ] **Step 4: Inspect the final diff, status, and commits**

```bash
rtk git diff --stat HEAD~6..HEAD
rtk git status --short
rtk git log --oneline -8
```

  Confirm only intended files were added/modified and unrelated dirty files remain intact.
