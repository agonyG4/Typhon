# Typhon Fullscreen / Restore Ghosting — Frame-Scene Authority Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (or superpowers:subagent-driven-development) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every composited framebuffer, its scene snapshot, and its scene-relative damage use one frozen resolved native frame scene, while keeping DirectPrimary content out of composited history and keeping fullscreen transitions geometrically coherent.

**Architecture:** Add `ResolvedNativeFrameScene<'a>` as the frame-scene authority. It borrows the normal server surface list and owns only the filtered fullscreen list when the existing visibility policy requires it; it derives decorations, popup/overlay classification, and a compact `NativeSceneSnapshot` from that same ordered list. CPU, GLES, current-scene damage, and the ready snapshot consume the same plan. `NativeSceneHistory` stores only confirmed composited snapshots and supports a real empty state; DirectPrimary uses the existing transaction/direct identity and invalidates composited history at the ownership boundary. Fullscreen/maximize/restore install the existing `ToplevelVisualGeometry` target before render resolution.

**Tech Stack:** Rust, Typhon compositor fixtures, native CPU/GLES output paths, EGL partial repaint, GBM/KMS transaction models, Cargo, and existing deterministic scene/framebuffer tests.

## Global Constraints

- Preserve all pre-existing tracked and untracked worktree changes; never run `git reset`, `git restore`, `git checkout --`, `git clean`, or `git stash`.
- Use `apply_patch` for source and documentation edits; reuse existing Cargo build caches.
- Follow TDD: every production behavior change has a focused test that was observed failing first.
- The invariant for every composited frame is `ResolvedNativeFrameScene == RenderedFrameScene == NativeFrameSceneSnapshot` by exact ordered surface and decoration identities.
- A DirectPrimary presentation is never represented as `NativeFrameSceneSnapshot::Composited`; it records direct transaction identity or invalidates/suspends composited history.
- Do not force full output repaint for ordinary fullscreen composition, resize, move, retry, or rejection; do not disable buffer age, triple buffering, render-ahead, KMS worker, or Direct Scanout globally.
- A full repair is allowed at first composited presentation or a genuine Direct-to-composited history discontinuity whose predecessor cannot be proven.
- Do not clone complete client pixel buffers into frame snapshots or rebuild decoration assets per frame; use the existing borrowed/filtered ownership and compact scene snapshots.
- Preserve the existing fullscreen overlay/layer policy and leave Eclipse Dock policy, Dock ordering, titlebar focus/hover behavior, idle CPU, and unrelated protocols untouched.
- Native qualification is reported only if a real Typhon DRM/KMS session can be run; otherwise the exact environmental blocker is recorded.

---

## File map and ownership

| File | Responsibility in this closure |
| --- | --- |
| `src/native_output/runtime/frame.rs` | Define/freeze `ResolvedNativeFrameScene`, expose plan-based CPU/GLES render requests, and retain renderer parity assertions. |
| `src/native_output/runtime/scene_history.rs` | Build snapshots only from a resolved plan; represent no composited scene; preserve token-ordered ready/submitted/presented ownership. |
| `src/native_output/runtime/presentation.rs` | Resolve a composited plan at the render boundary, feed damage/render/snapshot, and keep retries tied to the frozen frame identity. |
| `src/native_output/runtime/presentation_worker.rs` | Consume resolved scene snapshots for damage and composited ready frames; remove DirectPrimary fake-scene promotion. |
| `src/native_output/runtime/presentation_ready.rs` | Queue/promote only real composited snapshots and preserve direct transition settlement. |
| `src/native_output/runtime/cycle/pageflip.rs` | Promote composited history only for composited pageflips and invalidate it when DirectPrimary ownership ends. |
| `src/native_output/runtime/bootstrap.rs` | Start `NativeSceneHistory` empty rather than claiming fake frame zero. |
| `src/native_output/output/damage.rs` | Add no-predecessor/full-repair semantics and plan-snapshot damage helpers without re-querying server state. |
| `src/egl_renderer/damage.rs` | Ensure invalidated buffer-age history cannot be skipped by empty current damage. |
| `src/native_output/scanout/*.rs` | Thread the resolved plan through CPU/GLES render entry points without changing unrelated scanout policy. |
| `src/compositor/state/fullscreen.rs` | Separate fullscreen composited visibility from strict Direct Scanout eligibility. |
| `src/compositor/state/windows.rs` | Install and reconcile mode-transition visual geometry through existing `ToplevelVisualGeometry`. |
| `src/compositor/state/window_decoration.rs` | Keep SSD construction on the exact visual geometry and resolved surface slice. |
| `src/compositor/tests/fullscreen_frame_scene.rs` | Reproduce the pre-fix fullscreen surface/SSD identity mismatch with real decorated test windows. |
| `src/compositor/tests/windows.rs`, `src/compositor/tests/xwayland_resize_visual.rs`, `src/compositor/tests/windows_resize_liveness.rs` | Extend delayed-client geometry and XWayland coverage. |
| `src/native_output/tests/scene_history.rs` or existing runtime scene-history tests | Cover no bootstrap scene, Direct no-promotion, invalidation, token ordering, retry, and fallback. |
| `src/native_output/tests/output.rs`, `src/native_output/tests/output_retry.rs` | Add exact identity, layer/popup, partial/full framebuffer, stale-pixel, and buffer-age matrix tests. |
| `src/egl_renderer/damage_tests.rs` | Cover invalidated history plus empty current damage and age 1/2/3 planning. |
| `docs/superpowers/specs/2026-08-17-fullscreen-frame-scene-authority-design.md` | Design, findings, comparisons, rejected alternatives, and constraints. |
| `docs/superpowers/plans/2026-08-17-fullscreen-frame-scene-authority-closure.md` | This task-by-task execution plan. |
| `REPORT-2026-08-17-fullscreen-frame-scene-authority-closure.md` | Evidence-backed final report and qualification boundary. |

