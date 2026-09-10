# Typhon C2a Frame-Local Metadata Reuse and Graph Planning Implementation Plan

> **For agentic workers:** Execute this plan inline, task by task, with a fresh focused test cycle at every task boundary. Do not dispatch subagents.

**Goal:** Remove duplicate snapshot/hash work inside one resolved native frame, replace post-fusion render-graph peak-live analysis with an exact linear sweep, instrument the remaining metadata work, and produce a source-backed C2b recommendation without adding a persistent cache.

**Architecture:** `ResolvedNativeFrameScene` will publish one complete immutable `NativeSceneSnapshot` plus one cached exact-frame identity. Same-frame consumers borrow it; history/transaction boundaries explicitly clone it once. Render-graph liveness will use a post-fusion inclusive interval sweep over bounded pass events. Test-only counters and existing opt-in perf logging will distinguish legitimate per-frame work from eliminated duplication.

**Tech Stack:** Rust, Cargo, existing native perf logger, deterministic unit fixtures, `rtk`, current checkout and existing Cargo target directory.

## Global Constraints

- Work in `/home/agony/GitHub/Typhon`; do not create another checkout or worktree.
- Reuse the existing Cargo target/build directory; do not set `CARGO_TARGET_DIR` or create a benchmark build tree.
- Use `rtk` for Cargo, ripgrep, Git, and source-layout verification commands.
- Do not use subagents.
- Do not add persistent scene/snapshot/effect topology caches, `Arc` scene-history redesign, or cross-frame reuse keys in C2a.
- Preserve ready/submitted/presented exact-frame ownership, scene-history capacity, presentation animation policy, damage authority, Direct Scanout, C1 resource realization, B2 effect demand, and effect shader/resource behavior.
- Preserve `NativeSceneSnapshot::identity_signature()` fields and its separate effect-signature mixing semantics exactly.
- Keep `RenderGraphCompileStats` fields and values unchanged; only the peak-live derivation changes.
- Use RED → GREEN → REFACTOR and run focused tests after each migration.

---

### Task 1: Establish the failing frame-local duplication regression

**Files:**
- Modify: `src/native_output/runtime/frame.rs:10-175` for a `#[cfg(test)]` local work counter seam and the first regression.
- Test: the existing `frame.rs` test module around `src/native_output/runtime/frame.rs:582`.

**Interfaces:**
- Consumes: the current `ResolvedNativeFrameScene::snapshot()` and `scene_identity_signature()` implementations.
- Produces: a test-only frame-local counter API that can prove the current path performs duplicate finalization/hash work without changing production behavior when tests are disabled.

- [ ] **Step 1: Add the RED test before changing the snapshot API.**

  Add a deterministic test in the existing `frame.rs` test module. Build the current one-surface server fixture inline with the already-defined `test_surface()` helper and `OwnCompositorServer::bind_cpu_composition()`, call `install_native_frame_test_scene()`, and keep the server alive while the resolved scene is borrowed:

  ```rust
  #[test]
  fn scene_identity_and_damage_currently_finalize_snapshot_twice() {
      let socket_name = format!("typhon-c2a-snapshot-{}", process::id());
      let mut server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
      server.install_native_frame_test_scene(
          vec![test_surface(701, 320, 200, SurfacePlacement::root_at(0, 0))],
          &[(701, WindowId::from_raw(1).unwrap())],
          None,
      );
      let resolved = ResolvedNativeFrameScene::from_server_at(
          &server,
          AnimationTime::from_nanos(0),
      );
      reset_snapshot_work_counters();

      let _current = resolved.snapshot();
      let _identity = resolved.scene_identity_signature();

      assert_eq!(snapshot_work_counters().snapshot_finalizations, 1);
  }
  ```

  The counter must count each current `snapshot()` finalization. The expected value of one is intentionally the desired post-C2a contract; the current implementation performs two and must fail for that reason, not because fixture construction or compilation is broken. If the existing test fixture cannot construct effects/visibility, use the smallest existing `OwnCompositorServer` fixture and keep the test focused on the current frame API.

