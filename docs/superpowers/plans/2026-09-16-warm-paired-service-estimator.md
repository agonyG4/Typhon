# Ready-Time Warm Paired Service Estimator Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Activate the exact same-frame paired service estimator for warm scheduling and make Predictive O1 consume the same selected end-to-end service total.

**Architecture:** Keep `AdaptiveRenderJournal`’s independent render-risk model unchanged and calculate independent, warm paired, and independent-P90-floor totals side by side. Add an explicit selected total to `PipelineServiceEstimate`; its end-to-end and O1 deadline methods use that total while its component fields remain diagnostic compatibility data. Wire the native presentation cycle from `RenderPrediction::total_cost_ns` into that explicit authority.

**Tech Stack:** Rust, Cargo, native pacing unit/integration tests, serde performance snapshots, `rtk` command proxy.

## Global Constraints

- Compile and test in `/home/agony/GitHub/Typhon` so Cargo uses the existing local target directory.
- Use `rtk` for repository, Cargo, and search commands where available.
- Do not use subagents.
- Preserve exact sync-file render-service semantics and exact paired-sample eligibility.
- Do not modify Ready-Time Opportunity Pull-In, Adaptive KMS Dispatch Tail Guard constants/attribution, worker queue residency accounting, or Predictive O1 target/lifecycle identity.
- Keep independent totals authoritative in `ColdStart` and `MissRecovery`.
- Centralize `WARM_PAIRED_MIN_SAMPLES = 20` and `MISS_RECOVERY_PAIRED_SUCCESSES = 20`.
- Apply idle protection after estimator selection for every estimator mode.

## File Map

- Modify `src/native/adaptive_buffering.rs`: centralized thresholds, operational miss recovery, selected estimator arithmetic, diagnostics, and journal regressions.
- Modify `src/native/buffering/mod.rs`: explicit selected end-to-end authority used by O1 deadline/overlap methods, plus arithmetic tests.
- Modify `src/native_output/runtime/presentation_cycle.rs`: pass the selected prediction total into the existing O1 integration boundary.
- Modify `src/native_output/runtime/presentation_o1.rs`: add the integration-facing selected-total regression without changing successor identity logic.
- Modify `src/native_output/runtime/presentation_metrics.rs`: emit estimator-selection diagnostics in pacing trace fields.
- Modify `src/native_output/runtime/metrics.rs`: label independent snapshot components explicitly while preserving selected total semantics.
- Modify `src/control_snapshots.rs`: update serialized buffering snapshot names for independent component diagnostics if required by the runtime snapshot change.
- Update `docs/superpowers/specs/2026-09-16-warm-paired-service-estimator-design.md` only if implementation evidence reveals a design correction; otherwise leave the approved spec unchanged.

## Execution

### Task 1: Establish the telemetry-only RED regression

**Files:**
- Test: `src/native/adaptive_buffering.rs` test module

**Interfaces:**
- Consumes: existing `AdaptiveRenderJournal::record_frame_service_observation`, `prediction_estimator_mode`, and `prediction`.
- Produces: a regression proving the current mode can be `WarmPaired` while `total_cost_ns` still follows the independent render-risk path.

- [ ] **Step 1: Write the failing test**

Add a test that records at least `WARM_PAIRED_MIN_SAMPLES` exact paired observations, records a large render tail into the independent history, asserts `WarmPaired`, and changes the paired sample value while asserting the current total does not change with the paired value. The test must name the architectural gap, for example `warm_mode_is_currently_telemetry_only`.

- [ ] **Step 2: Run the regression and verify the expected RED**

Run:

```bash
rtk cargo test --locked adaptive_buffering warm_mode_is_currently_telemetry_only
```

Expected: the test compiles and fails because the current total is independent-only even though the mode is `WarmPaired`.

- [ ] **Step 3: Do not implement production code in this task**

Keep the failure as the proof of the pre-change split. The next task will make this exact test pass as part of the estimator implementation.

