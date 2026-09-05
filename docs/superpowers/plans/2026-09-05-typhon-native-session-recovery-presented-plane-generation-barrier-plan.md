# Typhon Native Session Recovery v1 — Presented Plane Physical-Provenance Generation Barrier Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. This checkout is explicitly being executed inline; do not dispatch sub-agents.

**Goal:** Close the native session-resume generation barrier so stale G1 presented-plane provenance cannot fail the first G2 scheduler wake, while preserving exact cursor, swapchain, transaction, DMA-BUF, O1, and pacing semantics.

**Architecture:** Prepare one replacement DRM generation before the synchronous recovery modeset and carry it through cursor preparation, timing, scanout, explicit-sync watches, worker restart, and runtime state. After the modeset and cursor recovery succeed, commit a snapshot-owned physical baseline that clears `primary`, records the exact cursor baseline, and advances the snapshot revision; the first genuine G2 pageflip restores exact primary provenance.

**Tech Stack:** Rust, Cargo, native DRM/KMS, existing Typhon session lifecycle, `AtomicOutputSwapchain`, typed `PresentedPlaneSnapshot`, native perf/session logging, and deterministic unit/state tests.

## Global Constraints

- Do not redesign frame pacing.
- Do not tune the predictor.
- Do not weaken pipeline identity validation.
- Do not modify Eclipse.
- Use the existing checkout and its existing `target/` directory for compilation.
- Use `rtk` for repository and Cargo commands.
- Do not use sub-agents.
- Preserve unrelated concurrent cursor/input edits; stage only this work.
- A synchronous recovery modeset is not a pageflip and must not manufacture pageflip or transaction identity.
- Native qualification may use only a physical/manual VT switch; do not use ydotool, synthetic keyboard injection, screenshots, configuration changes, or Eclipse.

## Files and responsibilities

- Modify `src/native_output/presentation/plane.rs`: own the narrow recovery rebase primitive, hidden cursor baseline constructor if needed, and snapshot-level state tests.
- Modify `src/native_output/output/cursor.rs`: promote the exact state synchronously programmed by recovery without inventing a cursor pageflip; preserve desired/submitted/current semantics.
- Modify `src/native_output/runtime/mod.rs`: store the pending recovery record containing the one prepared generation, scanout recovery token, and exact cursor baseline.
- Modify `src/native_output/runtime/session_io.rs`: prepare and consume the one generation, commit the physical baseline during pageflip retirement before generation rebind, add recovery observability, and update recorder ordering tests.
- Modify `src/native_output/runtime/presentation_pipeline.rs`: add physical-layer RED/GREEN tests proving strict stale-generation validation, `primary=None` validity, and exact first-G2 promotion; production validator code must remain strict.
- Modify `src/native_output/runtime/presentation_pipeline.rs` or the smallest existing validation-base test location: prove revision/generation discontinuity if the current test helpers expose that base directly.
- Modify only additional production files if compilation shows the pending record must be imported/re-exported; do not touch frame-pacing or unrelated cursor-reveal hunks.
- Create `docs/superpowers/specs/2026-09-05-typhon-native-session-recovery-presented-plane-generation-barrier-design.md`.
- Create `docs/superpowers/plans/2026-09-05-typhon-native-session-recovery-presented-plane-generation-barrier-plan.md`.
- Create `docs/superpowers/specs/REPORT-2026-09-05-typhon-native-session-recovery-presented-plane-generation-barrier.md` after implementation and qualification.

## Task 1: Record the approved design and implementation plan

**Files:**

- Create the design and plan files listed above.

**Interfaces:**

- Produces the approved state-transition contract used by Tasks 2–6.

- [x] **Step 1: Write the design document**

  Record the source root cause, strict-validator call path, physical-provenance invariants, one-generation architecture, primary/cursor semantics, slot safety, validation-base revision, ownership preservation, upstream comparison, test architecture, and out-of-scope systems.

- [x] **Step 2: Write this implementation plan**

  Keep every production/test step concrete, preserve the user’s inline/no-sub-agent constraint, and include the final static/native/report gates.

- [ ] **Step 3: Commit the documentation checkpoint**

  Run:

  ```bash
  rtk git add docs/superpowers/specs/2026-09-05-typhon-native-session-recovery-presented-plane-generation-barrier-design.md docs/superpowers/plans/2026-09-05-typhon-native-session-recovery-presented-plane-generation-barrier-plan.md
  rtk git commit -m "docs: design native session generation barrier"
  ```

