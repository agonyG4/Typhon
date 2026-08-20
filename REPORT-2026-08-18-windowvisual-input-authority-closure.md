# Typhon WindowVisual Input Authority Closure Report

Date: 2026-08-18

## Outcome

The focused SSD input-authority closure is implemented in the current dirty
checkout. Rendering and pointer hit testing now share `VisualStackGroup`
ownership. Native move/resize, titlebar actions, and decoration buttons retain
the exact resolved `WindowId`, root surface, client motion surface, or resize
edge instead of falling back to a lower client-only hit.

The requested native session could not complete qualification: the compositor
reached direct DRM/KMS and EGL setup, then the atomic `TEST_ONLY` pre-render
commit failed with `Permission denied`.

## Evidence classification

- **CONFIRMED:** client-only interaction authority was separate from the
  renderer's grouped visual order; decoration button handling re-hit-tested;
  same-window decoration focus re-entered the full focus path.
- **IMPLEMENTED AND TESTED:** shared visual grouping, popup-above-SSD order,
  SSD-above-ordinary-subsurface order, exact move/resize capture, exact button
  capture, same-window focus no-op, idempotent pointer-focus clearing, and a
  deterministic 1,000-iteration SSD/subsurface scene-hit stress loop.
- **NATIVE-UNQUALIFIED:** rendered titlebar/button/resize stress, because the
  native session stopped before a usable frame.
- **UNPROVEN:** exact native protocol event counts and full native presentation
  qualification.

## Implementation

- Added `VisualStackGroup` and `visual_stack_groups()` in
  `src/compositor/render.rs`; `WindowVisualGroup` now adapts the shared
  primitive for decoration rendering.
- Added cached visual-group order and per-position `PointerSceneHit` caching
  to the compositor state. Input traversal is front-to-back by visual group:
  popup clients, normal SSD geometry, then grouped client/subsurface content.
- Routed normal button dispatch through one resolved scene hit. Titlebar move,
  decoration resize, and button activation preserve the resolved owner.
- Replaced client-only move/resize ownership with `PointerSceneHit` branches;
  client-content resize still derives its edge from the resolved root geometry.
- Added a same-window focus no-op when both desktop focus and focused-root
  identity are valid. Empty pointer-focus teardown returns without repeating
  cursor or leave work.
- Kept higher-level popup, implicit, locked, confined, drag, CSD, fullscreen,
  XWayland, resize, and presentation paths in their existing routing domains.

## Focused validation

| Command or test | Result |
| --- | --- |
| `rtk cargo fmt --check` | pass |
| `rtk cargo check --locked --all-targets` | pass |
| `rtk git diff --check` | pass |
| `rtk cargo test --locked window_interaction -- --nocapture` | 64 passed |
| `rtk cargo test --locked pointer_scene_hit -- --nocapture` | 2 passed |
| `rtk cargo test --locked xwayland_pointer_batch -- --nocapture` | 15 passed |
| SSD overlap regression | 1 passed |
| popup/SSD renderer grouping regression | 1 passed |
| window-decoration tests | 15 passed |

The repository-wide `rtk cargo test --locked` run completed with 1,651 passed,
43 failed, and 2 ignored. The failures were outside the focused input closure:
Unix-socket path-length failures in Astrea/XWayland tests, direct-scanout
eligibility, several existing resize-pacing tests, and an XWayland lifecycle
trace retention test. The focused window-interaction, pointer-ordering, and
XWayland pointer-batch suites passed independently.

`rtk cargo clippy --locked --all-targets -- -D warnings` is blocked by the
pre-existing `large_enum_variant` diagnostic for `src/xwayland/xwm/event_types.rs`.
The closure's new `too_many_arguments` diagnostic was addressed locally.

`bash bin/check-source-layout` remains blocked by existing line-count limits in
`src/compositor/state/windows.rs`, `src/compositor/server.rs`, and
`src/compositor/mod.rs`.

## Native qualification attempt

Command:

```text
OBLIVION_ONE_SHELL_COMMAND=/home/agony/GitHub/Eclipse/build/release/Shell/astrea-shell \
ASTREA_COMPOSITOR_BACKEND=typhon TYPHON_XWAYLAND=eager \
./bin/start-oblivion-one-tty
```

Observed: the shell and launcher were present; DRM/KMS devices and the
connected output were found; libseat fell back to direct DRM; then the native
bootstrap failed at the pre-render atomic `TEST_ONLY` commit with `Permission
denied (os error 13)`. No native pointer stress was claimed.

## Working-tree policy

No commit, branch switch, staging, push, or destructive cleanup was performed.
The pre-existing dirty checkout and unrelated changes were preserved.
