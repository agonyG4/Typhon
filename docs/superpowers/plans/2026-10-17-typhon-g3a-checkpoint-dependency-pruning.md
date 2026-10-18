# Typhon G3a Checkpoint Dependency Pruning Implementation Plan

> **For agentic workers:** Execute this plan inline in the current Typhon checkout. Do not dispatch subagents or create a branch. Preserve unrelated dirty files and stage only G3a files.

**Goal:** Add opt-in, fail-closed pruning of exact checkpoint edges proven disjoint from G2 sampled backdrop support.

**Architecture:** Keep conservative graph compilation and precomputed eligibility first. Build a per-consumer immutable pruning plan from G2 support and exact instance/pass correspondence, validate it in full, then apply retained/pruned decisions to both dependency representations. Keep policy in EGL effects runtime and enable it only in ordinary frame planning.

**Tech Stack:** Rust, Typhon effect graph compiler, Codebase Memory MCP, GLES integration tests, Cargo.

## Global Constraints

- Keep the runtime default `off`; accept only `off` and `on`; invalid values warn once and fail closed.
- Never prune `precomputed_graph`; preserve precomputed eligibility based on the raw exact graph.
- Keep `compile_frame_execution_plan()` conservative and do not opt lifecycle rendering into G3a.
- Use only `EffectRegion` intersection against `EffectSampleSupportEnvelope::backdrop_region` as the pruning proof.
- Keep `CompiledEffectInstance::dependencies` and the consumer `SceneCapture::checkpoint_dependencies` synchronized and ordered.
- Leave pass/texture counts and IDs, layouts, capture domains/dimensions, damage, sampling, fusion semantics, and physical damage unchanged.
- Use `CARGO_TARGET_DIR=/mnt/Aether/Desktop/GitHub/Typhon-target` for every Cargo build/test/check; verify its resolved path before compiling.
- Preserve all unrelated dirty work and commit only G3a changes.

---

### Task 1: Add an auditable immutable pruning plan

**Files:**
- Create: `src/effects/checkpoint_pruning.rs`
- Modify: `src/effects/mod.rs` (or the actual effect module root)
- Test: `tests/effects_checkpoint_pruning.rs`

**Interfaces:**
- Consume: `CompiledFrameGraph`, `CompiledEffectInstance`, `CompiledRenderPass`, `EffectFrameDemand`, and `derive_effect_sample_support()`.
- Produce: `EffectCheckpointDependencyPruningPlan`, per-edge decisions, stats, and a plan/apply API that validates complete consumer metadata before any mutation.

- [ ] Test the geometric false edge (100,110,100,70) to (270,110,100,70): conservative edge count 1, sampled support disjoint, plan marks one pruned edge.
- [ ] Test true sampled overlap and ordered retained dependencies.
- [ ] Test multiple SceneCapture, SceneCapture plus SurfaceCapture, and Continuous demand as ineligible.
- [ ] Test duplicate/missing instances, duplicate/missing/ambiguous final passes, mismatched checkpoint lists, and invalid producer ordering as unresolved with no graph mutation.
- [ ] Run the focused graph test with Cargo output explicitly under `/mnt/Aether/Desktop/GitHub/Typhon-target`; verify the expected missing API assertion first.
- [ ] Implement immutable planning, full validation, atomic per-consumer application to both dependency vectors, and category accounting.
- [ ] Re-run the focused graph test and confirm `edges_before = retained + pruned + ineligible + unresolved` and `edges_after = before - pruned`.

### Task 2: Integrate options without changing compiler authority

**Files:**
- Modify: `src/effects/render_graph.rs`
- Modify: `src/egl_renderer/effects/checkpoint_dependency_pruning.rs` (new runtime policy module)
- Modify: `src/egl_renderer/effects/mod.rs`
- Modify: `src/egl_renderer/effects/runtime.rs`
- Modify: `src/egl_renderer/frame/planning.rs`
- Test: `tests/effects_checkpoint_pruning.rs`

**Interfaces:**
- Consume: Task 1 plan/apply API.
- Produce: `FrameExecutionCompileOptions.checkpoint_dependency_pruning`, default-off `CheckpointDependencyPruningPolicy`, and per-variant pruning stats.

- [ ] Add failing parser/options tests for absent, `off`, `on`, mixed-case, whitespace, and invalid values.
- [ ] Run the focused policy test and confirm default-off is currently missing.
- [ ] Add the EGL policy with once-only warning and fail-closed invalid parsing; store it in `EffectRuntime`.
- [ ] Add the explicit compile option, default it to false, and apply pruning only after exact raw eligibility and precomputed graph construction decisions complete.
- [ ] Keep the direct compile API and lifecycle path conservative; have only ordinary frame planning set the option from runtime policy.
- [ ] Add the regression where raw exact baseline dependence blocks precomputed optimization even though returned exact metadata is pruned.
- [ ] Compare off/on graph metadata after normalizing dependency vectors and pruning stats; assert pass/texture counts, IDs, layouts, domains, dimensions, damage, and terminal metadata match.
- [ ] Run focused tests and existing G2 sample-support tests.

### Task 3: Add pruning diagnostics and GLES execution qualification

**Files:**
- Modify: `src/egl_renderer/frame/planning.rs` or existing effect trace module
- Modify: `src/egl_renderer/tests.rs` (test module registration)
- Create: `src/egl_renderer/tests/checkpoint_pruning_gles.rs`
- Reuse: existing dependency-free replay and checkpoint causal test helpers

**Interfaces:**
- Consume: Task 2 runtime policy and stats.
- Produce: one concise frame compile pruning event and byte-for-byte GLES A/B tests for cold, warm, cache transition, source changes, true overlap, and partial repair.

- [ ] Add a deterministic cache-key transition test: dependent key at N is rejected for pruned capture at N+1; full population is required, and stable N+2 may reuse.
- [ ] Add GLES A/B for policy off/on in replay capture mode with replay-required scene authority; assert dependent framebuffer shader-copy changes to dependency-free replay and framebuffer bytes stay equal.
- [ ] Add cold, warm, outside-support source-change, true-overlap source-change, and partial-repair cases under Legacy and Regional demand planners.
- [ ] Assert terminal Kawase fusion on/off yields identical edge decisions.
- [ ] Record capture/downsample fusion candidate/execution/elided-pixel counters without changing fusion behavior.
- [ ] Run new GLES tests and existing dependency-free replay cache and checkpoint causal GLES suites.

### Task 4: Verify, qualify, and commit

**Files:**
- Modify only G3a implementation and test files above.

- [ ] Run G2 focused support tests, render-graph dependency tests, new pruning tests, cache and causal GLES tests, Legacy/Regional planner tests, and terminal-fusion sampling tests.
- [ ] Run `cargo fmt --check`, `cargo check --locked --all-targets`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked`, and `./bin/check-source-layout` where environment permits; report unrelated blockers precisely.
- [ ] Run native policy off/on A/B with precomputed blur off and GPU timing enabled where the native scene/workload is available; separate cold, warm, partial, source churn, overlap, and topology churn. Report missing environment evidence instead of inventing measurements.
- [ ] Re-read staged and unstaged diffs, verify only G3a paths are staged, and commit `perf(effects): prune disjoint checkpoint dependencies`.
- [ ] Complete the 21 adversarial questions and recommend whether future evidence supports default-on.