## Interfaces

The implementation may adjust module visibility and lifetimes, but the following contracts are fixed:

```rust
pub(crate) struct ResolvedNativeFrameScene<'a> {
    pub(crate) surfaces: Cow<'a, [RenderableSurface]>,
    pub(crate) decorations: Vec<DecorationRenderInstance>,
    pub(crate) popup_surface_ids: Vec<u32>,
    pub(crate) external_overlay_surface_ids: Vec<u32>,
    pub(crate) render_generation: u64,
    pub(crate) visibility: NativeFrameVisibilityPlan,
    pub(crate) snapshot: NativeSceneSnapshot,
}

impl<'a> ResolvedNativeFrameScene<'a> {
    pub(crate) fn from_server(server: &'a OwnCompositorServer) -> Self;
    pub(crate) fn surface_ids(&self) -> impl Iterator<Item = u32> + '_;
    pub(crate) fn decoration_identities(&self) -> impl Iterator<Item = (WindowId, u32)> + '_;
}

impl NativeFrameSceneSnapshot {
    pub(crate) fn from_resolved_frame_scene(
        frame_id: u64,
        resolved: &ResolvedNativeFrameScene<'_>,
        cursor_damage: NativeCursorDamageBounds,
    ) -> Self;
}
```

`NativeSceneHistory::presented_scene()` returns `Option<&NativeSceneSnapshot>` or an equivalent first-presentation signal. `replace_ready` accepts a snapshot already created from the exact resolved frame. Direct code has no API that accepts a server and creates a composited snapshot.

---

## Task 1: Prove the fullscreen surface and SSD identity mismatch

**Files:**
- Create: `src/compositor/tests/fullscreen_frame_scene.rs`
- Modify: `src/compositor/tests/mod.rs`
- Read: `src/compositor/tests/support/window_ops.rs`, `src/compositor/state/fullscreen.rs`, `src/native_output/runtime/scene_history.rs`

**Interfaces:**
- Consumes: Existing three-buffered-toplevel fixture, `OwnCompositorServer::native_frame_renderable_surfaces`, `NativeFrameSceneSnapshot::from_server`.
- Produces: A real decorated-window regression that fails until snapshot generation consumes the resolved native frame scene.

- [ ] **Step 1: Add the real compositor fixture test before production changes.**

```rust
#[test]
fn solitary_fullscreen_snapshot_matches_the_filtered_renderer_scene() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let (_client_state, surface_ids) = create_three_buffered_toplevels_then_toggle_mode(
        &socket_path,
        &commands,
        ServerCommand::ToggleFullscreenFocused,
        false,
    )
    .unwrap();
    let server = stop_controllable_test_server(commands, server_thread);

    let renderer_ids = server
        .native_frame_renderable_surfaces()
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    let snapshot = crate::native_output::NativeFrameSceneSnapshot::from_server(
        &server,
        1,
        Default::default(),
    );
    let snapshot_ids = snapshot
        .scene
        .surfaces
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();

    assert_eq!(renderer_ids.len(), 1, "fixture must activate solitary fullscreen");
    assert_eq!(renderer_ids, snapshot_ids);
    assert!(snapshot.scene.decorations.iter().all(|decoration| {
        decoration.root_surface_id() == renderer_ids[0]
    }));
    assert!(surface_ids.iter().take(2).any(|id| !renderer_ids.contains(id)));
}
```

Add the smallest public(crate) decoration identity accessor needed by the test; it must expose identity only, not rendering assets.

- [ ] **Step 2: Run the test and record the expected red failure.**

Run:

