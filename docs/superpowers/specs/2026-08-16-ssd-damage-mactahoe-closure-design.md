# Typhon SSD Damage and MacTahoe Closure Design

## Status

This note records the corrective design for the SSD implementation that exists at commit `3d835df`. The initial investigation begins with deterministic damage tests; native-session observations are recorded separately and are not inferred from unit tests.

## Baseline evidence

The implementation baseline was `3d835df4d4622580aafad67291f7d0d8ce440973` (`fix: align SSD rendering with resolved surface origins`). The worktree already contained unrelated dirty and untracked changes; those files remain outside this closure. Before the closure changes, the native output test suite passed its existing tests, while the new repeated-move regression failed on the first move because the old titlebar-only pixel was not in damage.

The pre-existing repository gates also recorded these unrelated failures: strict clippy reported `obfuscated_if_else` and `too_many_arguments` in the earlier SSD implementation plus `large_enum_variant` in XWayland, and source-layout reported line-count limits in `src/compositor/state/windows.rs`, `src/compositor/mod.rs`, and `src/compositor/server.rs`. They remain separately identified during final verification.

## Observed failure

Moving a server-decorated window across a reused native output buffer can leave old titlebars, borders, and control artwork behind. The existing scene damage tracks client surface bounds, while SSD is drawn as a separate decoration pass that extends above and around those bounds. A client-only old/new damage union therefore does not necessarily repair pixels belonging only to the previous titlebar or outer border.

## Root cause and ownership model

The root cause is incomplete damage ownership, not a placement offset or animation artifact. A decorated window is one logical visual: its client content and decoration have one render-space origin and one lifecycle. When that visual moves, the repair region is the union of its previous outer frame and current outer frame. When it disappears, the previous outer frame remains damage-owned until the underlying scene is repaired.

The design adopts the KWin principle described in the closure brief: decoration is a child of the window visual hierarchy, conceptually alongside the surface and shadow, rather than an unrelated overlay. Typhon’s equivalent is a stable `DecorationSceneSnapshot` associated with `WindowId` and root surface identity. Its logical outer bounds and visual signature participate in the same native damage accumulator as surface scene elements. The CPU and GLES renderers consume the same logical damage; renderer-specific code only performs copies, scissor repair, texture upload, or drawing.

## Coordinate contract

Decoration render bounds, hit testing, and damage bounds use `render::surface_origins(surfaces)`. That function is authoritative for cascaded placement, absolute placement, interactive `render_placement`, subsurface ancestry, and XWayland visual placement. Raw `RenderableSurface::x/y` are local surface data and are never treated as global decoration origins.

## Snapshot and damage flow

Each current decorated root produces:

```text
WindowId
root_surface_id
resolved logical origin
logical outer bounds
theme generation
visual signature
```

The previous snapshot is retained with the last presented surface scene. Damage compares stable identities, not vector indexes. It adds:

```text
previous bounds when identity moved, resized, changed state, or disappeared
current bounds when identity appeared, moved, resized, or changed state
```

The result is clipped/coalesced using the existing output damage rules and passed to both CPU frame-copy paths and GLES buffer-age/scissor planning. A state-only change is bounded to the decoration outer frame; it does not promote the entire output to full damage.

## Theme asset pipeline

`MacTahoe-Dark` is a real bundled package. Normal selection goes through the same safe package loader as external themes, preserving relative-path, size, and external-resource validation. Activation parses and rasterizes SVGs once into immutable, bounded, scale-keyed RGBA assets. Frame rendering never reads disk, parses SVG, or determines artwork by filename.

The package maps the supplied Labwc artwork as follows:

```text
iconify      → Minimize
max          → Maximize
max_toggled  → Restore
close        → Close
```

Controls remain on the right in the Astrea order Minimize → Maximize/Restore → Close. Active, inactive, and hover artwork are distinct when supplied; pressed falls back to hover then normal when the package has no pressed asset. Premultiplied alpha is preserved in CPU composition and GLES textures.

The first package uses the supplied compact metrics: an exact 26 logical pixel titlebar, 12 px horizontal padding, 16×16 controls, and 9 px spacing. Controls are vertically centered in the titlebar. The supplied colors are active `#333333`, inactive `#242424`, active text `#FFFFFF`, inactive text `#FFFFFF99`, active border `#010101`, and inactive border `#2C2C2C`.

## Typography

Theme typography is real configuration. Font selection happens during theme activation using the requested family/style and a cached system-font database with open fallback order Inter, Noto Sans, then generic sans. Glyph advances are measured from the selected font. Title layout is centered relative to the outer window, clipped before the right-side controls, and ellipsized by measured width. Unicode iteration is scalar-safe; no hardcoded 5×7 glyph table remains in the production path.

## Geometry and interaction

Maximized SSD geometry is defined in terms of the decorated outer frame. The configured client area accounts for decoration extents so the outer frame equals usable output geometry. Restore stores and returns the exact prior client/frame relationship. Fullscreen has no SSD and does not globally disable Direct Scanout.

Input gives button hit regions precedence over resize edges. Pointer capture remains owned by the exact `WindowId`, but pressed artwork is visible only while the pointer is inside the captured button. Release outside performs no action. Titlebar double-click requires both the existing time window and a logical spatial threshold.

## Intentional limitations

Rounded corners and shadows are not implemented by opaque black rectangles or ad hoc clipping. They remain a follow-up unless the existing render architecture provides a clean reusable visual clip/shadow abstraction during this closure. Blur and Liquid Glass effects are explicitly out of scope. Native-session results are reported only when a real Typhon/Astrea session is available.

## Validation evidence

The closure includes a pixel-level repeated-move regression over 30 positions, direct damage tests for move/disappearance/state changes, package/raster/alpha/typography tests, CPU/GLES scene-cache parity coverage, fractional-scale tests at 1, 1.25, 1.5, and 2, geometry/input stress tests, and the repository’s final locked verification gates. Existing unrelated clippy and source-layout baseline failures remain distinguished from changes made by this closure.

Native-session qualification was not available in this environment: `astreactl` and the normal Astrea launcher were not on `PATH`, so theme list/set/reload commands and Kitty/Qt/XWayland live exercises could not be performed. The repository has a Wayland environment and DRM device nodes, but that is not sufficient evidence of an Astrea-controlled Typhon session.
