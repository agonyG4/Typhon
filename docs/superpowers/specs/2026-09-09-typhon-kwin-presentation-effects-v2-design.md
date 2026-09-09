# Typhon KWin-Inspired Presentation Effects v2 Design

**Date:** 2026-09-09

**Status:** Approved for implementation

## Goal

Correct the geometry-authority and interaction-interruption defects exposed by
165 Hz hardware feedback, then add a short fixed-duration KDE-style geometry
policy while preserving Typhon's accepted presentation architecture.

## Observed failures and root causes

The current XDG, tiled, and managed XWayland paths mutate canonical placement
before `install_*_visual_geometry_with_animation` discovers its source by
querying mutable state. That makes the target placement available as a false
source and can produce a fullscreen or maximize transition that begins at
`(0, 0)` instead of the real pre-transition rectangle. The tiled path already
computes the correct pre-mutation geometry but discards it before installation.

The current interaction takeover also cancels a transition and installs the
physically presented rectangle, including its animated width and height, as
canonical `ToplevelVisualGeometry`. A frame-local scale can therefore rebuild
SSD decoration metrics in a different coordinate system while the client and
decoration were previously transformed as one visual group.

Finally, maximized move is currently accepted as if a maximized window could
own arbitrary floating placement. That leaves mode and geometry authorities
in conflict during pointer interaction.

## Architecture

### Explicit transition source

Introduce an explicit transition contract, with equivalent naming allowed by
the implementation:

```rust
enum VisualGeometryTransition {
    Immediate,
    Animated {
        source: WindowGeometry,
        kind: PresentationAnimationKind,
    },
}
```

The XDG and X11 visual installers consume this contract. They never query
current visual or root geometry to infer an animated source. Every animated
caller captures its source before the first canonical target mutation:

```text
source capture -> backend/configure/mode/placement mutation -> target install
```

XDG mode entry and restore retain restore-state bookkeeping as a separate
concept from animation source geometry. Tiled reflow passes its already-solved
`current` geometry into the installer. Managed XWayland captures its frame
geometry before changing frame and placement and maps mode changes to the same
semantic maximize/fullscreen kinds used by XDG.

### Presentation visual group

Keep `WindowVisualGroup`, `PresentationGroupTransform`, and
`DecorationRenderInstance::with_presentation_transform(...)` as the only
visual-group ownership model. Client root, descendants, SSD decoration, and
buttons continue to receive one frame-local transform. No KWin scene graph or
independent decoration animation track is introduced.

### Interaction interruption

Replace the broad takeover behavior with a narrowly named origin-rebase
operation. On normal floating move or resize, it may use the physically
presented origin for locality and pointer anchoring, but it retains canonical
width and height. It cancels the old presentation transition and leaves direct
pointer geometry as the sole owner. Any size snap during a size-changing
handoff is preferable to corrupting canonical decoration/window ownership.

Tiled resize continues to rebase the existing `TiledResizeHandle` from the
physically presented client edge, then cancels the presentation transition.
The Dwindle tree and solved layout remain canonical authority.

Maximized move uses a dedicated immediate interactive restore helper that
shares canonical mode/configure/state work with programmatic restore. It reads
the stored restore geometry without consuming it until the restore succeeds,
captures the physically presented rectangle and pointer anchor, changes mode to
`Normal`, positions the restore rectangle under the pointer, installs it
without animation, and starts direct move. Maximized resize and fullscreen
move/resize are rejected.

### Timing policy

Add `PresentationAnimationStyle::Kde` and cubic easing variants with analytic
values and derivatives. `Kde` is the default for `unset`, `default`, and
`kde`; explicit `macos` retains the current spring family. The global
`OBLIVION_ONE_ANIMATIONS=on|off` switch remains unchanged.

KDE geometry policy:

| Kind | Duration | Curve | Classification |
| --- | ---: | --- | --- |
| ProgrammaticMove | 160 ms | EaseOutCubic | Typhon adaptation |
| ProgrammaticResize | 200 ms | EaseOutCubic | Typhon adaptation |
| LayoutReflow | 200 ms | EaseOutCubic | Typhon adaptation |
| MaximizeEnter | 250 ms | EaseOutCubic | Directly aligned with observed KWin maximize timing |
| MaximizeExit | 250 ms | EaseOutCubic | Directly aligned with observed KWin maximize timing family |
| FullscreenEnter | 250 ms | EaseOutCubic | Typhon adaptation |
| FullscreenExit | 250 ms | EaseOutCubic | Typhon adaptation |

`XwaylandModeChange` is removed if semantic routing makes it unused. No new
multi-track effect engine is required; Typhon's existing window-rectangle
mapping expresses size plus translation.

Fixed-duration completion still samples the exact target, marks the
transition mathematically settled, and waits for the existing pageflip and
`TransitionId` acknowledgement before retirement.

## Testing strategy

Write RED tests through real compositor mode/layout/interaction entry points
before production fixes. Cover source authority for XDG maximize/fullscreen,
restore, Dwindle reflow, and managed XWayland; assert a non-zero source does
not become `(0, 0)` at transition start. Add cubic endpoint, midpoint,
derivative, and monotonicity tests; policy default/alias/macOS compatibility
tests; and real-path policy routing tests.

Add visual-group coherence coverage proving root, subsurface, SSD titlebar,
and buttons share one presentation transform, including identity behavior.
Add real interruption coverage for translation-only floating move,
size-changing decorated titlebar drag, tiled resize, and maximized titlebar
drag. Assert canonical width/height and SSD metrics remain coherent, direct
interaction owns subsequent geometry, maximized drag becomes `Normal`, and
unsupported maximized/fullscreen interactions are rejected.

The existing presentation-foundation tests for physical authority, hidden
dormancy, fullscreen coverage, direct scanout, frame membership, pageflip
settlement, popup ownership, and tiled ratio authority remain required.

## Scope boundaries

This task adapts KWin's observed window-item ownership, pre-change maximize
geometry capture, short cubic maximize timing, and cancellation-on-unexpected
geometry-change behavior. It does not copy GPL implementation code or create
a KWin `WindowItem` graph.

It deliberately does not implement previous-content crossfade, Scale, Glide,
Squash, open/close/minimize/workspace lifecycle effects, partial maximize
states, or a new retained-content lifetime mechanism. Eclipse remains
untouched. Hardware qualification is reported only if actually performed.
