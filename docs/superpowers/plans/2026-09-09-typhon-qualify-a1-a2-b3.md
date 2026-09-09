# Typhon A1/A2/B3 Qualification Implementation Plan

**Goal:** Add deterministic regression coverage that qualifies the already-landed A1 eviction, A2 trace-export, and B3 uniform-location performance mechanisms without changing correct production behavior.

**Architecture:** Keep cumulative eviction metrics separate from the pending deletion queue and add stress/error-path tests at the existing resource-pool seam. Keep trace delta selection unchanged, but factor the filesystem write plus cursor commit into a small helper so an injected filesystem failure can prove that retry ownership is retained. Keep shader uniform lookup generation-local and factor only the cache insertion/hit decision into a closure-backed helper for deterministic tests; generation replacement remains the existing fresh-cache assignment in `GlesSceneRenderer`.

**Tech Stack:** Rust 2024, Cargo locked dependency graph, existing unit-test modules, `rtk` command proxy, existing Cargo `target/` directory.

## Global Constraints

- Work on the current Typhon checkout.
- Treat current source as authoritative; do not reimplement already-correct A1, A2, or B3 behavior.
- Do not use subagents.
- Use `rtk` where applicable.
- Reuse the existing Cargo target/build directory.
- Do not create another checkout, target directory, benchmark target tree, or alternate build directory.
- Do not add dependencies or wall-clock assertions.
- Keep B1 unchanged.
- Run the focused tests and all verification commands from the approved brief.
- Commit only the focused qualification changes because this is a Git repository.

### Task 1: Qualify consumable effect-resource eviction state

**Files:**
- Modify: `src/egl_renderer/effects/resources.rs` in the existing `#[cfg(test)]` module.

**Interfaces:**
- Consumes: `EffectResourcePool::checkout`, `return_texture`, `drain_evicted_texture_ids`, `evicted_texture_ids`, and `metrics`.
- Produces: deterministic coverage for bounded pending eviction ownership, cumulative eviction metrics, checked-out resource protection, and failed-checkout retention.

- [ ] **Step 1: Add the churn-stress test**

Create one idle `16x16` Rgba8 texture under a `2048`-byte budget, alternate `16x16` and `32x16` acquisitions for a large fixed number of iterations, drain notifications after each acquisition, assert the pending queue is empty after every drain, assert the cumulative eviction count equals the iteration count, then perform repeated same-key cache hits and assert no historical notifications appear.

- [ ] **Step 2: Add the failed-acquisition retention test**

Create and return an idle `16x16` texture under a `2048`-byte budget, request an oversized `64x64` texture so idle eviction occurs before `BudgetExceeded`, assert the original ID remains pending after the failed checkout, then drain it once and assert the next drain is empty while the cumulative count remains one.

- [ ] **Step 3: Run the focused resource tests**

Run `rtk cargo test --locked --lib egl_renderer::effects::resources` and require zero failures.

### Task 2: Qualify incremental presentation trace export and retry ownership

**Files:**
- Modify: `src/native_output/presentation/trace.rs` in the existing trace unit tests.
- Modify: `src/native_output/runtime/cycle.rs` around `NativeRuntime::flush_presentation_trace`.

**Interfaces:**
- Consumes: `PresentationTransactionTraceRing::export_delta`, `TraceExport`, and `NativeRuntime::presentation_trace_export_cursor`.
- Produces: chronological multi-event append coverage, large-history bounded-delta coverage, and a small `write_presentation_trace_export` helper that advances the cursor only after successful append/replace I/O.

- [ ] **Step 1: Add the delta-size/content tests**

Fill a ring with a fixed historical sequence, save its cursor, push multiple new events, assert the result is `TraceExport::Append`, assert exactly the new JSONL lines are present in chronological order, and assert historical timestamps are absent from the delta. Keep the ring-overwrite test as the bounded replacement case.

- [ ] **Step 2: Extract the minimal write/cursor helper**

Move the existing append/replace filesystem operations into a helper with this shape:

```rust
fn write_presentation_trace_export(
    path: &std::path::Path,
    export: TraceExport,
    cursor: (usize, u64),
    export_cursor: &mut Option<(usize, u64)>,
) -> NativeResult<()>
```

Preserve the current behavior: `Unchanged` returns without I/O; an empty append records the cursor; append opens with create+append; replacement creates the parent and writes the full contents; the cursor assignment is after the fallible I/O. Have `flush_presentation_trace` compute the export/current cursor and delegate to this helper.

- [ ] **Step 3: Add deterministic write-failure coverage**

Use an exact per-test temporary directory path as an existing directory target so append I/O fails deterministically. Assert the cursor stays at its prior value after the failure, then write to a child file through the same helper and assert the pending cursor advances and the file contains the exported data. Remove only the exact temporary test directory.

- [ ] **Step 4: Run the focused trace/runtime tests**

Run `rtk cargo test --locked --lib native_output::presentation::trace` and the focused native runtime presentation tests that compile `cycle.rs`; require zero failures.

### Task 3: Qualify generation-local cached uniform locations

**Files:**
- Modify: `src/egl_renderer/effects/shader_cache.rs` in the cache implementation and existing unit-test module.

**Interfaces:**
- Consumes: `ShaderProgramCache::uniform_location`, `ShaderProgramCache::clear`, cache entries, and `GlesSceneRenderer::publish_effect_registry_generation` source behavior.
- Produces: deterministic positive-hit and missing-uniform cache coverage plus explicit fresh-cache generation coverage without constructing a live GLES renderer.

- [ ] **Step 1: Add the cache-state test seam and test**

Factor the map lookup/insertion logic into a private closure-backed helper used by `uniform_location`. Count resolver invocations for a `Some(glow::NativeUniformLocation(...))` result and for a `None` result; assert repeated lookups invoke each resolver once and preserve each result. Populate an old cache entry, create the replacement cache using `ShaderProgramCache::new`, and assert the replacement has no old entries.

- [ ] **Step 2: Run the focused shader-cache tests**

Run `rtk cargo test --locked --lib egl_renderer::effects::shader_cache` and require zero failures.

### Task 4: Full verification and focused commit

**Files:**
- Verify only; no additional production files.

- [ ] **Step 1: Format and static checks**

Run `rtk cargo fmt --check`, `rtk cargo check --locked --all-targets`, `rtk cargo clippy --locked --all-targets -- -D warnings`, `./bin/check-source-layout`, and `git diff --check`.

- [ ] **Step 2: Run the complete locked test suite**

Run `rtk cargo test --locked` using the existing `target/` directory and record the exact pass/skip counts.

- [ ] **Step 3: Re-run the final authority searches**

Run the four approved `rg` searches for A1, A2, B3, and B1 and classify raw production uniform calls separately from one-shot test fixtures.

- [ ] **Step 4: Review and commit the focused changes**

Inspect `git diff`, confirm no production behavior changed except the small tested trace-write seam, confirm only the planned files are modified, then commit with `test(perf): qualify bounded renderer resource paths`.
