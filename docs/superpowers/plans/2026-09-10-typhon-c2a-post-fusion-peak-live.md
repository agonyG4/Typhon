# Typhon C2a Post-Fusion Peak-Live Accounting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use inline execution with `superpowers:executing-plans`; subagents and new checkouts are prohibited by the task request. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `peak_live_intermediates` exact for stable, gapped `GraphPassId` values after fusion while retaining `O(P + T)` work and unchanged graph execution semantics.

**Architecture:** `compile_frame_execution_plan` will pass the final post-fusion `builder.passes` slice to a private peak-live helper. The helper will build one bounded `GraphPassId`→surviving-position vector, resolve each complete intermediate lifetime once, and sweep inclusive delta intervals. The test reference will receive the same pass sequence but use independent `passes.iter().position(...)` lookups.

**Tech Stack:** Rust, Cargo, existing render-graph unit tests, deterministic test-only counters, `rtk`, current checkout and existing Cargo target directory.

## Global Constraints

- Do not create a new checkout, target directory, benchmark build tree, or alternate Cargo build directory.
- Do not renumber `GraphPassId` values after fusion.
- Keep peak-live analysis after `fuse_compatible_local_stages(&mut builder)` and all lifetime rewrites.
- Preserve inclusive `first_use..=last_use` semantics.
- Preserve actual effect execution, texture release, resource ownership, B2, C1, snapshot, presentation, Direct Scanout, and shader behavior.
- Do not add persistent scene/effect topology caches or implement C2b.
- Keep `RenderGraphCompileStats::peak_live_intermediates` unchanged in name and meaning.
- Use RED → GREEN → REFACTOR and run each focused test cycle with `rtk`.
- Commit the source fix with `fix(effects): account for fused pass ids in peak-live stats`.

---

### Task 1: Replace the shared-invalid reference and add synthetic gapped-ID regressions

**Files:**
- Modify: `src/effects/render_graph.rs` test module around `lifetime_texture`, `brute_force_peak`, and interval tests.

**Interfaces:**
- Consumes: existing `GraphPassId`, `CompiledRenderPass`, `GraphTexturePlan`, and `lifetime_texture` helpers.
- Produces: `brute_force_peak(passes: &[CompiledRenderPass], textures: &[GraphTexturePlan]) -> usize` as the semantic test reference, plus explicit gapped-ID fixtures.

- [ ] **Step 1: Add a minimal `test_pass` constructor for ordered pass IDs.**

  Construct a `CompiledRenderPass` with the requested non-zero `GraphPassId`, `RenderPassKind::Fragment`, empty inputs/output/damage/dependencies, and the existing default effect/anchor fields. Keep it test-only and do not change production structs.

- [ ] **Step 2: Rewrite the reference by actual pass sequence.**

  Replace the `pass_count` argument and every `first.get() - 1` / `last.get() - 1` lookup in `brute_force_peak` with:

  ```rust
  fn brute_force_peak(passes: &[CompiledRenderPass], textures: &[GraphTexturePlan]) -> usize {
      passes
          .iter()
          .enumerate()
          .map(|(current_position, _)| {
              textures
                  .iter()
                  .filter(|texture| texture.source == GraphTextureSource::Intermediate)
                  .filter(|texture| {
                      let (Some(first), Some(last)) = (texture.first_use, texture.last_use) else {
                          return false;
                      };
                      let first_position = passes
                          .iter()
                          .position(|pass| pass.id == first)
                          .expect("reference lifetime first pass must survive");
                      let last_position = passes
                          .iter()
                          .position(|pass| pass.id == last)
                          .expect("reference lifetime last pass must survive");
                      first_position <= current_position && current_position <= last_position
                  })
                  .count()
          })
          .max()
          .unwrap_or(0)
  }
  ```

- [ ] **Step 3: Update compact-ID fixtures and run their focused test.**

  Represent compact fixtures with `test_pass` IDs `[1, 2, ...]`, then compare the old coverage cases—overlap, disjoint, nested, single-pass, unused intermediate, and non-intermediate—through the new reference signature. Run:

  ```bash
  rtk cargo test --locked interval_sweep_matches_brute_force_reference_fixtures -- --nocapture
  ```

  Expected result: compilation fails because production `peak_live_intermediates` has not yet migrated; do not change production code in this task.

- [ ] **Step 4: Add direct gapped-ID cases for the semantic contract.**

  Add fixtures for `[1, 2, 4, 5]` with an interval `4..=5`, `[1, 3, 5]` with `5..=5`, and `[1, 3, 6, 8]` with `3..=6` plus `6..=6`. Assert the expected peaks `1`, `1`, and `2`, respectively, and compare each to the semantic reference. Include compact IDs `[1, 2, 3, 4]` with all existing lifetime-shape cases.

