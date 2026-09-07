# Typhon KMS Worker Completion-Lane Hardening Implementation Plan

> **For agentic workers:** Inline execution is required for this task because the user explicitly prohibited sub-agents. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove worker completion consumer backpressure while preserving exact
KMS ownership, lifecycle, pageflip, shutdown, cursor, and session invariants.

**Architecture:** Keep `QUEUED_JOB_CAPACITY = 1` for admission. Retain the
mutex-protected internal result queue and eventfd, but make result publication
lossless and non-blocking with respect to the main-thread consumer. Use the
existing health-check and fatal-job fallbacks when eventfd notification fails.

**Tech Stack:** Rust 2024, Cargo, existing worker fixtures, `rtk`, existing
Cargo target directory, and codebase-memory structural evidence.

## Global Constraints

- Reuse the current checkout and Cargo target directory.
- Use `rtk` for repository inspection, tests, checks, and Git operations.
- Do not use sub-agents, worktrees, alternate source trees, or target overrides.
- Do not change the four accepted September 7 fixes, worker policy default,
  queue admission capacity, EBUSY retry policy, timeout semantics, session
  recovery, explicit synchronization, or Direct Scanout policy.
- Do not add dependencies or redesign multi-output scheduling.
- Stage only files belonging to this task; preserve unrelated changes.

## Task 1: Capture the design and baseline

**Files:**
- Add: `docs/superpowers/specs/2026-09-07-typhon-kms-worker-completion-lane-design.md`
- Add: `docs/superpowers/plans/2026-09-07-typhon-kms-worker-completion-lane-plan.md`

- [x] Read the supplied hardening specification and current worker/runtime
  architecture with the codebase graph and direct source checks.
- [x] Confirm the current branch is clean and the four accepted baseline
  commits are present.
- [x] Record the result-capacity defect, lock ordering, ownership fallback, and
  physical-qualification boundary in the design and plan.
- [ ] Commit the design and plan before implementation.

## Task 2: Add deterministic RED tests

**Files:**
- Modify: `src/native_output/kms_worker/tests.rs`
- Possibly modify: `src/native_output/kms_worker/task4_tests.rs` only if an
  existing sidecar fixture is required.

- [ ] Add a test that leaves at least nine `Submitted` results undrained and
  proves quiesce and join complete, then recovers all events exactly once.
- [ ] Add a fatal-after-undrained-results test that checks fatal reason,
  termination, fatal-job identity, and `uncertain_submit`.
- [ ] Strengthen eventfd-failure tests for lossless rejection, submitted
  ownership, fatal reason, and uncertain-submit retention.
- [ ] Add quiesce coverage with an active submit, queued next job, pending
  cursor sidecar, and prior undrained results.
- [ ] Add one-shot completion drain/count assertions covering duplicate
  settlement prevention.
- [ ] Run the focused tests red before changing production code.

## Task 3: Implement lossless completion publication

**Files:**
- Modify: `src/native_output/kms_worker/queue.rs`
- Modify: `src/native_output/kms_worker/thread.rs`

- [ ] Remove `RESULT_EVENT_CAPACITY`, `WorkerShared::result_space`, and all
  producer waits on result capacity.
- [ ] Keep every event in the internal queue and retain eventfd as only a
  best-effort wake mechanism.
- [ ] Preserve fatal state, fatal jobs, event ordering, and false-return
  semantics used by conservative ownership paths.
- [ ] Keep all ownership-bearing and advisory event variants lossless.
- [ ] Run the new tests green and inspect lock/ownership paths.

## Task 4: Neighboring regression verification

- [ ] Run focused KMS worker, presentation worker, pageflip arbitration,
  session suspend/recovery, shutdown, explicit-sync, Direct Scanout ownership,
  frame-pacing, and cursor-sidecar tests.
- [ ] Reconfirm the four accepted baseline commit diffs are untouched.
- [ ] Review the worker/runtime teardown and event reordering paths, including
  pageflip-before-Submitted handling.

## Task 5: Documentation and final fresh verification

**Files:**
- Add: `docs/superpowers/specs/REPORT-2026-09-07-typhon-kms-worker-production-hardening-auto-readiness-v1.md`

- [ ] Write the final English report with architecture, confirmed defect,
  rejected hypotheses, implementation, invariants, tests, verification,
  physical qualification status, worker-off/auto evidence, VT/165 Hz status,
  blockers, and default-policy decision.
- [ ] Run fresh final checks through the existing build state:
  `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`,
  `rtk cargo clippy --locked --all-targets -- -D warnings`, and
  `rtk cargo test --locked --all-targets`.
- [ ] Run `rtk git diff --check`, inspect status and commit history, then commit
  the implementation and report.

