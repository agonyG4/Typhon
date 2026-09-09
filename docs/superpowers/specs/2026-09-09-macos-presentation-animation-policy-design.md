# macOS Presentation Animation Policy Design

## Problem

Typhon already has a presentation-animation engine that samples one
window/toplevel-space transform per native frame, preserves retarget velocity,
and acknowledges a transition only after the physically presented frame settles.
The geometry entry point currently selects one spring directly, so compositor
paths cannot express whether a change is a programmatic resize, a tiled reflow,
a mode transition, or a managed XWayland transition.

This design adds the first macOS-style geometry preset without changing the
presentation engine, canonical geometry ownership, frame membership, input
authority, fullscreen culling, or direct-scanout rules.

## Decisions

### One policy boundary

`src/presentation_animation.rs` will own:

- `PresentationAnimationKind`, with the eight semantic geometry categories in
  the task brief;
- `PresentationAnimationStyle`, initially `Macos`;
- `PresentationAnimationPolicy`, which resolves a kind to its `AnimationCurve`;
- parsing for `OBLIVION_ONE_ANIMATION_STYLE`.

The macOS spring table is kept in one match expression behind
`PresentationAnimationPolicy::curve_for`. `default` aliases `macos`; an
unknown value emits a diagnostic and falls back to the macOS policy. The
existing `OBLIVION_ONE_ANIMATIONS=on|off` parser remains unchanged and is
still owned by `PresentationAnimator`.

`PresentationAnimator` remains a generic sampler/executor. It will not inspect
animation kinds, compositor modes, layout membership, or environment style.
`CompositorState` stores one resolved policy, so environment lookup happens at
state construction rather than on every transition.

### Explicit entry-point semantics

`CompositorState::animate_toplevel_visual_geometry` will take a
`PresentationAnimationKind` and obtain its curve from the stored policy. The
existing order is preserved:

1. resolve the batch or monotonic presentation time;
2. cancel when pointer-owned move/resize is active;
3. resolve presentation-space start and target rectangles;
4. retarget from the current sample when a transition is pending, otherwise
   start from the previous geometry.

No transition is sampled more than once by this change, and transition IDs,
velocity continuity, physical settlement, and hidden-transition dormancy stay
in the existing animator and frame-planning code.

### Installer routing

The visual-geometry installers will gain explicit animation-capable variants.
Their existing non-animated wrapper remains available for setup, client-driven
geometry, output-size reconciliation, and tests that only install a visual
fixture. A geometry path opts in by passing `Some(kind)`; an opt-out cancels a
stale transition for the root after changing the temporary visual geometry.

Routing is:

- compositor-requested floating resize: `ProgrammaticResize`;
- non-interactive managed XWayland geometry changes: `ProgrammaticMove` for a
  pure position change and `ProgrammaticResize` when size changes;
- tiled/Dwindle solved geometry: `LayoutReflow`, using the existing shared
  `layout_animation_epoch`;
- Wayland maximize enter/exit: `MaximizeEnter`/`MaximizeExit`;
- Wayland fullscreen enter/exit: `FullscreenEnter`/`FullscreenExit`;
- managed XWayland mode transitions: `XwaylandModeChange`;
- initial, output-reconfigure, client/setup, and pointer-preview paths: no
  policy animation.

The current compositor API has no independent XDG floating-move operation;
workspace moves remain workspace membership changes, and pointer movement stays
interactive. The programmatic-move category is therefore applied only where a
non-interactive managed geometry path actually changes a floating frame. This
keeps the category explicit without inventing a new movement API in this task.

Mode selection happens before mutating the mode. Restore paths use the previous
mode to choose the matching exit kind. If restore requires a tiled solve, the
solve owns the visual transition as `LayoutReflow`.

### Environment compatibility

The selector accepts:

- unset, empty, `default`, and `macos`: the macOS policy;
- any other value: a diagnostic plus the macOS fallback.

No configuration file, UI, per-kind tuning, animation thread, timer, or new
retained-lifecycle behavior is introduced.

### Invariants intentionally untouched

This change does not alter native-frame presentation membership, the separation
between stack roots and presentation owners, physical pageflip authority,
fullscreen-entry culling, exact transition acknowledgement, hidden transition
dormancy, direct-scanout blockers, shared SHM ownership, canonical
`RenderableSurface` geometry, or client configure/commit ownership.

Minimize, close, unmap/destroy, workspace-switch animation, opacity, blur, and
retained disappearing-window effects remain out of scope.

## Verification strategy

Tests will cover:

- all eight macOS policy curves and style aliases;
- explicit kind selection at the animation entry point;
- mode enter/exit, tiled reflow, XWayland mode, and programmatic geometry
  routing;
- pointer move/resize cancellation and takeover;
- existing fullscreen membership/settlement, hidden-transition, and
  direct-scanout behavior through the current regression suites.

The normal `target/` directory and `rtk` command wrapper will be reused. Final
verification will run formatting, locked all-target checking, locked clippy,
locked tests, diff whitespace checks, and source-layout validation. Known
oversized-module debt and any failures caused by unrelated in-progress
renderer/effects changes will be reported separately.