```bash
cargo test --locked solitary_fullscreen_snapshot_matches_the_filtered_renderer_scene -- --exact --nocapture
```

Expected pre-fix result: `renderer_ids` contains only the fullscreen owner, while `snapshot_ids` contains the owner plus at least one hidden rear root; the decoration assertion also observes a hidden rear SSD. This is the required failure, not a fixture setup error.

- [ ] **Step 3: Commit only the failing test and its test-module registration.**

```bash
git add src/compositor/tests/fullscreen_frame_scene.rs src/compositor/tests/mod.rs
git commit -m "test(render): reproduce fullscreen frame-scene mismatch"
```

---

## Task 2: Introduce the resolved native frame-scene authority

**Files:**
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/scene_history.rs`
- Modify: `src/native_output/runtime/mod.rs`
- Modify: `src/compositor/server.rs` only if a narrow forwarding method is required
- Test: `src/compositor/tests/fullscreen_frame_scene.rs`

**Interfaces:**
- Consumes: Existing native fullscreen filtering, decoration rendering, popup IDs, overlay IDs, render generation, and `NativeSceneSnapshot::from_surfaces`.
- Produces: `ResolvedNativeFrameScene::from_server`, compact ordered identity accessors, `NativeFrameSceneSnapshot::from_resolved_frame_scene`.

- [ ] **Step 1: Add the failing unit assertions for plan identity and snapshot identity.**

```rust
#[test]
fn resolved_plan_freezes_ordered_surfaces_and_decorations() {
    let plan = fixture_resolved_fullscreen_plan();
    let snapshot = NativeFrameSceneSnapshot::from_resolved_frame_scene(
        7,
        &plan,
        NativeCursorDamageBounds::default(),
    );
    assert_eq!(
        plan.surface_ids().collect::<Vec<_>>(),
        snapshot.scene.surfaces.iter().map(|surface| surface.surface_id).collect::<Vec<_>>()
    );
    assert_eq!(
        plan.decoration_identities().collect::<Vec<_>>(),
        snapshot.scene.decorations.iter().map(DecorationSceneSnapshot::identity).collect::<Vec<_>>()
    );
}
```

- [ ] **Step 2: Run the focused test and observe the missing plan API failure.**

Run:

```bash
cargo test --locked resolved_plan_freezes_ordered_surfaces_and_decorations -- --exact --nocapture
```

Expected result before implementation: compilation fails because `ResolvedNativeFrameScene` and `from_resolved_frame_scene` do not exist.

- [ ] **Step 3: Implement the minimal plan and snapshot conversion.**

Resolve in this order so all dependent data uses one surface slice:

```rust
let surfaces = server.native_frame_renderable_surfaces();
let decorations = server.native_decoration_render_instances_for_scale(surfaces.as_ref(), 1.0);
let popup_surface_ids = server.popup_surface_ids();
let external_overlay_surface_ids = server.external_overlay_surface_ids();
let snapshot_scene = NativeSceneSnapshot::from_surfaces(
    surfaces.as_ref(),
    decorations.iter().map(DecorationRenderInstance::scene_snapshot).collect(),
);
```

Use `Cow` so the normal path borrows `server.renderable_surfaces`; preserve the existing owned filtered list for a solitary fullscreen plan. Do not call `server.renderable_surfaces()` from the new snapshot conversion.

- [ ] **Step 4: Replace the failing fixture call with plan-derived snapshot construction and rerun.**

The test must pass with exact ordered surface IDs and exact `(WindowId, root_surface_id)` decoration identities. Keep the explicit rear-window/SSD negative assertions.

- [ ] **Step 5: Commit the authority boundary.**

```bash
git add src/native_output/runtime/frame.rs src/native_output/runtime/scene_history.rs src/native_output/runtime/mod.rs src/compositor/server.rs src/compositor/tests/fullscreen_frame_scene.rs
git commit -m "refactor(render): resolve native frame scene once"
```

---

## Task 3: Make CPU and GLES consume the same resolved scene

**Files:**
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/scanout/mod.rs`
- Modify: `src/native_output/scanout/gbm_cpu.rs`
- Modify: `src/native_output/scanout/dumb.rs`
- Modify: `src/native_output/scanout/egl_gbm.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm.rs`
- Modify: `src/egl_renderer.rs`
- Test: `src/native_output/tests/output.rs`, `src/native_output/tests/frame.rs`

**Interfaces:**
- Consumes: `&ResolvedNativeFrameScene`, `NativeInputState`, cursor mode, and output damage.
- Produces: CPU `NativeFrameRequest` and GLES `EglSceneDrawRequest` containing the exact same surfaces, decorations, popup IDs, overlays, and generation.

- [ ] **Step 1: Add parity tests that inspect both request paths.**

