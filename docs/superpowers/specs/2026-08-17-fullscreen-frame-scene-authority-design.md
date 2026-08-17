# Typhon Fullscreen / Restore Ghosting — Frame-Scene Authority Design

Date: 2026-08-17  
Baseline inspected: `abe4a9421ae372d423f359b8994a85aa85076a94` with a pre-existing dirty worktree

## Scope and status vocabulary

This design closes the native rendering-correctness boundary between resolved compositor visibility, rendered pixels, and presentation history. It does not change Eclipse Dock fullscreen policy, Dock ordering, titlebar focus/hover behavior, idle CPU policy, or unrelated Wayland protocols.

Each finding is labelled `CONFIRMED`, `STRONG HYPOTHESIS`, or `UNPROVEN`.

## User reproduction

The reproduced native sequence is:

```text
normal desktop -> enter fullscreen -> leave fullscreen
```

The observed result is persistent and popping ghosting: old decorations/titlebars, stale borders, and apparently complete old frames can resurface after restore. A separate report also describes whole-frame rollback during ordinary video playback. The fullscreen/restore failure is in scope here; the generic rollback remains a separate qualification boundary unless token, transaction, framebuffer, and kernel pageflip evidence proves the same cause.

## Confirmed current defects

### Rendered scene and snapshot diverge — CONFIRMED

`NativeFrameRenderer::render_server_frame` and `NativeFrameRenderer::egl_scene_draw_request` consume `OwnCompositorServer::native_frame_renderable_surfaces()`. That method applies the active fullscreen visibility plan and can return the fullscreen owner tree plus allowed overlay trees only.

`NativeFrameSceneSnapshot::from_server`, `native_decoration_scene`, and `native_scene_damage_for_server` instead consume `server.renderable_surfaces()` and derive decorations from that raw list. Therefore a solitary fullscreen framebuffer can contain only the owner while the recorded current scene contains rear windows, their SSD, wallpaper-relative content, or other culled objects. This is a direct violation of the presentation-history contract and is the source-level explanation for the fullscreen/restore stale-pixel path.

### Direct Scanout promotes a fake composited scene — CONFIRMED

The worker DirectPrimary path calls `replace_ready_scene`, which currently calls `NativeFrameSceneSnapshot::from_server`. A DirectPrimary transaction presents a client framebuffer on the KMS primary plane; it never renders the compositor scene. Promoting the raw logical desktop as a composited snapshot is semantically invalid. The Direct path must record typed direct primary identity or leave composited scene history invalidated/suspended.

### Bootstrap claims an unpresented scene — CONFIRMED

Bootstrap constructs `NativeSceneHistory::new(NativeFrameSceneSnapshot::from_server(..., frame_id = 0, ...))` before a corresponding native composited presentation is confirmed. The history already stores `Option`, so the correct initial state is no confirmed composited scene. The first real composited frame must therefore take the authoritative first-presentation repair path.

### Direct/composited buffer history is a discontinuity — CONFIRMED

The existing explicit scanout code invalidates the EGL partial-repaint history when DirectPrimary ownership returns to composition and has `inhibit_until_composited_present` settlement. That invalidation is correct but does not currently invalidate the separate native scene-history claim, and `PartialRepaintPlanner::plan` can still observe empty current damage before invalidated-history handling. The closure must make both histories agree that no composited predecessor is available.

### Fullscreen restore geometry can be hybrid — STRONG HYPOTHESIS

`set_root_window_mode` and `restore_floating_root_window` change mode and placement and send a configure while the committed XDG surface size may still describe the previous mode. The existing `current_visual_root_window_geometry` and `ToplevelVisualGeometry` machinery is the correct home for an explicit transition target. The required delayed-client tests will establish the exact current failure and guard against a mixed placement/size frame.

### Whole-frame rollback — UNPROVEN

The scene mismatch explains fullscreen/restore ghosting, including stale regions exposed by rotating buffers. It does not prove that a complete old framebuffer can be presented during ordinary playback. If that persists after this closure, the bounded KMS trace must be used to compare frame IDs, resolved signatures, buffer slots, GBM framebuffer IDs, transaction/submission tokens, kernel pageflip tokens/sequences, and the actually presented framebuffer.

## Current ownership diagram

