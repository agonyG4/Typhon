# Typhon X11 Iconic Toplevel Lifetime and Restore Closure

## Goal

Keep the Astrea task identity of an admitted managed X11 window attached to its persistent `DesktopWindow`/`WindowId` across Iconic minimization, Xwayland association loss, and surface replacement, while preserving real withdrawal and destruction teardown and making Restore/Activate remap the X11 window through the backend boundary.

## Scope and invariants

- The change is limited to Typhon compositor X11/Xwayland lifecycle, Astrea toplevel publication, and exact Restore/Activate behavior.
- Eclipse, XDG publication semantics, viewport/damage work, and application-specific behavior are out of scope.
- `XWM` remains authoritative for the distinction between a WM-owned Iconic unmap and a real client withdrawal.
- `DesktopWindow` lifetime owns X11 task identity; a Wayland surface attachment owns only the current renderable content epoch.
- No stale or synthetic Wayland surface is retained to keep an Astrea task alive.

## Design

### Publication eligibility

`CompositorState::astrea_toplevel_kind_if_eligible` will keep its current XDG branch. The X11 branch will continue to admit only `DesktopWindowKind::Managed` windows with `X11DesktopRole::Toplevel` or `X11DesktopRole::Dialog`, but will stop consulting `x11_surface_id` and `surface_resource_by_id`. An associationless Iconic window therefore remains in the collection and publishes `MINIMIZED` from its persistent `WindowState`. Override-redirect, auxiliary/client-leader, excluded notification/pop-up, withdrawn, destroyed, and stale-generation windows remain excluded by their existing admission/removal and role policy.

### Association and replacement

`XwmAssociationEvent::Removed` will continue to retire presentation resources and clear the map-local `x11_surface_id`. It will not close the Astrea handle by itself. Existing `Associated` handling will attach a replacement surface to the same `DesktopWindow`; no new `WindowId` or Astrea handle is created.

### Restore/remap backend boundary

Add a narrow `WindowBackendCommand::Map { window: WindowId }`. Restoring an X11 window with no current attachment queues this command from compositor state. `OwnCompositorServer::take_xwayland_backend_commands` translates it to `XwmCommand::Map(handle)`, and the XWM command path marks the existing Iconic X11 record as map-requested before applying its normal map-command deduplication and X11 map operation. The compositor does not call XWM internals or recreate a `DesktopWindow`.

### Activation after attachment loss

An exact Activate on a minimized, associationless managed X11 window restores its compositor state and requests the backend map. Because there is no Wayland surface to focus synchronously, it records one bounded pending activation intent containing the persistent `WindowId` and its current `X11WindowHandle`. When the same handle receives a replacement association, the compositor attaches the surface and attempts the normal desktop focus/raise path; successful focus consumes the intent. Withdrawal, destruction, generation loss, or removal of the target `DesktopWindow` cancels it. A surface removal between map epochs does not cancel it while the persistent X11 window remains alive.

## Test design

Tests will be added before production changes and run in RED/GREEN cycles:

1. At the Astrea protocol boundary, minimize a managed X11 window, apply the real association-removal path, service publication, and assert that the same handle, identifier, kind, manager total, and `MINIMIZED` state remain while `x11_surface_id` is `None` and no close/reannounce occurs.
2. Apply a genuine `XwmEvent::WindowWithdrawn` and assert `DesktopWindow` removal and Astrea handle closure. Existing destruction behavior will remain covered and be run adjacent to the new test.
3. Remove association A and attach association B for the same X11 handle; assert stable `WindowId`/Astrea handle and no duplicate announcement.
4. Invoke exact Restore from an associationless Iconic state and assert `Accepted`, stable identity, and a translated X11 map command; then attach the replacement surface and assert non-minimized state.
5. Invoke exact Activate from the same state and assert it is accepted, queues remap, and eventually focuses only the intended replacement map. Add cancellation assertions for withdrawal/generation loss where the pending intent is exercised.
6. Extend rather than rely on `authorized_v2_exact_managed_x11_actions_complete_on_the_manager`; the asynchronous unmap, Iconic, association removal, and replacement sequence will be explicitly represented.
7. Run the existing XWM Iconic/withdrawal regressions, compositor Xwayland lifecycle tests, Astrea publication/action tests, formatting, locked all-target checks, diff checks, and the full suite when the environment permits.

## Non-goals

- No changes to Eclipse.
- No Steam/Proton/Wine/title/process conditions.
- No changes to `consume_wm_unmap` semantics.
- No generic focus queue or broad refactor.