```rust
#[test]
fn cpu_and_gles_requests_share_the_resolved_scene_identity() {
    let plan = fixture_resolved_fullscreen_plan();
    let cpu = cpu_request_identity(&plan);
    let gles = gles_request_identity(&plan);
    assert_eq!(cpu.surface_ids, gles.surface_ids);
    assert_eq!(cpu.decoration_ids, gles.decoration_ids);
    assert_eq!(cpu.popup_surface_ids, gles.popup_surface_ids);
    assert_eq!(cpu.external_overlay_surface_ids, gles.external_overlay_surface_ids);
}
```

- [ ] **Step 2: Run the parity test and observe the pre-refactor independent-resolution failure.**

Run:

```bash
cargo test --locked cpu_and_gles_requests_share_the_resolved_scene_identity -- --exact --nocapture
```

Expected result: the request helpers are absent or the current entry points still query `server.native_frame_renderable_surfaces()` independently; the test must not be made green by changing expected identities.

- [ ] **Step 3: Change renderer entry points to consume the plan.**

`render_server_frame` and `egl_scene_draw_request` must no longer call any server scene-list, popup-list, overlay-list, or decoration-list resolver. They set renderer state from the supplied plan and only obtain mutable cursor pixels from server when the cursor mode requires it.

- [ ] **Step 4: Thread the plan through all CPU/GLES scanout wrappers.**

Update `paint_server_frame`, `render_to_slot`, and their backend-specific wrappers to pass the same plan. Preserve frame-copy and EGL partial repaint behavior. Do not create a second plan inside a backend.

- [ ] **Step 5: Add a debug/test identity assertion at the render boundary.**

Compare the renderer-consumed ordered surface IDs and decoration identities with `resolved.snapshot`. In tests use an assertion; in debug builds emit one bounded high-severity diagnostic and include the frame ID/generation/signature. Do not hash pixel buffers.

- [ ] **Step 6: Run focused CPU/GLES tests and commit.**

Run:

```bash
cargo test --locked native_frame_renderer -- --nocapture
cargo test --locked cpu_and_gles_requests_share_the_resolved_scene_identity -- --exact --nocapture
```

Then commit:

```bash
git add src/native_output/runtime/frame.rs src/native_output/scanout src/egl_renderer.rs src/native_output/tests/output.rs src/native_output/tests/frame.rs
git commit -m "refactor(render): share resolved scene with CPU and GLES"
```

---

## Task 4: Make current-scene damage and ready snapshots derive from the plan

**Files:**
- Modify: `src/native_output/runtime/presentation.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/scene_history.rs`
- Modify: `src/native_output/output/damage.rs`
- Test: `src/native_output/tests/output.rs`, `src/native_output/tests/output_retry.rs`

**Interfaces:**
- Consumes: The frozen plan snapshot and `NativeSceneHistory`’s confirmed composited predecessor.
- Produces: `native_output_damage_for_resolved_scene(previous, resolved, cursor)` and `NativeFrameSceneSnapshot` built before render from the exact plan.

- [ ] **Step 1: Add the failing damage identity test.**

```rust
#[test]
fn fullscreen_damage_compares_the_filtered_current_scene() {
    let normal = scene_with_rear_window_and_ssd();
    let fullscreen = scene_with_fullscreen_owner_only();
    let damage = native_output_damage_for_scene_snapshots(
        1280,
        800,
        &normal,
        &fullscreen,
        NativeCursorDamageBounds::default(),
    );
    assert!(damage_covers_old_rear_window_and_ssd(&damage));
}
```

The test must be extended to use the real plan snapshot once Task 2 lands; its purpose is to prevent raw server state from re-entering damage.

- [ ] **Step 2: Run the test and capture the pre-fix damage mismatch.**

Run:

```bash
cargo test --locked fullscreen_damage_compares_the_filtered_current_scene -- --exact --nocapture
```

Expected pre-fix failure: the current damage helper reconstructs the scene from `server.renderable_surfaces()` and cannot prove that the fullscreen framebuffer lacked the rear window/SSD.

- [ ] **Step 3: Replace server reconstruction with the plan snapshot.**

The render branch resolves the plan once before damage conversion. It builds the frame snapshot from that plan, computes damage against the confirmed presented composited scene, and passes the same plan into the renderer. `replace_ready_scene` accepts the already-frozen snapshot and never takes `&OwnCompositorServer`.

- [ ] **Step 4: Preserve retry and rejection semantics.**

When a frame is rejected, discard only its ready/submitted token-owned snapshot. The presented snapshot remains unchanged. A retry reuses the original snapshot/plan identity and does not reconstruct from the later mutable server state.

- [ ] **Step 5: Run scene, decoration, retry, and damage tests.**

Run:

