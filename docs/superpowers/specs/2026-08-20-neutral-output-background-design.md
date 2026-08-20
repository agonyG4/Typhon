# Neutral Output Background Design

**Date:** 2026-08-20  
**Status:** Approved by the supplied closure brief; implementation follows the repository baseline.  
**Repository:** Typhon `0ef9f7b99fa38d0fc04bf5ffa8f494db5a6eade6`

## Current implementation and ownership problem

`src/compositor/render.rs` defines four purple/green/brown gradient corner colors and `draw_wallpaper()`. `DesktopSceneRenderer` retains an output-sized `wallpaper: Vec<u32>` with dimensions and generation metadata; full rebuilds copy or regenerate it and partial repair copies rectangles from it. `src/egl_renderer.rs` separately creates an output-sized CPU pixel array, uploads it to an output-sized EGL texture, retains `wallpaper_resource`, and emits `EglDrawLayer::Wallpaper` as the first scene command.

This is compositor-generated artwork, not Paper state. It duplicates the user-wallpaper concept, consumes memory proportional to output size, and lets the old gradient show through when Paper is absent or incorrectly sized.

## Evidence classification

- **CONFIRMED:** The CPU renderer owns gradient constants, a persistent wallpaper buffer, `ensure_wallpaper()`, `copy_wallpaper_rect_to_scene()`, and gradient-based empty-scene tests.
- **CONFIRMED:** The EGL renderer owns `wallpaper_resource`, `ensure_wallpaper_resource()`, output-sized upload work, destruction on resize, and `EglDrawLayer::Wallpaper`.
- **NATIVE-PROVEN:** Existing frame, buffer-age, render-ahead, fullscreen, cursor, and scanout paths are separate from the pixel source used for the scene base.
- **PROTOCOL-DEFINED:** Generic layer-shell `-1`, `0`, and positive exclusive-zone semantics remain unchanged.
- **PROPOSED:** The compositor base becomes the bounded solid `OUTPUT_BACKGROUND` color, with a dedicated `ServerFrameColor::OutputBackground` 1x1 EGL resource.
- **UNPROVEN:** Native visual qualification with Paper, Topbar, and Dock requires the actual Astrea session and is not claimed from unit tests.

## Selected architecture

The CPU renderer fills the scene with `OUTPUT_BACKGROUND` for full rebuilds and fills only clipped damaged rectangles during partial scene repair. It redraws intersecting client/decorative elements after each repair, preserving the existing repair ordering and buffer-age history.

The EGL renderer removes the output-sized wallpaper texture. It creates the existing 1x1 solid resource for `ServerFrameColor::OutputBackground` and emits:

```text
Solid(OutputBackground)
background/bottom surfaces
normal window scene
top/overlay surfaces
cursor
```

The first command remains the neutral base and Paper remains an ordinary Background layer surface. Transparent pixels reveal lower client surfaces or, if none exist, the neutral solid. No Paper configuration is read by Typhon, no image is loaded by Typhon, and no compositor wallpaper service is added.

## Partial repaint and history

`fill_output_background_rect()` clips to output bounds and writes only the requested repair region. The renderer still repairs the base before redrawing intersecting scene elements. Existing buffer-age 1/2/3, render-ahead, presentation-domain history, Direct Scanout, fullscreen culling, KMS ownership, and cursor arbitration are not disabled or redesigned.

## Terminology

The `DesktopVisualState::wallpaper_only()` API and the external `astreactl wallpaper` / Paper concepts remain untouched. Renderer-only `wallpaper` cache/resource names and gradient constants are removed. Existing performance metric names such as `fullscreen_wallpaper_culled` remain temporarily if they are external-facing; their semantics are documented as background culling rather than artistic wallpaper ownership.

## Performance rationale

At 1920x1080, an ARGB8888 output-sized buffer is about 8 MiB; at 3840x2160 it is about 32 MiB, before the parallel EGL texture. The new CPU path has no persistent artistic background buffer, and the EGL path uses a 1x1 solid resource with no resize-time full-output upload.

## Rejected alternatives

- Keeping the gradient hidden underneath Paper preserves duplicate ownership and wastes memory.
- Replacing the gradient with another output-sized texture retains the scaling cost without benefit.
- Disabling partial repaint or buffer age would trade correctness for a shortcut and is forbidden.
- Hardcoding the Paper namespace in Typhon would violate generic layer-shell behavior.
- Moving Paper assets or selection state into Typhon is outside this closure.