## Task 2: Add the snapshot barrier RED tests

**Files:**

- Modify `src/native_output/presentation/plane.rs`.
- Modify `src/native_output/runtime/presentation_pipeline.rs`.

**Interfaces:**

- Consumes existing `PresentedPlaneSnapshot`, `PresentedPrimaryState`, `PlanePageflipIdentity`, `AtomicOutputSwapchain`, and pipeline test fixtures.
- Produces `PresentedPlaneSnapshot::rebase_after_session_recovery(PresentedCursorState)` and deterministic tests for its exact semantics.

- [ ] **Step 1: Write a physical snapshot RED test before implementation**

  Build a snapshot with a valid G1 composed primary and a distinct cursor state, call the planned barrier API, and assert `primary == None`, the cursor equals the supplied baseline, and the revision changes. The test must initially fail because the API does not exist.

- [ ] **Step 2: Run the focused test and capture the intended RED**

  Run:

  ```bash
  rtk cargo test --lib native_output::presentation::plane -- --exact session_recovery_rebase_retires_primary_and_advances_revision
  ```

  Expected: compile/test failure for the missing recovery barrier API, or—if the surrounding concurrent checkout is not compiling—an exact unrelated compile error must be recorded without changing that code.

- [ ] **Step 3: Add the stale G1/G2 pipeline RED**

  Reuse the existing completed-composed fixture. Validate the G1 snapshot at generation 1, then call the closest production builder at generation 2 while retaining the G1 primary and assert:

  ```rust
  Err(PipelineSnapshotError::IdentityMismatch {
      owner: "current_composed",
      field: "output_generation",
      ..
  })
  ```

  This locks the validator as correct and prevents “fixing” the failure by weakening it.

- [ ] **Step 4: Commit the RED tests**

  Stage only the test hunks/files after checking `rtk git diff` against the pre-existing cursor edits, then commit:

  ```bash
  rtk git commit -m "test: reproduce stale presented provenance after recovery"
  ```

## Task 3: Implement the snapshot and cursor synchronous-modeset primitives

**Files:**

- Modify `src/native_output/presentation/plane.rs`.
- Modify `src/native_output/output/cursor.rs`.

**Interfaces:**

- `PresentedPlaneSnapshot::rebase_after_session_recovery(cursor: PresentedCursorState)` clears only old primary provenance, stores the exact cursor baseline, and increments `PlaneStateRevision`.
- `NativeAtomicCursor` exposes an accurately named synchronous-modeset promotion path used by both initial setup and recovery, with no `PageFlipToken` or presentation transaction.

- [ ] **Step 1: Implement the minimal snapshot barrier**

  Add:

  ```rust
  pub(crate) fn rebase_after_session_recovery(&mut self, cursor: PresentedCursorState) {
      self.primary = None;
      self.cursor = cursor;
      self.revision = self.revision.next();
  }
  ```

  Use a repository-appropriate hidden constructor if the recovery path needs to represent the absence of an atomic cursor. Do not add `RecoveredBaseline` unless a test demonstrates that `None` breaks an existing ownership invariant.

- [ ] **Step 2: Extract/reuse synchronous cursor promotion**

  Preserve the behavior currently in `mark_initial_submitted`: clone the exact KMS state (or construct hidden state), set `submitted` and `current` to that state, synchronize the submitted epoch, update presented cursor revision, clear dirty state, and confirm initial-clear lifecycle. Give the common helper a name that states it promotes a synchronous modeset, and have initial setup and recovery call it.

- [ ] **Step 3: Run the focused physical tests**

  Run:

  ```bash
  rtk cargo test --lib native_output::presentation::plane
  rtk cargo test --lib native_output::runtime::presentation_pipeline
  ```

  Expected: the snapshot barrier and strict validator tests pass; any failure in unrelated concurrent cursor/input code is recorded exactly.

- [ ] **Step 4: Commit the primitive checkpoint**

  Stage only the intended hunks and commit:

  ```bash
  rtk git commit -m "fix: add session recovery physical provenance barrier"
  ```

## Task 4: Carry and consume one exact recovery generation

**Files:**

- Modify `src/native_output/runtime/mod.rs`.
- Modify `src/native_output/runtime/session_io.rs`.

