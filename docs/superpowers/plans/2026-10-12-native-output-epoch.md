# First-Class Output Epoch and Reconfiguration Barrier Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split DRM session and physical output lifetimes, then make output reconfiguration seal and publish epochs through one ownership barrier.

**Architecture:** Keep the existing prepared Atomic candidate, Display transaction, KMS worker, render worker, and presentation ledger authoritative. Add typed session/output IDs, turn the current safe-boundary checks into a quiescence proof, centralize READY terminalization, and rebase all epoch-local state in one publication method.

**Tech Stack:** Rust, Cargo workspace tests, existing DRM/Atomic EGL GBM scanout and native-output worker infrastructure.

## Global Constraints

- Work on the current `main`; do not create a branch.
- Do not modify Eclipse.
- Keep all build/test output under `/mnt/Aether/Desktop/GitHub`; use `/mnt/Aether/Desktop/GitHub/Typhon-target` for Cargo.
- Commit each completed logical change and stage only task-owned files.
- Keep the production Display qualification constant false and preserve the explicit `OBLIVION_ONE_OUTPUT_MODE_MUTATION=qualification` gate.
- Do not add hotplug support or migrate the normal pipeline wholesale to FrameLanes.
- Keep pool generation distinct from output epoch and keep Settings `OutputConfigurationGeneration` semantic.
- Do not add locks, allocations, virtual dispatch, or global lookups to ordinary frame presentation.

---

## Task 1: Typed native-output lifetimes

**Files:**
- Modify: `src/native_output/output/configuration.rs`, `src/native_output/runtime/bootstrap.rs`, `src/native_output/runtime/mod.rs`, `src/native_output/runtime/session_io.rs`, `src/native_output/scanout/feedback.rs`, `src/native/explicit_sync.rs`
- Migrate physical identity fields in: `src/native_output/kms_worker/`, `src/native_output/presentation/`, `src/native_output/runtime/`, `src/native_output/scanout/`, and their focused tests.

**Interfaces:**
- Produce `DrmSessionGeneration` and `OutputEpochId`, each a `#[repr(transparent)]` copyable newtype with `initial`, `get`, and typed advance operations.
- Preserve `OutputConfigurationGeneration` without conversion to `OutputEpochId`.
- Keep `pool_generation` a separate scalar identity.

- [ ] Add tests proving output epoch advances independently of session generation and that initial IDs are nonzero.
- [ ] Run the focused output identity and explicit-sync tests with `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target`.
- [ ] Change runtime bootstrap to initialize both IDs; session recovery allocates a new session ID and stages the next output epoch.
- [ ] Migrate former `drm_file_generation` uses according to lifetime: physical worker/pageflip/cursor/timing/cache/watch uses become output epoch; only session-file recovery identity becomes `DrmSessionGeneration`.
- [ ] Verify candidate scanout pool generation remains independently sourced and no Settings generation is used as a physical epoch.
- [ ] Commit as `refactor(output): split DRM session and output epochs`.

## Task 2: Shared reconfiguration boundary and commit proof

**Files:**
- Modify: `src/native_output/output/configuration.rs`, `src/native_output/runtime/output_configuration.rs`, `src/native_output/runtime/session_io.rs`, and `src/native_output/runtime/output_configuration_qualification_tests.rs`.

**Interfaces:**
- Rename `OutputConfigurationSafeBoundary` to `NativeOutputReconfigurationBoundary`.
- Add `QuiescedOutputReconfiguration { sealed_epoch: OutputEpochId }`, constructible only from a safe boundary after the current epoch matches.
- Require the proof in the real structural commit path after TEST_ONLY.

- [ ] Add boundary tests for existing pageflip, worker inflight/queued, arbiter, transaction, Direct Scanout, cursor, pacing reservation, render fence, deferred event, and explicit-sync ownership checks.
- [ ] Add a test that a mismatched epoch cannot produce a quiescence proof.
- [ ] Keep the existing candidate object unchanged from TEST_ONLY through real commit and adoption.
- [ ] Run output-configuration qualification tests and commit as `refactor(output): require quiescence before structural commits`.

## Task 3: Authoritative unsubmitted-frame terminalization

**Files:**
- Modify: `src/native_output/runtime/cycle/pageflip.rs`, `src/native_output/runtime/output_configuration.rs`, `src/native_output/pacing.rs`, `src/native_output/runtime/scene_history.rs`, `src/native/scheduler.rs`, `src/native_output/presentation/ledger.rs`, `src/native_output/scanout/output_swapchain.rs`.

