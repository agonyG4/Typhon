# Typhon Direct Scanout One-Shot Diagnostics Implementation Plan

> **For agentic workers:** Execute this plan inline in this session. Do not dispatch subagents.

**Goal:** Add a cheap, one-shot `astreactl doctor` Direct Scanout diagnostic that reports scene, runtime, and downstream physical-attempt evidence without changing runtime policy.

**Architecture:** Keep the existing control protocol and output snapshots unchanged. In the existing `ControlCommand::Doctor` branch, derive one scene analysis and format it with current runtime state and optional Atomic counters; separately update Atomic blocker history and CLI human formatting.

**Tech Stack:** Rust, existing compositor scene-analysis types, existing native scanout counters, serde control snapshots, focused unit tests, Cargo verification gates.

## Global Constraints

- Do not add a control command, protocol version, output snapshot field, frame-loop logger, background sampler, or synchronous diagnostic file I/O.
- Call `OwnCompositorServer::direct_scanout_scene_analysis()` only for the requested doctor diagnostic and exactly once for that diagnostic.
- Preserve `candidate.is_some() <=> blockers.is_empty()` and all existing scene, fullscreen, presentation, VRR, KMS, and fallback authorities.
- Use the existing `DoctorCheck.detail` field and sanitize detail in human output.
- Keep `first_blocker` historical, update `last_blocker` on every blocker, and retain the blocker union.

### Task 1: Add RED regressions

**Files:**
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/astreactl/output.rs`

- [ ] Add focused tests for blocker history, doctor detail formatting for blocked and accepted scenes, human detail sanitization, and doctor-only diagnostic construction.
- [ ] Run the smallest matching Cargo test filters and confirm they fail for the missing field/behavior rather than due to unrelated compilation errors.

### Task 2: Implement Atomic latest-blocker tracking

**Files:**
- Modify: `src/native_output/scanout/atomic_direct.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`

- [ ] Add `last_blocker: Option<&'static str>` to `DirectScanoutCounters`.
- [ ] Make `note_direct_blocker()` preserve the first blocker, update the latest blocker, and union `blocker_set`.
- [ ] Run the blocker-focused tests until GREEN.

### Task 3: Implement one-shot doctor detail

**Files:**
- Modify: `src/native_output/runtime/cycle_dispatch.rs`
- Modify: `src/compositor/presentation_coverage.rs` only if a stable opacity spelling helper is needed.

- [ ] Add a compact formatter with stable names for scene candidate/root/opacity/blockers, informational semantic solitary fullscreen, feature state, runtime gates, and all required downstream counters.
- [ ] In the existing `ControlCommand::Doctor` arm, call `self.server.direct_scanout_scene_analysis()` once, query semantic solitary fullscreen only for a candidate root, capture current runtime fields, and set `direct_scanout.state` detail.
- [ ] Represent unavailable non-Atomic counters explicitly without inventing zero evidence.
- [ ] Run focused doctor tests until GREEN.

### Task 4: Show sanitized human detail

**Files:**
- Modify: `src/astreactl/output.rs`

- [ ] Append a sanitized `  ` detail line in `format_doctor_check()` when `DoctorCheck.detail` is present.
- [ ] Keep JSON serialization unchanged and run the formatting regression.

### Task 5: Review scope and verify

**Files:**
- Review: `src/native_output/runtime/presentation_direct.rs`
- Review: `src/native_output/runtime/presentation_cycle.rs`
- Review: `src/native_output/runtime/cycle_dispatch.rs`
- Review: `src/native_output/scanout/atomic_egl_gbm/direct.rs`

- [ ] Confirm no periodic/frame-loop path calls the diagnostic scene formatter and the existing Direct Scanout-off fallback guard remains intact.
- [ ] Run `cargo fmt --check`, `cargo check --locked --all-targets`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, `./bin/check-source-layout`, and release builds for `astreactl` and `oblivion-one`.
- [ ] Compare source-layout violations against a fresh baseline and report unrelated failures accurately.
- [ ] Commit only this feature’s files, preserving concurrent unrelated worktree changes.