**Interfaces:**

- Add a private pending recovery record containing `NativeScanoutRecovery`, `generation: u64`, and the exact `PresentedCursorState` baseline.
- `recover_kms_pipeline` allocates once, passes that generation to cursor preparation, and stores the record only after the synchronous modeset succeeds.
- `rearm_explicit_sync` consumes the stored generation for timing, scanout, watches, worker/runtime state; it does not allocate a second generation.
- `retire_quarantined_pageflip` commits the pending record's physical baseline after scanout/transaction retirement and before `rearm_explicit_sync` calls the exact generation rebind.

- [ ] **Step 1: Add the pending recovery record and baseline observability**

  Keep the existing method order. Add a recovery-only `PresentedPlaneRebase` observation immediately after `retire_quarantined_pageflip` commits the snapshot baseline and before explicit-sync rearm. Preserve all existing lifecycle and failure assertions.

- [ ] **Step 2: Make recovery preparation transactional**

  In `recover_kms_pipeline`, call `allocate_native_drm_file_generation()` once before `prepare_for_recovery`, pass the local value to `NativeAtomicCursor::prepare_for_recovery`, and install the pending record only after `recover_with_cursor` succeeds. On a modeset error, leave `self.drm_file_generation` and the old presented snapshot unchanged.

- [ ] **Step 3: Promote exact cursor state after successful modeset**

  Pass the same cloned `cursor_kms_state` to KMS and the synchronous cursor promotion helper. Store `cursor.presented_plane_state()` after promotion. If no atomic cursor plane is available or software delivery is selected, store a hidden hardware-plane baseline and preserve the existing hard-error/fallback policy.

- [ ] **Step 4: Rebind the prepared generation**

  In `rearm_explicit_sync`, read the pending record’s generation, reconfigure timing, scanout, acquire watches, and worker using that exact value, then set `self.drm_file_generation` to the consumed value. Do not call the allocator here. Keep the existing worker restart and parked-watch behavior.

- [ ] **Step 5: Commit the physical baseline before generation rebind**

  Keep the pending record through `retire_quarantined_pageflip`. After the recovery scanout token and suspended transactions are retired, clear stale primary provenance, store the exact cursor baseline, advance snapshot revision, and log one recovery-only generation-barrier event. Retain the pending generation for `rearm_explicit_sync`, then clear the record after exact generation rebind succeeds. The operation must not create a pageflip, transaction, serial, timestamp, or DMA-BUF correlation.

- [ ] **Step 6: Run the focused recovery tests**

  Run:

  ```bash
  rtk cargo test --lib native_output::runtime::session_io
  rtk cargo test --lib native_output::runtime::presentation_pipeline
  rtk cargo test --lib native_output::native_output -- --list
  ```

  Use the repository’s actual module test path if the last listing command differs; do not alter unrelated code to satisfy the command.

- [ ] **Step 7: Commit the generation/recovery checkpoint**

  Stage only the pending-record, session, and direct physical-layer test hunks and commit:

  ```bash
  rtk git commit -m "fix: carry one generation through native session recovery"
  ```

## Task 5: Complete deterministic GREEN coverage and non-regression checks

**Files:**

- Modify the smallest relevant existing test modules under `src/native_output/presentation`, `src/native_output/runtime`, `src/native_output/native/kms`, and `src/native_output/scanout`.

**Interfaces:**

- Tests consume the production snapshot, cursor, swapchain, pageflip, KMS-validation, direct-ownership, transaction, O1, and Wake Authority types; no test-only Recorder can substitute for physical state coverage.

- [ ] **Step 1: Prove immediate G2 scheduler validity**

  Start with a valid G1 composed primary, apply the successful recovery barrier, rebind the test generation to G2, and call the closest `validate_output_pipeline`/scheduler-wake builder immediately. Assert success without a pageflip.

- [ ] **Step 2: Prove first real G2 provenance**

  From `primary == None`, complete a genuine G2 composited pageflip and assert the promoted primary contains the exact G2 token/bundle, transaction, slot, framebuffer, pool generation, and presentation serial. Keep a copy of the original G1 identity and assert it is still G1.

- [ ] **Step 3: Cover late and repeated recovery**

  Reuse existing stale-generation pageflip handling to assert a late G1 event cannot promote or resurrect state. Run the state model through G1→G2→G3, asserting no stale primary, stale validation base, or stale confirmed presentation at each barrier.