**Interfaces:**
- Add `NativeRuntime::terminalize_unsubmitted_output_frame(owner, at) -> NativeResult<()>`.
- Add sidecar read/retire operations that validate the exact frame and epoch before consuming ownership.
- Preserve `abandon_ready_frame()` failure semantics when no exact pacing owner exists.

- [ ] First add the regression fixture for a physical READY frame deferred on its render fence while pacing owns the same exact frame in the active lane; assert terminalization and transaction settlement happen once.
- [ ] Run the regression and confirm it fails because the existing READY-only retirement rejects the active owner.
- [ ] Implement pre-mutation validation across physical READY, pacing active/READY, scene READY, scheduler state, and output transaction.
- [ ] Settle the transaction and protocol obligations, then scene, scheduler, pacing, and physical swapchain ownership exactly once.
- [ ] Add a deliberately mismatched-owner case that returns a typed invariant error without clearing any sidecar.
- [ ] Re-run the regression and commit as `fix(output): terminalize unsubmitted READY ownership atomically`.

## Task 4: Coherent epoch publication and rollback

**Files:**
- Modify: `src/native_output/runtime/output_configuration.rs`, `src/native_output/runtime/session_io.rs`, `src/native_output/runtime/bootstrap.rs`, `src/native_output/pacing.rs`, `src/native_output/runtime/mod.rs`, and affected scanout/presentation identity modules.

- [ ] Add a publication test proving scheduler and pacing old-frame ownership are empty before new visual work is admitted.
- [ ] Replace independent publication statements with `publish_new_output_epoch(target, output_epoch, ...)`.
- [ ] Rebase scanout/direct validation, Settings configuration generation, server/input dimensions, cursor, presented planes, scene history, timing/deadline, scheduler, pacing, render journal, and adaptive state within that boundary.
- [ ] Assert temporary apply and rollback each publish a distinct output epoch while Settings configuration generation remains separate.
- [ ] Run the six existing Display qualification scenarios and commit as `refactor(output): publish output epochs through one rebase`.

## Task 5: Stale asynchronous completion hardening

**Files:**
- Modify: `src/native_output/runtime/cycle/pageflip.rs`, `src/native_output/runtime/kms_worker.rs`, `src/native_output/runtime/render_worker.rs`, `src/native_output/runtime/presentation_cycle.rs`, `src/native_output/runtime/presentation_worker.rs`, `src/native_output/scanout/render_worker.rs`, `src/native_output/kms_worker/`, `src/native/explicit_sync.rs`, `src/native_output/presentation/`, and focused tests.

- [ ] Add tests for a KMS completion, pageflip, render-worker completion, and explicit-sync wake carrying the previous output epoch.
- [ ] Run those tests and verify each currently-current-state mutation is observable before the fix.
- [ ] On stale work, retire its own buffer/fence/worker transport ownership and telemetry, then return before current output presentation, pacing, scheduler, transaction, or timing state mutates.
- [ ] Include `OutputEpochId` in Direct Scanout and plane validation identities; preserve independent pool-generation checks.
- [ ] Run focused KMS worker, render worker, explicit-sync, pageflip, and Direct Scanout tests; commit as `fix(output): retire stale epoch completions safely`.

## Task 6: Qualification and full verification

**Files:**
- Modify: the Display qualification document and focused runtime test modules only if test results require additional coverage.
- Inspect: production mode-mutation policy and qualification harness without enabling the production constant.

- [ ] Add the READY-at-reconfiguration crash scenario to the qualification document, including the fixed lifecycle and the requirement for explicit opt-in hardware policy.
- [ ] Run `cargo fmt --check`.
- [ ] Print and verify `realpath -m /mnt/Aether/Desktop/GitHub/Typhon-target` before each Cargo command.
- [ ] Run `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target cargo check --bin oblivion-one`.
- [ ] Run `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target cargo test --workspace --all-targets --no-run`.
- [ ] Run the focused output-configuration, qualification, swapchain, pacing, ledger, KMS worker, render worker, explicit-sync, session-recovery, and stale-completion suites.
- [ ] Review the diff for accidental production-gate, Eclipse, hotplug, FrameLanes, and ordinary-frame synchronization changes; commit as `test(output): cover READY epoch reconfiguration failures`.
- [ ] If deterministic regressions pass and the NVIDIA display is available, run only `1920x1080@165 → 1920x1080@60 → Apply → Revert` with the three specified qualification environment variables; capture journal and Display transaction state.

## Spec coverage

Typed lifetimes, boundary reuse, quiescence proof, exact READY retirement, publication rebase, rollback epochs, stale asynchronous work, Direct Scanout validation, explicit-sync lifetime, six existing qualification scenarios, production gating, performance constraints, and qualification documentation are assigned above. Hotplug and FrameLanes remain explicit non-goals.
