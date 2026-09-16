# Diagnostic Framebuffer Safety Design

## Goal

Make the replay/framebuffer by partial/full diagnostic matrix safe to execute with ordinary presentation damage unchanged. Framebuffer capture may enlarge temporary scene reconstruction, but pixels outside presentation repair must retain the framebuffer contents that existed before diagnostic scene work began.

## Root cause

The current executor derives `scene_work_rects` by adding selected `SceneCapture` domains to `repaint_rects` when framebuffer capture is enabled. It clears and replays those larger regions, while `Composite` and `OutputPostProcess` remain clipped to their original effect demand. The extra regions therefore receive scene reconstruction without a complete final effect result. The same geometry is not passed to `plan_effect_surface_consumers`, so a surface drawn during final scene replay can be synchronized too late or not at all.

## Design

`EffectDebugConfig` remains process-scoped at the production boundary through its existing `OnceLock`. The public production wrappers obtain one immutable config value and delegate to explicit-config entry points. The explicit entry points are available inside the crate for deterministic tests. All config-dependent executor helpers, direct capture checks, trace summaries, scene-work derivation, and Kawase-demand planning use the value passed through that call chain.

The executor and consumer planner share one pure scene-work helper. Given repaint rectangles, the compiled graph, the selected passes, output size, lifecycle state, and explicit debug config, it returns disjoint `scene_work_rects` plus `extra_scene_work`, where `extra_scene_work` is the exact set difference from the presentation repaint rectangles. Both command-range consumer planning and every executor scene replay use the returned `scene_work_rects`.

For ordinary framebuffer diagnostic execution with non-empty `extra_scene_work`, the executor checks out one named preservation texture for the current graph execution. It captures the current active output framebuffer into that texture before destructive scene-work clearing. The texture is owned by the current execution, is fully overwritten before any read, and is released at the end of that execution; its pooled storage has no temporal meaning. After effect passes and final scene replay complete, the executor restores only `extra_scene_work` from the preservation texture, then performs ordinary presentation overlays over `repaint_rects`. Error cleanup also releases the temporary ownership and restores preserved pixels when possible. Pixels inside `repaint_rects` are never restored.

The preservation transfer uses explicit framebuffer-origin mapping and nearest filtering. It does not use `glFlush` or `glFinish`, and it does not broaden presentation damage or final visible-output clipping. Existing capture semantics, checkpoint ordering, raster-aware coverage, bounded-region handling, and full-Kawase internal expansion remain unchanged.

## Test design

The existing serialized surfaceless GLES3 harness will host deterministic pixel tests. A non-uniform uploaded background texture and a translucent target surface are rendered through a blur effect. The first frame is a full repaint and its framebuffer is saved as the prior valid frame. The second frame uses a small repair while the selected framebuffer capture domain is substantially larger. A separate full-reference render of frame two supplies the expected pixels inside the repair.

The matrix test runs all four explicit configurations in one process:

* replay + partial
* replay + full
* framebuffer + partial
* framebuffer + full

For each configuration it compares pixels outside repair with the saved pre-frame framebuffer and pixels inside repair with the full-reference frame, allowing only the existing GLES readback tolerance. A focused RED test preserves the current destructive behavior until the restoration implementation is present. Additional unit regressions verify exact shared scene-work geometry, consumer coverage for an upper surface and capture-source surfaces below the anchor, capture/replay ordering, stacked backdrop ordering, explicit configuration selection, and full-Kawase planning.

## Scope boundaries

This correction does not add `PersistentBackdropCache`, does not continue into production backdrop architecture, does not alter the planner-level full-Kawase implementation, and does not claim that the native visual root cause is resolved. The native 2x2 gate remains downstream of the automated regressions.
