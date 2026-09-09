# Typhon Presentation Animation Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a deterministic presentation-animation core and the confirmed low-risk performance foundation without creating a second layout, scene, or frame-clock authority.

**Architecture:** Keep canonical compositor geometry in the existing WM/compositor state. Add a pure `presentation_animation` module that samples typed per-window transitions at an absolute scheduler timestamp, then carry frame-local presentation metadata through renderer, input, damage, scanout, and native scene-history seams. In parallel, remove confirmed unbounded/copy-heavy bookkeeping with consumable eviction IDs, incremental trace export, and copy-on-write SHM snapshots.

**Tech Stack:** Rust 2024, existing Cargo test harness, `Arc<[u32]>`, existing native scheduler/presentation target types, existing EGL/effects resources.

## Global Constraints

* Work in `/home/agony/GitHub/Typhon`; reuse the existing `target/` artifacts.
* Preserve all pre-existing working-tree edits; do not reset or overwrite them.
* Use the native scheduler's `PresentationTarget.presentation_time`; do not add a timer, sleep loop, or animation thread.
* Animation sampling must not mutate canonical layout, `ToplevelVisualGeometry`, `RenderableSurface`, or `ActiveSceneView`.
* Preserve early SHM buffer release and immutable historical snapshots.
* Do not implement window-open/close retention, workspace retention, scrolling layouts, Infinite Canvas, camera transforms, or a public animation DSL.

### Task 1: Establish compiling baseline repairs and animation RED tests

**Files:**
- Modify: `src/compositor/mod.rs`
- Modify: `src/compositor/state/frame_callbacks.rs`
- Modify: `src/compositor/state/surface_commits.rs`
- Create: `src/presentation_animation.rs`
- Modify: `src/lib.rs`
- Test: `src/presentation_animation.rs` module tests

**Interfaces:**
- `PresentationAnimator::retarget(key, target, now, curve)` creates/replaces a typed transition.
- `PresentationAnimator::sample(key, now) -> Option<PresentationWindowSample>` is pure with respect to transition state.
- `PresentationAnimator::active_count() -> usize`, `has_pending_visible(&[u32]) -> bool`, and `metrics() -> PresentationAnimationMetrics` expose bounded scheduling and observability state.

- [ ] Add the failing mathematical and authority-facing tests first: easing endpoints, spring arbitrary-time sampling, retarget continuity, deterministic repeated sampling, settlement, geometry damage rounding, and disabled/immediate behavior.
- [ ] Run `rtk cargo test --locked presentation_animation` and record the expected missing-module/API failures.
- [ ] Repair only the existing explicit-sync compile errors: import `ActiveSurfacePresentationCommit` where it is used and add `presentation_feedbacks: Vec::new()` to both pending-commit literals.
- [ ] Run `rtk cargo test --locked --lib` to confirm the baseline repairs are isolated before implementing animation behavior.

### Task 2: Implement the pure analytical presentation-animation core

**Files:**
- Modify: `src/presentation_animation.rs`

**Interfaces:**
- `AnimationTime(u64)` is an absolute monotonic nanosecond value.
- `PresentationRect { x, y, width, height }` stores finite logical geometry.
- `PresentationVelocity { x, y, width, height }` stores derivative values per second.
- `AnimationCurve::{Easing(EasingCurve), Spring(SpringSpec)}` evaluates position and velocity from absolute elapsed time.
- `PresentationTransition` retains start/target state, start velocity, start time, curve, and settlement epsilon.
- `PresentationSceneSample` contains sampled time, immutable window samples, shared group transforms, active/sample counts, and a deterministic geometry signature. Damage is derived from the same frame-local presentation state rather than maintained as an independent animation authority.

- [ ] Implement finite/valid geometry constructors and outward damage union helpers.
- [ ] Implement time-bounded easing with clamped progress and deterministic endpoints.
- [ ] Implement closed-form under-damped, critically damped, and over-damped spring position/velocity with mass fixed at one.
- [ ] Implement retargeting from a sampled position and velocity, preserving continuity and replacing only the destination/curve.
- [ ] Implement settlement by displacement and velocity epsilon, including exact target output after settlement.
- [ ] Run the focused tests and confirm the RED tests become GREEN.
- [ ] Run the full library tests and refactor only while tests stay green.

### Task 3: Remove unbounded effect eviction checkout work

**Files:**
- Modify: `src/egl_renderer/effects/resources.rs`

**Interfaces:**
- `EffectResourcePool::evicted_texture_ids()` remains a compatibility snapshot only for tests/diagnostics.
- `EffectResourcePool::drain_evicted_texture_ids()` consumes pending deletion IDs exactly once.
- `EffectResourceMetrics.eviction_count` is cumulative and does not depend on pending queue length.

