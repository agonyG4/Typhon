# Typhon Workspace Runtime v1 — Implementation Report

Date: 2026-08-20

## Outcome

Implemented Typhon Workspace Runtime v1 in the current checkout. The implementation adds runtime workspace switching and focused-family moves while keeping workspace changes as scene-visibility state. Hidden windows remain mapped, alive, and stateful; switching does not mutate geometry, layout membership, configure state, or client buffers.

The requested report is complete, but the full test suite has an environment-only failure cluster described below. No commit or staging operation was performed.

## Implemented

- Added typed `WorkspaceId`, workspace manager activation outcomes, EWMH conversion, and layout-preserving workspace membership.
- Added native Super+number workspace switching and Super+Shift+number focused-family moves for workspaces 1–10, with typed actions, no-repeat behavior, inhibition handling, and preserved session-command modifier forwarding.
- Added one-generation workspace switch/move causes with no generation for no-op activation.
- Centralized workspace visibility for scene rendering, hit testing, focus, decorations, fullscreen composition, idle inhibition, and pointer targeting. Layer-shell surfaces remain global.
- Kept hidden desktop windows mapped and retained their geometry, buffers, lifecycle, and global registry entries. Hidden frame callbacks remain pending until their surface is visible; they are not presented while hidden.
- Added focus restoration using visible managed windows’ focus serial and stacking order. Hidden, minimized, auxiliary, and unmapped windows are excluded.
- Made inactive authorized activation navigate to the target workspace before focusing, while initial transient mapping inherits workspace membership without navigating/focusing.
- Added XDG/X11 transient and parent workspace inheritance, parent-removal preservation, and family moves that canonicalize from a focused transient to the top-level family root. Family moves cancel active interaction, constraints, implicit grabs, popup grabs, held buttons, and drag sessions.
- Preserved fullscreen mode across workspace changes while excluding inactive owners/popups from the active scene and direct scanout decisions.
- Added typed XWayland `_NET_CURRENT_DESKTOP`, `_NET_WM_DESKTOP`, and desktop-state publication through the compositor backend command boundary. Invalid desktop values are rejected; hidden X11 windows are not unmapped.
- Added XWayland root EWMH properties for ten desktops, zero-based current desktop, geometry, viewport, and workarea.
- Added control/snapshot workspace state while reporting mapped status unchanged.
- Added focused domain, scene-visibility, workspace-family, callback, native-binding, XWayland, fullscreen, and geometry-regression tests.

The implementation deliberately excludes layout-engine changes, tiled geometry changes, chrome/animation changes, Spatial Canvas behavior, and output coupling.

## Verification

Passed:

- `rtk cargo fmt -- --check`
- `git diff --check`
- `rtk cargo test --locked --no-run`
- Desktop/window behavior: 62 passed
- Native input bindings: 2 passed
- XWayland event normalization: 64 passed
- Workspace domain tests: 13 passed
- Surface-frame tests: 40 passed
- Direct-scanout tests: 3 passed
- Resize-visual tests: 10 passed

Full locked suite:

```text
1684 passed; 35 failed; 2 ignored
```

All 35 failures are environment/setup failures reporting `path must be shorter than SUN_LEN` while creating Unix-socket paths in ASTREACTL/XWayland startup tests. Several subsequent XWayland tests report `PoisonError` from the shared test lock after the initial setup failure. The focused workspace/runtime suites passed, and no feature assertion failure was observed. The complete run is recorded by RTK at `~/.local/share/rtk/tee/1787250448_cargo_test.log`.

## Review and evidence

Two independent read-only implementation reviews were completed. Both reviewers identified the same transient-family-root and transition-teardown risks; the additional review findings about generic XDG focus fallback, inactive fullscreen popups, and inherited X11 desktop publication were also addressed.

The codebase-memory coverage audit was refreshed after the final changes. All 38 operated feature paths reported `no_recorded_issue` with matching metadata in generation `2026-08-20T18:26:15Z`. This is a best-effort index signal and does not replace source-level verification.

## Key design references

- `docs/superpowers/specs/2026-08-20-typhon-workspace-runtime-v1-design.md`
- `docs/superpowers/plans/2026-08-20-typhon-workspace-runtime-v1.md`