```bash
cargo test --locked fullscreen -- --nocapture
cargo test --locked scene_history -- --nocapture
cargo test --locked output_retry -- --nocapture
cargo test --locked native_output_damage_for_scene_snapshots -- --nocapture
```

- [ ] **Step 6: Commit snapshot/damage authority.**

```bash
git add src/native_output/runtime/presentation.rs src/native_output/runtime/presentation_worker.rs src/native_output/runtime/scene_history.rs src/native_output/output/damage.rs src/native_output/tests/output.rs src/native_output/tests/output_retry.rs
git commit -m "fix(presentation): snapshot exact composited frame scene"
```

---

## Task 5: Close Direct Scanout ownership and bootstrap history

**Files:**
- Modify: `src/native_output/runtime/bootstrap.rs`
- Modify: `src/native_output/runtime/scene_history.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/presentation_ready.rs`
- Modify: `src/native_output/runtime/cycle/pageflip.rs`
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Modify: `src/native_output/scanout/direct_transition.rs`
- Test: `src/native_output/runtime/scene_history.rs`, `src/native_output/scanout/atomic_direct_tests.rs`, `src/native_output/tests/output_retry.rs`

**Interfaces:**
- Consumes: Direct transaction identity and existing `invalidate_presented_damage_history`, `mark_composited_submission`, and `complete_composited_transition` paths.
- Produces: no fake composited snapshot for DirectPrimary; an explicit empty/invalidate composited-history state at bootstrap and Direct → composition.

- [ ] **Step 1: Add red history tests.**

```rust
#[test]
fn bootstrap_has_no_confirmed_composited_scene() {
    let history = NativeSceneHistory::default();
    assert!(history.presented_scene().is_none());
}

#[test]
fn direct_primary_does_not_promote_a_composited_scene() {
    let mut history = NativeSceneHistory::default();
    history.record_direct_primary(direct_identity_fixture());
    assert!(history.presented_scene().is_none());
    assert!(history.ready_scene().is_none());
}

#[test]
fn direct_to_composited_invalidates_the_composited_predecessor() {
    let mut history = history_with_presented_composited_frame(4);
    history.invalidate_composited_history();
    assert!(history.presented_scene().is_none());
    assert!(history.requires_authoritative_repair());
}
```

- [ ] **Step 2: Run the history tests and observe the fake-frame/API failure.**

Run:

```bash
cargo test --locked bootstrap_has_no_confirmed_composited_scene direct_primary_does_not_promote_a_composited_scene direct_to_composited_invalidates_the_composited_predecessor -- --nocapture
```

Expected pre-fix result: bootstrap requires a fake snapshot and `replace_ready_scene` has no direct/composited type boundary.

- [ ] **Step 3: Make `NativeSceneHistory` honestly optional.**

Remove the fake bootstrap constructor call. Keep `presented`, `ready`, and `submitted` token ownership. Make first-presentation damage/full-repair APIs return an explicit full-repair signal rather than panic. Keep out-of-order pageflip protection.

- [ ] **Step 4: Remove DirectPrimary calls to composited snapshot construction.**

Delete the `replace_ready_scene` call from `finish_direct_worker_queued` and any equivalent DirectPrimary path. Direct diagnostics retain transaction ID, token, surface ID, client buffer identity/content epoch, framebuffer ID, and candidate key. A rejected Direct admission does not mutate composited history.

- [ ] **Step 5: Invalidate both histories at Direct → composition settlement.**

At the existing successful direct release/composited transition boundary, invalidate `NativeSceneHistory` and the EGL partial repaint journal. Keep `inhibit_until_composited_present` until a real composited frame is confirmed.

- [ ] **Step 6: Add the empty-damage invalidation regression.**

Extend `src/egl_renderer/damage_tests.rs` so `invalidate()` followed by `plan(OutputDamage::Empty, BufferAge::Value(1..=3))` returns a repair plan and never `RepaintMode::Skip`. Add the same assertion through native scene damage when the presented scene is `None`.

- [ ] **Step 7: Run direct and history tests and commit.**

Run:

```bash
cargo test --locked scene_history -- --nocapture
cargo test --locked atomic_direct -- --nocapture
cargo test --locked damage -- --nocapture
cargo test --locked direct -- --nocapture
```

Commit:

```bash
git add src/native_output/runtime/bootstrap.rs src/native_output/runtime/scene_history.rs src/native_output/runtime/presentation_worker.rs src/native_output/runtime/presentation_ready.rs src/native_output/runtime/cycle/pageflip.rs src/native_output/scanout/atomic_egl_gbm/direct.rs src/native_output/scanout/direct_transition.rs src/egl_renderer/damage.rs src/egl_renderer/damage_tests.rs src/native_output/tests/output_retry.rs
git commit -m "fix(scanout): separate direct and composited scene history"
```

---

