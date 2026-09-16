# Typhon Visual Scanout Source Selection Implementation Plan

> **For agentic workers:** Execute this plan inline in the current checkout. Do not dispatch subagents or create a worktree. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Select the exact rendered application surface as the Direct Scanout source while retaining the application root as the owner of window policy and semantic presentation state.

**Architecture:** Extend Presentation Coverage with a source-level record selected in renderer painter order. Direct scene analysis uses the coverage root for owner checks and the selected source for all buffer/content metadata, then forwards both identities unchanged through candidate keys and direct leases. Extend the existing doctor-only snapshot with bounded visual-group details.

**Tech Stack:** Rust, Cargo, Wayland/XWayland compositor state fixtures, native Atomic Direct Scanout, explicit-sync and presentation-mode tests, Codebase Memory MCP, `rtk`.

## Global Constraints

- Keep compilation artifacts in the repository’s existing `target` directory.
- Use `rtk` for shell commands.
- Do not use subagents or create a worktree.
- Preserve unrelated dirty worktree changes; stage only files for this feature.
- Follow TDD: every production behavior change follows a RED test that failed for the intended reason.
- Keep Presentation Coverage separate from semantic fullscreen, async permission, VRR, and physical KMS qualification.
- Preserve `candidate.is_some() <=> blockers.is_empty()`.
- Preserve explicit-sync publication, DMA-BUF import, format/modifier proof, TEST_ONLY, submit, lease, pageflip, and composition fallback behavior.
- Keep diagnostics one-shot and bounded; add no frame-loop logging, file I/O, or persistent scene copy.
- Do not implement generalized multi-plane allocation, ARGB opaque-region support, or game-specific rules.

---

### Task 1: Add source-selection and scene-analysis RED regressions

**Files:**
- Modify: `src/compositor/presentation_coverage.rs` — add pure coverage tests for painter-order source selection and same-tree above-source tracking.
- Modify: `src/compositor/state/desktop_window_tests.rs` — add XWayland-style root/child scene tests using existing X11 ownership and DMABUF/SHM fixtures.
- Modify: `src/compositor/presentation_modes.rs` — add/retain async separation regression for a child-source candidate’s root policy.
- Modify: `src/native_output/scanout/direct_lease.rs` or `src/native_output/scanout/atomic_direct_tests.rs` — add the child-source/root-owner identity assertion against the existing lease constructor if a test fixture needs a small test-only constructor adjustment.
- Modify: `src/native_output/runtime/cycle_dispatch.rs` — add doctor-shape assertions for the new fields before formatter/production support exists, using the existing private test helpers.

**Interfaces:**
- Tests should target the existing `analyze_presentation_coverage`, `CompositorState::direct_scanout_scene_analysis`, `EffectivePresentation::decide`, `DirectPrimaryLease::new`, and `format_direct_scanout_doctor_detail` boundaries.
- Before production changes, assert behavior through current public/test-visible fields or stable strings so RED failures represent the missing source split rather than unresolved test APIs.

- [ ] **Step 1: Add the pure coverage RED tests.**

  Extend the existing `analyze` fixture with an owner root painted first using SHM-like data and a later output-sized child in the same visual group. Add tests that expect the child to be the selected covering source, a surface before that source not to block, a surface after it intersecting the output to be recorded as above-source, and a later surface outside the output not to block. Add the inverse case where the root is painted after the child and expect conservative unknown/root rejection behavior.

- [ ] **Step 2: Add the state-level XWayland-style RED regression.**

  Reuse `x11_snapshot`, `x11_scanout_surface`, `insert_x11`, `install_x11_scanout_surface`, and `CommittedSurfaceBuffer::shm_snapshot`. Install root `301` as an XWayland output-sized SHM surface and child `302` as an output-sized XRGB DMA-BUF `SurfacePlacement::subsurface(301, 0, 0)`, publish both generations, rebuild the active scene, and assert that the desired candidate has `root_surface_id == 301` and `surface_id == 302`. Add the same-tree-above-source and off-output cases with precise expected blocker strings where the enum is not yet present.

- [ ] **Step 3: Add RED assertions for metadata and async semantics.**

  Assert the desired candidate’s source-specific identity/generation/commit/buffer fields come from the child. Add the borderless async policy regression asserting Vsync with `NotSolitaryFullscreen`, and keep the semantic fullscreen case independent from `root_surface_id == surface_id`.

