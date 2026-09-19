# Checkpoint Capture Path Diagnostic Design

## Goal

Measure whether checkpoint-dependent `SceneCapture` cost is specific to
`glBlitFramebuffer`/native framebuffer resolve behavior or is already present
when the native output image is read as a texture. The production default stays
FramebufferBlit.

## Architecture

`AtomicOutputSlot` remains the owner of the imported EGL/GBM texture and its
framebuffer. `EglOutputRenderTarget` gains an optional, borrowed-by-handle
`sampleable_texture`; `GlesSceneRenderer` temporarily records it as
`active_output_texture` while drawing a target and restores it with the existing
capture renderer state. The screenshot/test target supplies no output texture;
the Atomic EGL/GBM target supplies `Some(slot.texture)` without transferring
ownership or adding deletion responsibility to the renderer.

The existing direct checkpoint capture continues to bind the pooled graph target
as DRAW and the native output framebuffer as READ. A new diagnostic selector,
`TYPHON_EFFECT_DEBUG_CHECKPOINT_CAPTURE_PATH`, accepts only `blit` (the default)
or `shader-copy`. For checkpoint-dependent direct `SceneCapture` with replay
capture policy, non-lifecycle rendering, and an available active output texture,
`shader-copy` binds the pooled graph target as DRAW and performs one exact
`texelFetch` copy from the output texture. It never binds the output framebuffer
as DRAW while sampling that texture. Missing capability falls back to the
existing blit path with a bounded reason.

The copy shader maps destination graph pixels back to the logical top-left
capture domain using the output size, domain, graph target size, and explicit
`OutputFramebufferOrigin`. The target remains a normal pooled
`GraphTextureSource::CapturedScene` texture with
`GraphTextureOrigin::BottomLeft`; graph validation, checkpoint validity,
Kawase, lifetime, and dependency semantics are untouched.

## Diagnostics

Capture timing gains a separate `FramebufferShaderCopy` mode and explicit
path-specific duration/pass/pixel fields while retaining the aggregate
framebuffer-backed fields. With GPU timing enabled, every resolved capture pass
emits one bounded `effect_capture_gpu_timing` event containing frame/pass/
instance/kind/path/checkpoint-count/pixels/duration. The ordinary effect pass
trace records both the requested and executed path, including a bounded fallback
reason.

## Error and state rules

Invalid checkpoint-path values warn once and use `blit`. Shader-copy errors are
returned like other capture errors; capability absence is the only normal
fallback. No `glFinish`, `glFlush`, EGL wait, readback, or automatic path
selection is introduced. Renderer state is restored after every copy, and the
active output texture is saved/restored for nested renderer operations.

## Tests

Pure mapping tests cover the 1920x1080 Dock, TopBar-left, TopBar-right, and
interior domains for both framebuffer origins and compare the shader mapping to
the existing blit planner. A real GLES fixture renders a non-uniform,
alpha-varying output texture attached to an FBO, captures matching domains by
blit and shader-copy, and requires exact readback equality, including all four
output edges. Focused state tests prove `CaptureRendererState` restores the
output texture and that shader-copy never uses the active output framebuffer as
DRAW. Existing native-faithful Dock, TopBar, multi-dependency, validity,
scene-work, and orientation regressions run with both acquisition paths without
changing expected reference output.

## Explicit non-goals

- No zero-copy graph texture alias or output-domain UV support.
- No internal composition framebuffer or persistent backdrop cache.
- No Kawase, checkpoint-domain, checkpoint-validity, partial-capture, or graph
  resource-lifetime changes.
- No production auto-selection based on measured speed.
