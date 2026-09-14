# Typhon Fullscreen Presentation Dominance Design

## Problem

Typhon currently uses the canonical active-scene ordering to decide whether a
fullscreen owner is solitary. `renderable_root_stack_key` includes normal
`window_stacking`, so an unrelated regular application placed after the
fullscreen owner makes `solitary_tree_active` false. Native-frame selection
then returns the complete active scene, allowing regular applications and
ordinary layer-shell chrome to appear above the game.

The canonical active scene remains the logical workspace scene. Fullscreen
presentation needs a separate, non-destructive policy over that scene.

## Design

Add `FullscreenCompositionPlan` to the compositor fullscreen policy. The plan
is derived from the authoritative `FullscreenPresentationState`, active scene,
workspace relationships, layer-shell state, and presentation-animation state.
It has three modes:

- `Inactive`: no fullscreen owner exists; the canonical scene is presented.
- `Transitioning`: an owner exists but is missing, hidden, does not cover the
  output, or has an unsettled presentation transition; the broader scene is
  retained so the transition has no background holes.
- `Dominant`: the owner is visible, exactly covers the output, and its physical
  presentation transition has settled; fullscreen membership is enforced.

The plan stores compact bounded root-ID vectors for the owner family,
explicitly allowed application roots, and explicitly allowed layer roots. It
also records root and surface culling counts, strict owner-only status, and a
primary above-fullscreen reason for diagnostics. No canonical windows are
removed or reordered by the plan.

The plan exposes one surface-membership predicate. Native frame selection and
pointer hit testing pass the same plan through their traversal paths. Popup
surfaces use their existing `PopupNode::owner_root_id`; subsurface descendants
use their existing root placement; transient/toplevel children use the
existing canonical scene owner relationship.

## Root classification policy

One compositor method classifies every active presentation root as one of:

- `OwnerFamily`: the fullscreen root itself, an existing canonical transient
  or parent family member, or an XDG popup logically owned by that family.
- `AllowedAboveFullscreen`: an explicitly visible special-workspace
  application; an application `Notification`, `Overlay`, or explicit `Above`
  stack category; or a mapped `Layer::Overlay` root.
- `CulledByFullscreen`: an unrelated regular-workspace application, an
  unrelated popup/ordinary auxiliary root, a global/background root, or a
  mapped `Layer::Background`, `Bottom`, or `Top` root.

The application category is resolved from existing `DesktopStackLayer` and
workspace/relationship state. Normal stack order never grants permission to
defeat dominance. Owner-family membership is resolved through
`canonical_scene_owner_window_id`, `parent`, and `transient_for`; it is not
reimplemented as a second window hierarchy.

Layer-shell policy is explicit: Background, Bottom, and Top are culled in
Dominant mode, while Overlay remains allowed. Eclipse therefore needs no
visibility IPC or game-specific behavior. An allowed overlay or application
root makes `solitary_tree_active` false, but does not disable composition
culling.

## Metrics

`FullscreenRenderPlanMetrics` retains owner and Direct Scanout diagnostic
information and adds separate composition fields:

- `fullscreen_composition_active` and `fullscreen_transition_pending`;
- allowed application/layer root counts;
- culled application/layer root counts;
- `fullscreen_above_reason`.

`fullscreen_active` continues to mean that an authoritative owner exists.
`fullscreen_composition_active` means that Dominant filtering is active.
`solitary_tree_active` means Dominant mode has no visible root beyond the
owner's strict root-only tree. The renderer keys filtering from
`fullscreen_composition_active`, never from `solitary_tree_active`.

Direct Scanout remains independently stricter and continues to reject visible
special applications, overlays, popups, extra owner-tree surfaces, animation,
and other existing blockers.

## Window-management coherence

Fullscreen entry may raise the owner through the existing normal stack/focus
path if the current transition path warrants it. That raise is only for normal
window-management coherence. The plan classifies membership independently,
and a regression will restack an unrelated regular application above the owner
after dominance is active, proving rendering and input remain correct.

## Tests

Add deterministic compositor regressions for:

1. the Cyberpunk-shaped stack order where an unrelated regular application is
   above the owner;
2. restacking that application after activation and restoring the normal scene
   after fullscreen exit;
3. Background/Bottom/Top layer culling and Overlay allowance, including input;
4. special-workspace application allowance while unrelated regular and Top
   roots remain culled;
5. owner XDG popup/transient-family allowance while unrelated roots remain
   culled;
6. transition-time broad-scene retention followed by Dominant culling;
7. pointer hits never selecting a visually culled application or Top layer;
8. the existing XWayland owner path consuming the same plan where its test
   harness makes focused coverage inexpensive.

Existing fullscreen origin, scene-order, workspace-selection, popup,
Direct-Scanout, and liveness tests remain unchanged unless their assertions
describe the old binary-solitude behavior. The old stack-position blocker is
removed or retained only as a diagnostic and is not used to enable/disable
fullscreen filtering.

## Non-goals

This change does not modify Predictive O1, frame scheduling, KMS, explicit
sync, pointer constraints, terminal-client lifetime, Direct Scanout rules,
Eclipse code, or canonical workspace contents.
