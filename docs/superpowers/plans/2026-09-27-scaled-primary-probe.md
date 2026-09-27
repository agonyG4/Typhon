# Scaled Primary Plane Capability Probe Implementation Plan

> **For agentic workers:** This plan is executed inline because the repository instructions prohibit subagents. Steps use checkbox syntax for tracking.

**Goal:** Add an opt-in, diagnostic-only DRM primary-plane scaling TEST_ONLY probe for otherwise-valid fullscreen DMA-BUF scenes whose source dimensions differ from output mode dimensions.

**Architecture:** Preserve the production direct candidate API and derive a distinct probe candidate from the same scene analysis, tolerating only the size mismatch. Build a reusable full-geometry primary-plane atomic request and execute the probe through the existing KMS worker only when the worker is completely idle. Store accepted and rejected results in a bounded probe-only cache, expose gated counters, and leave all frame planning and ownership untouched.

**Tech Stack:** Rust, DRM atomic KMS, Wayland scene analysis, existing KMS worker, Rust unit tests.

## Global Constraints

- Never place compilation output or caches inside the checkout or on the system SSD; all build and test output is under `/mnt/Aether/Desktop/GitHub`.
- Set `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target` for every Cargo build or test command.
- Verify the effective target directory before running any compilation command.
- Work on `main`; do not create branches.
- Commit only completed logical changes from this task; preserve pre-existing worktree edits.
- Do not change production direct-scanout eligibility or presentation ownership.

---

### Task 1: Full source-to-output geometry and atomic request builder

**Files:**
- Modify: `src/native/kms/atomic.rs`
- Modify: `src/native/kms/tests.rs`
- Modify: `src/native/kms/submitter.rs`

**Interfaces:**
- Produce `AtomicPlaneGeometry::full_source_to_output(source_width, source_height, output_width, output_height)`.
- Produce a reusable atomic primary request builder that writes FB_ID, CRTC_ID, SRC_X/Y/W/H, and CRTC_X/Y/W/H.
- Produce an explicit-geometry TEST_ONLY submitter entry point that does not require a pageflip token.

- [ ] Add tests for identity parity, 1600x900 to 1920x1080 values, zero sizes, overflow, and every atomic property assignment.
- [ ] Run the focused KMS unit test target and verify the new assertions fail before implementation.
- [ ] Implement geometry construction and request assignments; preserve existing cursor, connector content-type, and explicit-fence TEST_ONLY rules.
- [ ] Re-run the focused KMS unit test target.

### Task 2: Separate diagnostic scene candidate

**Files:**
- Modify: `src/compositor/direct_scanout.rs`
- Modify: `src/compositor/state/direct_scanout.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/compositor/tests/direct_scanout.rs`
- Modify: `src/compositor/tests/support/server_runtime.rs`

**Interfaces:**
- Produce a distinct `DirectScanoutProbeCandidate` with actual buffer dimensions and output dimensions.
- Keep `direct_scanout_scene_candidate()` and `DirectScanoutSceneAnalysis::candidate` production semantics unchanged.
- Reuse scene analysis and materialization; allow only a source/output-size mismatch in the probe-specific mode.

- [ ] Add tests for same-size production candidacy, eligible simple mismatch, actual source/output dimension distinction, unit scale, Normal transform, full-source viewport, full-output destination, and existing scene blockers.
- [ ] Run the focused compositor direct-scanout test target and verify the new assertions fail before implementation.
- [ ] Implement the probe-only analysis mode and keep `direct_scanout_viewport_compatibility()` strict.
- [ ] Re-run the focused compositor direct-scanout test target.

### Task 3: Safe worker-owned TEST_ONLY probe

**Files:**
- Modify: `src/native_output/kms_worker/queue.rs`
- Modify: `src/native_output/kms_worker/thread.rs`
- Modify: `src/native_output/kms_worker/presentation_executor.rs`
- Modify: `src/native_output/scanout/direct.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/scanout/mod.rs`

**Interfaces:**
- Produce an out-of-band synchronous KMS worker diagnostic request that is admitted only from a fully idle worker state and cannot issue a real commit.
- Produce uncached, RAII-managed DMA-BUF framebuffer import for this probe; do not modify the production framebuffer-import cache.
- Produce a typed probe outcome for TEST_ONLY success, rejection, or busy skip.

- [ ] Add recording-executor tests for idle admission, worker-busy skip, TEST_ONLY-only execution, and no normal commit/transaction ownership.
- [ ] Run focused KMS worker and scanout test targets and verify the new assertions fail before implementation.
- [ ] Implement worker serialization, TEST_ONLY submission, and temporary framebuffer import/cleanup.
- [ ] Re-run focused KMS worker and scanout test targets.

### Task 4: Probe cache, runtime integration, and telemetry

**Files:**
- Create: `src/native_output/scanout/scaled_probe.rs`
- Modify: `src/native_output/scanout/atomic_direct.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: focused scanout/runtime test modules

**Interfaces:**
- Produce a bounded probe cache keyed by output identity/generation, CRTC, primary plane, mode, format/modifier, source dimensions, plane layout hash, and request presentation state.
- Produce gated counters for observations, TEST_ONLY attempts, acceptance, rejection, cache hits, busy skips, and size-mismatch-only direct-scanout rejections.
- Invoke the probe without passing its result into frame planning.

- [ ] Add positive-cache, negative-cache, key-identity, feature-off, and composition-fallback tests.
- [ ] Run focused scanout/runtime tests and verify the new assertions fail before implementation.
- [ ] Implement the probe cache and invoke it only when `TYPHON_SCALED_DIRECT_PROBE=1` and the current KMS state is safe.
- [ ] Add concise one-time debug output under `TYPHON_DIRECT_SCANOUT_DEBUG=1` and expose probe telemetry only when enabled.
- [ ] Re-run focused scanout/runtime tests.

### Task 5: Review, verification, and commit

**Files:**
- Review all changes from Tasks 1–4.
- Update `docs/superpowers/specs/2026-09-27-scaled-primary-probe-design.md` if implementation details materially change.

- [ ] Adversarially inspect for real commits, stale cache reuse, dimension confusion, ownership mutation, worker races, duplicate TEST_ONLY construction, probe-result influence on composition, and physical KMS state mutation.
- [ ] Print and verify `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target` before Cargo commands.
- [ ] Run `cargo fmt --check`, targeted tests, the full Rust test suite, and `cargo clippy` using the Aether target directory.
- [ ] Review `git diff --check`, stage only task-owned files, and commit the completed logical change on `main`.
