# Typhon Presentation Engine v2 — Opacity Implementation Plan

> **For agentic workers:** Execute this plan inline in the current checkout. Do not dispatch subagents. Each task uses test-first steps and must preserve unrelated dirty work.

**Goal:** Implement Opacity as Typhon's second transactional `SceneNodeId` presentation property while keeping persistent canonical opacity on `DesktopWindow`, freezing immutable frame evidence, rendering inherited opacity exactly once, and settling only from exact physical promotion.

**Architecture:** Extend the existing explicit sparse engine with `geometry_tracks` and `opacity_tracks`; share transaction validation, ID reservation, transaction membership, sampling, and exact retirement without introducing dynamic property objects. Resolve opacity from `PresentationSceneSample` plus a frozen per-surface presentation-owner-root projection, then feed one scalar into CPU/EGL command composition and final owned effects output. Use `NativeSceneHistory` as the only physical authority and a focused sibling module for opacity damage.

**Tech Stack:** Rust, Cargo, Wayland compositor state, native output frame snapshots, CPU premultiplied composition, GLES 3 shaders, existing effects DAG, existing `rtk` command proxy.

## Global Constraints

- Canonical opacity is `DesktopWindow` state; `PresentationEngine` stores only temporary opacity tracks.
- `PresentationOpacity` is finite and bounded to `[0.0, 1.0]`; do not use an unclamped canonical `f32`.
- Preserve Geometry v2 IDs, transaction membership, velocity-preserving retargeting, target-time sampling, immutable evidence, physical promotion, and exact ACK behavior.
- Keep Geometry and Opacity sparse and explicitly typed; do not add a boxed-property abstraction or placeholder property variants.
- Do not change input, layout/configure behavior, Lamp/lifecycle migration, or user-visible effects.
- Do not add a GPU pass, per-window framebuffer, render thread, animation thread, timer, or canonical-scene traversal.
- Compile in the current checkout with `rtk cargo ...`; never use a separate build directory.
- Do not stage or modify pre-existing dirty work; re-check overlap and HEAD before every implementation commit.

---

### Task 0: Reconfirm overlap gate and capture the execution checkpoint

**Files:**
- Read only: `src/compositor/server.rs`, `src/compositor/state/fullscreen.rs`, `src/egl_renderer/effects/mod.rs`, and all paths reported by `git status --short`
- Modify: none until the user resolves or explicitly hands off overlapping dirty files

**Interfaces:**
- Consumes: starting HEAD `1ab0e209693481f4db31c1cc69279bc96e25f881`, branch `main`, current dirty-state baseline, approved generalized-engine design spec.
- Produces: a safe edit set and a refreshed starting checkpoint.

- [ ] **Step 1: Re-check the worktree and overlap paths**

  Run:

  ```bash
  rtk git status --short
  rtk git rev-parse HEAD
  rtk git branch --show-current
  ```

  Expected: unrelated dirty files remain untouched; if a dirty file is required for opacity, stop and request handoff/clearance.

- [ ] **Step 2: Refresh Codebase Memory status after any external change**

  Run the graph `index_status` and `check_index_coverage` calls for the exact paths used by the next task. If HEAD changed, re-index the repository before structural queries.

- [ ] **Step 3: Do not commit this checkpoint**

  Preserve the initial dirty files and record any newly changed paths in the final report.

---

### Task 1: Add typed canonical opacity and the compositor mutation seam

**Files:**
- Create: `src/presentation_animation/opacity.rs`
- Modify: `src/presentation_animation/mod.rs`
- Modify: `src/compositor/desktop_window.rs`
- Modify: the clean compositor state/server module selected by the current API boundary
- Test: focused unit tests colocated with `opacity.rs` and desktop-window/state tests in an unmodified test module

**Interfaces:**
- Consumes: `SceneNodeId`, `WindowId`, existing `DesktopWindow::new_xdg`, `DesktopWindow::new_x11`, and existing `RenderGenerationCause` scheduling.
- Produces: `PresentationOpacity`, `DesktopWindow::canonical_opacity`, getters/setters with compositor visibility, and an internal operation accepting `WindowId`, a new opacity, and immediate-or-curve mode.

