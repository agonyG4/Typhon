# Typhon Presentation Animation Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the presentation-animation integration defects so transitions are retired only by physical presentation, complete visual groups use one frame-local transform, and input, damage, scanout, and renderer caches consume the same presented state.

**Architecture:** Keep the existing analytical absolute-time animator and native `NativeSceneHistory` ownership. Add typed transition identities, a small presentation metadata snapshot carried by each frame scene, and one `PresentationGroupTransform` used for renderer-space, decoration, effects, damage, and input mapping. The compositor owns mathematical transitions; the native runtime promotes physically presented metadata back to the compositor through one acknowledgement/projection seam.

**Tech Stack:** Rust, Wayland compositor state, native KMS/DRM presentation runtime, EGL/GLES renderer, existing Typhon scene-history and damage systems, deterministic unit/integration tests.

## Global Constraints

- Preserve absolute-time analytical sampling, scheduler-owned presentation timestamps, spring velocity continuity, frame ownership, explicit synchronization, visual stack groups, Direct Scanout validation, SHM `Arc` sharing, effect eviction ownership, incremental trace export, and shader uniform caching.
- Treat all existing working-tree changes as valuable; do not reset, revert unrelated changes, create another scene tree, add an animation timer/thread, or create another build directory.
- Keep canonical geometry and `RenderableSurface` state authoritative; animation work is frame-local only.
- Do not add open/close/workspace-retention/scrolling/Infinite Canvas animation.
- All documentation and implementation notes are English.

### Task 1: Transition identity and physical-settlement model

**Files:**
- Modify: `src/presentation_animation.rs`
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/state/frames.rs`
- Modify: compositor teardown paths that remove toplevel roots
- Test: `src/presentation_animation.rs` and compositor state tests

- [x] Add red model tests for mathematical settlement without retirement, exact final acknowledgement, abandoned final samples, and stale retarget acknowledgement.
- [x] Add overflow-safe `TransitionId`, store it in transitions and samples, and replace time-only settlement removal with exact-ID acknowledgement.
- [x] Keep mathematically settled transitions sampled at their exact target until acknowledged; make visible pending state drive frame demand.
- [x] Cancel transitions when roots/trees are destroyed or hidden according to existing visibility semantics.
- [x] Verify focused lifecycle tests.

### Task 2: Shared frame-local transform and deterministic signature

**Files:**
- Modify: `src/presentation_animation.rs`
- Modify: `src/compositor/state/fullscreen.rs`
- Modify: `src/compositor/state/active_scene.rs`
- Modify: `src/compositor/render.rs`
- Test: presentation-transform and render geometry tests

- [x] Add `PresentationGroupTransform` with forward/inverse point and rect mapping, outward damage mapping, scale accessors, identity checks, and bounded deterministic signature.
- [x] Store transforms in `PresentationSceneSample` with canonical basis and settlement state.
- [x] Apply transforms to root, ordinary descendants, and following popup groups without changing stack-group membership.
- [ ] Preserve fractional values through the presentation metadata and transform math; the current legacy integer `RenderableSurface`/SSD/effect consumers quantize at their compatibility boundary.
- [x] Verify translation, resize/scale, nested subsurface, popup ownership, and signature tests.

### Task 3: Decorations and effects in presentation space

**Files:**
- Modify: `src/compositor/state/window_decoration.rs`
- Modify: `src/compositor/effects.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Test: decoration/effect scene tests

- [x] Build canonical SSD layout once and transform its rendered instance for the frame-local sample.
- [x] Inverse-map decoration input through the same presented transform.
- [x] Derive frame-local effect regions and bounds through visual-group ownership without changing effect parameters or protocol bindings.
- [x] Ensure scene snapshots record the transformed decoration/effect geometry.
- [x] Verify root and child effect movement/scale and decoration coherence.

### Task 4: Native scene-history presentation authority

**Files:**
- Modify: `src/native_output/runtime/scene_history.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/presentation_ready.rs`
- Modify: `src/native_output/runtime/presentation_cycle.rs`
- Modify: `src/native_output/runtime/cycle/pageflip.rs`
- Modify: `src/compositor/server.rs`
- Test: scene-history and runtime promotion tests

- [x] Carry small presentation metadata in `NativeFrameSceneSnapshot` through ready/submitted/presented ownership.
- [x] Publish the latest physically presented projection to compositor input only from successful immediate/pageflip promotion.
- [x] Centralize acknowledgement so stale transition IDs cannot retire newer transitions and abandoned/rejected frames cannot become input authority.
- [x] Audit all promotion, rejection, suspend, reset, worker, compatibility, explicit, and immediate paths.
- [x] Verify render-ahead, abandoned-frame, final-settlement, and retarget-race tests.

### Task 5: Input, scheduler, Direct Scanout, and XWayland parity

**Files:**
- Modify: `src/compositor/state/hit_testing.rs`
- Modify: `src/compositor/state/fullscreen.rs`
- Modify: `src/compositor/state/xwayland_mode.rs`
- Modify: `src/compositor/state/tiled_layout.rs`
- Modify: `src/compositor/state/surfaces.rs`
- Modify: `src/compositor/state/active_scene.rs`
- Test: compositor interaction, scanout, and mixed Wayland/XWayland tests

- [x] Remove independent animation-clock sampling from hit testing and use the physically presented projection.
- [x] Keep pointer ownership/locality and nested subsurface mapping correct under translation and scale.
- [x] Drive scheduler demand from visible stored transitions pending acknowledgement, with no hidden-window wakes.
- [x] Keep Direct Scanout blocked until physical identity settlement and preserve `animation_transform` diagnostics.
- [x] Give each layout batch one mutation epoch and pass it to both Wayland and XWayland visual installs.
- [x] Verify interactive takeover and mixed-backend reflow behavior.