- [ ] **Step 5: Run the synthetic gapped-ID test before the production fix.**

  Run:

  ```bash
  rtk cargo test --locked post_fusion_peak_live -- --nocapture
  ```

  Expected result: the test must fail because the old production helper cannot accept the surviving pass sequence and still applies the invalid `id - 1` domain. If the failure is a compile error from the intentionally unchanged production signature, keep this RED checkpoint limited to the semantic test changes and migrate the call in Task 2 before continuing.

### Task 2: Add the real compiled fusion regressions and observe RED

**Files:**
- Modify: `src/effects/render_graph.rs` tests near `compatible_zero_footprint_local_stages_fuse_in_canonical_order`.

**Interfaces:**
- Consumes: existing effect-program validation and `compile_frame_execution_plan` fixtures.
- Produces: real compiler/fusion tests that assert gaps and compare `graph.stats.peak_live_intermediates` with the semantic reference.

- [ ] **Step 1: Add a reusable local-stage scene fixture with a later non-fusible stage.**

  Build a validated program with `Backdrop → ColorMatrix → Tint → CustomFragment`, register it, compile an `OutputPostProcess` instance, and use a non-empty region. The custom stage must be after the fusible pair so the resulting compiled graph has surviving passes after the gap.

- [ ] **Step 2: Add the single-mid-graph fusion regression.**

  Compile the fixture through `compile_frame_execution_plan`, collect `graph.passes` IDs, assert they are not `[1, 2, ..., graph.passes.len()]` and that at least one later surviving ID follows a gap, assert one pass has a fused stage, and compare `graph.stats.peak_live_intermediates` to `brute_force_peak(&graph.passes, &graph.textures)`.

- [ ] **Step 3: Add multiple real fusible instances.**

  Compile at least two fusible local-stage instances in one complete scene. Assert the complete graph has more than one missing integer in its surviving ID sequence and compare the graph statistic to the semantic reference. Ensure a lifetime endpoint is after the second gap by using the actual compiled textures, not a hand-written production statistic.

- [ ] **Step 4: Run the real RED tests.**

  Run:

  ```bash
  rtk cargo test --locked compatible_zero_footprint_local_stages_fuse_in_canonical_order -- --nocapture
  rtk cargo test --locked post_fusion -- --nocapture
  ```

  Inspect the failure and confirm it is a peak-live mismatch caused by stable IDs versus compact pass order, not a fixture validation or fusion failure.

### Task 3: Implement the bounded post-fusion ID-to-position sweep

**Files:**
- Modify: `src/effects/render_graph.rs` production helper, compile call, and test-only work counters.

**Interfaces:**
- Consumes: final `&[CompiledRenderPass]`, post-fusion `&[GraphTexturePlan]`, `MAX_GRAPH_PASSES`, and current interval/sweep counter hooks.
- Produces: `peak_live_intermediates(passes: &[CompiledRenderPass], textures: &[GraphTexturePlan]) -> usize` with exact inclusive post-fusion semantics.

- [ ] **Step 1: Extend the deterministic counter only for map construction.**

  Add `pass_position_map_entries` to `PeakLiveWorkCounters`, initialize it to zero, add `note_peak_live_pass_position_map_entry`, and update expected counter literals. Count one entry per surviving pass while building the map; retain interval insertion and sweep counters unchanged.

- [ ] **Step 2: Change the compiler call after fusion.**

  Replace `peak_live_intermediates(&builder.textures, builder.passes.len())` with `peak_live_intermediates(&builder.passes, &builder.textures)` at the existing post-fusion location. Do not move fusion or alter stats fields.

- [ ] **Step 3: Build a bounded stable-ID map.**

  At the start of the helper, return zero for an empty pass slice. Allocate `vec![None; MAX_GRAPH_PASSES + 1]`; for each `(position, pass)`, assert in debug builds that the stable ID is within the graph bound, store `Some(position)` when bounded, and increment the map-entry counter. Allocate the delta array as `passes.len() + 1`.

- [ ] **Step 4: Resolve every complete intermediate lifetime through the map.**

  Preserve the existing handling for no lifetime and one-sided lifetime. For complete lifetimes, look up both stable IDs. If both resolve and `first_position <= last_position`, insert `[first_position, last_position]` inclusively. If either endpoint is missing, `debug_assert!(false, ...)` and conservatively insert the full surviving range; do not reinterpret, clamp, or silently discard the missing ID. If resolved positions are reversed, debug-assert and skip as malformed metadata.