### Task 2: Implement centralized thresholds and selected estimator arithmetic

**Files:**
- Modify: `src/native/adaptive_buffering.rs`
- Test: `src/native/adaptive_buffering.rs` test module

**Interfaces:**
- Consumes: exact paired history and the existing independent guard samples.
- Produces: `RenderPrediction` fields for independent total, warm total, P90 floor, worker non-ioctl lead, and recovery remaining; `total_cost_ns` selected by mode.

- [ ] **Step 1: Add constants and diagnostics to the failing test surface**

Centralize the two thresholds beside `SAMPLE_CAPACITY`, add the recommended diagnostic fields to `RenderPrediction`, and update the RED test assertions so it expects the warm calculation rather than telemetry-only behavior. Keep `render_risk_ns` tied to the independent model.

- [ ] **Step 2: Run the updated regression to verify it fails for the missing implementation**

Run:

```bash
rtk cargo test --locked adaptive_buffering warm_mode_is_currently_telemetry_only
```

Expected: compile failure for missing selected-total diagnostics or an assertion failure showing the old total is still selected.

- [ ] **Step 3: Implement the minimal arithmetic helpers**

In `base_prediction`, calculate:

```rust
let independent_total = render_risk
    .saturating_add(main_event_loop_wake_guard)
    .saturating_add(kms_dispatch_budget)
    .saturating_add(kms_apply_guard_ns);
let worker_non_ioctl_lead = kms_dispatch_budget.saturating_sub(p95_ioctl);
let warm_paired_total = paired_service_p95_ns
    .saturating_add(main_event_loop_wake_guard)
    .saturating_add(worker_non_ioctl_lead)
    .saturating_add(kms_apply_guard_ns);
let independent_p90_floor = p90
    .saturating_add(main_event_loop_wake_guard)
    .saturating_add(kms_dispatch_budget)
    .saturating_add(kms_apply_guard_ns);
let selected = match estimator_mode { ... };
```

Use `WarmPaired` only for `max(warm_paired_total, independent_p90_floor)` and keep both cold and recovery on `independent_total`. Apply the existing idle max to `selected` after that match. Populate every diagnostic field without changing the exact sampling code.

- [ ] **Step 4: Run the regression and focused adaptive tests**

Run:

```bash
rtk cargo test --locked adaptive_buffering
```

Expected: the telemetry-only regression and existing adaptive-buffering tests pass; any failure must be fixed in production code or the test’s exact expectation, never hidden by weakening the independent model.

- [ ] **Step 5: Add and run arithmetic/recovery regressions**

Add tests for budget greater-than, equal-to, and less-than P95 ioctl; paired render plus ioctl counted once; sticky independent deviation remaining high while warm selection changes; P90 floor; 20-sample cold/warm threshold; 20-success recovery; reset on a second miss; and approximate/incomplete observations not decrementing recovery. Run:

```bash
rtk cargo test --locked adaptive_buffering
```

Expected: all new formula and mode tests pass.

### Task 3: Update miss recovery without changing paired sampling

**Files:**
- Modify: `src/native/adaptive_buffering.rs`
- Test: `src/native/adaptive_buffering.rs` test module

**Interfaces:**
- Consumes: existing `record_frame_service_observation` and `note_proven_deadline_miss` call sites.
- Produces: bounded recovery semantics where the newest proven miss requires exactly `MISS_RECOVERY_PAIRED_SUCCESSES` exact paired successes.

- [ ] **Step 1: Make the recovery regression assert the new horizon**

Update the existing recovery test to assert that one miss sets the full horizon, `MISS_RECOVERY_PAIRED_SUCCESSES - 1` exact observations remain in `MissRecovery`, and the final exact observation enters `WarmPaired` only when the warm sample threshold is also satisfied. Assert that a second miss halfway through resets the remaining count to the full horizon.

- [ ] **Step 2: Run the recovery test to verify the expected RED**

