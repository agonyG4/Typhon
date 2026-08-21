# Typhon `ext-workspace-v1` bridge

Typhon exposes the official `ext_workspace_manager_v1` staging protocol as a
native Wayland global. The bridge is deliberately a thin protocol adapter:
the canonical `WorkspaceManager` remains the only workspace authority, and
the compositor publishes protocol state from that manager.

## Contract

- One workspace group is advertised for every bound manager.
- The default manager supplies ten workspaces (`1` through `10`). The bridge
  also uses the manager's live iterator, so a future configured count is not
  hard-coded into the protocol layer.
- Workspace IDs are stable strings `typhon.workspace.N`; names remain the
  human-readable numeric strings `N`.
- Coordinates are one-dimensional and zero-based: workspace `N` has
  coordinates `[N-1]`.
- Exactly the canonical active workspace carries `state.active`.
- Group capabilities are empty. Workspace capabilities contain only
  `activate`.
- Unsupported group and workspace mutation requests are ignored. No create,
  destroy, remove, assign, deactivate, or output-selection capability is
  advertised.

The global is version 1 and is generated from the official
`ext-workspace-v1.xml` definition supplied by `wayland-protocols`.

## Atomic requests and publication

`ext_workspace_handle_v1.activate` is recorded in the manager binding's
transaction. Requests from another client or for a workspace that is not in
the canonical manager are rejected before they enter the transaction. Within
one transaction, the last valid activation wins. `manager.commit` consumes the
pending activation and calls the canonical
`CompositorState::activate_workspace`; the manager is never mutated directly
by the protocol adapter.

A canonical switch publishes a state event for every live handle followed by a
single manager `done`. This covers protocol-originated switches and internal
Typhon switches because `activate_workspace` is the shared authority path.
The activation side effects (focus, scene, idle inhibition, toplevel
publication, render generation, and pointer focus) therefore stay identical
for both callers.

## Output topology

Typhon has one workspace group. Output membership is derived from the existing
`CompositorState::output_resources` registry:

1. A newly bound manager receives `output_enter` for already-bound outputs
   belonging to the same Wayland client.
2. A later output bind receives one enter for that same client's managers.
3. Output leave is sent before the output resource is removed.
4. A per-group object-id set prevents duplicate enters.

Output bind order is therefore irrelevant, and output resources are never
leaked across clients.

## Lifecycle

Manager `stop` sends `finished` and removes the binding. Manager, group, and
workspace-handle destruction removes the corresponding tracking state.
Client teardown removes all manager bindings for that client. Dead resources
are also pruned before publication. This keeps output and activation tracking
bounded by live Wayland resources.

## Verification

The protocol unit tests cover stable IDs, numeric names, zero-based
coordinates, the single-active invariant, and last-valid-request semantics.
The compositor's existing registry/lifecycle test infrastructure remains the
place for live socket integration coverage; this v1 bridge does not introduce
an alternate shell command, subprocess, polling, or legacy compositor API.