- [ ] **Step 1: Write failing typed-value tests**

  Add tests named `presentation_opacity_rejects_non_finite_values`, `presentation_opacity_rejects_out_of_range_values`, and `presentation_opacity_accepts_boundaries_and_helpers`.

  Required assertions: `NaN`, positive infinity, negative infinity, values below zero, and values above one are rejected; `0.0` and `1.0` are accepted; `get`, `is_transparent`, and `is_opaque` report the typed value correctly.

- [ ] **Step 2: Run the focused tests and observe the expected missing-type failure**

  Run:

  ```bash
  rtk cargo test --locked presentation_opacity --lib
  ```

  Expected: compile/test failure because `PresentationOpacity` does not yet exist.

- [ ] **Step 3: Implement the minimal typed module**

  Use a private `f64` field, `#[derive(Debug, Clone, Copy, PartialEq)]`, explicit `TRANSPARENT` and `OPAQUE` constants, and a constructor returning `Option<Self>` or the repository's equivalent validation result. Make any test-only raw constructor private to tests.

- [ ] **Step 4: Add canonical field/default tests before wiring constructors**

  Add `xdg_windows_default_to_opaque` and `x11_windows_default_to_opaque`; assert both constructors set canonical opacity to `PresentationOpacity::OPAQUE`.

- [ ] **Step 5: Add the canonical mutation seam test**

  Add `canonical_opacity_mutation_advances_render_generation_without_layout_change`. Assert the field changes, no geometry/layout/configure counters change, and the existing render-generation mechanism is advanced for both immediate and animated requests.

- [ ] **Step 6: Implement canonical state and the seam**

  Add the field to `DesktopWindow`, initialize both constructors, expose a focused compositor-internal operation, validate owner/value before mutation, install the optional presentation request only after canonical target validation, and preserve the canonical value if request preparation fails.

- [ ] **Step 7: Run the focused tests and commit only this slice**

  Run:

  ```bash
  rtk cargo test --locked presentation_opacity --lib
  rtk cargo test --locked desktop_window --lib
  rtk cargo fmt --check
  ```

  Commit only the typed opacity/canonical-state files with:

  ```bash
  git add src/presentation_animation/opacity.rs src/presentation_animation/mod.rs src/compositor/desktop_window.rs <clean-seam-files> <focused-tests>
  git commit -m "feat(animation): add typed canonical window opacity"
  ```

---

### Task 2: Generalize the engine to explicit Geometry and Opacity tracks

**Files:**
- Modify: `src/presentation_animation/ids.rs`
- Modify: `src/presentation_animation/transaction.rs`
- Modify: `src/presentation_animation/engine.rs`
- Modify: `src/presentation_animation/frame.rs`
- Modify: `src/presentation_animation/mod.rs`
- Test: `src/presentation_animation/transactions_tests.rs`, `src/presentation_animation/math_tests.rs`, and focused opacity tests

**Interfaces:**
- Consumes: `PresentationOpacity`, existing `AnimationCurve::sample_scalar`, `PresentationTransactionMember`, Geometry v2 exact ACK behavior.
- Produces: `PresentationPropertyKind::Opacity`, `PresentationOpacityMutation`, `opacity_tracks`, mixed transaction constructors, `cancel_geometry`, `cancel_opacity`, `cancel_all`, opacity samples, and `PresentedOpacityAck`.

- [ ] **Step 1: Write failing engine tests**

  Add tests for: one-member opacity commit; mixed Geometry+Opacity commit with one transaction and distinct revisions; atomic validation failure; no-op elimination; moving retarget; settled-unACKed same-target preservation; upper/lower spring overshoot clamping; exact stale/wrong-output ACK rejection; sibling-property settlement; disable clearing tracks without changing canonical state; and property-specific cancellation.

- [ ] **Step 2: Run the new engine tests and confirm they fail for missing API/behavior**

  Run:

  ```bash
  rtk cargo test --locked opacity --lib
  rtk cargo test --locked geometry_opacity --lib
  ```

  Expected: failures must be caused by absent Opacity types/fields or behavior, not malformed tests.

