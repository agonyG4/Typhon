# Bounded Cursor Freshness Implementation Plan

> **For agentic workers:** Use inline execution in this session. Work on `main`; do not create a branch or worktree. Keep all build output under `/mnt/Aether/Desktop/GitHub`.

**Goal:** Give hardware cursor motion a bounded path to the next legal KMS opportunity without starving ready primary frames or exceeding bounded worker/arbiter admission.

**Architecture:** Extend `NativeCursorOutputArbitration` with absolute freshness and retry timing, derive cursor physical targets from the existing `PresentationDeadlinePlanner` and `KmsSubmitWindow`, and feed the existing runtime wake plan. Use actual primary carrier/ownership state, the single `AtomicCommitArbiter`, and the existing worker/sidecar path. Add a pre-acquire checkpoint only for a matured Auto opportunity with no attachable ready carrier.

**Tech Stack:** Rust, Cargo, existing presentation/KMS timing and transaction APIs, existing bounded cursor-responsiveness trace, deterministic Rust unit tests, Python tooling tests.

## Global Constraints

- Work on `main`; do not create branches.
- Preserve all pre-existing unrelated worktree changes and stage only files belonging to this task.
- Set `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target`; verify the resolved path before every Cargo build/test/check/clippy command.
- Keep `QUEUED_JOB_CAPACITY = 1`, `AtomicCommitArbiter`, output transactions/tokens/generations, cursor framebuffer pinning, sidecar coupling, explicit sync, direct scanout, and VRR/tearing legality unchanged.
- Use no direct DRM submission from input handling and do not add a thread or event loop.
- Use deterministic tests before production changes; run requested verification and commit each logical change on `main`.

## Planned files

- `src/native_output/runtime/frame.rs`, `frame_tests.rs`, `planner.rs`: policy state, freshness/retry timing, arbitration reasons, pure policy/stress tests.
- `src/native_output/presentation/kms_timing.rs` and `src/native/presentation_deadline.rs` only if a small reusable service-window/target helper is needed; test through their real timing/authority rules.
- `src/native_output/runtime/cursor_cycle.rs`, `scene_liveness.rs`, `wake_plan.rs`, `presentation_cycle.rs`, `presentation_worker.rs`, `plane_cycle.rs`, `kms_worker.rs`, `kms_worker/rejection.rs`: derive/rearm service windows, distinguish carrier/ownership/admission, integrate checkpoint, preserve debt through completion/retry/reset.
- `src/native_output/runtime/cursor_responsiveness.rs` and trace types: add bounded evidence and policy counters behind existing trace gates.
- `src/native_output/runtime/*_tests.rs` and existing cursor/KMS/presentation/pageflip tests: regression coverage at the owning boundaries.
- `docs/cursor-responsiveness-architecture.md`, `docs/cursor-responsiveness-diagnostics.md`: update scheduling semantics, metrics, and qualification workflow.

## Tasks

### Task 1: Specify Auto and legacy policy behavior in failing tests

1. Replace the shared 1,024-epoch expectation with explicit Auto and Piggyback cases. Piggyback must yield zero independent PlaneDelta while primary work persists. Auto must coalesce before its service deadline, submit once at maturity when the primary is unready, and select the newest cursor epoch for an attachable ready carrier.
2. Add failures for a deadline that moves later on coalesced input, a service deadline that slides after EBUSY, and pending age that resets on a newer epoch.
3. Run the targeted tests with `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target` after printing and verifying the directory is under `/mnt/Aether/Desktop/GitHub`.
4. Implement only the policy state needed to make those tests pass; retain exact Piggyback semantics and keep Software out of hardware admission.
5. Run targeted arbitration/responsiveness tests and commit `perf(cursor): make auto scheduling bounded-freshness`.

### Task 2: Derive physical cursor service targets from KMS timing

1. Add failing tests for target authority: advisory scheduled targets do not become carriers; reserved binding targets are not stolen; a binding reservation without a carrier pushes independent service to the next legal opportunity.
2. Add the mandatory paired test with identical KMS timing and scene prediction costs of near-zero and 500 ms; target and service timing must match.
3. Add refresh-period fixtures at 60, 120, 144, 165, and 240 Hz and prove a past deadline produces no repeated wake deadline.
4. Implement a cursor target planner using zero scene-render cost, actual binding/owned primary claims, `KmsSubmitWindow` worker wake/dispatch/apply timing, and a bounded cursor preparation guard. Do not let a primary advisory target become cursor ownership.
5. Commit `perf(cursor): decouple plane-delta targets from scene render cost` after targeted target-planning and KMS timing tests pass.

