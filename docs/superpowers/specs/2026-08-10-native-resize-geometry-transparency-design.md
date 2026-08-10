# Native Resize Geometry and Transparency Regression Design

**Goal:** Stabilize CSD/native Wayland resize rendering while preserving correct XWayland stale-content behavior and Typhon's current client-side decoration policy.

## Root causes

Typhon currently has two independent regressions coupled together:

1. `xwayland_visual_backing_target()` treats an absolute root placement as proof that a surface is XWayland. Recent native XDG placement changes intentionally made ordinary roots absolute, so native XDG surfaces can receive the opaque black backing during resize. That backing hides the scene below transparent Zen/Kitty pixels and produces Firefox-like black rectangles.
2. XDG `set_window_geometry` is committed into `surface_window_geometries` before the first `RenderableSurface` exists. The render-placement correction is only applied to existing renderables, so the first root renderable starts without the derived buffer-origin offset. The first resize then installs that correction for the first time, causing a visible jump.

The current decoration path explicitly sends `ClientSide`; Firefox's extra corner radius is therefore client-owned and is not part of this geometry fix.

## Architecture

The existing state is retained but its ownership is made explicit:

- `surface_placements` and `RenderableSurface::placement` represent the compositor-owned logical frame placement.
- `surface_window_geometries` represents the latest valid committed XDG client window rectangle, including non-zero CSD/shadow margins.
- `RenderableSurface::render_placement` is derived render state only.
- A single helper derives the root render origin by subtracting the committed XDG window-geometry offset from the compositor frame origin. The helper is used after first renderable creation, on committed geometry changes, and during resize preview.

Resize preview changes the compositor's target frame geometry. It does not activate a second coordinate system. The root buffer remains at the deterministic frame-to-window-geometry offset from the first valid commit through the end of the resize transaction.

Renderable surfaces carry explicit backend identity. Native Wayland/XDG publication uses the native identity; the XWayland publication path marks confirmed XWayland surfaces. Opaque `XwaylandBacking` is emitted only for confirmed XWayland roots with an active visual aperture. Absolute placement, root status, and resize-preview state are not sufficient. CPU scene composition, GLES command generation, and server-frame inspection use the same predicate.

The aperture continues to describe valid stale/current client content regions. Native Wayland apertures never cause an opaque fill; transparent pixels are composited directly over the already-composed scene. The retained XWayland backing is scoped to the opaque X11 compatibility invariant: stale X11 content does not provide transparent background pixels while the compositor target grows or shrinks.

## Decoration policy

No server-side decoration advertisement is introduced. Typhon continues to negotiate client-side decoration and preserves client-owned Firefox/GTK/Qt titlebars, shadows, and corner radii. A future SSD design note will cover compositor-owned frame extents, decoration input region, resize borders, titlebar, border radius, shadow, maximize/fullscreen radius rules, CSD/SSD negotiation, and rendering ownership.

## Regression tests

The implementation will add or update deterministic tests for:

- first renderable creation with non-zero XDG window geometry and stable frame-to-buffer origin;
- at least 100 repeated first/subsequent resize cycles with unchanged anchor semantics;
- native transparent XDG resize previews with no XWayland backing and matching CPU/GLES scene decisions;
- Firefox-like CSD buffer margins and stale content during grow/shrink resize;
- confirmed XWayland grow/shrink, stale-buffer, and override-redirect behavior;
- decoration negotiation remaining client-side.

The existing test-compilation defect on `main` where `desktop_window_tests` cannot access `desktop_window_frame` will receive only the minimal visibility correction needed to run the repository's current regression suite.

## Scope constraints

This design does not change KMS scheduling, triple buffering, direct scanout, explicit-sync ownership, XWayland lifecycle, global alpha behavior, tiling/window-management policy, or decoration rendering ownership.