- [ ] **Step 4: Cover no prior primary and direct primary**

  Assert `primary == None` remains valid through recovery with no prior primary. Build a valid G1 direct primary, recover to G2, and assert it is cleared while existing suspended direct ownership/terminal settlement remains authoritative.

- [ ] **Step 5: Cover cursor baselines**

  Test visible hardware recovery to replacement framebuffer, hidden recovery, atomic cursor-plane disappearance in hard-error and fallback modes, and exact equality of kernel-requested/current/submitted/presented cursor state. Assert the old framebuffer cannot be presented as current after recovery.

- [ ] **Step 6: Cover revision and slot safety**

  Assert snapshot revision changes at the barrier, validation bases include the new generation/revision, and `primary=None` does not increase renderable slots or select `AtomicOutputSwapchain::current()` as a render target.

- [ ] **Step 7: Run focused non-regression tests**

  Run:

  ```bash
  rtk cargo test --lib native_output::native::kms
  rtk cargo test --lib native_output::scanout
  rtk cargo test --lib native_output::runtime::presentation_o1
  rtk cargo test --lib native_output::runtime::wake_plan
  rtk cargo test --lib native_output::runtime::session_io
  ```

  Retain existing direct ownership, OutputTransaction, DMA-BUF, PrimaryRefreshClaim, predictor, worker, and Wake Authority assertions; do not modify their policy.

- [ ] **Step 8: Commit the deterministic coverage checkpoint**

  ```bash
  rtk git commit -m "test: cover native recovery generation barrier states"
  ```

## Task 6: Verify, qualify, and write the final report

**Files:**

- Create `docs/superpowers/specs/REPORT-2026-09-05-typhon-native-session-recovery-presented-plane-generation-barrier.md`.

**Interfaces:**

- Consumes source/test evidence, exact command outputs, binary provenance, and any available physical VT qualification evidence.
- Produces an English report that answers every adversarial review question from the approved spec, including inconclusive native items when the environment prevents qualification.

- [ ] **Step 1: Run fresh static verification**

  Run each command in the existing checkout and record exact output/result:

  ```bash
  rtk cargo fmt --check
  rtk cargo check
  rtk cargo clippy --all-targets --all-features -- -D warnings
  rtk cargo test
  rtk git diff --check
  rtk git status --short
  ```

  If unrelated concurrent work blocks a command, identify the exact failure, run focused verification for this task, and report the checkout as not globally green.

- [ ] **Step 2: Build and record binary provenance**

  ```bash
  rtk git rev-parse HEAD
  rtk git status --short
  rtk cargo build --release
  rtk sha256sum target/release/oblivion-one
  rtk stat target/release/oblivion-one
  ```

- [ ] **Step 3: Perform manual VT qualification if the physical host permits**

  Start the approved command with `TYPHON_FRAME_PACING_DEBUG=1` and the existing native session/perf logging enabled. While Typhon is active, use only the physical keyboard to switch to another VT and back; exercise hover/magnification, settings, XWayland client scrolling/popups, idle, and interaction after resume. Repeat three cycles where practical. Record exact transitions, G1→G2 evidence, first G2 pageflip evidence, counters, and clean shutdown. If the environment cannot safely perform the roundtrip, state that native qualification is inconclusive rather than implying success.

- [ ] **Step 4: Write the final report**

  Include source root cause, before/after state diagram, changed files, RED/GREEN results, cursor and slot-aliasing findings, generation audit, static/build provenance, manual VT evidence, cycle count, first post-resume pageflip, O1 reconciliation, fast-client attribution, 165 Hz cadence, Wake Authority, transactions, DMA-BUF, protocol, and SafeDisable. Answer each adversarial question directly with source or test evidence.

- [ ] **Step 5: Commit the report and final implementation**

  Run `rtk git diff --check`, inspect `rtk git diff --cached`, stage only this task’s final report and code hunks, and commit:

  ```bash
  rtk git commit -m "fix: close native session recovery provenance barrier"
  ```

- [ ] **Step 6: Perform final completion verification**

  Re-run `rtk git status --short`, `rtk git log -5 --oneline`, and the required verification command(s) after the final commit. Do not claim global green status if unrelated work remains or a required check is blocked.