- [ ] **Step 4: Add RED coverage for lease identity and doctor shape.**

  Construct a `DirectScanoutSceneCandidate` with different root and source IDs and verify the lease constructor preserves both. Extend the doctor test fixture with a selected source and bounded group surface details; assert the formatted detail contains owner root, source, order, backend, buffer source, format, target, relation, external content, and truncation markers.

- [ ] **Step 5: Run the RED tests and record the original failures.**

  Run:

  ```bash
  cargo test --locked compositor::presentation_coverage --lib
  cargo test --locked compositor::state::desktop_window_tests --lib
  cargo test --locked compositor::presentation_modes --lib
  cargo test --locked native_output::scanout::direct_lease --lib
  cargo test --locked native_output::runtime::cycle_dispatch --lib
  ```

  Expected failures are the existing blanket `OwnerTreeHasAdditionalSurface`/root-buffer assumptions, missing source-level coverage fields, the root/source equality async gate, and the old doctor detail shape. Fix test setup/compiler mistakes only; do not modify production behavior in this task.

- [ ] **Step 6: Commit only the RED tests.**

  ```bash
  git add src/compositor/presentation_coverage.rs src/compositor/state/desktop_window_tests.rs src/compositor/presentation_modes.rs src/native_output/scanout/direct_lease.rs src/native_output/scanout/atomic_direct_tests.rs src/native_output/runtime/cycle_dispatch.rs
  git commit -m "test(scanout): reproduce visual source identity gaps"
  ```

### Task 2: Implement Presentation Coverage source selection

**Files:**
- Modify: `src/compositor/presentation_coverage.rs` — add `PresentationCoverageSurface`, selected source, and above-source IDs; select by renderer painter order and rendered targets.
- Modify: `src/compositor/state/presentation_coverage.rs` — pass source opacity proof and use the selected source’s target/buffer, not the root’s buffer.
- Test: `src/compositor/presentation_coverage.rs` and `src/compositor/state/desktop_window_tests.rs` — run Task 1 coverage/state tests.

**Interfaces:**
- `PresentationCoverageSurface { surface_id: u32, target: SurfaceTargetRect, opacity: PresentationCoverageOpacity }`.
- `PresentationCoverageApplicationGroup { root_surface_id, surface_ids, covering_surface: Option<PresentationCoverageSurface>, visible_surface_ids_above_covering }`.
- Preserve `PresentationCoverageAnalysis::opacity` as the selected source opacity for compatibility, or derive all consumers from `covering_surface.opacity` without duplicating a second truth.

- [ ] **Step 1: Implement source selection in authoritative painter order.**

  In `analyze_presentation_coverage`, materialize each selected group’s surface IDs in `group.surface_indices()` order. Find the last surface whose renderer target contains the output rectangle. Compute its opacity through the source-level closure. Scan only later same-group indices for targets intersecting the output and record their IDs. Do not inspect surface IDs, parent depth, creation order, or commit order.

- [ ] **Step 2: Preserve external above-content behavior.**

  Leave popup, layer-shell, application-group, and SSD traversal after the selected group intact. Decorations in the selected group remain above its client surfaces. Do not let behind-source or off-output same-tree surfaces enter external `visible_content_above`.

- [ ] **Step 3: Update compositor opacity proof.**

  Change `presentation_coverage_opacity` to inspect the selected source surface and its rendered target. Require DMA-BUF XRGB8888, output-sized buffer, full-output target, no visual clip, no resize projection, identity-compatible rendered placement/mapping, and the existing conservative conditions. Unknown source opacity remains non-occluding.

- [ ] **Step 4: Run the coverage/state GREEN tests.**

  Run:

  ```bash
  cargo test --locked compositor::presentation_coverage --lib
  cargo test --locked compositor::state::desktop_window_tests --lib
  ```

  Confirm the root-SHM/child-XRGB case now selects the child at coverage level and harmless behind/outside surfaces no longer cause additional-surface rejection at that layer.

- [ ] **Step 5: Commit the coverage task.**

  ```bash
  git add src/compositor/presentation_coverage.rs src/compositor/state/presentation_coverage.rs
  git commit -m "feat(compositor): select visual scanout source"
  ```

### Task 3: Split Direct Scanout root policy from source content state

