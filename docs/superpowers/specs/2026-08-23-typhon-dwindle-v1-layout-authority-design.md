# Typhon Dwindle v1 Layout Authority Design

## Goal

Turn `LayoutMembership::Tiled` into real per-location Dwindle geometry ownership while preserving Typhon's existing workspace, presentation-mode, client lifecycle, and visual-geometry authorities.

## Architecture

The WM layer gains a pure `src/wm/layout` package. Each `WorkspaceLocation` owns a lazy `DwindleTree` containing only `WindowId`s in a generational arena. Tree mutations produce immutable `TiledLayoutPlan` values from a root `LayoutRect` and immutable window snapshots. Horizontal means left/right and vertical means top/bottom; ratios are normalized fractions in `[0.10, 0.90]` and split rounding assigns the shared boundary once.

The compositor owns one `TiledLayoutManager`, reconciles it with canonical `DesktopWindow` management state, and applies visible plans through a focused batch path. The batch updates surface placement, visual geometry, X11 frame state, configures, scene work, and one final `LayoutReflow` generation. Hidden locations update topology and dirty state without eager configure or active-scene work.

## Behavior

- Floating admission remains the default; Super+V is the v1 entry point.
- Insertion anchors focused tiled windows first, then an explicit deterministic fallback, and finally the tree's first leaf. Pointer hints choose first/second child ordering without storing pointer state.
- Removal promotes the sibling into the removed parent position.
- Minimized leaves remain in topology but collapse out of the active calculation; fullscreen/maximized leaves remain in topology and keep their presentation geometry until returning to Normal, when the current Dwindle target is applied.
- Workspace and Special moves perform source removal and destination insertion before committing location state, using the same manager.
- Minimal server chrome is resolved from `WindowChromePolicy` without changing XDG decoration ownership; CSD pixels remain client-owned. Tiled interaction starts are rejected.
- Scene ordering derives Regular/Special and Tiled/Floating bands from canonical owner membership.

## Validation

Pure layout tests cover arena stale IDs, invariants, insertion orientation/order, removal, rounding, minimized collapse/restore, location isolation, and deterministic stress. Integration tests cover Super+V, reflow batching, moves, mode restore, work-area changes, chrome/hit testing, scene order, and interaction rejection. Existing floating, XDG, X11, fullscreen, and O1/KMS behavior remains regression-tested.
