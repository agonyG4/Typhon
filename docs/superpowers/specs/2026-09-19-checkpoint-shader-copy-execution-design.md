# Checkpoint Shader-Copy Execution Design

## Goal

Connect the existing checkpoint shader-copy diagnostic path to production execution without changing checkpoint semantic validity, capture domains, Kawase behavior, or the existing framebuffer-blit implementation.

## Design

Introduce one small, pure execution planner in the effects execution layer. It receives the pass kind, checkpoint dependency count, lifecycle-backdrop flag, global debug capture mode, requested checkpoint path, and whether the renderer currently exposes a sampleable output texture. It returns the requested path, the executed path, and an optional fallback reason.

Shader-copy is selected only for a replay-mode `SceneCapture` with at least one checkpoint dependency, no lifecycle backdrop, an explicit shader-copy request, and an available sampleable output texture. All other direct captures continue to use framebuffer blit. A shader-copy request without the output capability falls back to blit with `NoSampleableOutputTexture`. The absence of the environment setting and invalid values retain the existing blit default and warning behavior.

`execute_capture` consumes the planner result to choose the existing blit function or the dedicated shader-copy function. The shader-copy function uses the same pooled destination graph texture, the existing capture-copy program and fullscreen quad, and explicit draw-framebuffer/output-texture separation. It does not alias textures, add an internal composition framebuffer, change graph input validation, or introduce synchronization calls.

Trace summaries, capture timing metadata, GPU aggregate accounting, and the per-capture GPU timing event derive their path attribution from the same planner result. Existing aggregate framebuffer fields remain compatibility totals that include both physical framebuffer acquisition paths; new bounded fields expose blit and shader-copy separately.

## Verification

The implementation is verified in layers:

1. Pure planner tests cover eligible shader-copy, unavailable-output fallback, ordinary replay, surface capture, lifecycle backdrop, framebuffer mode, default blit, and invalid-value parsing.
2. Existing coordinate mapping tests remain unchanged.
3. A real GLES test renders a non-uniform, alpha-bearing pattern into a texture-backed output framebuffer and compares blit and shader-copy readbacks for bottom-left and top-left origins across interior, dock-like, and top-bar domains.
4. Native-faithful Dock, TopBar, and multi-dependency graphs compare blit and shader-copy final pixels, checkpoint ordering, presentation repair, and semantic validity. The traces must prove shader-copy actually executed.
5. GPU timing tests verify path-specific aggregates and one bounded event per resolved timed capture span.
6. The diagnostic tests and the requested fmt, check, and clippy commands are run with repository-local build artifacts.

## Explicit non-goals

- Zero-copy aliasing.
- An internal composition framebuffer.
- Auto-selection beyond the explicit checkpoint debug request.
- Changes to checkpoint semantic validity, dependency semantics, capture domains, or Kawase.