Run:

```bash
rtk cargo test --locked adaptive_buffering miss_recovery
```

Expected: failure against the old incrementing/one-sample recovery behavior.

- [ ] **Step 3: Implement reset-to-horizon miss handling**

Change `note_proven_deadline_miss` to assign the centralized recovery constant. Leave decrementing exclusively inside the exact-success path after all exact eligibility checks have passed.

- [ ] **Step 4: Run adaptive recovery tests**

Run:

```bash
rtk cargo test --locked adaptive_buffering miss_recovery
```

Expected: all recovery tests pass, including approximate observation exclusion.

### Task 4: Give PipelineServiceEstimate an explicit selected-total authority

**Files:**
- Modify: `src/native/buffering/mod.rs`
- Test: `src/native/buffering/mod.rs` test module

**Interfaces:**
- Consumes: existing component constructor and simulator callers.
- Produces: a constructor/override that stores a selected end-to-end total; `end_to_end_service_ns`, `latest_successor_render_start`, and `overlap_required_ns` use it.

- [ ] **Step 1: Write the failing O1 authority tests**

Add a test proving a selected total differs from the independent component sum and that `end_to_end_service_ns`, `latest_successor_render_start`, and `overlap_required_ns` use the selected value. Keep the existing `new` test as the compatibility/component arithmetic case.

- [ ] **Step 2: Run buffering tests and confirm RED**

Run:

```bash
rtk cargo test --locked buffering
```

Expected: compile failure for the new constructor/override or assertions showing O1 still uses component reconstruction.

- [ ] **Step 3: Implement selected-total storage and method usage**

Retain the current public component fields. Add a private selected-total field initialized by `new` to the component sum and a focused constructor/`const` override for the native selected prediction. Make `end_to_end_service_ns` return the stored selected value; make `latest_successor_render_start` subtract that value exactly once. Preserve `render_ready_service_ns` and `kms_lead_ns` for component diagnostics and simulator behavior.

- [ ] **Step 4: Run buffering and simulator tests**

Run:

```bash
rtk cargo test --locked buffering
```

Expected: selected-total authority tests and existing overlap/simulator tests pass.

### Task 5: Unify presentation-cycle O1 with the selected total

**Files:**
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Modify: `src/native_output/runtime/presentation_o1.rs`
- Test: `src/native_output/runtime/presentation_o1.rs`

**Interfaces:**
- Consumes: `RenderPrediction::total_cost_ns`, `PipelineServiceEstimate` selected-total constructor, and the existing `overlap_required_for_current_opportunity` boundary.
- Produces: ordinary planning and O1 overlap demand using the same selected end-to-end authority.

- [ ] **Step 1: Write the integration regression before changing the wiring**

Create a deterministic test crossing `overlap_required_for_current_opportunity` with a `PipelineServiceEstimate` whose independent components sum to about 13 ms and whose selected total is about 6.5 ms. Assert latest successor render start and overlap demand reflect 6.5 ms. Do not assert only that a number became smaller.

- [ ] **Step 2: Run the O1 regression and confirm RED**

Run:

```bash
rtk cargo test --locked presentation_o1
```

Expected: failure because the current constructor path reconstructs the independent component total.

- [ ] **Step 3: Wire the selected prediction total in presentation_cycle**

Replace the current `PipelineServiceEstimate::new(prediction.main_event_loop_wake_guard_ns, prediction.render_risk_ns, prediction.kms_dispatch_budget_ns, prediction.kms_apply_guard_ns)` use with the selected-total-capable constructor/override, preserving the diagnostic components and passing `prediction.total_cost_ns` as the authoritative total.

- [ ] **Step 4: Run O1 and planner identity tests**

Run:

```bash
rtk cargo test --locked presentation_o1
rtk cargo test --locked presentation_deadline
```

Expected: selected-total integration passes, and immediate-successor, unreachable-pressure, proven-miss, forced-validation, and physical-claim identity tests remain green.