### Task 3: Preserve freshness debt through liveness, submission settlement, and KMS retries

1. Add failing tests for absolute pending age, `service_deadline_ns` versus `retry_not_before_ns`, repeated EBUSY, newer epochs during an older submission, generation invalidation, suspend/resume, modeset, DPMS, and software fallback clearing hardware work.
2. Make liveness observation preserve an armed Auto service window; on exact submitted-epoch settlement, clear only the submitted/current epoch and replan any newer epoch through the cursor KMS target helper.
3. Change EBUSY paths to update only retry timing and preserve overdue state. Keep non-retryable capability failure on the existing fallback/quarantine path.
4. Feed the existing wake plan from the next future service/retry boundary; filter past boundaries so an overdue cursor does not spin.
5. Commit `perf(cursor): preserve freshness debt across KMS backpressure` after cursor and KMS worker tests pass.

### Task 4: Integrate ready-carrier and KMS-lane arbitration

1. Add deterministic tests for dirty unready scene work, READY attachable primary, advisory target, binding target, primary worker queued, primary kernel submitted, full worker queue, direct scanout, sidecar replacement, async/tearing restriction, and a newer motion epoch while a prior cursor transaction is in flight.
2. Replace Auto's decisive `primary_work_pending` boolean with explicit carrier-ready, target-owned, queued/submitted, and independent-admission state. Keep Piggyback's primary-first decision separate.
3. Require actual attachability to piggyback; use the sidecar mailbox before primary freeze. Do not cancel, mutate frozen state, reorder ahead of an owned transaction, or evict a ready primary.
4. Record reasons for ready carrier, waiting window, target ownership, lane busy, software required, async blocked, and transition blocked.
5. Commit the arbitration/admission changes as `perf(cursor): preserve primary KMS ownership` once the focused KMS, presentation-transaction, pageflip, direct-scanout, and VRR/tearing tests pass.

### Task 5: Service matured Auto cursor debt before expensive primary preparation

1. Add a deterministic cycle-order test that injects a matured Auto hardware cursor, no attachable ready carrier, and slow primary acquire/prepare; assert cursor admission happens before preparation can overrun its KMS worker wake.
2. Add guard cases proving pageflip drain and input dispatch precede the checkpoint, session/shutdown/output-transition checks gate it, Software/Piggyback do not enter it, ready carriers bypass it, and one opportunity can trigger at most one independent attempt.
3. Implement the smallest checkpoint using the existing arbiter and worker; it must only build/admit or concretely defer the cursor transaction and must do no scene rendering/effects. Then continue normal preparation and primary work.
4. Run the cycle, cursor, worker, and primary scheduling tests; commit `perf(cursor): checkpoint matured cursor service before prepare`.

### Task 6: Extend bounded telemetry and throughput model

1. Add trace-schema tests for policy/reason, pending age, service/retry deadlines, carrier state, target sequence/time, worker wake/complete deadline, and age at arbitration/queue/submit.
2. Add deterministic 1,000-Hz epoch versus 165-Hz opportunity stress with primary readiness before/around/after the service window. Assert latest-wins coalescing, at most one independent service per opportunity, bounded worker occupancy, ready-primary preservation, and eventual cursor service during unready scene activity.
3. Track practical counts for independent cursor submissions, ready-primary piggybacks, unready-primary independent services, busy retries, and cursor target misses. Do not add per-motion clocks/logging outside existing trace gates.
4. Commit telemetry/model tests as `perf(cursor): qualify cursor freshness scheduling`.

### Task 7: Documentation and verification

1. Update architecture and diagnostics docs to state exact Auto/Piggyback/Software semantics, explain pageflip limits, and document same-binary A/B commands and required report counters.
2. Capture same-checkout baseline outcomes from the relevant cursor, worker, transaction, pageflip, direct-scanout, and VRR/tearing tests before classifying unrelated failures.
3. Verify the output path, then run `cargo fmt --all -- --check`, `cargo check --locked --all-targets`, focused requested suites, broad Rust tests, relevant Python/tooling tests, and Clippy where practical, always under `/mnt/Aether/Desktop/GitHub/Typhon-target`.
4. Inspect `git diff --check`, status, and staged paths. Commit documentation/final fixes without staging any unrelated changes.
5. Review each adversarial question from the request against a concrete test, source invariant, or clearly disclosed hardware-only measurement limitation.
