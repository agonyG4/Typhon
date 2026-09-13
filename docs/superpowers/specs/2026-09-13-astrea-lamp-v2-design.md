# Astrea Lamp v2: visual bounds and directional deformation

## Goal

Improve `minimize.lamp` so the complete root visual group (client surface,
root-owned subsurfaces, and server-side decoration) moves as one bounded,
reversible Genie/Magic-Lamp presentation object. Preserve Typhon's existing
lifecycle ownership, physical settlement, suppression, transition IDs, stale
pageflip rejection, render evidence, overlay ordering, and configuration
semantics.

## Frozen transition geometry

At semantic lifecycle takeover, capture one immutable presentation geometry:

- the canonical client/root rectangle;
- the canonical visual rectangle, formed from retained root render bounds,
  retained root-owned subsurface bounds, and the current SSD
  `DecorationRenderInstance::scene_snapshot().bounds()`;
- the pageflip-confirmed presented client rectangle;
- the Dock anchor, inferred direction, and geometry-dependent shape values.

The presented visual rectangle is derived by applying the affine mapping from
canonical client rectangle to presented client rectangle to the complete
canonical visual rectangle. It is never independently offset or recaptured.
The complete geometry remains frozen until physical pageflip-confirmed
settlement. A mid-flight reversal gets a fresh `LifecycleTransitionId` but
reuses the exact frozen visual group, source geometry, anchor, direction, and
shape parameters. A later transition, after settlement, captures fresh
canonical bounds.

## Damage authority

`lamp_footprint` evolves into a pure visual-group footprint contract. Both EGL
renderer lifecycle damage and native scene-history repair consume the same
contract. The bounded footprint includes presented and canonical visual bounds,
the anchor, and any conservative bump excursion, clipped to finite output
bounds. It must repair SSD pixels above the client, subsurface pixels outside
the root, the old visual group after settlement, and the anchor without
falling back to full-output repaint.

## Lamp deformation

Keep one scalar lifecycle progress `p` in `[0, 1]`; restore samples the same
representation backwards. Direction is one of Top, Right, Bottom, or Left,
selected from the nearest output edge to the frozen anchor, with source-to-
anchor displacement as the deterministic fallback for ambiguous anchors.

A documented constant set defines the default duration, initial shape factor,
stage weights, easing, endpoint opacity interval, and optional overlap bump.
The pure stage channels are derived from `p`:

- `bump_progress`, only when the window crosses/overlaps the Dock along the
  movement axis;
- `stretch_progress`, weighted approximately `0.7 * shape_factor`;
- `squash_progress`, the dominant final stage.

Normalized continuous stage boundaries feed one CPU reference warp function.
The directional model uses axis rotation so all four directions are the same
deformation, with the near edge delayed spatially less than the far edge,
progressive neck formation, and exact normalized convergence into the anchor.
The GLSL implementation mirrors the CPU equations. The CPU tests are the
behavioral authority for identity, endpoint mapping, finiteness, monotonic
movement, continuity, rotational equivalence, reversal continuity, and small
or extreme geometry.

Opacity stays at full strength through almost all of the transition and only
collapses in a narrow final endpoint interval. The settled minimized endpoint
is fully invisible, and restore remains symmetric.

## Mesh and renderer integration

Retain a static/bounded Lamp mesh and progress-only uniform updates. Target
approximately 30–36 physical pixels per cell, raise per-axis caps so the
target is attainable on a 1920-pixel visual group, and enforce the existing
global `MAX_LAMP_VERTICES` budget with deterministic coarsening for concurrent
windows. Client/subsurface render commands and SSD primitives use the same
visual-group transform. The Dock and cursor remain outside the warped group,
and LegacyScene and EffectGraph ordering stays unchanged.

## Verification strategy

First add and run a deterministic RED regression that constructs a client
rectangle beginning below an SSD outer rectangle and proves the existing
client-only footprint omits the titlebar. If that regression does not fail,
stop and trace the real ownership boundary before changing production code.

Then implement focused red-green tests covering visual bounds, presentation
affines, frozen reversal behavior, damage and clipping, CPU warp mathematics,
mesh bounds/reuse, decoration command retention, and lifecycle visibility
eligibility. Re-run the existing stale-ACK, physical-settlement, suppression,
disable, teardown, Direct Scanout, Wayland, and managed XWayland regressions.
Run the requested focused filters and full formatting/checking/clippy/test
commands using the repository's existing `target` directory. Do not alter
KMS or pacing code unless a deterministic test proves an independent defect.

Native visual acceptance is reported separately and is only claimed if run on
the 1920x1080@165 Hz setup with Blur disabled. The known Lamp plus Blur /
`ResolvedOwnedEffects` invisibility issue remains out of scope.
