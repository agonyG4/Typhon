# Typhon Dwindle v1.1 Constraint-Aware Resize Design

## Goal

Complete the Dwindle v1 geometry foundation with a pure constraint-aware
solver, exact tile/client separation, transactional workspace/Special
migration, work-area invalidation, and a single frame-coalesced split-resize
path for XDG, X11, native, SSD, and CSD interactions.

The attached v1.1 brief is the authoritative scope. The existing dirty
worktree remains authoritative and v1 architecture is preserved.

## Frozen authority hierarchy

`DesktopWindow` and `WindowManagementState` remain the canonical window and
location authorities. `TiledLayoutManager` owns one pure `DwindleTree` per
`WorkspaceLocation`; trees contain only `WindowId`, topology, and preferred
ratios. Pure snapshots enter a solver that returns an immutable solution. The
compositor applies a visible solution through the existing
`ToplevelVisualGeometry` and configure paths inside one `LayoutReflow` batch.
No compositor, protocol, surface, renderer, or client lifecycle object enters
the WM layer.

## Pure constraints and solution

`LayoutConstraints` mirrors the existing neutral constraint data:

```text
min_width, min_height, max_width, max_height
base_width, base_height
width_increment, height_increment
min_aspect, max_aspect
```

Snapshot construction and solve input validation reject contradictory bounds,
zero increments, non-finite/non-positive aspect values, and reversed aspect
ranges. Invalid input produces a typed error and never enters ratio math.

The solver performs two linear passes. First it aggregates active-leaf
minimum tile footprints bottom-up. A horizontal split adds child widths and
takes the larger child height; a vertical split takes the larger child width
and adds child heights. Minimized leaves contribute nothing. The second pass
derives each split's feasible boundary interval from complete subtree
requirements, intersects it with the typed absolute ratio range `[0.05, 0.95]`,
clamps the preferred ratio only for this solution, and records a
`ResolvedSplit` trace. Preferred ratios are unchanged unless an explicit
resize stores a clamped requested ratio.

Tile rectangles continue to partition the root exactly. Each active leaf also
gets a legal `client_rect`: the largest deterministic size satisfying min/max,
base/increments, and aspect bounds inside its tile, centered with integer
rounding. Max/fixed-size slack never changes topology or creates tile holes.
An impossible root returns a typed infeasibility witness before any plan can be
applied.

The ICCCM resolver uses bounded arithmetic alignment and a small fixed
convergence loop, not pixel-by-pixel search. The neutral helper is reused by
the existing floating/X11 clamp path so solved Tiled targets are idempotent
through configure routing.

## Transactional topology and constraint reconciliation

Insertion, removal, and workspace/Special migration operate on a cloned
candidate manager. Candidate snapshots are derived from candidate tree leaf
membership rather than canonical locations that have not committed yet. All
affected trees are solved before topology, location, or membership changes are
committed.

If an incoming tiled family makes a destination infeasible, existing
destination members stay Tiled and incoming members are deterministically
downgraded to Floating until the destination solves. Every downgraded window
keeps its destination location and saved floating geometry. A committed Tiled
membership always has exactly one tree leaf.

Dynamic XDG and X11 constraint changes follow the same reconciliation path. A
feasible visible tree reflows once; a feasible hidden tree is marked dirty. An
impossible update auto-floats the constraint-changing window, preserving the
location and reflowing survivors. Work-area shrink uses solver witnesses and
the most recently inserted applicable leaf, repeating only as necessary while
keeping one fitting Tiled window when possible.

## Work-area and dirty-location semantics

Layer-shell arrangement and output-size changes call one shared usable-area
change helper. The helper batches stateful mode reconfiguration and visible
Regular/Special Dwindle solutions, while hidden trees are only marked dirty.
Successful visible application clears the location's dirty bit. Activation or
Special opening solves a location only when it is dirty or has no valid
resolved geometry. Hidden clients never receive configures solely because the
work area changed.

## Split resize and interaction lifecycle

Pure tree APIs resolve the nearest adjustable ancestor for each requested edge:
right/left map to horizontal first/second-child ancestry, and bottom/top map
to vertical first/second-child ancestry. A `TiledResizeAxis` is a compact pure
handle containing the split, axis, start ratio, start parent rect, and start
boundary. Corners may carry one handle per axis. Handles are invalidated by
topology or relevant-constraint changes.

The existing `window_interaction` remains the sole interaction owner. Move is
still rejected for Normal Tiled windows; Resize starts a `TiledResizeSession`.
Raw pointer motion computes the requested ratio from interaction-start pointer
and geometry and replaces one bounded `PendingTiledResize` slot. It performs no
tree solve, plan allocation, configure, or visual mutation. `prepare_frame()`
flushes the latest slot before the existing resize-configure flush, performing
at most one combined horizontal/vertical solve and one layout batch. Unchanged
effective ratios are deduplicated. Release force-flushes the latest intent and
sends the owner's terminal configure; cancellation discards unflushed intent.

The owner gets normal XDG `Resizing` state during the interaction and a
terminal configure without it. Siblings receive ordinary layout targets.
XDG resize, X11 `_NET_WM_MOVERESIZE`, native modifier resize, Minimal SSD edge
input, and CSD requests all route through this same ratio mutation path; move
requests remain rejected. Minimal chrome gains only logical adjustable edge
zones and accurate cursor feedback, without titlebar or floating move affordances.

## Validation and non-goals

Tests cover solver arithmetic, client rectangles, nested requirements,
transactional migration, dynamic constraints, work-area visibility, resize
anchors, corners, constraint stops, release/cancel/topology invalidation,
backend routing, generation/configure contracts, deterministic mutation
stress, and synthetic trees of 1/10/50/100/500 windows. Metrics expose raw
updates, pending replacement, frame flushes, clamps, unchanged flushes,
constraint reflows/fallbacks, migration fallbacks, and work-area reflows.

The implementation adds no gaps, directional focus/movement, animations,
tiled-by-default admission, Niri/Infinite Canvas behavior, O1/KMS changes,
timers, layout threads, or client-ack barriers. Task-owned oversized test
modules are split into focused Dwindle/Tiled files without raising source-size
limits.