```text
OwnCompositorServer mutable state
        |
        +--> renderable_surfaces --------------------------+
        |                                                   |
        +--> native_frame_renderable_surfaces()             |
        |        (fullscreen filtering)                     |
        |                                                   |
        +--> decoration instances / popup IDs / overlays    |
                                                            |
NativeFrameRenderer::render_server_frame ------------------+--> CPU pixels
NativeFrameRenderer::egl_scene_draw_request ---------------+--> GLES request
NativeSceneDamageForServer --------------------------------+--> output damage
NativeFrameSceneSnapshot::from_server ----------------------+--> presented history

DirectPrimary KMS transaction ------------------------------+--> client framebuffer
                                                           |
                                                           +--> currently fake composited snapshot
```

The duplicated arrows are the defect. A frame needs one resolved authority and one frozen identity.

## Chosen Typhon architecture

### ResolvedNativeFrameScene

Introduce a one-per-composited-frame plan, conceptually:

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
```

The exact module placement may follow the existing runtime ownership, but the plan has these properties:

* `surfaces` is borrowed for the normal scene and uses the existing filtered owned list only when fullscreen resolution requires it; it does not clone client pixel buffers for normal frames.
* `decorations` are derived from the exact resolved surface slice and exact visual geometry used by the renderer.
* popup and external-overlay classification is frozen with the surface ordering.
* `snapshot` is built from the plan, not from mutable server state after rendering.
* `visibility` separates fullscreen composited visibility from Direct Scanout admission. Direct eligibility may use the visibility facts plus stricter buffer/KMS/sync requirements; it does not decide whether a non-direct fullscreen frame is culled.

The native CPU and GLES entry points consume a plan view. Damage consumes the plan snapshot. The plan is resolved before output damage is converted and remains the identity of the frame through render, retry, and submission.

### Snapshot boundary

Replace the rendered-frame use of `NativeFrameSceneSnapshot::from_server` with:

```rust
NativeFrameSceneSnapshot::from_resolved_frame_scene(
    frame_id,
    render_generation,
    &resolved_scene,
    cursor_damage,
)
```

The plan and snapshot are frozen before a later logical state can be observed. Render-ahead and rejected-frame retries retain the snapshot produced by the original frame; they do not re-query current server state.

### Damage authority

The current scene used for damage is the plan's snapshot. The previous scene is the confirmed composited snapshot in `NativeSceneHistory`, or no scene. No confirmed scene means a full authoritative repair requirement for the first composited presentation. Ordinary fullscreen composition, movement, resize, and retry remain snapshot-relative partial damage decisions.

The existing cursor ownership remains separate. Software cursor content continues to contribute through `NativeCursorDamageBounds`; hardware cursor plane state is not inserted into the window scene.

### DirectPrimary ownership

`NativeSceneHistory` remains a composited-scene history. A DirectPrimary transaction does not call `replace_ready_scene` and never queues a `NativeFrameSceneSnapshot`. Direct identity remains in the existing transaction/direct ownership model: transaction ID, pageflip token, surface ID, client buffer identity/content epoch, framebuffer ID, and candidate key.

The Direct → Composited boundary performs both actions when ownership changes:

1. invalidate/suspend the composited scene history, including ready/submitted composited snapshots that cannot describe the DirectPrimary contents;
2. invalidate the compositor swapchain partial-repaint journal using the existing scanout transition mechanism.

The first returned composited frame resolves a real scene, derives a real snapshot, and repairs from an unknown predecessor. This full repair is justified only by the ownership discontinuity.

A rejected Direct attempt does not mutate composited scene history. A fallback composited frame resolves, renders, and records its own plan snapshot. A Direct frame's diagnostics use `render path = DIRECT` and direct identity fields; no composited signature is fabricated.

### Visual geometry transition

Fullscreen, maximize, and restore install a target `ToplevelVisualGeometry` through the existing visual-geometry machinery before the next frame is resolved. The target remains authoritative until the configure/commit reconciliation path proves convergence, then the existing visual override is retired.

The same visual geometry drives:

* root surface render assignment;
* SSD layout and decoration button anchoring;
* decoration hit testing;
* surface/decorative damage bounds;
* direct candidate geometry checks.

No new geometry cache is introduced. A transition frame therefore has one coherent `(x, y, width, height)` and cannot combine floating placement with fullscreen committed dimensions or the reverse.

## KWin comparison

The relevant KWin design lessons are ownership and invalidation, not a literal port:

* `WindowItem` owns the visual tree around a window, including surface/decorative/shadow visual children, so render collection follows active scene items rather than a global decoration pass.
* `Item` geometry changes repaint old and new visual bounds, preserving a coherent visual object during movement and resize.
* `WorkspaceScene`/render-view collection derives damage from items valid for the active view.
* DRM EGL layer scanout explicitly forgets compositor-surface damage journals when a scanout buffer bypasses the compositor swapchain.
* the damage journal is attached to the actual rendered output layer, not to arbitrary logical workspace state.

Typhon already has `WindowVisualGroup`, `ToplevelVisualGeometry`, and output-owned partial repaint state. This design connects those existing pieces through one frame plan.

## Hyprland comparison

The relevant Hyprland lessons are separation of visibility and repair:

* fullscreen rendering uses a dedicated fullscreen render path that selects the windows actually drawn for the output;
* the output damage ring accumulates actual output damage and buffer-age repair independently;
* direct scanout is an output ownership decision, not a reason to describe a raw workspace list as compositor pixels;
* fullscreen geometry is applied as a coherent monitor/window geometry transition.

Typhon will keep its own Rust/KMS/GBM ownership model and existing overlay policy. It adopts the separation: resolved visibility determines the composited frame, while buffer-age and scene history repair the output that was actually rendered.

## Rejected alternatives

1. **Change only `renderable_surfaces()` to `native_frame_renderable_surfaces()`.** Rejected because renderer, decorations, popup classification, damage, and snapshot timing would still be independently resolved.
2. **Record the raw server scene and force a full repaint on fullscreen.** Rejected because it keeps presentation history false and hides the bug with an unrelated performance regression.
3. **Disable buffer age, triple buffering, render-ahead, KMS worker, or Direct Scanout.** Rejected because these are valid mechanisms and the defect is ownership/identity.
4. **Keep a fake frame-zero snapshot.** Rejected because no backend proof establishes that the initial framebuffer contained that scene.
5. **Make DirectPrimary look like a composited snapshot.** Rejected because the primary-plane content types are different and their histories have different repair semantics.
6. **Add an application-specific fullscreen exception.** Rejected because this is compositor scene ownership.

## Test architecture

The tests proceed red/green and remain deterministic:

* a pre-fix fullscreen identity test creates two decorated scene objects, resolves a solitary fullscreen owner plan, and proves the old snapshot contained a culled rear window/SSD;
* the fixed test compares exact ordered surface IDs and decoration identities from the renderer plan and snapshot;
* layer/overlay and popup fixtures assert all consumers see the same visibility plan;
* CPU and GLES request tests consume the same plan object;
* scene-history tests cover no bootstrap scene, DirectPrimary no-promotion, rejected Direct fallback, out-of-order pageflips, and Direct → composition invalidation;
* framebuffer fixtures render normal A, fullscreen F1…F20, and restore R1…R3 through reused slots and compare partial output to a fresh full-reference render at buffer ages 1, 2, and 3;
* delayed-client tests cover target fullscreen and target restore visual geometry before configure/commit convergence, including SSD width and hit-test agreement;
* repeated mode tests run at least 100 transitions with immediate, one-cycle-late, and multi-cycle-late commits;
* existing XWayland, CSD, layer, popup, WindowVisual, SSD, and Direct Scanout model suites are extended only where the shared invariant is missing.

Every composited test can assert:

```text
ordered renderer surface identities == ordered snapshot surface identities
renderer decoration identities == snapshot decoration identities
resolved_scene_signature == rendered_scene_signature == snapshot_scene_signature
```

Diagnostics are bounded and debug/test gated. They record frame ID, render generation, scene/signature hashes, path, slot, age, transaction/submission/pageflip tokens, and direct identity without manufacturing a Direct composited signature.

## Performance and safety constraints

The implementation preserves normal partial repaint and buffer age. It does not globally disable buffer age, triple buffering, render-ahead, KMS worker, or Direct Scanout; it does not wait synchronously for pageflip; it does not copy complete pixel buffers into snapshots or rebuild decoration assets unnecessarily. A full repair is emitted only for first composited presentation or a genuine Direct/composited history boundary whose predecessor cannot be proven.

The original dirty worktree remains the implementation baseline. Only task-owned files are edited, and unrelated dirty files are not staged or rewritten.

