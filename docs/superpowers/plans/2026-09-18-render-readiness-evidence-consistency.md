# Render-readiness evidence consistency closure Implementation Plan

> **For agentic workers:** This plan is executed inline because sub-agents are explicitly disallowed.

**Goal:** Unify pageflip render-readiness evidence for physical classification, deadline assessment, and diagnostics without changing native pacing behavior.

**Architecture:** Add a pageflip-local typed observation with exact-fence, approximate-fence, and rendered-at-fallback sources. Resolve pending proven evidence before local assessment; use the observation timestamp for the existing physical classifier and new source-independent diagnostic fields while preserving fence-only payload fields.

**Tech Stack:** Rust, native compositor pageflip policy, existing KMS timing classifier, existing pacing trace fields, repository-local Cargo target directory, and `rtk` command proxy.

## Global Constraints

- Preserve all scheduler, target-selection, KMS adaptation, O1, READY-frame wake, recovery, and pacing constants.
- Preserve `payload_ready_at_ns` and `render_readiness_lateness_ns` as fence-derived fields.
- Use `rendered_at` only as an explicitly identified observation/lower bound when no fence timing exists.
- Preserve pending proven render-miss precedence and advisory ReactiveDouble dispatch behavior.
- Compile in `/home/agony/GitHub/Typhon` and invoke repository commands through `rtk`.
- Preserve unrelated dirty-worktree changes and stage only files belonging to this closure.

### Task 1: Add typed RED coverage

**Files:**
- Modify: `src/native_output/runtime/cycle/pageflip_tests.rs`

- [ ] Add tests for exact fence precedence, approximate-fence guarded mapping, rendered-at fallback source/timestamp, fallback payload timestamp non-fabrication, fallback observation lateness, on-time fallback classification, and pending evidence precedence.
- [ ] Run `rtk cargo test --locked pageflip` and confirm the new tests fail because the typed authority and helper APIs are not yet present.
- [ ] Keep the existing advisory dispatch regression in the same focused test target and confirm it remains part of the RED test set.

### Task 2: Implement the single pageflip evidence authority

**Files:**
- Modify: `src/native_output/runtime/cycle/pageflip.rs`

- [ ] Add the local source enum and observation type with constructors/accessors for observed timestamp, source name, fence quality, fence payload timestamp, and both lateness meanings.
- [ ] Change the composited pageflip path to construct one observation and pass its observed timestamp to `KmsPresentationOutcome::classify`.
- [ ] Make deadline assessment consume the observation and map fallback render misses to `ExactRender` only after physical classification has returned `RenderReadinessMiss`.
- [ ] Resolve pending proven evidence before pageflip-local reconstruction.
- [ ] Build the existing fence-derived diagnostics and new observation-level diagnostics from the same observation.

### Task 3: Verify, document, and commit

**Files:**
- Review: `src/native_output/runtime/cycle/pageflip.rs`
- Review: `src/native_output/runtime/cycle/pageflip_tests.rs`
- Review: `docs/superpowers/specs/2026-09-18-render-readiness-evidence-consistency-design.md`

- [ ] Run the focused and full verification commands requested by the user, using the repository-local build directory.
- [ ] Run the source-layout gate if present and record its status.
- [ ] Inspect the final diff against all render-evidence, pending-precedence, advisory-slip, policy-preservation, logging, and dirty-worktree review gates.
- [ ] Run native qualification when the repository exposes the normal blur-enabled native workload; report if it is unavailable in this environment.
- [ ] Stage only the closure files and commit with `fix(pacing): unify render readiness evidence`.
