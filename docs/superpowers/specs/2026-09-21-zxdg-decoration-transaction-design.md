# Typhon zxdg-decoration Transactional State Design

## Problem and verified root cause

Typhon currently stores only one decoration value per xdg toplevel. The
`set_mode` and `unset_mode` requests overwrite that value, advance the visible
render generation, and send a decoration configure immediately. Rendering and
hit testing then derive the visible server-side frame from the same value.

The existing xdg lifecycle tracks configure serials, but its records contain no
decoration payload. Consequently, the decoration mode is not owned by the
xdg configure serial that the client acknowledges and commits. A client can
therefore observe the decoration event, while Typhon renders the new frame
before the matching `xdg_surface.ack_configure` and `wl_surface.commit`.

This explains the native Zen/OBS symptom without any XWayland or scene-node
deduplication issue: the compositor's chrome state is published on receipt of
the preference request instead of at the client content transaction boundary.

## Invariants

For xdg toplevels with a decoration object, three concepts remain distinct:

1. `preference`: the latest client request (`Unset`, `ClientSide`, or
   `ServerSide`).
2. `configured`: the effective decoration mode stored in one exact
   `XdgConfigureRecord`, alongside the serial sent by `xdg_surface.configure`.
3. `applied`: the mode currently used by native rendering and decoration hit
   testing.

`set_mode`, `unset_mode`, and decoration object creation/destruction may alter
the preference or queue a configure, but they do not alter `applied` or the
visible scene. Acknowledging a serial selects that record. The next accepted
surface commit consumes the latest acknowledged record and applies its
decoration payload. If multiple configures are outstanding, the latest
acknowledged configure before the commit wins, matching the existing xdg
configure lifecycle.

The renderer, hit testing, and native decoration layout use only `applied`.
An applied mode transition invalidates visible rendering once; a preference
request by itself does not invalidate the visible scene.

## Architecture

`XdgConfigureRecord` gains an optional typed decoration payload. The existing
toplevel configure sender becomes the single owner of configure sequencing:
it sends the decoration configure first when an object exists, sends the
toplevel configure, sends `xdg_surface.configure`, and records the effective
decoration mode under the resulting serial. Initial configure and later
geometry/state configures use the same path.

`XdgSurfaceLifecycle` retains the selected acknowledged record until the next
surface commit. The surface-tree publication path applies that record before
publishing the commit's visual content. It consumes no decoration state when a
configure has not been acknowledged, and it does not advance visible
generation for a request that has not become applied.

The decoration state remains associated with the xdg surface even briefly after
a v1 decoration object is destroyed. The object resource disappears
immediately, but the current applied mode remains visible until a subsequent
commit applies the client-side transition. Recreation is accepted only where
the advertised v1 protocol permits it; mapped v1 surfaces with committed
content reject recreation with the protocol's unconfigured-buffer error.

Fullscreen uses internal `DecorationMode::None` in the configured/applied
state, while the wire event is represented as `server_side`, matching the
reference compositor behavior so clients do not infer that they should draw
CSD merely because compositor chrome is suppressed for fullscreen.

Repeated identical preference requests are ignored. Independent toplevel state
changes still send their normal configure sequence, including the decoration
payload current for that transaction.

## Testing strategy

The registry test state will record decoration configure modes, xdg configure
serials, event ordering, and optional suppression of automatic ack/commit. New
integration tests will drive real client protocol objects and assert:

- initial decoration configure precedes the initial toplevel and xdg surface
  configures and produces one mapped visual;
- client-side/server-side transitions and `unset_mode` remain visually stable
  until ack plus commit, then change once;
- repeated requests do not emit unbounded configures or render generations;
- multiple outstanding serials apply only the newest acknowledged transaction;
- destroy follows next-commit semantics and does not expose a transient mixed
  CSD/SSD state;
- close input remains owned by the exact associated `WindowId`;
- fullscreen wire and visual modes are coherent across both transitions.

Existing native decoration, move, resize, focus, stacking, close, maximize,
tiling, animation, and XWayland tests remain part of the focused and full
verification runs.

