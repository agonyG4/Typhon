# Scene-Work Preservation Regional Transfer Design

## Goal

Reduce scene-work preservation transfer bandwidth by capturing exactly the
`extra_scene_work` rectangles while retaining the existing output-sized pooled
RGBA8 preservation texture and all checkpoint/replay semantics.

## Current context

`capture_scene_work_preservation` currently acquires an output-sized
`EffectTextureKey` and blits `full_output_rect(output_size)`. Restore already
operates on `extra_scene_work`, but recomputes its transfer geometry. This
change makes capture and restore use one stored plan derived from the existing
`scene_work_preservation_blit_rects` helper.

## Design

- Add a small pure `SceneWorkPreservationPlan` containing the individual
  `GraphTextureCaptureBlit` transfers and a saturating `u64` pixel total.
- Build the plan from `extra_scene_work`, output size, and framebuffer origin.
  The plan preserves rectangle order, does not merge or bound disjoint regions,
  and represents empty work without a texture allocation.
- Keep acquiring exactly one output-sized `EffectTextureKey` per graph
  execution that needs preservation.
- Bind the active output framebuffer and preservation draw/read target once per
  phase, then blit every plan transfer without per-rectangle framebuffer or
  scissor rebinding.
- Store the exact plan with `SceneWorkPreservation`; restore consumes it
  directly and never recomputes geometry from `extra_scene_work`.
- Preserve existing ordinary-state restoration, texture release, error
  precedence, and cleanup behavior.
- Add bounded `effect_scene_work_preservation` trace events for capture and
  restore with phase, rectangle count, transferred pixels, output pixels, and
  optional basis-point coverage. No rectangle list or new environment variable
  is introduced.

## Testing

- Pure plan tests cover Dock-sized work, TopBar-sized work, two disjoint
  rectangles, clipping, both framebuffer origins, and empty work.
- A real GLES pixel regression uses a non-uniform framebuffer, two disjoint
  preservation regions, an overwritten output, and verifies that only the
  planned regions restore for both TopLeftScanout and BottomLeft mappings.
- Existing native-faithful Dock/TopBar checkpoint, full-Kawase control,
  dependency-validity, SurfaceConsumerPlan, coordinate-space, fullscreen, and
  diagnostic configuration regressions remain unchanged and are rerun.
- Verification reports work quantity (rectangles and pixels), check/clippy
  status including modified-file lint ownership, and native trace evidence.
  No timing assertion or GPU-time claim is added.

## Explicit non-goals

This change does not introduce compact preservation textures, a fallback
threshold, per-checkpoint scene-work timelines, partial checkpoint capture,
Kawase changes, blur redesign, resource-pool changes, or semantic changes to
checkpoint geometry/validity.