### Task 6: Renderer cache/resource separation and observability

**Files:**
- Modify: `src/egl_renderer.rs`
- Modify: `src/compositor/render.rs`
- Modify: compositor metrics structures
- Test: EGL cache and resource-counter tests

- [x] Add bounded presentation geometry identity to command/VBO cache keys without changing content generations.
- [x] Keep unchanged SHM/dmabuf/shader/effect resources reusable while allowing geometry-only rebuilds.
- [x] Expose bounded transition and geometry metrics required by the closure report.
- [x] Verify geometry changes invalidate commands but do not reupload/import/recompile unchanged content.

### Task 7: Documentation and closure verification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-08-typhon-presentation-animation-foundation-design.md`
- Modify: `docs/superpowers/plans/2026-09-08-typhon-presentation-animation-foundation.md`
- Create: `docs/reports/2026-09-08-typhon-presentation-animation-closure.md`

- [x] Update design/plan claims to match only verified source behavior.
- [x] Record inherited work, defects, final architecture, physical authority, visual groups, Effects, XWayland, scanout, performance evidence, tests, verification, hardware qualification status, and deferred scope.
- [x] Run focused tests, `rtk run -- cargo fmt --check`, `rtk run -- cargo check --locked --all-targets`, `rtk run -- cargo clippy --locked --all-targets -- -D warnings`, `rtk run -- cargo test --locked`, `rtk git diff --check`, and the source-layout check.
- [x] Perform the final authority/timing/settlement/race/damage/cache/resource/lifecycle/scanout/idle review and record the remaining fractional, source-layout, and hardware-qualification limitations in the closure report.

## Post-review corrective closure

- [x] Resolve one fresh scene at the scheduled presentation timestamp and
  freeze it into owned metadata before atomic or compatibility rendering.
- [x] Use the frozen scene for damage lineage, scene signature, presentation
  snapshot, compatibility paint, and atomic GPU draw without a second live
  scene resolution.
- [x] Keep SHM payload backing shared while freezing scene metadata.
- [x] Separate popup visual-stack roots from presentation-transform owners in
  every input path and make inverse affine mapping unbounded.
- [x] Begin move/resize from physically presented root geometry, cancel the
  transition at takeover, preserve the old physical blocker until publication,
  and avoid first-motion teleport or end-of-interaction catch-up.
- [x] Make sampled-window metrics truthful and wire the minimal
  `OBLIVION_ONE_ANIMATIONS=on|off` policy with normal fallback.
- [x] Re-run formatting, locked check, strict Clippy, focused tests, the full
  locked suite, whitespace validation, and source-layout reporting; record
  fractional and hardware qualification limits honestly.

## Final physical-root and tiled-resize authority follow-up

- [x] Extend every pageflip-confirmed `PresentationFrameSnapshot`, including
  identity frames, with metadata-only presented geometry for each visible
  toplevel root, derived from the materialized render surface geometry.
- [x] Carry the root projection through ready/submitted/presented scene
  history and promote it only for immediate or pageflip physical publication.
- [x] Map input from the current canonical root rectangle into the last
  physically promoted root rectangle, while keeping popup visual-stack roots
  distinct from presentation owners and preserving SSD hit behavior.
- [x] Preserve physical interaction baselines before takeover cancellation.
- [x] Rebase tiled resize start boundaries by the presented-versus-canonical
  client-edge delta while retaining the canonical solution, tile/client offset,
  parent, split, and topology. Add pure constrained-edge coverage.
- [x] Repair the GLES helper declaration-order regression and the infinite
  explicit-sync test iterator exposed by final verification.
- [x] Complete the final gates: formatting, locked all-target check, strict
  Clippy, locked full suite, diff check, and source-layout reporting. The full
  suite passed with 2,250 library tests, 1,297 main-binary tests, and zero
  failures; source-layout remains a documented 42-file existing debt, and
  hardware qualification remains unavailable.

## Final window-space authority follow-up (2026-09-09)

The remaining correctness closure is narrowly scoped to coordinate-space
authority. Canonical presentation rectangles, sampled transforms, physical
presentation metadata, interaction geometry, tiled resize rebasing, and
Direct Scanout ownership will all describe the toplevel/window rectangle.
Root `wl_surface` geometry remains render content inside that window frame and
is never used as the presentation target.

- Add a real CSD compositor regression for the `100,100 944x526` window frame
  versus the `100,124 944x502` client root, including identity and active
  transition samples.
- Build `presentation_window_targets()` from the shared canonical
  `presentation_rect_for_geometry()` helper.
- Rename the physical metadata record to `PresentedWindowGeometry`, derive
  composed-frame records from canonical window geometry plus the frame sample,
  and centralize integer materialization of physically presented window state.
- Make presented visual takeover and input use the canonical-window delta to
  the last physically presented window rectangle; keep popup stack roots and
  presentation owners distinct.
- Carry the validated window rectangle through Direct Scanout candidate,
  lease, submission, and pageflip ownership. Publish it only after a
  successful direct pageflip, preserving direct assignment and the accepted
  candidate during canonical races; the next composed pageflip replaces it.
- Add direct ownership/race/fallback, lifecycle, input, interaction, and
  tiled CSD/constraint regressions, then rerun all locked verification gates
  in the existing checkout and update the closure report without hardware
  qualification claims.
