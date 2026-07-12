# Fullscreen and Maximized Absolute Placement Design

## Goal

Ensure fullscreen and maximized root toplevels render in authoritative output-space coordinates regardless of root insertion, focus, or raise order, while preserving cascaded placement for floating windows.

## Decision

Stateful window geometry owns the placement mode:

- floating roots use `SurfacePlacement::root()` / `root_at(...)` and remain `RootPlacementMode::CascadedWindow`;
- fullscreen roots use `SurfacePlacement::absolute_root_at(0, 0)` with output-sized geometry;
- maximized roots use `SurfacePlacement::absolute_root_at(usable.x, usable.y)` with usable-output-sized geometry.

The renderer's existing cascade formula remains unchanged. The fix does not numerically compensate for `FIRST_SURFACE_OFFSET` or `SURFACE_CASCADE_STEP`.

## Eligibility

Fullscreen exact-cover detection will require output-sized geometry, `RootPlacementMode::Absolute`, and local coordinates `(0, 0)`. A cascaded placement with compensating coordinates will not qualify. Existing conservative opacity behavior and fullscreen presentation ownership remain unchanged.

## Entry and restore behavior

Compositor shortcut fullscreen and client-requested fullscreen continue to call `set_root_window_mode(...)`; no protocol-specific placement path is added. Existing restore geometry capture remains authoritative so a stateful transition returns a floating root to its prior cascaded placement and exact size.

## Regression coverage

Tests will exercise later-root fullscreen and maximize placement, focus/raise independence, both fullscreen entry paths, reserved usable geometry, exact-cover eligibility, floating restore placement, output resize, layer-shell reservation changes, CSD window geometry, and root-relative subsurfaces. CSD and resize production code will only change if a deterministic regression fails after the placement-mode fix.

## Scope

No renderer redesign, clipping workaround, direct scanout, opacity change, Wayland compliance work, multi-output work, or changes to existing resize cursor behavior are included.