- [ ] **Step 5: Keep the one-pass inclusive delta sweep.**

  Add one at `delta[first_position]`, subtract at `delta[last_position + 1]`, sweep only `passes.len()` positions, and retain the existing non-negative conversion. This must remain one map build plus one texture traversal plus one pass sweep.

- [ ] **Step 6: Run the focused RED tests to GREEN.**

  Run:

  ```bash
  rtk cargo test --locked interval_sweep_matches_brute_force_reference_fixtures -- --nocapture
  rtk cargo test --locked post_fusion -- --nocapture
  rtk cargo test --locked effects::render_graph -- --nocapture
  ```

  Expected result: all focused peak-live, fusion, inclusive-gap, and existing render-graph tests pass.

### Task 4: Lock deterministic complexity and audit the narrow diff

**Files:**
- Modify: `src/effects/render_graph.rs` tests and any expected counter literals.

**Interfaces:**
- Consumes: mapped production helper and semantic reference.
- Produces: deterministic `P=256, T=512` evidence for `O(P + T)` and no obsolete pass-count-only references.

- [ ] **Step 1: Update the large synthetic counter assertion.**

  Use 256 compact test passes and 512 intermediate textures with lifetime `1..=256`. Assert peak `512`, `pass_position_map_entries == 256`, `interval_insertions == 512`, and `sweep_steps == 256`. Do not use wall-clock timing or nested candidate counters.

- [ ] **Step 2: Search and classify remaining ID conversions.**

  Run:

  ```bash
  rtk rg -n 'peak_live_intermediates|brute_force_peak|first_use|last_use' src/effects/render_graph.rs
  rtk rg -n '\.get\(\) - 1|GraphPassId' src/effects/render_graph.rs
  rtk rg -n 'fuse_compatible_local_stages|passes\.remove' src/effects/render_graph.rs
  ```

  Confirm no production peak-live or execution-order calculation maps a stable `GraphPassId` with `id - 1`. Leave `GraphTextureId` storage-index conversions and pre-fusion graph-ID creation unchanged.

- [ ] **Step 3: Verify telemetry-only consumers.**

  Run:

  ```bash
  rtk rg -n 'peak_live_intermediates|peak_live_textures|render_graph_peak_live_textures' src
  ```

  Confirm the field flows only into compile stats/effect metrics/frame telemetry and does not control allocation, release, admission, or execution decisions.

- [ ] **Step 4: Inspect popup cleanup without changing it unless mechanical.**

  Search `src/native_output` for `popup_surface_ids`, `finalize_snapshot`, and `from_surfaces_with_popup_ids`. If the duplicate copy remains an immediate redundant `to_vec()` with no overwrite semantics, remove it only with the existing popup-ID coverage; otherwise leave it unchanged and report that decision.

- [ ] **Step 5: Run formatting and diff checks.**

  ```bash
  rtk cargo fmt --check
  ./bin/check-source-layout
  git diff --check
  rtk git status --short
  ```

  Record any pre-existing source-layout debt separately.

- [ ] **Step 6: Commit the narrow source change.**

  ```bash
  rtk git add src/effects/render_graph.rs
  rtk git commit -m "fix(effects): account for fused pass ids in peak-live stats"
  ```

### Task 5: Full verification and handoff

**Files:**
- No source changes expected; inspect the committed diff and command output.

**Interfaces:**
- Consumes: committed implementation and test changes.
- Produces: fresh verification evidence and final C2a closure report.

- [ ] **Step 1: Run locked compilation and lint.**

  ```bash
  rtk cargo check --locked --all-targets
  rtk cargo clippy --locked --all-targets -- -D warnings
  ```

- [ ] **Step 2: Run the full locked test suite.**

  ```bash
  rtk cargo test --locked
  ```

- [ ] **Step 3: Re-run final repository checks.**

  ```bash
  ./bin/check-source-layout
  git diff --check
  rtk git status --short
  rtk git log -2 --oneline --decorate
  ```

- [ ] **Step 4: Report exact outcomes.**

  Include the defect, concrete gapped sequence, independent reference, map/sweep architecture, `O(P + T)` counters, malformed-ID fallback, inclusive semantics, all regression categories, unchanged execution/resource semantics, current telemetry consumers, popup-copy decision, test commands and fresh results, changed files, both relevant commit hashes, and explicit answers to every required yes/no question. Do not claim a command passed unless its fresh exit status and output were inspected.
