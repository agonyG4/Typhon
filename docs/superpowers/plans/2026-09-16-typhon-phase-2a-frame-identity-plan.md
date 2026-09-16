# Phase 2A Immutable Frame Scene Identity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Carry the exact canonical `SceneNodeId` for every physically snapshotted surface, server decoration, and client cursor through native frame resolution and `NativeSceneHistory` promotion without changing rendering or scheduling behavior.

**Architecture:** Keep `RenderableSurface`, renderer IDs, presentation snapshots, lifecycle snapshots, damage signatures, and Direct Scanout policy unchanged. Materialize an ordered `ActiveSceneView` node sidecar, return fullscreen-filtered surfaces and nodes as one aligned projection, freeze that identity in `NativeSceneSnapshot` and cursor/decoration evidence, and let existing ready/submitted/pageflip/immediate ownership promote the frozen package without registry queries.

**Tech Stack:** Rust, existing compositor/native-output modules, `Cow`-backed borrowed frame projections, focused unit/integration tests, Cargo and the repository `rtk` wrapper.

## Global Constraints

- Do not add `SceneNodeId` to `RenderableSurface`.
- Do not merge identity namespaces or reconstruct node IDs from surface, window, output, render, or frame IDs.
- Preserve existing render ordering, geometry, damage, animation, pacing, KMS, cursor, lifecycle, and Direct Scanout behavior.
- Keep `NativeSceneHistory` as the only physical scene authority.
- Do not query `CanonicalSceneRegistry` after a frame snapshot is created.
- Keep normal unfiltered frame resolution borrowed and avoid per-surface canonical-registry lookups.
- Keep substantial identity/projection helpers out of the 1489-line `src/native_output/runtime/frame.rs`.
- Do not modify unrelated dirty files or stage the whole repository.
- Use `rtk` for compilation, tests, formatting, and source-layout checks.

---

### Task 1: Close and document the existing scene-foundation invariants

**Files:**
- Modify: `src/compositor/state/scene_tests.rs`
- Modify: `src/compositor/state/window_decoration_tests.rs` or the focused scene test module selected by current source layout
- Modify: `docs/superpowers/specs/2026-09-16-typhon-scene-identity-foundation-design.md`

**Interfaces:**
- Consumes: existing `CanonicalSceneRegistry`, `visual_parent`, stable server-decoration node, and current render/stack helpers.
- Produces: regression coverage proving topology metadata is independent of render order and decoration node identity survives SSD/CSD/fullscreen mode transitions; documentation marked verified.

- [ ] **Step 1: Add the failing render-order parity regression.** Build an existing representative surface/decorations stack, record the current render/visual ordering, mutate only `visual_parent` metadata through the scene registry, and assert the ordering remains identical.
- [ ] **Step 2: Run the focused scene test and confirm it fails because no parity assertion exists or the required accessor is missing.**
- [ ] **Step 3: Add the failing decoration stability regression.** For one `WindowId`, obtain the server-decoration node, exercise the existing SSD/CSD/fullscreen visibility-mode transitions, and assert the node ID is unchanged whenever the window remains alive.
- [ ] **Step 4: Run the focused decoration/scene test and confirm the new assertion fails against the current test seam if necessary.**
- [ ] **Step 5: Make only the minimal test-facing accessors or fixture adjustments required by the already-implemented scene foundation.** Do not change render ordering or decoration visibility policy.
- [ ] **Step 6: Run the focused scene and decoration suites and verify they pass.**
- [ ] **Step 7: Update the scene-identity design document from `implementation in progress` to verified implementation, retaining the explicit non-goals.**
- [ ] **Step 8: Commit only these files with `test(scene): close scene identity foundation invariants`.**

### Task 2: Add the ordered ActiveScene surface/node projection