- [ ] **Step 3: Extend the request and validation model**

  Add `opacity: Vec<PresentationOpacityMutation>` while preserving `geometry(...)` as a compatibility helper. Add mixed/opacity-only constructors without duplicating allocation logic. Validate all members, duplicate `(SceneNodeId, property)` keys, values, active-track start samples, and effective no-ops before reserving IDs or mutating tracks.

- [ ] **Step 4: Add typed opacity track sampling**

  Store node, transaction, revision, start/target opacity, start velocity, start time, curve, and preserve-velocity state. Use `sample_scalar`; clamp only the visible sample. If the raw sample is clamped at either boundary, report zero visible retarget velocity. Keep mathematical settlement based on the raw curve and target.

- [ ] **Step 5: Add shared accounting and exact ACK behavior**

  Count both maps in `active_count`, `metrics`, and visible pending checks. Retire Opacity only when output, node, property, transaction, revision, mathematical settlement, and settled target all match. Remove exactly one transaction member; retain sibling members until their own ACKs arrive.

- [ ] **Step 6: Replace ambiguous cancellation callsites**

  Make all interactive Geometry handoffs call `cancel_geometry`; make root/window teardown call `cancel_all`. No resize interaction may cancel canonical opacity or an active opacity track.

- [ ] **Step 7: Run Geometry plus Opacity tests and commit**

  Run:

  ```bash
  rtk cargo test --locked presentation_animation --lib
  rtk cargo fmt --check
  ```

  Commit:

  ```bash
  git add src/presentation_animation
  git commit -m "feat(animation): add transactional opacity presentation tracks"
  ```

---

### Task 3: Freeze opacity samples and owner-root evidence in native frames

**Files:**
- Modify: `src/presentation_animation/frame.rs`
- Modify: clean compositor target/sample wiring
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/frame_scene_identity.rs`
- Modify: `src/native_output/runtime/scene_history.rs`
- Modify: `src/native_output/output/damage.rs` only for minimal field wiring; put behavior in Task 6's sibling module
- Test: presentation frame tests, frame identity tests, native scene-history tests, popup/XWayland frame tests

**Interfaces:**
- Consumes: mixed-property engine samples, `presentation_owner_root_for_surface`, existing surface/SceneNode aligned filtering.
- Produces: `canonical_opacity` on `PresentationWindowTarget`, sparse `PresentationGroupOpacity`, sample/snapshot opacity accessors, frozen `presentation_owner_root_surface_id`, and immutable physical opacity evidence.

- [ ] **Step 1: Write failing frame-evidence tests**

  Add tests for static nonidentity opacity with no revision, active opacity transition evidence with transaction/revision, implicit identity lookup, old frame opacity surviving a later canonical change, popup owner-root inheritance, root A to root B continuity, and cardinality/alignment after fullscreen/lifecycle filtering.

- [ ] **Step 2: Run the tests and observe the missing frame fields**

  Run:

  ```bash
  rtk cargo test --locked presentation_frame --lib
  rtk cargo test --locked frame_scene_identity --lib
  rtk cargo test --locked scene_history --lib
  ```

- [ ] **Step 3: Add sparse opacity evidence and snapshot freezing**

  Emit active opacity entries with exact evidence; emit static canonical entries only when nonidentity; omit identity. Copy the entries into `PresentationFrameSnapshot` and include effective opacity bits, not transaction/revision identity, in the generalized visual signature.

- [ ] **Step 4: Extend the aligned projection as one abstraction**

  Extend `filter_surface_scene_nodes` to carry `(surface, SceneNodeId, presentation_owner_root)` together. Assert equal cardinality before and after filtering. Resolve owner roots while the frame is current; never reconstruct them during pageflip.

- [ ] **Step 5: Extend physical snapshot evidence and publication**

  Add the frozen owner-root field to `NativeSceneSurfaceSnapshot`; keep `NativeSceneHistory` as the only ready/submitted/presented authority. Publish opacity ACKs only from the physically promoted frame snapshot.

- [ ] **Step 6: Run focused native tests and commit**

  Run:

  ```bash
  rtk cargo test --locked presentation_animation --lib
  rtk cargo test --locked frame_scene_identity --lib
  rtk cargo test --locked scene_history --lib
  rtk cargo fmt --check
  ```

  Commit:

  ```bash
  git add src/presentation_animation/frame.rs src/native_output/runtime/frame.rs src/native_output/runtime/frame_scene_identity.rs src/native_output/runtime/scene_history.rs <clean-compositor-wiring> <focused-tests>
  git commit -m "feat(animation): freeze opacity in presentation frame samples"
  ```

---

### Task 4: Add the generalized CPU/EGL visual projection

**Files:**
- Modify: clean compositor projection files
- Modify: `src/compositor/render.rs` with wiring-only changes and focused CPU helpers
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/egl_renderer.rs` with wiring/uniform-state changes
- Modify: `src/egl_renderer/geometry.rs`
- Modify: `src/egl_renderer/program.rs`
- Test: CPU render tests, EGL geometry/shader tests, cache-key tests, popup/SSD/XWayland tests