- [ ] **Step 2: Run the single test and record the RED evidence.**

  Run:

  ```bash
  rtk cargo test --locked resolved_native_frame_scene_currently_finalize_snapshot_twice -- --exact --nocapture
  ```

  Expected: FAIL with the observed current finalization count of two versus the desired one. Record the exact failure in the implementation notes/report; do not weaken the assertion.

- [ ] **Step 3: Add only the test-only counter plumbing needed by the RED test.**

  Keep counters local to the frame module or a test-only helper. Do not introduce a process-global production metric. The counter must be reset/read by the test and increment only in the existing `snapshot()` path. Re-run the exact test to confirm the failure remains the intended behavior.

- [ ] **Step 4: Commit the baseline regression evidence.**

  ```bash
  git add src/native_output/runtime/frame.rs src/native_output/runtime/mod.rs
  git commit -m "test(native): capture C2a snapshot duplication"
  ```

### Task 2: Finalize the exact snapshot once and cache identity

**Files:**
- Modify: `src/native_output/runtime/frame.rs:11-175`.
- Modify: `src/native_output/output/damage.rs:285-374` to keep final snapshot field initialization in one construction path.
- Test: `src/native_output/runtime/frame.rs` tests and existing native output scene tests.

**Interfaces:**
- Consumes: `NativeSceneSnapshot::from_surfaces_with_popup_ids()`, `FullscreenRenderPlanMetrics`, `ResolvedEffectScene`, and current signature mixing.
- Produces: `snapshot_ref(&self) -> &NativeSceneSnapshot`, explicit `snapshot_owned(&self) -> NativeSceneSnapshot`, and cached `scene_identity_signature: u64` inside `ResolvedNativeFrameScene`.

- [ ] **Step 1: Add failing assertions for finalized fields and cached reads.**

  Extend the existing `frame.rs` server fixture with popup/overlay/effect/fullscreen inputs. Before changing production code, assert the current stored `resolved.snapshot` dynamic fields and the current `snapshot()` work counter against the desired finalized values. The test must fail because the stored snapshot currently has default/unfinished dynamic fields and because repeated identity reads currently re-finalize it.

  Use the exact semantic comparison shape:

  ```rust
  let resolved = ResolvedNativeFrameScene::from_server_at(&server, at);
  assert_eq!(resolved.snapshot.popup_surface_ids, expected_popups);
  assert_eq!(resolved.snapshot.external_overlay_surface_ids, expected_overlays);
  assert_eq!(resolved.snapshot.visibility_signature, expected_visibility);
  assert_eq!(resolved.snapshot.effect_damage, expected_effect_damage);
  assert_eq!(resolved.snapshot.effect_identity_signature, effects.signature);

  reset_snapshot_work_counters();
  let first = resolved.scene_identity_signature();
  let second = resolved.scene_identity_signature();
  assert_eq!(first, second);
  assert_eq!(snapshot_work_counters().snapshot_finalizations, 0);
  ```

- [ ] **Step 2: Move effect resolution before snapshot finalization.**

  In `from_server_at()`, resolve `effects` immediately after presentation-transformed surfaces/decorations and before constructing the final snapshot. Build the base snapshot once, then install:

  ```rust
  snapshot.popup_surface_ids = popup_surface_ids.to_vec();
  snapshot.external_overlay_surface_ids = external_overlay_surface_ids.clone();
  snapshot.visibility_signature = visibility_signature(visibility);
  snapshot.effect_damage = effects.instances.iter().fold(...);
  snapshot.effect_identity_signature = effects.signature;
  ```

  Keep the exact existing visibility mixing order and the exact effect-damage union semantics. Extract the pure visibility helper only if that avoids borrowing issues; do not alter the values or FullscreenRenderPlan identity semantics.

- [ ] **Step 3: Compute and store the exact identity during construction.**

  Preserve the old conceptual formula:

  ```rust
  let mut signature = snapshot.identity_signature();
  signature ^= effects.signature;
  signature = signature.wrapping_mul(0x1000_0000_01b3);
  ```

  Store the result in the frame scene. `scene_identity_signature()` must only return that field. Do not use `render_generation` as a substitute.