## Task 6: Separate fullscreen composition visibility from Direct Scanout admission

**Files:**
- Modify: `src/compositor/state/fullscreen.rs`
- Modify: `src/compositor/server.rs` only for narrow forwarding of the visibility plan
- Modify: `src/native_output/runtime/frame.rs`
- Modify: `src/native_output/runtime/presentation_transactions.rs`
- Test: `src/compositor/tests/direct_scanout.rs`, `src/compositor/tests/fullscreen_frame_scene.rs`

**Interfaces:**
- Consumes: Current fullscreen owner, allowed overlay trees, popup/layer policy, visual geometry, and strict Direct Scanout candidate checks.
- Produces: `NativeFrameVisibilityPlan` for compositor rendering and a separate `DirectScanoutSceneCandidate` admission result.

- [ ] **Step 1: Add the rejected-direct-but-composed visibility regression.**

Use the existing fullscreen fixture with a non-dmabuf or popup blocker and assert that the composited plan still follows the intended fullscreen visibility policy, while Direct Scanout returns its explicit rejection. Renderer IDs, snapshot IDs, and plan IDs must remain equal.

- [ ] **Step 2: Run the test against current coupled policy and record its red failure.**

Run:

```bash
cargo test --locked rejected_direct_scanout_keeps_composited_fullscreen_visibility -- --exact --nocapture
```

Expected pre-fix failure: `fullscreen_render_plan_metrics.solitary_tree_active` is directly derived from `direct_scanout_scene_candidate().is_ok()`, so failed Direct admission changes composited culling.

- [ ] **Step 3: Add the explicit visibility plan.**

Compute fullscreen composited culling from fullscreen presentation state and allowed overlay policy. Feed Direct Scanout eligibility the same owner/geometry facts plus dmabuf, format/modifier, buffer-size, device, sync, cursor, and KMS validation requirements. Do not change layer policy.

- [ ] **Step 4: Verify popup and overlay parity.**

The layer/overlay test asserts `plan.surface_ids == renderer.surface_ids == snapshot.surface_ids`; the popup test asserts all three observe the same policy result. Run and commit:

```bash
cargo test --locked direct_scanout -- --nocapture
cargo test --locked fullscreen_frame_scene -- --nocapture
git add src/compositor/state/fullscreen.rs src/compositor/server.rs src/native_output/runtime/frame.rs src/native_output/runtime/presentation_transactions.rs src/compositor/tests/direct_scanout.rs src/compositor/tests/fullscreen_frame_scene.rs
git commit -m "refactor(render): separate fullscreen visibility from scanout eligibility"
```

---

## Task 7: Install coherent fullscreen/maximize/restore visual geometry

**Files:**
- Modify: `src/compositor/state/windows.rs`
- Modify: `src/compositor/state/fullscreen.rs` only where target geometry is selected
- Modify: `src/compositor/state/window_decoration.rs` only if an existing visual-geometry call site is bypassed
- Modify: `src/compositor/state/window_resize.rs` only for reconciliation with the existing preview path
- Test: `src/compositor/tests/windows.rs`, `src/compositor/tests/windows_resize_liveness.rs`, `src/compositor/tests/xwayland_resize_visual.rs`

**Interfaces:**
- Consumes: `window_geometry_for_surface_mode`, saved restore geometry, `ToplevelVisualGeometry`, `current_visual_root_window_geometry`, configure ACK/commit reconciliation.
- Produces: one target `WindowGeometry` for mode-transition render, SSD, hit testing, damage, and Direct candidate checks until convergence.

- [ ] **Step 1: Add delayed-client red tests.**

For enter fullscreen, leave the committed floating buffer at `900x700` while the configure target is `1920x1080`; assert the visual target is one fullscreen geometry. For restore, leave the committed buffer at `1920x1080` while the target is `(x, y, 900, 700)`; assert root visual, SSD width, button anchor, hit-test titlebar, and damage bounds all use `(x, y, 900, 700)`.

```rust
#[test]
fn delayed_restore_commit_uses_saved_visual_geometry_for_ssd_and_hit_test() {
    let fixture = delayed_fullscreen_restore_fixture();
    assert_eq!(fixture.visual_geometry, fixture.restore_geometry);
    assert_eq!(fixture.ssd_client_width, 900);
    assert_eq!(fixture.hit_test_titlebar, fixture.rendered_titlebar);
}
```

- [ ] **Step 2: Run the tests and observe the hybrid-geometry failure.**

Run:

```bash
cargo test --locked delayed_restore_commit_uses_saved_visual_geometry_for_ssd_and_hit_test -- --exact --nocapture
```

Expected pre-fix failure: mode/placement has changed while the committed client geometry remains the previous mode, allowing a mixed position/size visual.

- [ ] **Step 3: Install target visual geometry before mode configure/placement publication.**

