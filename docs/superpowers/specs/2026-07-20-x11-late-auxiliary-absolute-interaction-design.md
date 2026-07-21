# X11 Late Auxiliary and Absolute Interaction Design

## Problem

Steam exposes two production failures that are independent but visually
compound each other.

First, Steam maps a tiny client-leader support window before all of its X11
properties have stabilized. Typhon can admit that window as a normal desktop
toplevel before `WM_CLIENT_LEADER` and related support properties arrive.
Later property refreshes update metadata but never reconsider desktop
admission, so popup-family raises can expose the support window as a black,
unmovable surface.

Second, X11 windows enter the compositor with absolute root placement. The
generic move and resize interaction path replaces that placement with a
cascaded-root placement whenever it updates coordinates. The numeric X/Y
values survive, but their coordinate interpretation changes. Rendering,
hit-testing, and subsequent interactions therefore drift after the first move
or resize.

Live evidence distinguishes the popup from the black helper: an Xwayland
`GetImage` capture of the mapped Steam popup contains the correct menu pixels,
while the 10x10 support XID has the generic self-client-leader signature and is
incorrectly present in `_NET_CLIENT_LIST`.

## Design

### Reversible late auxiliary classification

The XWM window registry owns the auxiliary-support predicate and the lifecycle
transition. A managed X11 window is auxiliary support when it is at most 16x16,
has no declared window type, has no `WM_HINTS` input model, and identifies
itself as its `WM_CLIENT_LEADER`. A distinct `_NET_WM_USER_TIME_WINDOW` remains
useful evidence but is not required, because Steamwebhelper uses the same
support-window pattern without that property.

The registry re-evaluates this predicate after every completed property
refresh:

- If an admitted snapshot now matches, the registry removes only its ready
  snapshot, marks the lifecycle `Auxiliary`, and the XWM emits
  `WindowWithdrawn`.
- The compositor's existing withdrawal path removes the desktop, focus,
  hit-test, render, and EWMH representation without unmapping or destroying the
  client-owned X window.
- If later properties make the window typed or input-capable, the predicate
  becomes false. The existing readiness path can then create a fresh snapshot
  and re-admit it, so classification is not an irreversible heuristic.
- Initial classification continues to happen before the first `WindowReady`.

This contains the policy in the XWM registry and avoids Steam names, process
IDs, or timing delays.

### Preserve root placement semantics

Window interaction updates must change coordinates without changing their
coordinate space. Move and resize compute new `local_x` and `local_y`, then
copy the interaction's starting `SurfacePlacement` and replace only those two
fields. Parent identity and `RootPlacementMode` remain unchanged.

Consequences:

- Native/XDG cascaded windows remain cascaded.
- X11 and other absolute roots remain absolute.
- Backend X11 configure commands receive the same numeric geometry used by the
  visual preview.
- Repeated hit-testing and resize operations use one stable coordinate model.

## Alternatives rejected

Changing only the compositor desktop role would leave a live desktop/render
record that can still be hit-tested or raised. Delaying every tiny window would
introduce arbitrary latency and false positives. Suppressing override-redirect
popup windows would hide real menus; direct capture proves Steam's popup pixels
are valid.

## Tests and verification

Tests must first fail against the current behavior and then cover:

1. A ready tiny self-client-leader is withdrawn when late properties complete.
2. A tiny self-client-leader without `_NET_WM_USER_TIME_WINDOW` is auxiliary.
3. A withdrawn helper can be re-admitted after becoming typed and input-capable.
4. Move and resize updates preserve an absolute root placement through preview
   and backend command generation.
5. Existing XWM, desktop-window, interaction, and native Xwayland reactor tests
   remain green.

After the normal full validation and release build, native verification must
confirm that Steam's main window is the only normal EWMH client, support XIDs
remain alive but absent from desktop publication, real popup pixels remain
visible, and repeated move/resize does not change absolute placement semantics
or drift geometry.