**Interfaces:**
- Consumes: immutable sample opacity and aligned owner-root projection.
- Produces: explicit command opacity, generalized `presentation_visual_signature`, identity-default shader state, premultiplied CPU opacity, and translucent opaque-region suppression.

- [ ] **Step 1: Write failing CPU tests**

  Add pixel tests for identity fast path, 0.5 premultiplied RGBA composition, XWayland backing plus client content, SSD parity, popup inheritance, and no alpha-based input change.

- [ ] **Step 2: Write failing EGL/command tests**

  Add tests asserting command opacity resolves by presentation owner, popup visual groups inherit the toplevel opacity exactly once, nonidentity commands have empty `opaque_regions`, and equal opacity bits reuse a cache key despite different transaction/revision evidence.

- [ ] **Step 3: Run the tests red**

  Run:

  ```bash
  rtk cargo test --locked render --lib
  rtk cargo test --locked egl_renderer --lib
  ```

- [ ] **Step 4: Implement the CPU projection**

  Carry the sample and aligned owner roots through `DesktopComposeRequest`; multiply premultiplied source RGBA by the effective owner opacity before source-over. Preserve identity fast paths and include the generalized visual signature in scene reuse authority.

- [ ] **Step 5: Implement the EGL projection**

  Add `u_opacity` to ordinary/capture shader sources and initialize it to `1.0`. Carry effective opacity on each final scene draw command. Set it for every command, including identity, so stale GL uniform state cannot leak. Clear opaque regions whenever opacity is below one.

- [ ] **Step 6: Generalize the signature without changing Geometry policy**

  Rename or compatibility-wrap geometry signature flow as `presentation_visual_signature`; include effective opacity bits and preserve current Geometry signature semantics. Do not include transaction/revision IDs solely for Opacity pixels.

- [ ] **Step 7: Run CPU/EGL tests and commit**

  Run:

  ```bash
  rtk cargo test --locked render --lib
  rtk cargo test --locked egl_renderer --lib
  rtk cargo fmt --check
  ```

  Commit:

  ```bash
  git add src/compositor/render.rs src/native_output/runtime/frame.rs src/egl_renderer.rs src/egl_renderer/geometry.rs src/egl_renderer/program.rs <clean-projection-files> <focused-tests>
  git commit -m "feat(renderer): composite WindowGroup presentation opacity"
  ```

---

### Task 5: Integrate opacity exactly once with effects

**Files:**
- Modify: clean `src/compositor/effects.rs`
- Modify: `src/egl_renderer/effects/executor.rs`
- Modify: `src/egl_renderer/effects/mod.rs` only after the existing dirty edit is resolved
- Modify: `src/egl_renderer.rs` effect wiring only
- Test: effect renderer tests and compositor effect tests

**Interfaces:**
- Consumes: visual-group command opacity, owner-root sample lookup, existing `BeforeSurface`, `ReplaceSurface`, `AfterSurface`, backdrop capture, and `OutputPostProcess` semantics.
- Produces: one attenuation of the complete WindowGroup contribution; identity for global output post-process.