- [ ] Add failing tests for repeated eviction, draining, no duplicate IDs, cumulative metrics, checked-out survival, and steady-state queue emptiness after consumption.
- [ ] Run the focused resource tests and verify they fail because history is retained.
- [ ] Replace `evicted_ids: Vec<u64>` with a pending queue/set plus `eviction_count: u64`; enqueue each removed ID once.
- [ ] Update GL checkout and cleanup paths to drain only pending IDs and delete each GL texture once.
- [ ] Run `rtk cargo test --locked resources` and the full library suite.

### Task 4: Make SHM surface representation clones O(1)

**Files:**
- Modify: `src/render_backend/buffer.rs`
- Modify: `src/compositor/state_data.rs`
- Modify: `src/compositor/surface.rs`
- Test: `src/render_backend/buffer.rs` tests

**Interfaces:**
- `ShmBufferSnapshot` stores `Arc<[u32]>` and exposes read-only `pixels()`.
- Mutable publication uses copy-on-write and returns `&mut [u32]` only for the new owned snapshot.
- `CommittedSurfaceBuffer` and `RenderableSurface` clones share immutable payload storage.

- [ ] Add a failing pointer-identity/clone test proving representation clones share pixel backing and a mutation-isolation test proving copy-on-write.
- [ ] Run the focused buffer tests and verify the expected deep-copy behavior is observable.
- [ ] Convert snapshot storage to `Arc<[u32]>`, use `Arc::make_mut` for partial damage publication, and preserve size validation/release semantics.
- [ ] Run SHM lifecycle, surface-frame, and full library tests.

### Task 5: Export presentation traces incrementally

**Files:**
- Modify: `src/native_output/presentation/trace.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/native_output/runtime/cycle.rs`
- Test: `src/native_output/presentation/trace.rs` tests

**Interfaces:**
- `PresentationTransactionTraceRing::export_delta(previous) -> TraceExport` returns unchanged, append-only JSONL, or bounded full replacement.
- The runtime stores only a `(len, dropped)` cursor; it rewrites only after ring overwrite/drop or failed/initial export.

- [ ] Add failing tests for unchanged export, append-only export, dropped-event replacement, bounded storage, and writer failure cursor preservation.
- [ ] Run the focused trace tests and verify the current full-ring export behavior fails the new contract.
- [ ] Add event-level JSONL serialization and delta selection without a background worker.
- [ ] Update cycle-tail export to append when safe, replace when the ring dropped entries, and advance the cursor only after successful I/O.
- [ ] Run native presentation/trace tests and the full test suite.

### Task 6: Integrate sampled presentation state at the safe frame seam

**Files:**
- Modify: `src/compositor/server.rs`
- Modify: `src/compositor/mod.rs`
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/compositor/render.rs`
- Test: compositor state and native runtime model tests

**Interfaces:**
- `OwnCompositorServer::presentation_scene_sample(at: AnimationTime) -> PresentationSceneSample` returns a frame-local immutable sample.
- `ResolvedNativeFrameScene::from_server_at(server, at)` resolves canonical scene resources plus the sample without rebuilding `ActiveSceneView`.
- `PresentationAnimator` is updated only by canonical geometry mutation paths; frame sampling never writes canonical placement.

- [ ] Add failing integration tests asserting animation sample count changes while layout solve, configure, active-scene rebuild, and SHM copy counters remain unchanged.
- [ ] Run the focused tests to confirm no current frame-local sampling seam exists.
- [ ] Add visible-root transition creation to non-interactive geometry mutation batching; bypass it for active move/resize and disabled animations.
- [ ] Derive root/subsurface/SSD/policy-owned-popup frame-local transforms from one sample and reuse stable surface buffers.
- [ ] Pass the selected `PresentationTarget.presentation_time` through native frame resolution and preserve the existing no-target identity path.
- [ ] Integrate any future presentation-specific damage optimization with existing presented-scene/buffer-age history; the current closure uses the shared transform and scene-history authority and exposes the animation Direct Scanout rejection reason.
- [ ] Run focused native/compositor suites and inspect the diff for accidental canonical mutations.

### Task 7: Qualification and documentation closure

**Files:**
- Modify: `docs/superpowers/specs/2026-09-08-typhon-presentation-animation-foundation-design.md`
- Create: `docs/reports/2026-09-08-typhon-presentation-animation-foundation.md`

- [ ] Run `rtk git diff --check` and the full normal test workflow using the existing `target/` directory.
- [ ] Run the bounded benchmark/metrics fixtures for one, tens, and 100+ visible surfaces, no effects, active effects, reflow, and animation cadence.
- [ ] Record CPU frame preparation, scene/effect preparation, animation sampling, active/sample counts, damage area/rects, scene rebuilds, SHM copy evidence, Dwindle solves, configures, scanout blockers, GPU/KMS/submit/pageflip/deadline data separately.
- [ ] State hardware limitations explicitly if DRM/165 Hz qualification is unavailable.
- [ ] Self-review the design and report for stale assumptions, duplicated authorities, lifetime/timing ambiguity, placeholders, and scope creep.