**Files:**
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/state/fullscreen.rs`
- Modify: `src/compositor/server.rs`
- Modify: `src/compositor/state/scene_tests.rs` or a focused active-scene test module

**Interfaces:**
- Consumes: `ActiveSceneView.surfaces`, existing surface/node hash maps, fullscreen composition plan, and current active-scene rebuild/refresh seams.
- Produces: `surface_scene_nodes_in_order: Vec<SceneNodeId>`, a borrowed ordered-slice accessor, and an output-frame projection returning surfaces and node IDs with equal ordering/length.

- [ ] **Step 1: Add a failing test asserting the ordered node slice has the same length/order as `ActiveSceneView::surfaces`, including reorder and content-only refresh cases.**
- [ ] **Step 2: Run the focused active-scene test and confirm the ordered accessor/vector is absent.**
- [ ] **Step 3: Add the ordered vector and accessor.** Build it during `rebuild_active_scene_view()` from the already-selected ordered surfaces and reuse it during incremental content/placement refreshes.
- [ ] **Step 4: Add a failing fullscreen/lifecycle pairing test that proves filtering removes the matching node entry rather than independently rebuilding IDs.**
- [ ] **Step 5: Implement one paired surface/node filtering helper at the compositor projection boundary.** For unfiltered frames return borrowed surface and node slices; for fullscreen-filtered frames zip, filter, and own both vectors together. Keep existing surface-only APIs as compatibility wrappers.
- [ ] **Step 6: Expose the paired projection through `OwnCompositorServer` and add debug cardinality/order assertions.**
- [ ] **Step 7: Run active-scene, fullscreen, workspace, and Direct Scanout regression suites and verify unchanged surface order/visibility behavior.**
- [ ] **Step 8: Commit only the ordered projection files with `refactor(frame): preserve ordered SceneNode projection`.**

### Task 3: Add the focused native frame identity module and freeze surface evidence

**Files:**
- Create: `src/native_output/runtime/frame_scene_identity.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/output/damage.rs`
- Modify: focused native-output frame tests, preferably `src/native_output/runtime/frame_scene_identity_tests.rs`

**Interfaces:**
- Consumes: paired active-scene projection, existing fullscreen output, lifecycle suppression predicate, presentation geometry transform, and `RenderableSurface` list.
- Produces: aligned borrowed/owned surface/node projection helpers; `ResolvedNativeFrameScene.surface_scene_node_ids`; explicit production `NativeSceneSnapshot` construction; `NativeSceneSurfaceSnapshot.scene_node_id`.

- [ ] **Step 1: Add failing focused tests for equal-length alignment, fullscreen filtering, lifecycle suppression, and presentation geometry preserving node IDs.**
- [ ] **Step 2: Run the focused tests and confirm the new projection type/field does not exist.**
- [ ] **Step 3: Implement the focused aligned projection helper using `Cow`.** Its constructor asserts equal lengths; its pair filter owns matching entries together; its presentation transformation path changes only surface geometry, never node identity.
- [ ] **Step 4: Add `surface_scene_node_ids` to `ResolvedNativeFrameScene`, populate it from the ordered paired server projection, filter it atomically with lifecycle suppression, and clone it in `into_owned()`.**
- [ ] **Step 5: Move only the aligned projection/snapshot-consistency helpers needed for layout headroom out of `frame.rs`; do not move scheduling or rendering logic.**
- [ ] **Step 6: Extend `NativeSceneSurfaceSnapshot` and add an explicit-ID constructor for `NativeSceneSnapshot`.** Assert surface/node cardinality in production and keep old deterministic constructors test-only.
- [ ] **Step 7: Update resolved-scene consistency assertions and all production snapshot construction to use the exact aligned IDs.** Do not alter `identity_signature`, surface order signatures, damage comparison, or renderer IDs.
- [ ] **Step 8: Add tests proving changing only SceneNodeId retains the same pixel/visual signature while preserving distinct evidence.**
- [ ] **Step 9: Run focused frame, damage, fullscreen, and direct-scene suites and verify they pass.**
- [ ] **Step 10: Commit with `feat(frame): freeze SceneNode identity in native scene snapshots`.**

### Task 4: Carry decoration and client-cursor SceneNode evidence

**Files:**
- Modify: `src/compositor/render.rs`
- Modify: `src/compositor/state/window_decoration.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/input_resources.rs`
- Modify: `src/native_output/output/damage.rs`
- Modify: focused decoration/cursor tests

**Interfaces:**
- Consumes: stable `SceneSource::ServerDecoration(WindowId)` and cursor-surface scene nodes from the canonical compositor registry.
- Produces: `DecorationRenderInstance.scene_node_id`, `DecorationSceneSnapshot.scene_node_id`, `ClientCursorRenderState.scene_node_id`, and `NativeClientCursorDamageState.scene_node_id`.

- [ ] **Step 1: Add failing tests for explicit decoration snapshot node identity, frozen decoration clone preservation, and client cursor damage identity.**
- [ ] **Step 2: Run the focused tests and confirm the fields/accessors are absent.**
- [ ] **Step 3: Add the decoration node fields and a separate `scene_node_id()` accessor while preserving `(WindowId, root_surface_id)` from `identity()`.** Keep old bounds constructors test-only conveniences with deterministic test IDs.
- [ ] **Step 4: Resolve the stable server-decoration node in the normal decoration builder and preserve it through `with_presentation_transform()` and lifecycle clones.**
- [ ] **Step 5: Add the exact cursor surface SceneNodeId to `ClientCursorRenderState` at the existing cursor-state resolution seam, then carry it into `NativeClientCursorDamageState`.** Compositor-owned static cursors remain synthetic-node-free.
- [ ] **Step 6: Audit cursor comparisons and damage routines so node metadata is not mixed into pixel/damage signatures solely by being present.**
- [ ] **Step 7: Run decoration, cursor, lifecycle, and damage regression suites.**
- [ ] **Step 8: Commit with `feat(frame): preserve decoration and cursor SceneNode evidence`.**

### Task 5: Prove immutable physical promotion semantics

**Files:**
- Create: `src/native_output/runtime/frame_scene_identity_tests.rs` if not created in Task 3
- Modify: `src/native_output/runtime/scene_history.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs` only if focused inspection proves an accessor is required
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs` only if direct snapshot capture needs an explicit-ID call-site update
- Modify: `src/native_output/tests/fullscreen_frame_scene.rs`, `src/native_output/tests/output_retry.rs`, and narrowly selected focused helpers as required by compilation

