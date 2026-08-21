# Typhon O1 Credit Controller v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task with verification checkpoints.

**Goal:** Separate O1 demand from ownership and global pacing mode, then prove natural drain and useful overlap with a deterministic integrated simulator.

**Architecture:** Extract a pure `O1CreditDemandController` under `src/native/buffering/credit.rs`; keep physical depth in `OutputPipelineSnapshot`; pass explicit future-primary budget through scheduler/pipeline admission. Replace the arithmetic simulator with a deterministic event-driven model that reuses the production demand controller and reports bounded outcome counters.

**Tech Stack:** Rust, Cargo locked tests, existing native scheduler/presentation models, deterministic `std` event queue and bounded counters.

## Global Constraints

- Preserve unrelated dirty work and do not reset or discard it.
- Do not run `cargo clean` or a long benchmark campaign.
- Keep physical future-primary depth `<= 2`.
- KMS dispatch/apply misses update timing models only and never directly grant O1 credit.
- Desired credit changes admission only; immutable target role controls submission timing.
- Existing slot, generation, target-order, queue-capacity, and prepared-capacity invariants remain enforced.

### Task 1: Extract pure O1 demand policy

**Files:**
- Create: `src/native/buffering/credit.rs`
- Modify: `src/native/buffering/mod.rs`
- Modify: `src/native/adaptive_buffering.rs`
- Test: `src/native/buffering/credit.rs` and `src/native/adaptive_buffering.rs`

**Interfaces:**
- Produce `O1CreditDemandReason::{PredictedOverlap, ProvenRenderReadinessMiss, ForcedValidation}`.
- Produce `O1CreditDemandController::observe_opportunity(PresentationOpportunityId, u64)`, `observe_render_readiness_miss()`, `force()`, `set_ceiling(u8)`, `desired_credit()`, `grants()`, and `revokes()`.
- Change adaptive overlap observation so it accepts no `future_primary_owned` argument.
- Keep compatibility accessors for existing metrics, but make `pacing_mode()` diagnostic only.

- [ ] Write failing tests proving KMS dispatch/apply misses do not grant, and negative overlap revokes while ownership is conceptually two.
- [ ] Run `cargo test --locked native::adaptive_buffering native::buffering` and confirm the old behavior fails.
- [ ] Implement the pure controller and route Auto/Off/Force through it.
- [ ] Run the focused tests and confirm KMS-only evidence leaves desired credit unchanged while render pressure grants.
- [ ] Commit with `git commit -am "refactor(native): separate O1 credit demand from ownership"` after staging only owned task files.

### Task 2: Make admission budget explicit and mode-independent

**Files:**
- Modify: `src/native/scheduler/pipeline.rs`
- Modify: `src/native_output/presentation/pipeline.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/presentation_pipeline.rs`
- Test: existing scheduler/pipeline tests plus new focused regressions

**Interfaces:**
- Add `future_primary_limit()` to `PresentationPipelineView`.
- Add `future_primary_limit: u8` to `OutputPipelineSnapshot` and use it in `can_render_composed()`/`can_pre_admit_primary()`.
- Make `decision_with_pipeline()` use explicit limit and `render_ahead_allowed`, not `pacing_mode()` branches, for fixed-VSync admission.

- [ ] Add failing tests for `desired=1, depth=2` no-refill and for Auto credit transitions not changing ordinary scheduling decisions.
- [ ] Run the focused scheduler/pipeline tests and capture the failures.
- [ ] Implement explicit-limit admission while preserving all physical validation checks.
- [ ] Run the focused scheduler/pipeline tests and confirm depth two drains without a new owner.
- [ ] Commit with `git commit -m "fix(native): drain O1 future depth without refill"`.

### Task 3: Attribute pressure and preserve immutable target timing

**Files:**
- Modify: `src/native_output/runtime/planner.rs`
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `src/native_output/runtime/presentation_metrics.rs`
- Modify: `src/native/presentation_deadline.rs` only if target-role helpers need adjustment
- Test: planner and fixed-VSync presentation tests