- [ ] **Step 1: Write failing effect regressions**

  Add pixel-level or existing diagnostic assertions for ordinary opacity, SSD opacity, `BeforeSurface`, `AfterSurface`, `ReplaceSurface` where supported, backdrop/blur capture, and two overlapping visual groups with different opacity. Add an assertion that the effect's own captured source is not pre-attenuated before final group attenuation.

- [ ] **Step 2: Run effect tests red**

  Run:

  ```bash
  rtk cargo test --locked effects --lib
  rtk cargo test --locked egl_renderer --lib
  ```

- [ ] **Step 3: Preserve effect graph ownership**

  Keep `VisualGroupId` frame-local and keep the Effects DAG authoritative. Resolve presentation owner from the current frame projection; do not add canonical opacity to `EffectNode` or `BlendSpec`.

- [ ] **Step 4: Apply capture/final-boundary rules**

  During capture of an effect's own WindowGroup, draw that group's source at identity while preserving opacity on background groups. At a final contribution associated with `visual_group: Some(group)`, multiply the complete output by the group's opacity once. Keep `OutputPostProcess` at identity unless an existing explicit rule says otherwise.

- [ ] **Step 5: Run effect tests and commit**

  Run the focused effect tests, CPU/EGL tests, and `rtk cargo fmt --check`, then commit:

  ```bash
  git add src/compositor/effects.rs src/egl_renderer/effects/executor.rs src/egl_renderer.rs <resolved-effects-module> <focused-tests>
  git commit -m "feat(effects): apply presentation opacity once to owned effect output"
  ```

---

### Task 6: Add physical presentation-opacity damage in a focused module

**Files:**
- Create: `src/native_output/output/presentation_damage.rs`
- Modify: `src/native_output/output/mod.rs`
- Modify: `src/native_output/output/damage.rs` only for minimal call wiring and snapshot field access
- Modify: `src/native_output/runtime/presentation_worker.rs` or current equivalent damage caller
- Test: focused native-output presentation damage tests

**Interfaces:**
- Consumes: immutable previous/current `NativeFrameSceneSnapshot` scene evidence, owner-root fields, presentation snapshots, surface/decorations/popup footprints, and existing effect damage.
- Produces: opacity-only damage union for previous/current owner footprints without consulting current canonical state.

- [ ] **Step 1: Write failing damage tests**

  Cover `1.0→0.5`, `0.5→0.25`, `0.5→0.5`, `0.5→1.0`, stale submitted frame ordering, popup footprint inclusion, SSD footprint inclusion, and unrelated-window exclusion.

- [ ] **Step 2: Run the tests red**

  Run:

  ```bash
  rtk cargo test --locked presentation_damage --lib
  rtk cargo test --locked native_output::output --lib
  ```

- [ ] **Step 3: Implement bounded immutable comparison**

  Compare only frozen previous/current frame opacity by presentation owner. For changed owners, union all previous/current surface bounds whose frozen owner matches, plus matching decoration bounds and effect-owned damage where required. Return no opacity-only damage for equal values.

- [ ] **Step 4: Wire the sibling module without growing `damage.rs`**

  Keep `src/native_output/output/damage.rs` below its baseline size; add only imports/call wiring and field construction. Do not move the physical authority into a second ledger.

- [ ] **Step 5: Run damage tests and commit**

  Run focused native-output tests and source-layout gate. Commit:

  ```bash
  git add src/native_output/output/presentation_damage.rs src/native_output/output/mod.rs src/native_output/output/damage.rs <damage-caller> <focused-tests>
  git commit -m "feat(damage): track physical presentation opacity transitions"
  ```

---

### Task 7: Add Direct Scanout qualification and scheduler integration

**Files:**
- Modify: clean `src/compositor/direct_scanout.rs`
- Modify: clean `src/compositor/state/direct_scanout.rs`
- Modify: clean scheduler/render-generation callsites
- Test: Direct Scanout and scheduler regression tests