**Interfaces:**
- Consumes: immutable `NativeFrameSceneSnapshot`, existing ready/submitted queues, immediate/pageflip promotion, and direct-scanout `snapshot_owned()` capture.
- Produces: presented surface/decoration/cursor SceneNode inspection helpers and regression coverage for historical identity preservation.

- [ ] **Step 1: Add failing tests for ready-versus-presented, submitted-versus-presented, exact pageflip/immediate promotion, discard/rejection, destroyed canonical node after submit, and wrong/stale token behavior.**
- [ ] **Step 2: Add the XWayland A→B replacement test: resolve/submit A, replace backing surface, resolve B, then prove the old physical frame contains SA and the new one contains SB while WindowGroup G is stable.**
- [ ] **Step 3: Add the canonical-node-removal-after-submit test using the compositor registry and `NativeSceneHistory`; prove historical evidence survives without tombstones or registry lookup.**
- [ ] **Step 4: Add focused inspection helpers delegating only to the promoted immutable snapshot.**
- [ ] **Step 5: Verify Direct Scanout and compatibility paths use the same resolved snapshot and do not introduce a second identity mapping.**
- [ ] **Step 6: Run focused scene-history, direct-scanout, compatibility, XWayland, cursor, damage, and presentation suites.**
- [ ] **Step 7: Commit with `test(frame): prove physical SceneNode promotion semantics`.**

### Task 6: Documentation, audit, and final verification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md`
- Modify: `docs/superpowers/specs/2026-09-16-typhon-scene-identity-foundation-design.md` only if verification wording needs final correction

**Interfaces:**
- Consumes: verified implementation and focused test output.
- Produces: accurate Phase 2A status with legacy PresentationAnimator/Lamp boundaries explicitly retained.

- [ ] **Step 1: Update the generalized Presentation Engine gate to mark immutable frame-level SceneNode evidence and physical SceneNode promotion implemented, while keeping Presentation Engine v2 gated.**
- [ ] **Step 2: Run the final architectural checklist: no frame-time registry reconstruction, no node/pixel signature coupling, no render ordering changes, no `RenderableSurface` churn, no renderer-ID migration, and no behavior-policy changes.**
- [ ] **Step 3: Run `rtk run cargo fmt --check`, `rtk run cargo check --locked --all-targets`, `rtk run cargo clippy --locked --all-targets -- -D warnings`, `rtk run cargo test --locked`, and `rtk run ./bin/check-source-layout`.**
- [ ] **Step 4: Record exact exit output, source-layout baseline/final counts, concurrent dirty files, and any blockers without modifying unrelated work.**
- [ ] **Step 5: Commit documentation with `docs(scene): mark physical frame identity foundation implemented`.**

