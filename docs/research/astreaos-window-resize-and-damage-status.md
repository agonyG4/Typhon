# AstreaOS Window Resize and Damage Status

Date: 2026-06-15

## Current status

Window shadows are disabled in active compositor rendering. The temporary
resize visual is deliberately plain: a neutral preview backdrop plus a one
logical-pixel outline. Shadow extents do not affect hit testing, scene bounds,
damage, CPU rendering, GLES command generation, or native output damage.

Interactive resize separates compositor visual geometry from committed client
content. Pointer motion updates the visual target immediately, queues the latest
configure target, and advances compositor render generation without changing the
client surface generation. Slow clients can lag in content freshness, but they
do not block the outer resize target.

The CPU and GLES renderers share the resize render plan. Growing a window keeps
stale content at committed size and exposes preview backdrop. Shrinking crops
the sampled source region according to the grabbed edge instead of scaling the
stale buffer down.

Native output damage covers old and new visible bounds for move, resize, and
surface commits that change logical bounds. Surface commit damage is combined
with bounds-change damage so browser commits cannot leave the previous window
rectangle behind.

## Verified by tests

- Visual resize target updates before client commit.
- A previous pending resize commit does not block the next configure.
- Configure delivery remains coalesced through the existing pending-target path.
- ACK does not replace client content.
- Left-edge shrink keeps the opposite edge anchored.
- Min-size constraints clamp preview/configure geometry.
- CPU stale-content shrink crops instead of scaling.
- GLES scene command cache invalidates for resize preview crop/anchor changes.
- Native output damage covers old bounds when a surface commit changes geometry.
- Prototype window shadows do not emit shadow-only pixels.

## Remaining validation

Manual native-session validation is still required for Firefox/Zen, Chromium
family browsers, GTK, Qt, WebKitGTK, hardware cursor, software cursor, and
multiple refresh rates. Delayed-client resize validation should also be added
when the test client infrastructure can intentionally delay commits at 16, 33,
50, 100, and 250 ms.

Future decoration work may reintroduce shadows, rounded-corner masking, blur, or
server-side titlebars, but that must be a separate decoration milestone with
explicit visible-bounds and damage integration.