**Interfaces:**
- Consumes: canonical opacity, visible active opacity tracks, physically presented opacity snapshot, and separate `PresentationCoverageOpacity` analysis.
- Produces: `PresentationOpacity` blocker, static/active/recovery behavior, and visible-only pending work scheduling.

- [ ] **Step 1: Write failing scanout tests**

  Add tests for canonical opacity below one, active visible opacity, hidden unrelated opacity, recovery after physical opacity-one promotion, and source coverage remaining a separate qualification.

- [ ] **Step 2: Run the tests red**

  Run:

  ```bash
  rtk cargo test --locked direct_scanout --lib
  ```

- [ ] **Step 3: Implement the distinct blocker**

  Add `PresentationOpacity` with diagnostic string `presentation_opacity`. Block if the covering WindowGroup's canonical or physically presented effective opacity is nonidentity; block visible active tracks only. Do not use `AnimationTransform` for opacity.

- [ ] **Step 4: Preserve scheduler and recovery semantics**

  Reuse existing pending-visible frame scheduling. Static canonical nonidentity opacity does not create continuous work. Direct Scanout recovery remains blocked until a physically promoted opacity-one frame is in `NativeSceneHistory`.

- [ ] **Step 5: Run scanout/scheduler tests and commit**

  Commit:

  ```bash
  git add src/compositor/direct_scanout.rs src/compositor/state/direct_scanout.rs <clean-scheduler-files> <focused-tests>
  git commit -m "refactor(scanout): qualify Direct Scanout by presentation opacity"
  ```

---

### Task 8: Add integration regressions, docs, and complete verification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-15-typhon-generalized-presentation-engine-design.md`
- Modify: focused existing test modules or create focused sibling test modules where source-layout permits
- Modify: no Lamp/lifecycle or user-facing configuration files

**Interfaces:**
- Consumes: all prior slices and exact physical publication path.
- Produces: proof of mixed transactions, XWayland continuity, popup/SSD inheritance, no input/layout change, and final verification evidence.

- [ ] **Step 1: Add mixed-property and multi-window regressions first**

  Prove Geometry+Opacity share a transaction but have distinct revisions; ACK Geometry first; keep Opacity/transaction alive; ACK Opacity last. Add the A/B/C effective-member-count case and stale/wrong-output/dropped-frame cases.

- [ ] **Step 2: Add XWayland/backing and visual-group regressions**

  Prove canonical opacity and active revision survive root A→B replacement, old submitted frame A publishes root A/opacity/revision evidence, popup groups inherit once, SSD matches client, and decoration mode changes do not reset opacity.

- [ ] **Step 3: Add input/layout invariants**

  Assert alpha does not alter hit testing, focus, pointer constraints, keyboard routing, move/resize, Dwindle solves, configure events, or X11 geometry configuration.

- [ ] **Step 4: Update the approved architecture document**

  Mark Opacity implemented and explicitly leave Clip, retained lifecycle/Lamp migration, and new visible effects deferred. Record source-layout baseline/final measurements and hardware qualification availability.

- [ ] **Step 5: Run the complete required verification from the current checkout**

  Run:

  ```bash
  rtk cargo fmt --check
  rtk cargo check --locked --all-targets
  rtk cargo clippy --locked --all-targets -- -D warnings
  rtk cargo test --locked
  rtk run ./bin/check-source-layout
  ```

  Source-layout output must be compared against the recorded baseline; existing debt is not silently reclassified as new opacity debt.

- [ ] **Step 6: Re-check HEAD, dirty overlap, and diff before the final commit**

  Run:

  ```bash
  rtk git status --short
  rtk git diff --stat
  rtk git diff --check
  rtk git rev-parse HEAD
  ```

  Stage only this feature's files. Commit:

  ```bash
  git commit -m "docs(animation): mark opacity property implemented"
  ```

- [ ] **Step 7: Report evidence, not assumptions**

  Include starting/ending HEAD, initial dirty state and overlap decisions, graph status/generation/coverage caveat, canonical authority, typed representation, transaction/ACK proof, rendering/effects/damage/scanout behavior, unchanged input/layout semantics, performance/resource count, focused and full Cargo results, source-layout comparison, and hardware qualification status.