On enter, install the fullscreen/maximized target generated by `window_geometry_for_surface_mode`. On restore, install the saved restore geometry. Keep it in `toplevel_visual_geometries`, use `current_visual_root_window_geometry` everywhere already taught to use it, and retire it only through the existing configure/commit reconciliation path.

- [ ] **Step 4: Add 100-cycle delayed convergence coverage.**

Run floating → fullscreen → floating 100 times with immediate, one-cycle-late, and three-cycle-late commit schedules. Assert original placement/size exactly and reject any frame containing mixed mode geometry.

- [ ] **Step 5: Run XWayland and decoration suites and commit.**

Run:

```bash
cargo test --locked windows -- --nocapture
cargo test --locked windows_resize_liveness -- --nocapture
cargo test --locked xwayland_resize_visual -- --nocapture
```

Commit:

```bash
git add src/compositor/state/windows.rs src/compositor/state/fullscreen.rs src/compositor/state/window_decoration.rs src/compositor/state/window_resize.rs src/compositor/tests/windows.rs src/compositor/tests/windows_resize_liveness.rs src/compositor/tests/xwayland_resize_visual.rs
git commit -m "fix(windowing): preserve visual geometry across fullscreen transitions"
```

---

## Task 8: Add deterministic framebuffer, buffer-age, retry, layer, popup, and XWayland regressions

**Files:**
- Modify: `src/native_output/tests/output.rs`
- Modify: `src/native_output/tests/output_retry.rs`
- Modify: `src/native_output/tests/mod.rs`
- Modify: `src/egl_renderer/damage_tests.rs`
- Modify: `src/compositor/tests/fullscreen_frame_scene.rs`
- Modify: `src/compositor/tests/direct_scanout.rs`
- Modify: `src/compositor/tests/xwayland_resize_visual.rs`

**Interfaces:**
- Consumes: Plan-derived scene snapshots, `NativeSceneHistory`, `native_output_damage_for_scene_snapshots`, CPU/GLES request identity, and repaint planner ages.
- Produces: Pixel-reference evidence across normal/fullscreen/restore, direct OFF, direct enabled model, rejected direct fallback, buffer ages 1/2/3, overlays, popups, XWayland, and retry/rejection.

- [ ] **Step 1: Add the direct-disabled scene identity test.**

Sequence the deterministic scenes `A -> F -> R` with Direct Scanout disabled. For every frame assert exact renderer/snapshot identity and compare the partial/reused framebuffer with a fresh full-reference render.

- [ ] **Step 2: Add direct-to-composited ownership tests.**

Use the existing direct transaction model for `composited A -> Direct D -> composited R`. Assert D has no composited snapshot, the composited history is invalidated, and R repairs to the full reference. Test Direct rejection leaves the previous composited history unchanged until the real fallback frame is presented.

- [ ] **Step 3: Add the F1…F20 stale-pixel matrix.**

Render two decorated windows, enter fullscreen, render at least 20 fullscreen content generations, restore, rotate three slots, and sample old rear titlebar, traffic-light, old floating owner titlebar, fullscreen content, restored buttons, and wallpaper positions. Compare every age 1/2/3 partial result pixel-for-pixel to the full-reference exact plan.

- [ ] **Step 4: Add render-ahead/reject/retry and out-of-order pageflip tests.**

Cover `A presented -> F1 rejected -> F2 presented -> R rejected -> R retry presented`. Advance mutable logical state C between resolution and pageflip. A pageflip for A promotes A’s frozen snapshot, not B/C/current server state.

- [ ] **Step 5: Add layer/overlay/popup and XWayland cases.**

Assert allowed overlay trees remain in plan/render/snapshot together; assert popup policy is one shared result; cover managed decorated XWayland fullscreen/restore and `_NET_FRAME_EXTENTS`/SSD geometry coherence.

- [ ] **Step 6: Run focused matrix and commit.**

```bash
cargo test --locked native_output::tests::output -- --nocapture
cargo test --locked native_output::tests::output_retry -- --nocapture
cargo test --locked egl_renderer::damage_tests -- --nocapture
cargo test --locked fullscreen_frame_scene -- --nocapture
cargo test --locked xwayland_resize_visual -- --nocapture
```

```bash
git add src/native_output/tests/output.rs src/native_output/tests/output_retry.rs src/native_output/tests/mod.rs src/egl_renderer/damage_tests.rs src/compositor/tests/fullscreen_frame_scene.rs src/compositor/tests/direct_scanout.rs src/compositor/tests/xwayland_resize_visual.rs
git commit -m "test(render): cover fullscreen restore buffer-age repair"
```

---

## Task 9: Add bounded presentation diagnostics and self-review evidence

