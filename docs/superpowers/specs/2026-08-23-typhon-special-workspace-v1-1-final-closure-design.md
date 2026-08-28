# Typhon Special Workspace v1.1 Final Closure Design

## Status

Approved for autonomous implementation against the complete dirty checkout at
`9d3fb34b45f6ce4ffc4582c3231e220b3643e959`. This document records the closure
of the existing Special Workspace runtime; it does not introduce a second
workspace architecture.

## Architectural freeze

The canonical WM identity remains `WindowId`, `WorkspaceId`,
`SpecialWorkspaceId`, and `WorkspaceLocation::{Regular, Special}`. A
`WindowManagementState` continues to own canonical WM membership and
`LayoutMembership` independently owns Floating/Tiled membership. The
`WorkspaceManager` owns active regular selection and optional visible Special
selection. `ActiveSceneView` remains derived presentation/input state, and
`SceneWorkIndex` remains derived readiness bookkeeping. None of these derived
structures becomes lifecycle ownership for protocol objects, surfaces,
transactions, fences, or interactions.

Truly global/output-owned surfaces, including layer-shell and cursor surfaces,
remain `SceneWorkOwner::Global`. Auxiliary surfaces in a managed application
tree do not receive independent `WorkspaceLocation`; their scene visibility,
scene-work ownership, and render band inherit from the canonical managed root
through the existing parent/transient relationship resolver. A relationship
change therefore migrates derived scene ownership together with the existing
canonical workspace-membership reconciliation.

## Fullscreen composition rule

Composited fullscreen uses one ActiveScene-aware predicate:

> A fullscreen tree is solitary only when its owner is eligible, covers the
> output, is not minimized, has no visible popup that must remain composited,
> and no visible application tree outside the fullscreen owner exists.

The predicate classifies application content by canonical scene ownership and
does not special-case Special or promote Special content into layer-shell
overlay roots. Layer-shell fullscreen overlays remain an independent overlay
exception. `FullscreenRenderPlanMetrics` and
`native_frame_renderable_surfaces()` consume the same solitary result, so
metrics cannot claim culling that the final frame does not perform. Direct
Scanout remains conservative and rejects visible Special application content.

## Scene-selection transition

Special open/close is a real derived `ActiveSceneSelection` transition while
WM membership remains unchanged. The transition captures old and new
selection, classifies canonical managed application families as visible-before
and visible-after, and identifies only `visible-before && !visible-after`
owners as departing. Departure cleanup runs before the ActiveScene cache is
rebuilt.

Interaction cleanup uses `end_window_interaction_by_id_with_reason()` so an
active resize follows its existing terminal configure path exactly once. The
same transition clears pointer constraints, implicit/popup grabs, held-button
ownership, last press, and stale pointer/focus state only for departing owners.
Regular interactions survive Special close; opening Special does not globally
cancel regular interactions. Layer-shell focus authority remains intact.

The ActiveScene rebuild reports both `selection_changed` and
`visual_scene_changed`. Visual identity includes ordered active surface IDs,
active popup IDs, and cached surface origins. Empty Special selection changes
may therefore update visibility bookkeeping without advancing scene render
generation. Populated Special open/close advances exactly once, and hidden-only
reordering advances zero times.

## Visible work and callback-only classification

`SceneWorkIndex` is rebuilt at existing event boundaries from the canonical
pending collections. Active callback-only classification requires visible
frame callbacks and no visible frame-prepare work, no flushable resize
configure, no visible presentation feedback, and no other visible color work.
Hidden explicit-sync and surface-tree work is not allowed to make an active
output cease being callback-only. No global hidden collection scan is added.

## Geometry and protocol invariants

Special visibility, fullscreen composition, scene ordering, and scene-work
updates do not mutate `ToplevelVisualGeometry`, `SurfacePlacement`, XDG
geometry, X11 `ConfigureWindow`, fullscreen protocol mode, EWMH desktop
numbering, or ext-workspace regular handles. The sole permitted configure is
the existing terminal resize configure required when a departing owner already
owns an active interactive resize.

## Source-layout boundary

Only regressions caused by this Special closure are moved. Presentation root
ordering is extracted from `subsurfaces.rs` into a focused scene-order module.
The new Special native input regression test is moved from the over-limit input
test module into a focused Special Workspace test module. Existing unrelated
source-layout violations remain documented rather than being rewritten.

## Validation contract

The implementation must pass formatting, locked all-target checking, diff
whitespace checking, source-layout checking for the task-owned regressions,
focused fullscreen/scene/interaction/callback/ordering tests, and a full locked
test attempt. Environment-specific SUN_LEN failures are reported separately
and are not hidden by weakening tests.