**Files:**
- Modify: `src/compositor/direct_scanout.rs` — replace blanket additional-surface rejection with `OwnerTreeContentAboveSource` and expose its diagnostic string.
- Modify: `src/compositor/state/direct_scanout.rs` — use coverage root for owner checks and selected source for buffer/content metadata.
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs` — query semantic solitary fullscreen by root only; remove root/source equality assumption.
- Test: `src/compositor/state/desktop_window_tests.rs`, `src/compositor/tests/direct_scanout.rs`, `src/compositor/presentation_modes.rs` — run source identity, external blocker, and async regressions.

**Interfaces:**
- `DirectScanoutSceneCandidate { surface_id: exact source, root_surface_id: logical owner, ... }`.
- Root-owned checks: owner existence, minimized state, semantic fullscreen/presentation animation, workspace/window policy, resize/lifecycle policy, and logical window geometry.
- Source-owned checks: buffer handle, buffer identity/size/format, scale/transform/viewport, visual clip, rendered target, presentation generation, generation, commit sequence, content epoch, and source presentation metadata.

- [ ] **Step 1: Remove root-only buffer lookup and group-length rejection.**

  In `direct_scanout_scene_analysis`, obtain the root from `covering_group.root_surface_id` only for owner checks. Obtain `source_surface_id` from `covering_group.covering_surface`, find that exact active surface, and add `OwnerRootBufferMissing`/`OwnerDoesNotCoverOutput` only when the selected source is unavailable. Do not reject because `surface_ids.len() > 1`.

- [ ] **Step 2: Add precise same-tree above-source blocking.**

  If `visible_surface_ids_above_covering` is non-empty, push `OwnerTreeContentAboveSource`. Keep existing external `ApplicationContentAbove`, `PopupVisible`, `OverlayVisible`, and `ServerSideDecorationVisible` blockers unchanged. Preserve effects, animations, pending publication, and all geometry blockers.

- [ ] **Step 3: Populate every candidate field from the source.**

  Read DMA-BUF, format, size, scale, transform, viewport, clip, generation, commit sequence, buffer identity, presentation generation, content epoch, and `SurfaceData::current_presentation()` from the selected source. Keep `presented_window_rect` and root policy geometry keyed by the owner root. Set `buffer_size` from the source buffer and keep `output_size` as the physical output size.

- [ ] **Step 4: Run focused GREEN scene tests.**

  Run:

  ```bash
  cargo test --locked compositor::state::desktop_window_tests --lib
  cargo test --locked compositor::tests::direct_scanout --lib
  cargo test --locked compositor::presentation_modes --lib
  ```

  Confirm candidate source/root IDs differ in the positive case, source metadata is child-owned, same-tree above-source is precise, external blockers remain, and `candidate.is_some()` still matches empty blockers.

- [ ] **Step 5: Remove the Atomic async identity assumption.**

  Replace `server.direct_scanout_solitary_fullscreen(candidate.root_surface_id) && candidate.root_surface_id == candidate.surface_id` with only the semantic root query. Run the direct/native focused tests and retain the borderless Vsync/NotSolitaryFullscreen assertion.

- [ ] **Step 6: Commit the direct-analysis task.**

  ```bash
  git add src/compositor/direct_scanout.rs src/compositor/state/direct_scanout.rs src/native_output/scanout/atomic_egl_gbm/direct.rs
  git commit -m "fix(scanout): separate owner and visual source identity"
  ```

### Task 4: Validate native lease and physical source identity

**Files:**
- Modify: `src/native_output/scanout/direct_lease.rs` only if test support needs to expose a distinct root/source fixture; production constructor already consumes both candidate IDs.
- Modify: `src/native_output/scanout/atomic_direct_tests.rs` — add/adapt lease identity regression.
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs` — only if focused tests expose an exact-source propagation defect.
- Test: `src/native_output/tests/direct_scanout_stage4.rs`, `src/native_output/tests/presentation_transactions.rs`, and native Atomic direct tests.

**Interfaces:**
- `DirectScanoutCandidateKey.content.surface_id` remains the exact source ID.
- `DirectPrimaryLease::surface_id()` remains source ID and `root_surface_id()` remains owner ID.
- Output transaction direct obligations continue using the physical source ID; window-level presentation ownership remains root-owned.

- [ ] **Step 1: Run the lease RED/GREEN test after scene candidate changes.**

  Construct a candidate with `surface_id != root_surface_id`, create a lease through `DirectPrimaryLease::new`, and assert both IDs plus key content ID. Confirm the framebuffer/cache/damage key remains source-owned.

- [ ] **Step 2: Audit all direct source identity comparisons.**

  Use `rtk rg` for `candidate.root_surface_id == candidate.surface_id`, `candidate.surface_id`, `lease.surface_id()`, `root_surface_id`, and `OutputContentKey`. Classify each match as root-owner policy or exact-buffer identity; change only the Atomic async guard and any actual source identity defect exposed by tests.