### Task 6: Make pacing diagnostics auditable and snapshot names truthful

**Files:**
- Modify: `src/native_output/runtime/presentation_metrics.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `src/control_snapshots.rs` only if field names change
- Test: existing serialization/runtime metric tests

**Interfaces:**
- Consumes: all new `RenderPrediction` diagnostics and selected mode.
- Produces: trace/snapshot output where selected total is unambiguous and old independent components are explicitly diagnostic.

- [ ] **Step 1: Add failing diagnostics assertions**

Extend the existing render-begin/pacing snapshot test surface to assert emission of independent total, warm paired total, independent P90 floor, worker non-ioctl lead, and recovery remaining. Assert the selected total and estimator mode are emitted together.

- [ ] **Step 2: Run focused tests and verify RED**

Run:

```bash
rtk cargo test --locked pacing
rtk cargo test --locked native_output
```

Expected: failure because the new fields are not emitted and/or the old snapshot component labels are still ambiguous.

- [ ] **Step 3: Implement diagnostics only**

Add fields to `build_render_begin_fields` and pacing `snapshot_fields` without adding per-frame debug spam outside the existing trace. In the performance snapshot, rename old component fields to explicitly independent names if needed and keep `predicted_total_service_ns` tied to `prediction.total_cost_ns`. Update serde fixtures with defaults only when backward decoding requires it.

- [ ] **Step 4: Run focused diagnostics tests**

Run:

```bash
rtk cargo test --locked pacing
rtk cargo test --locked native_output
```

Expected: all diagnostics and snapshot serialization tests pass.

### Task 7: Full verification and focused commit(s)

**Files:**
- Verify all modified files and the approved design/plan docs.

- [ ] **Step 1: Run all requested focused tests**

Run:

```bash
rtk cargo test --locked adaptive_buffering
rtk cargo test --locked buffering
rtk cargo test --locked presentation_deadline
rtk cargo test --locked presentation_o1
rtk cargo test --locked pacing
rtk cargo test --locked native_output
```

- [ ] **Step 2: Run formatting, check, clippy, full tests, and diff validation**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo check --locked --all-targets
rtk cargo clippy --locked --all-targets -- -D warnings
rtk cargo test --locked
rtk git diff --check
```

- [ ] **Step 3: Inspect the final diff and working-tree safety**

Run:

```bash
rtk git status --short
rtk git diff --stat
rtk git diff -- src/native/adaptive_buffering.rs src/native/buffering/mod.rs src/native_output/runtime/presentation_cycle.rs src/native_output/runtime/presentation_o1.rs src/native_output/runtime/presentation_metrics.rs src/native_output/runtime/metrics.rs src/control_snapshots.rs
```

Confirm `.codebase-memory/` and unrelated work remain untouched. Re-index or run graph impact analysis only after source edits if structural evidence is needed; any changed runtime file with a coverage gap must be read directly.

- [ ] **Step 4: Commit focused implementation changes**

Prefer:

```bash
rtk git add src/native/adaptive_buffering.rs src/native/buffering/mod.rs src/native_output/runtime/presentation_cycle.rs src/native_output/runtime/presentation_o1.rs src/native_output/runtime/presentation_metrics.rs src/native_output/runtime/metrics.rs src/control_snapshots.rs
rtk git commit -m "fix(pacing): activate warm paired service estimator"
```

If the selected-total representation and O1 wiring are inseparable in the final diff, use one focused pacing commit. Do not include KMS tail tuning or unrelated files.

## Native qualification

Do not claim native acceptance from automated verification. If the native 165 Hz workload is run, compare mode, independent/warm/selected totals, O1 overlap requirement and abandonment, useful extra-credit outcomes, submit-limited attribution, ReactiveDouble misses, pageflip percentiles, cadence, target identity, and Predictive O1 validity. If it is not run, report that explicitly.