- [ ] **Step 4: Replace the misleading owned accessor with explicit APIs.**

  Implement:

  ```rust
  pub(crate) fn snapshot_ref(&self) -> &NativeSceneSnapshot;
  pub(crate) fn snapshot_owned(&self) -> NativeSceneSnapshot;
  ```

  Remove the generic `snapshot()` method after all callers are migrated. Make debug consistency checks allocation-free by comparing iterators directly:

  ```rust
  debug_assert!(
      self.surface_ids().eq(self.snapshot.surfaces.iter().map(|surface| surface.surface_id))
  );
  ```

  Do the equivalent for decoration identities. These checks must not allocate temporary `Vec`s.

- [ ] **Step 5: Run focused frame tests and verify GREEN.**

  ```bash
  rtk cargo test --locked native_output::runtime::frame -- --nocapture
  rtk cargo test --locked resolved_native_frame_scene -- --nocapture
  ```

  Expected: the finalized-field, signature-equivalence, and repeated-read tests pass, and the Task 1 counter now reports one finalization during construction rather than duplicate finalization on consumers.

- [ ] **Step 6: Commit the frame-local authority change.**

  ```bash
  git add src/native_output/runtime/frame.rs src/native_output/output/damage.rs
  git commit -m "perf(native): reuse resolved frame metadata"
  ```

### Task 3: Migrate damage, history, compatibility, and atomic consumers

