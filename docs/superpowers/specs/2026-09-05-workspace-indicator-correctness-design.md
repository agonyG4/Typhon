# Eclipse / Typhon Workspace Indicator Correctness Design

## Goal

Make the Eclipse workspace indicator vertically centered and make Typhon expose
only the active or occupied regular workspaces while preserving ten logical
workspaces and stable `ext-workspace-v1` handles.

## Architecture

Typhon remains the authority for shell-visible workspace presence. A workspace
is visible when it is active or when it contains an eligible mapped Astrea
application toplevel. Eligibility is obtained from the existing
`CompositorState::astrea_toplevel_snapshot` predicate, and the workspace is
obtained from the canonical `WindowManagementState::regular_workspace` value.
Special-workspace members therefore never occupy a regular workspace.

`WorkspaceProtocolSnapshotItem` gains explicit `hidden` state. Both initial
manager binding and subsequent state publication derive `Active` and `Hidden`
from the same snapshot, while all logical workspace handles remain allocated
and ordered. Empty inactive handles are hidden rather than destroyed.

Presence publication is dirtied alongside the existing Astrea toplevel
structure/removal boundaries and reconciled through the existing controlled
Astrea publication gate. Metadata-only dirty updates do not trigger workspace
visibility publication. The existing immediate active-workspace publication
clears the same dirty state.

Eclipse keeps its ten-slot reserved surface width and existing workspace
tokens. `LauncherSurface.qml` anchors `WorkspaceStrip` to the content row's
vertical center, matching the legacy Quickshell reference. No QML filtering,
polling, subprocess, or fabricated `occupied` value is introduced.

## Test strategy

Typhon protocol tests cover the four active/occupied visibility combinations,
stable IDs, names, coordinates, and exactly one active workspace. Focused
compositor-state coverage exercises mapped XDG and eligible X11 application
entries, rejects unmapped and auxiliary/override-redirect entries, counts
minimized applications, ignores special members, and covers workspace movement
and visibility transitions without changing handle identity.

Eclipse's QML smoke test instantiates the production `LauncherSurface.qml`,
maps the real `workspaceStrip` center into `launcherPill` coordinates, and
asserts vertical-center equality. The shared Typhon workspace-state test
covers hidden filtering, active visibility, and stable-handle reappearance.

## Verification

Run the repository's required Typhon format, focused protocol test, library
test, all-target check, clippy, and diff checks. Build and run the requested
Debug Eclipse targets and focused CTest matrix, followed by the available
relevant shell/shared regression matrix and final diff checks.
