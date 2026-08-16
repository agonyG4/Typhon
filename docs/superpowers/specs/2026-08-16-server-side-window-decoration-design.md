# Typhon Server-Side Window Decorations v1

## Scope and current state

Typhon currently owns XDG/XWayland surface placement, client-content geometry,
focus, stacking, compositor-owned move/resize interactions, and exact-window
close/minimize actions. `zxdg_decoration_manager_v1` is advertised, but the
current implementation always configures `ClientSide` and does not retain a
per-toplevel preference. Native rendering has a CPU scene path and a GLES
scene-command path; both currently render client surfaces, wallpaper, cursor,
and XWayland backing only. Existing frame colors and the 6 px frame action are
compatibility scaffolding, not an SSD implementation.

This design adds native compositor-owned decorations for participating XDG
toplevels and managed normal X11 windows while preserving existing client
geometry and input state machines. Fullscreen hides visible decorations, and
CSD windows retain zero server extents.

## Architecture

The implementation is split into five focused decoration units:

* `types.rs` contains stable mode, extent, button, state, hit, and layout
  concepts.
* `layout.rs` is pure geometry. It calculates outer frame, client, titlebar,
  button visual/input, text-safe, visible-border, and resize-only regions from
  logical window state, metrics, and output scale.
* `theme.rs` validates bounded declarative JSON, resolves package-relative
  assets, constructs immutable snapshots, applies state fallbacks, and exposes
  the built-in MacTahoe-Dark theme. Theme files cannot execute code, load
  external URLs, or escape their package root.
* `render_plan.rs` converts state, layout, metadata, and a snapshot into
  renderer-independent solid/image/text primitives. CPU and GLES consume this
  same logical plan; neither parses JSON, SVG, or discovers fonts in the frame
  loop.
* `state/window_decoration.rs` owns per-window effective mode, hover/press
  capture, titlebar double-click tracking, and theme-generation linkage. It
  delegates movement and resize to the existing compositor interaction APIs.

The compositor remains the only owner of window-management semantics. Themes
contain colors, metrics, font/text appearance, and image assets only. Button
placement is a v1 product invariant: right side, from left to right,
Minimize, Maximize/Restore, Close.

## Geometry and rendering contract

Decoration metrics are logical pixels and are rounded through one deterministic
scale helper used by layout, hit testing, and rendering. Floating SSD adds the
theme titlebar and visible border around client content. The invisible resize
target remains the existing approximately 6 logical px region and is not made
into a visible border. Maximized windows keep the titlebar but fit the usable
output/work area; fullscreen and CSD expose zero visible server extents.

XDG surface window geometry remains client-content metadata. Decoration frame
geometry is compositor-owned and is not folded into the client configured
height. X11 continues to use its existing client/frame geometry split.

The shared render plan contains final logical geometry, RGBA colors, selected
asset state, title-safe bounds, and clipping. CPU composition draws its
primitives into the existing scene; GLES emits equivalent solid/image/text
commands and caches scale/state-specific resources. Scene signatures include
decoration generation/state so stale titlebars cannot survive partial repaint.

## XDG negotiation and lifecycle

The decoration object is tracked per XDG root surface. `set_mode(ClientSide)`
and `set_mode(ServerSide)` become the requested preference; `unset_mode` returns
to undefined. Undefined defaults to SSD only after a decoration object exists.
Clients that never create a decoration object remain CSD/no-SSD in v1. The
effective visible mode is independently suppressed by fullscreen. Duplicate
decoration objects are protocol errors, and destruction/unmap/remap/client
disconnect remove state without leaving references to dead resources.

## XWayland policy

SSD applies only to managed normal X11 application windows. Override-redirect,
desktop, dock/panel, splash, notification, menu, tooltip, popup, and other
special windows remain undecorated. Fullscreen and explicit Motif no-decoration
requests suppress the frame. `_NET_FRAME_EXTENTS` is published from the same
decoration extents used by frame/client conversion and is refreshed on mode,
theme, maximize, fullscreen, and removal changes.

## Input and actions

Decoration hit testing runs before client dispatch for SSD windows. Empty
titlebar presses focus/raise the exact `WindowId` and begin the existing
compositor-owned move interaction. Buttons capture the exact target and only
act on release over the same button; leaving the button cancels activation.
Destroy/unmap clears capture. Hover damages only old/new button regions and
never fabricates client pointer motion. Empty-titlebar double-click toggles
the exact target between normal and maximized through an exact-window API.
Resize edges retain precedence over decoration visuals.

## Theme schema, security, and persistence

Schema v1 is JSON with `schema_version: 1`, `deny_unknown_fields`, bounded
metrics, validated colors, required base assets, optional-state fallback, and
relative package asset paths. JSON is capped at 64 KiB; source assets at 256
KiB; raster cache entries at 128x128 physical pixels unless a future scale
policy changes that bound. Absolute paths, `..` traversal, symlink escapes,
external URLs, and SVG external resource loading are rejected.

Selection is stored independently from Eclipse at
`$XDG_CONFIG_HOME/AstreaOS/ui/window-decoration.json`, or the normal HOME
fallback. Theme packages are searched in bounded XDG data, HOME fallback,
`/usr/local/share/AstreaOS/decorations`, and `/usr/share/AstreaOS/decorations`
locations. Persistence uses the repository's descriptor-relative, atomic
configuration posture. Loading publishes an immutable snapshot only after all
validation and resource preparation succeeds. Reload failure keeps the
last-known-good snapshot and records the error. A generation increments only
after successful activation.

The shipped first theme is `MacTahoe-Dark`: 32 px titlebar, 16 px artwork,
9 px spacing, 12 px right padding, centered title, and the supplied
MacTahoe-derived traffic-light colors. Asset provenance and licensing are
documented next to the package; no executable plugin or script mechanism is
introduced.

## Control plane and Eclipse boundary

The local control plane adds `astreactl decoration status`,
`astreactl decoration set-theme <name>`, and `astreactl decoration reload`.
`set-theme` validates and prepares before switching or persisting. The
compositor remains functional when Eclipse, QML, Qt, or the shell is absent.
Eclipse may call the control API in the future, but it is not a runtime owner.

## Damage, Direct Scanout, and limitations

Focus, hover, press/release, title, resize, mode, maximize/restore, fullscreen,
theme, and scale changes advance scene decoration state and damage the narrow
affected regions. A visible SSD is compositor content and blocks Direct
Scanout for a normal window; protocol fullscreen hides SSD and preserves the
existing solitary-fullscreen eligibility path.

V1 intentionally excludes blur/acrylic, animation, executable or scripted
themes, arbitrary button ordering, left-side layouts, context menus, and a
general typography/effects framework. Shadows and richer assets can be added
later without changing frame ownership or negotiation boundaries.