**Files:**
- Modify: `src/native_output/runtime/presentation.rs`
- Modify: `src/native_output/runtime/presentation_worker.rs`
- Modify: `src/native_output/runtime/metrics.rs`
- Modify: `src/native_output/presentation/trace.rs` or the existing trace owner discovered by graph search
- Modify: `src/native_output/scanout/atomic_egl_gbm/direct.rs`
- Test: existing presentation trace/model tests

**Interfaces:**
- Consumes: `ResolvedNativeFrameScene` signatures, frame ID/generation, path, slot/age, transaction/submission/pageflip tokens, and direct candidate identity.
- Produces: bounded debug/test trace fields proving `resolved == rendered == snapshot` for composited frames and direct identity without a composited signature.

- [ ] **Step 1: Add the diagnostic assertion test.**

Feed a composited trace three equal signatures and assert no high-severity mismatch event; feed a deliberately different snapshot signature and assert one bounded invariant event. Feed a Direct trace and assert `scene_signature` is absent while direct identity fields are present.

- [ ] **Step 2: Implement the bounded trace fields.**

Record exactly the fields required by the design and user acceptance matrix; keep tracing disabled by default and avoid complete pixel hashes.

- [ ] **Step 3: Run trace tests and inspect all independent scene resolution call sites.**

```bash
cargo test --locked presentation_trace -- --nocapture
rg -n "renderable_surfaces\(|native_frame_renderable_surfaces\(|native_decoration_render_instances|popup_surface_ids\(|external_overlay_surface_ids\(" src/native_output src/egl_renderer.rs
```

Any remaining composited renderer/snapshot/damage call site must either consume `ResolvedNativeFrameScene` or be an explicitly documented non-frame policy query.

---

## Task 10: Verify, qualify, and write the final report

**Files:**
- Create: `REPORT-2026-08-17-fullscreen-frame-scene-authority-closure.md`
- Read: all task-owned diffs and the final dirty-state baseline

**Interfaces:**
- Consumes: Test output, source evidence, KWin/Hyprland comparison notes, native availability, and commit history.
- Produces: A complete English report with no overclaim about generic whole-frame rollback.

- [ ] **Step 1: Capture fresh focused verification.**

Run:

```bash
cargo test --locked scene_history -- --nocapture
cargo test --locked output_retry -- --nocapture
cargo test --locked fullscreen_frame_scene -- --nocapture
cargo test --locked windows_resize_liveness -- --nocapture
cargo test --locked xwayland_resize_visual -- --nocapture
cargo test --locked direct_scanout -- --nocapture
cargo test --locked damage -- --nocapture
```

Record exit codes and exact failure counts before making any success statement.

- [ ] **Step 2: Capture fresh global verification.**

Run:

```bash
cargo fmt --check
cargo check --locked --all-targets
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
bash bin/check-source-layout
git diff --check
```

If a baseline command already failed before task-owned changes, record the baseline output and prove the final run adds no new failure. Do not relabel new failures as baseline failures.

- [ ] **Step 3: Attempt native qualification without altering scope.**

Check for a usable DRM/TTY session and the current Direct Scanout environment switch. If unavailable, record Direct Scanout OFF/ON as unrun with the exact blocker. If available, run the MacTahoe-themed overlap/video/fullscreen/restore loop at least 30 times with Kitty, a video-playing browser, a GTK application, and XWayland, and capture the bounded trace once. Do not claim native qualification when unavailable.

- [ ] **Step 4: Perform the final self-review checklist.**

Answer in the report with source/test evidence:

```text
all composited renderers consume one plan
snapshot never reconstructs a rendered frame from mutable server state
hidden fullscreen SSD cannot enter the snapshot
DirectPrimary never promotes a fake composited scene
Direct -> composition invalidates both histories and cannot skip on empty damage
bootstrap does not claim an unrendered frame
fullscreen visibility is separate from Direct admission
visual, SSD, hit-test, and damage geometry agree during delayed restore
ages 1/2/3 match clean framebuffer references
CPU and GLES consume the same identities
XWayland/layer/popup cases remain coherent
no global buffer-age/triple/KMS-worker/Direct disable was added
unrelated dirty files remain unchanged
whole-frame rollback is reported separately unless KMS evidence closes it
```

- [ ] **Step 5: Write the report and record the final status.**

The report must include baseline HEAD, original dirty state, native reproduction, confirmed mismatch, failing test, Direct findings, bootstrap, geometry, KWin/Hyprland comparison, final architecture, damage/buffer-age results, CPU/GLES, XWayland/layer/popup, native qualification status, whole-frame rollback status, commands/results, task commits, blockers, and:

```bash
git status --short
```

```bash
git add REPORT-2026-08-17-fullscreen-frame-scene-authority-closure.md
git commit -m "docs: report fullscreen frame-scene authority closure"
```