- [ ] **Step 3: Run physical-path focused tests.**

  Run the existing native direct scanout, direct lease, presentation transaction, explicit-sync, TEST_ONLY, pageflip, and fallback filters. No KMS scheduling or generalized plane allocation changes are allowed.

- [ ] **Step 4: Commit native identity validation.**

  ```bash
  git add src/native_output/scanout/direct_lease.rs src/native_output/scanout/atomic_direct_tests.rs src/native_output/scanout/atomic_egl_gbm/direct.rs
  git commit -m "test(native): preserve direct lease source identity"
  ```

### Task 5: Extend one-shot doctor diagnostics

**Files:**
- Modify: `src/native_output/runtime/cycle_dispatch.rs` — add bounded direct-scene group/source diagnostic fields and format them only in `ControlCommand::Doctor`.
- Modify: `src/compositor/server.rs` only if a narrow read-only active-scene snapshot accessor is needed; reuse existing `renderable_surfaces`/visual-order authorities.
- Test: `src/native_output/runtime/cycle_dispatch.rs` — doctor formatting and bounded/truncation tests.

**Interfaces:**
- Keep `DirectScanoutDoctorScene::from_analysis` one-shot and extend it with `scene_source`, bounded `group_surfaces`, `visible_above`, and truncation state.
- Each reported surface includes ID, painter order, backend, SHM/DMA-BUF source, DRM format/fourcc, rendered target, and relation (`below_source`, `source`, `above_source`).

- [ ] **Step 1: Add the doctor RED formatter assertions.**

  Extend the existing doctor test fixtures with a root/source pair and assert the formatted detail contains `scene_root`, `scanout_source`, `group_surfaces`, source relation, backend, buffer source, format, target, `visible_above`, and `group_truncated=false` (plus a bounded truncation test).

- [ ] **Step 2: Implement the bounded snapshot.**

  In the Doctor dispatch only, derive the same active scene targets/order from existing server accessors and cap reported group entries at a fixed small maximum. Set an explicit truncation flag when more surfaces exist. Use stable string formatting; do not print or write during frame cycles.

- [ ] **Step 3: Run doctor-focused GREEN tests.**

  Run:

  ```bash
  cargo test --locked native_output::runtime::cycle_dispatch --lib
  ```

  Confirm no production hot path invokes the diagnostic helper.

- [ ] **Step 4: Commit diagnostics.**

  ```bash
  git add src/native_output/runtime/cycle_dispatch.rs src/compositor/server.rs
  git commit -m "feat(control): expose visual scanout source diagnostics"
  ```

### Task 6: Full verification and qualification handoff

**Files:**
- No new files unless verification identifies a direct test defect.

- [ ] **Step 1: Verify graph coverage for changed runtime paths.**

  Refresh Codebase Memory status and call `check_index_coverage` for every changed source path relied on in the final explanation. Read any reported partial ranges directly; qualify graph claims if coverage is partial.

- [ ] **Step 2: Run focused suites.**

  Run coverage, direct scene, XWayland visual-tree, fullscreen semantics, presentation mode/tearing, explicit-sync, native Atomic direct scanout, direct lease, pageflip, and doctor/control filters. Record each result.

- [ ] **Step 3: Run required repository verification.**

  ```bash
  cargo fmt --check
  cargo check --locked --all-targets
  cargo clippy --locked --all-targets -- -D warnings
  cargo test --locked
  ./bin/check-source-layout
  rtk git diff --check
  ```

- [ ] **Step 4: Compare source-layout baseline and final state.**

  Capture the baseline gate result from the starting SHA and compare the final count. Report pre-existing violations separately; claim the gate green only if the final repository actually passes.

- [ ] **Step 5: Trace the final owner/source lifecycle manually.**

  Verify root → renderer painter order → coverage source → scene candidate → source-keyed import/damage/explicit-sync → physical validation → lease `{root=owner, surface=source}` → submit/pageflip. Verify the inverse ordinary constrained motion/normal presentation invariants remain unchanged.

- [ ] **Step 6: Commit only final verification changes and report qualification boundary.**

  Run `rtk git status --short`, inspect `rtk git diff HEAD~1`, and commit any final feature changes. Report starting SHA, ending SHA, RED causes, test/check results, doctor output shape, source-layout baseline/final delta, `git diff --check`, and state explicitly that Cyberpunk must still be retested with `OBLIVION_ONE_DIRECT_SCANOUT=experimental-auto`; preserve any genuine `overlay_visible` blocker.