**Interfaces:**
- Add budget-based target helpers that preserve an existing scheduled target when demand drops and only allocate a new render-ahead successor when admission permits.
- Select direct/composited submit target from `scheduled_presentation_target` and its `PresentationTargetReason`, falling back to the ordinary reactive target only when no immutable successor is armed.
- Remove the runtime call that passes physical depth into policy observation.

- [ ] Add failing tests proving a KMS-only pageflip miss does not grant credit and a credit transition cannot retarget an armed target.
- [ ] Run planner/pageflip/presentation tests and confirm old mode coupling fails them.
- [ ] Implement target-role timing and render-readiness-only demand attribution.
- [ ] Run focused tests and verify target sequence/time stability.
- [ ] Commit with `git commit -m "fix(native): attribute O1 demand only to render pressure"`.

### Task 4: Replace arithmetic simulation with integrated virtual time

**Files:**
- Create: `src/native/buffering/simulator.rs`
- Modify: `src/native/buffering/mod.rs`
- Test: `src/native/buffering/simulator.rs`

**Interfaces:**
- Define deterministic `SimulatedO1Event` variants for visual work, callback progress, render start/completion, fence readiness, worker wake, submit start/return, pageflip, generation change, render failure, and timing-constraint change.
- Define simulator state for armed opportunity, desired credit, owned depth, kernel/worker/prepared owners, worker transport, and visual work.
- Return bounded counts for demand observations, depth/drain, KMS misses, and useful/unnecessary/ineffective/granted-not-consumed credit.
- Keep `simulate_o1` and `simulate_o1_with_render_services` as compatibility entry points backed by the event model.

- [ ] Add failing deterministic scenarios for low load, sustained overlap, one spike, revoke-at-depth-two, KMS-only misses, worker on/off, generation change, and usefulness classes.
- [ ] Run only the simulator tests and confirm the linear model cannot satisfy them.
- [ ] Implement the priority-queue event loop using production `PipelineServiceEstimate` and `O1CreditDemandController`.
- [ ] Add bounded property sweeps across refresh/service/dispatch/apply/spike ranges.
- [ ] Run simulator tests and confirm depth, target immutability, drain, and KMS attribution invariants.
- [ ] Commit with `git commit -m "test(native): model O1 ownership in virtual time"`.

### Task 5: Add useful-overlap telemetry

**Files:**
- Modify: `src/native_output/pacing.rs`
- Modify: `src/control_snapshots.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `src/astreactl/output.rs` if snapshot formatting requires fields
- Test: pacing/metrics tests and simulator classifications

**Interfaces:**
- Expose bounded `o1_credit2_useful_hits`, `o1_credit2_unnecessary_hits`, `o1_credit2_ineffective_misses`, `o1_credit2_granted_not_consumed`, `o1_credit2_drain_events`, and `o1_credit2_refill_suppressed_while_draining` fields.
- Preserve existing telemetry fields and aggregate-only behavior.

- [ ] Add failing snapshot/metrics assertions for all new counters.
- [ ] Run focused pacing/metrics tests.
- [ ] Wire bounded counters to immutable target/admission metadata and simulator outcomes.
- [ ] Run focused tests and confirm serialization remains compatible with current snapshot policy.
- [ ] Commit with `git commit -m "feat(native): report useful O1 overlap outcomes"`.

### Task 6: Final review and validation

**Files:**
- Review all changed files; modify only defects found in the O1 scope.
- Update: `docs/superpowers/specs/2026-08-21-o1-credit-controller-v2-design.md` if implementation decisions differ.

- [ ] Review the diff for ownership-as-pressure, global mode admission, target mutation, refill during drain, KMS direct grants, depth overflow, duplicate worker timing, and queue-residency leakage.
- [ ] Run focused suites: native buffering/adaptive buffering, presentation deadline, scheduler/pipeline, render-ahead, pacing/fixed-VSync, and KMS outcome attribution.
- [ ] Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk cargo test --locked`, `./bin/check-source-layout`, and `git diff --check`.
- [ ] Perform only the brief live smoke checks named in the supplied task if a usable native Wayland session is available; do not run long vkmark campaigns.
- [ ] Commit final scoped fixes with a reviewable message.
