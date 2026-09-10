# Astrea Animation Control Plane v1

## Purpose

Typhon owns the authoritative animation configuration, catalog, runtime
resolution, persistence, and control-socket representation. The first version
creates stable semantic identities for future animation implementations while
preserving the already-qualified KDE-style geometry behavior.

The animation catalog is intentionally separate from the trusted GPU effect
registry. The catalog answers which semantic animation should occur; the GPU
registry answers how trusted rendering programs execute. A future animation
may use the GPU registry internally without merging the two abstractions.

## Catalog and resolution

Slots use stable wire IDs: `window.move`, `window.resize`, `layout.reflow`,
`window.maximize`, `window.fullscreen`, `window.open`, `window.close`,
`window.minimize`, `window.restore`, `workspace.switch`, and
`workspace.window-move`.

Effects use stable IDs: `none`, `geometry.kde`, `geometry.macos`,
`window.scale`, `window.glide`, `minimize.lamp`, `minimize.squash`, and
`workspace.slide`. Only `none`, `geometry.kde`, and `geometry.macos` are
available in v1. The other IDs are explicitly planned and cannot be selected
by a manual override.

Presets are `astrea`, `kde`, and `macos`. KDE and macOS select their respective
geometry effect for the five existing geometry slots and `none` elsewhere.
Astrea selects KDE geometry and requests `minimize.lamp` for minimize and
restore. Because Lamp is planned, its effective effect is `none` until the
same catalog entry becomes available. This allows Lamp to activate later
without a configuration migration.

Configuration stores user intent only:

```json
{
  "version": 1,
  "enabled": true,
  "preset": "astrea",
  "speed": 1.0,
  "overrides": {}
}
```

Requested and effective resolution are kept distinct in every control
snapshot. Overrides are keyed by slot ID, inherit from the selected preset
when absent, and are preserved when changing presets. Resetting overrides is a
separate operation.

## Runtime policy

The semantic runtime policy resolves typed slot/effect values before entering
the presentation hot path. Geometry operations map to the existing
`PresentationAnimationPolicy` tables; their constants are not duplicated or
retuned. `PresentationAnimator` remains a generic sampler and does not know
about catalog IDs.

At speed 1.0 the selected curve is exactly the current curve. Easing duration
is divided by speed with a non-zero clamp. Spring stiffness is multiplied by
speed squared and damping by speed; settlement displacement is unchanged and
velocity tolerance follows the same time scaling. Preset, speed, and override
changes affect new transitions only. A transition retains the curve captured
when it started, including during geometry retargeting. Disabling animations
uses the animator’s existing cancellation path and does not rewrite physical
presentation history.

Legacy `OBLIVION_ONE_ANIMATIONS` and `OBLIVION_ONE_ANIMATION_STYLE` values are
read as startup compatibility overrides after persisted configuration. They
are not written back as the primary configuration. A runtime control mutation
supersedes them for the running compositor; the environment is evaluated again
on restart.

## Persistence and control

Typhon persists the bounded, versioned document at
`$XDG_CONFIG_HOME/AstreaOS/typhon/animations.json`, with the normal
`$HOME/.config` fallback. Reads validate schema, size, ownership, and values;
malformed or unsupported data falls back safely to built-in defaults. Writes
use the existing secure Astrea persistence conventions, a private temporary
file, `fsync`, atomic replacement, and directory synchronization.

`animation.config.get` returns a bounded deterministic snapshot containing
generation, source metadata, the user configuration, requested/effective
effect maps, and catalog capabilities. `animation.config.set` accepts one
complete validated user configuration. The transaction is validate, persist,
publish runtime state, increment generation once, and return the authoritative
snapshot. A persistence failure leaves both runtime and generation unchanged.

The existing protocol name/version and request/response limits remain in force.
`astreactl animation get` and a compact full-configuration set command use the
same control commands and decode the same snapshot; the CLI is a diagnostic
surface, not a second configuration owner.

## Dock anchor contract

The authenticated private Astrea toplevel protocol is extended additively to
version 3 with `set_minimize_anchor` and `clear_minimize_anchor` on the exact
toplevel handle. The rectangle is compositor-global logical geometry, allows
negative coordinates for outputs left or above the origin, and has bounded
positive dimensions. Anchor state is keyed by exact `WindowId` and records the
publishing client and resource. Resource teardown clears an anchor only when
that resource still owns it; window teardown always removes it. Anchors are
live metadata and are never persisted or consumed by v1 rendering.

The next Lamp implementation will combine this destination with the existing
`WindowState::minimized_surfaces` retention. This version deliberately leaves
minimize and restore behavior unchanged.

## Verification boundary

The Typhon tests cover catalog resolution, speed mathematics, strict
configuration/persistence transactions, bounded control snapshots, and v3
anchor ownership. Existing geometry tests remain the regression authority for
the qualified KDE and macOS curves. No Lamp deformation, mesh renderer,
workspace animation, or new GPU effect is part of this design.