**Files:**
- Modify: `src/native_output/runtime/presentation_worker.rs:271-330`.
- Modify: `src/native_output/runtime/scene_history.rs:14-26`.
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs` and its `direct.rs`/`worker.rs` companion call sites found by the final source audit.
- Modify: `src/native_output/runtime/presentation_cycle.rs` and `src/native_output/runtime/presentation_worker.rs` only where resolved scene snapshots/signatures are consumed.
- Test: `src/native_output/runtime/scene_history.rs`, `src/native_output/output/damage.rs`, `src/native_output/tests/fullscreen_frame_scene.rs`, atomic rendered-scene identity tests, and presentation animation tests.

**Interfaces:**
- Consumes: `snapshot_ref()`, `snapshot_owned()`, and O(1) `scene_identity_signature()` from Task 2.
- Produces: zero current-snapshot clones during damage comparison and exactly one explicit metadata clone at each freeze/transaction ownership boundary.

- [ ] **Step 1: Add the borrowed-damage RED regression.**

  Add a test-only clone counter or a compile-time signature assertion around `native_scene_damage_for_resolved_scene()`. The test must exercise a presented previous snapshot plus a resolved current scene and assert damage equality against the pre-C2 reference while expecting no current snapshot clone. The failing baseline should identify that `resolved_scene.snapshot()` creates an owned clone.

- [ ] **Step 2: Migrate damage comparison to a borrow.**

  Change:

  ```rust
  native_output_damage_for_scene_snapshots(
      width,
      height,
      previous_scene,
      &resolved_scene.snapshot(),
      cursor_damage,
  )
  ```

  to:

  ```rust
  native_output_damage_for_scene_snapshots(
      width,
      height,
      previous_scene,
      resolved_scene.snapshot_ref(),
      cursor_damage,
  )
  ```

  Preserve all `NativeSurfaceDamageEvidence`, effect transition damage, surface membership transitions, and cursor transition behavior.

- [ ] **Step 3: Migrate scene-history freezing to one explicit clone.**

  In `NativeFrameSceneSnapshot::from_resolved_frame_scene()`, use `resolved.snapshot_owned()`. Prove that this is the only clone required for a frozen ready/history snapshot and that history still owns independent `NativeSceneSnapshot` data.

- [ ] **Step 4: Audit atomic and compatibility paths.**

  Migrate all `resolved_snapshot`/`resolved_scene_signature` paths so the signature is an O(1) read and an owned snapshot is created only when a transaction or ready-scene boundary requires it. Ensure `replace_ready_scene_and_signature()` uses the same finalized metadata as compatibility and atomic rendering. Do not alter transaction identity, Direct Scanout eligibility, release ownership, KMS worker ownership, or scene-history promotions.

- [ ] **Step 5: Add and run ownership/animation regressions.**

  Cover:

  ```text
  freeze frame A -> mutate current scene -> resolve B -> A remains unchanged
  t1 and t2 animation samples -> transformed bounds/snapshots differ
  same canonical scene generation -> different frame-local identity when geometry differs
  ready/submitted/presented promotion -> exact existing ownership semantics
  ```

  Run:

  ```bash
  rtk cargo test --locked scene_history -- --nocapture
  rtk cargo test --locked native_scene_damage -- --nocapture
  rtk cargo test --locked fullscreen_frame_scene -- --nocapture
  rtk cargo test --locked presentation_animation -- --nocapture
  rtk cargo test --locked atomic -- --nocapture
  ```

- [ ] **Step 6: Commit the migrated ownership consumers.**

  ```bash
  git add src/native_output/runtime/presentation_worker.rs src/native_output/runtime/scene_history.rs src/native_output/runtime/presentation_cycle.rs src/native_output/scanout/atomic_egl_gbm.rs src/native_output/scanout/atomic_egl_gbm/direct.rs src/native_output/scanout/atomic_egl_gbm/worker.rs
  git commit -m "perf(native): borrow resolved snapshots for damage"
  ```

### Task 4: Add the exact post-fusion interval sweep

**Files:**
- Modify: `src/effects/render_graph.rs:15-17,620-793`.
- Test: `src/effects/render_graph.rs` test module around line 1129.

**Interfaces:**
- Consumes: post-fusion `GraphTexturePlan.first_use/last_use`, `GraphPassId`, `MAX_GRAPH_PASSES`, and existing fused lifetime rewrites.
- Produces: a private `peak_live_intermediates(&[GraphTexturePlan], usize) -> usize` helper and a test-only brute-force/reference work counter.

- [ ] **Step 1: Add deterministic RED tests for the exact definition.**

  Add fixture tests for one-pass, overlap, disjoint, nested, same first/last, unused, non-intermediate, and many-interval lifetimes. Define the reference exactly as the current implementation:

  ```rust
  fn brute_force_peak(textures: &[GraphTexturePlan], pass_count: usize) -> usize {
      (0..pass_count)
          .map(|pass_index| {
              textures
                  .iter()
                  .filter(|texture| {
                      texture.source == GraphTextureSource::Intermediate
                          && texture.first_use.is_some_and(|first| usize::from(first.get() - 1) <= pass_index)
                          && texture.last_use.is_some_and(|last| usize::from(last.get() - 1) >= pass_index)
                  })
                  .count()
          })
          .max()
          .unwrap_or(0)
  }
  ```

  The first production-equivalence test must fail until the new helper exists and is wired into stats.

- [ ] **Step 2: Implement the inclusive event sweep.**

  Use a fixed `Vec<i32>` sized to `pass_count + 1` (or an equally bounded structure). For each intermediate with `Some(first)` and `Some(last)`:

  ```rust
  let first = usize::from(first.get() - 1);
  let last = usize::from(last.get() - 1);
  if first <= last && last < pass_count {
      delta[first] += 1;
      if last + 1 < pass_count {
          delta[last + 1] -= 1;
      }
  }
  ```

  Sweep all pass indices, track the maximum, and keep malformed metadata conservative. Use `debug_assert!(first <= last)` for invariant diagnostics; do not panic in production solely for this diagnostic statistic. A texture with `first == last` must remain live for one pass.

- [ ] **Step 3: Switch production stats only after the helper matches the reference.**

  Call the helper after `fuse_compatible_local_stages(&mut builder)`. Do not move the calculation before fusion. Preserve every other `RenderGraphCompileStats` field and the effect execution graph.

- [ ] **Step 4: Add post-fusion and work-scaling tests.**

  Compile an existing compatible fragment-stage fixture that fuses, compare the sweep against the brute-force reference after fusion, and add a synthetic `P=256`, `T=512` fixture whose test-only work accounting reports `T` interval insertions plus `P` sweep steps rather than `P*T` candidate checks. Do not assert elapsed time.

- [ ] **Step 5: Run focused graph tests and commit.**

  ```bash
  rtk cargo test --locked effects::render_graph -- --nocapture
  rtk cargo test --locked effect_graph_fusion -- --nocapture
  rtk cargo test --locked B2 -- --nocapture
  git add src/effects/render_graph.rs
  git commit -m "perf(effects): linearize graph lifetime analysis"
  ```

### Task 5: Instrument remaining scene/effect metadata work

**Files:**
- Modify: `src/native_output/runtime/frame.rs`, `src/native_output/output/damage.rs`, and `src/compositor/render.rs` for bounded snapshot/group/projection counters.
- Modify: `src/effects/render_graph.rs`, `src/effects/validation.rs`, and `src/effects/registry.rs` for bounded graph compile, instance compile, node visit, and lookup/output map work counters.
- Modify: existing native perf metric modules only where the opt-in logger already defines CPU preparation fields; do not add unconditional clocks.
- Test: unit counter tests alongside the affected modules.

**Interfaces:**
- Consumes: existing `NativePerfLogger` and effect compile helpers.
- Produces: exact counter snapshots usable by deterministic fixtures and explicit opt-in timing fields for scene/effect CPU preparation, without allocator replacement.

- [ ] **Step 1: Add disabled-path counter tests.**

  Define a bounded local `C2WorkCounters` value or equivalent test seam with fields for snapshot builds, owned clones, identity computations, surface projection visits, visual-group builds, graph compiles, instance compiles, node visits, program lookup-map builds, output-map builds, interval insertions, and sweep steps. Assert counters are deterministic and resettable within one test; avoid process-global production state.

- [ ] **Step 2: Increment counters at semantic work boundaries.**

  Count `NativeSceneSnapshot::from_surfaces_with_popup_ids()` once per actual build, count each surface projection and visual group construction, count explicit owned snapshot clones, count cached identity computation only during construction, count `compile_frame_execution_plan()` and `compile_instance()` calls, count topological node visits, count per-instance node lookup/output map builds, and count interval/sweep operations. Do not count every field read or add an `Instant::now()` to disabled production paths.

- [ ] **Step 3: Integrate opt-in CPU timing only where existing infrastructure supports it.**

  If the current native perf logger is enabled, record separate `resolved_scene_prepare_us`, `native_snapshot_build_us`, `scene_identity_us`, and `effect_graph_compile_us` domains. Build timing fields only inside the existing enabled closure. If the local path does not support a clean domain, leave it counter-only and document why.

- [ ] **Step 4: Run counter tests and commit.**

  ```bash
  rtk cargo test --locked c2 -- --nocapture
  rtk cargo test --locked render_graph -- --nocapture
  git add src/native_output/runtime/frame.rs src/native_output/output/damage.rs src/compositor/render.rs src/effects/render_graph.rs src/effects/validation.rs src/effects/registry.rs src/native_output/perf.rs src/native_output/metrics.rs
  git commit -m "perf: instrument C2 metadata preparation"
  ```

### Task 6: Run deterministic workload qualification and write the C2a report

**Files:**
- Create: `docs/research/2026-09-10-typhon-c2a-frame-local-metadata-reuse-report.md`.
- Modify: affected Rust test modules to host deterministic workload helpers.

**Interfaces:**
- Consumes: C2 work counters, graph stats, exact identity/ownership tests, and any opt-in native perf output.
- Produces: source-backed before/after evidence and a ranked C2b recommendation, with no cache implementation.

- [ ] **Step 1: Add deterministic workload fixture coverage.**

  Cover one ordinary surface, 16 and 100+ ordinary surfaces, a deep popup/subsurface tree, repeated presentation samples at distinct timestamps, one effect, 8 effects, and 32 effects. If a fixture reaches a configured graph or test-resource limit before 32 effects, record the largest successful size and the exact limiting invariant. Also cover uniform-only parameter changes and changing effect geometry. Record exact counters for each workload. Preserve SHM payload identity and canonical generation in the animation cases while asserting transformed snapshot bounds change.

- [ ] **Step 2: Collect before/after work-unit evidence from Git history and current tests.**

  Use the committed baseline regression and the current counters to report:

  ```text
  snapshot builds/finalizations
  owned metadata clones by consumer path
  identity computations and surface/decorations visits
  visual-group/projection work
  graph compile/instance/node/map work
  old P*T candidate checks versus new T event insertions + P sweep steps
  ```

  Do not invent heap bytes. If `perf`, `heaptrack`, or native hardware execution is unavailable, say so and use exact deterministic counters only. If timing is available, report median, p95 when sample count supports it, and sample count separately for CPU preparation domains.

- [ ] **Step 3: Write the source-backed report.**

  Include the 30 requested report items and explicitly answer every required yes/no question. Explain every production snapshot clone remaining as one of:

  ```text
  required exact ownership freeze
  small deliberate metadata copy
  remaining optimization candidate
  ```

  Classify C2b as one or more ranked choices only from the measurements. Document structural invalidators, uniform-only parameter behavior, future Footprint/Structure handling, and why transformed presentation geometry remains frame-local.

- [ ] **Step 4: Run the report/source audit commands.**

  ```bash
  rtk rg -n 'ResolvedNativeFrameScene|snapshot\\(|snapshot_ref|scene_identity_signature' src/native_output
  rtk rg -n 'NativeSceneSnapshot::from_surfaces|visual_stack_groups|visual_root_by_surface_id|render_scene_elements_for_surfaces' src/native_output src/compositor
  rtk rg -n 'compile_frame_execution_plan|compile_instance|peak_live_intermediates|HashMap::<EffectNodeId|collect::<HashMap' src/effects
  rtk rg -n 'NativeFrameSceneSnapshot|replace_ready|submitted|presented' src/native_output/runtime
  ```

  For each remaining clone, cite the call site and ownership reason in the report. Confirm no `last_graph_cache`, topology LRU, persistent pass template cache, previous snapshot cache, or cross-frame effect/scene reuse key was added.

- [ ] **Step 5: Commit the qualification report.**

  ```bash
  git add docs/research/2026-09-10-typhon-c2a-frame-local-metadata-reuse-report.md docs/research/2026-09-07-current-performance-resource-audit.md
  git commit -m "docs: qualify Typhon C2a measurements"
  ```

### Task 7: Full verification and final handoff

**Files:**
- No source changes unless verification exposes a test failure from the C2a changes.

**Interfaces:**
- Consumes: all C2a commits and the report.
- Produces: fresh verification evidence, final commit hashes, and a clean or explicitly explained working-tree status.

- [ ] **Step 1: Run focused required tests.**

  ```bash
  rtk cargo test --locked native_resolved_frame_scene -- --nocapture
  rtk cargo test --locked NativeSceneSnapshot -- --nocapture
  rtk cargo test --locked scene_history -- --nocapture
  rtk cargo test --locked fullscreen_frame_scene -- --nocapture
  rtk cargo test --locked presentation_animation -- --nocapture
  rtk cargo test --locked atomic -- --nocapture
  rtk cargo test --locked effects::render_graph -- --nocapture
  rtk cargo test --locked effect_graph_fusion -- --nocapture
  rtk cargo test --locked B2 -- --nocapture
  rtk cargo test --locked C1 -- --nocapture
  ```

- [ ] **Step 2: Run full verification in the existing target directory.**

  ```bash
  rtk cargo fmt --check
  rtk cargo check --locked --all-targets
  rtk cargo clippy --locked --all-targets -- -D warnings
  rtk cargo test --locked
  ./bin/check-source-layout
  git diff --check
  rtk git status --short
  ```

  Report any pre-existing source-layout debt separately from C2a failures. Do not claim completion until every command has fresh exit-status evidence.

- [ ] **Step 3: Inspect commits and hand off.**

  ```bash
  rtk git log --oneline --decorate -8
  rtk git status --short --branch
  ```

  Final handoff must name changed files, commit hashes, exact counters/workloads, clone classifications, verification results, hardware/timing limitations, and the ranked C2b recommendation. State explicitly that no persistent graph or scene cache was introduced.
