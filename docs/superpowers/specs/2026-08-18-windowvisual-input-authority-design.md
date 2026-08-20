# Typhon WindowVisual Input Authority Design

## Status and scope

This is a focused corrective closure for server-side decoration input. It covers
visual ownership, pointer motion, decoration buttons, compositor move/resize,
focus churn, and the tests that protect those paths. It does not change the
recent resize configure pipeline, native presentation history, render-ahead
damage journal, buffer-age handling, fullscreen frame-scene authority, or
unrelated desktop subsystems.

## Native residual report

**NATIVE-UNQUALIFIED (current session):** The requested native launcher reached
the direct DRM/KMS and EGL setup, but stopped before the compositor could render
because the atomic `TEST_ONLY` pre-render commit returned `Permission denied`.
The previously reported inactive-feeling or briefly laggy SSD titlebar remains
an explicit native residual to qualify once the session has DRM/input access.

**CONFIRMED:** The dirty working tree contains the prior `PointerSceneHit`
closure. Normal pointer motion can return `Decoration(A)`, but native move and
resize entrypoints still query client-only `surface_id_at()` before checking
decorations. A titlebar press also resolves the decoration and then calls a
move helper that queries the scene again.

## Root cause

**CONFIRMED:** Typhon currently has separate authorities for pointer motion,
window interaction, and decoration buttons. `surface_id_at()` only sees
client `wl_surface`s, so a lower client B can be selected underneath an
interactive SSD A. The exact decoration owner is discarded before move/resize
capture begins.

**CONFIRMED:** Rendering already groups ordinary client descendants with their
root and paints the SSD after that group. XDG popup roots are split into their
own visual groups so they paint above the parent SSD. `pointer_scene_hit_at()`
instead walks the raw surface vector backwards and checks SSD only when it
reaches the root, allowing an ordinary subsurface to steal input from pixels
painted below the SSD.

**STRONG HYPOTHESIS:** Re-running the full desktop focus pipeline on every
same-window decoration motion contributes to the native titlebar micro-hitch.
The current implementation calls `focus_desktop_window()` even when the
resolved decoration owner is already the focused desktop window.

## Reference architecture comparison

**CONFIRMED from current upstream source:** KWin models decoration input as
part of the owning `Window`. Its window hit path checks the decoration input
region in frame coordinates rather than allowing an underlying client to
become the owner. See [KWin `Window::input*` implementation](https://github.com/KDE/kwin/blob/master/src/window.cpp).

**CONFIRMED from current project structure:** Hyprland keeps topmost view/window
hit testing separate from the later client/decoration decision and retains the
resolved view in its drag controller. Typhon adopts that ownership invariant
without importing Hyprland's C++ or event machinery. The reference project is
[Hyprland](https://github.com/hyprwm/Hyprland); its current tree contains the
`DragController` and view hit-testing implementation under `src/`.

## Selected architecture

Create a lightweight `VisualStackGroup` primitive in `src/compositor/render.rs`.
It contains only:

- the visual root surface ID;
- the ordered surface indices owned by that visual group;
- whether the group is a popup group.

The primitive is produced by the existing root-relationship and popup
classification algorithm. The renderer maps each group to its decoration
render instance. Pointer hit testing reuses the same group order through a
small compositor cache keyed by scene-generation changes; it never constructs
SVG, text, raster, or decoration render plans.

For front-to-back input, the authority is:

1. popup group client surfaces, in reverse paint order;
2. normal decorated group SSD geometry;
3. normal group client/subsurface surfaces, in reverse paint order;
4. CSD, layer-shell, and unowned client surfaces without a fabricated SSD.

This preserves popup-above-SSD behavior while making ordinary subsurfaces
remain below their owner's SSD.

## Interaction capture ownership

`PointerSceneHit` is resolved once for a normal scene input event. A compact
interaction target derived from it preserves `WindowId`, root surface ID,
client pointer-motion surface ID when present, and the exact decoration hit.

- Native move/resize derives its target from the resolved scene hit.
- A titlebar move starts directly from the captured decoration owner.
- A decoration resize uses the captured edge and owner directly.
- A compositor titlebar or resize interaction owns motion until release or
  cancellation; motion does not re-hit-test another window.
- Native client-content bindings continue to derive their root and motion
  surface from `PointerSceneHit::Client`.

The client-only helper is retained only for operations that explicitly need a
client surface and is documented as such. It is not an interaction authority.

## Focus and pointer protocol behavior

Desktop focus is independent from Wayland pointer focus. Entering A's SSD
keeps desktop and keyboard focus on A, clears client pointer focus once, and
does not focus B. Returning to A's client emits one enter. Repeated decoration
motions are a no-op when client pointer focus is already empty.

Same-window decoration focus returns `NoChange` when desktop focus and its
focused root are already valid. It does not re-run keyboard-focus reconciliation
or pointer-constraint handling. A real mismatch still follows the full focus
path.

## Test matrix

| Area | Regression | Expected invariant |
| --- | --- | --- |
| Titlebar move | A titlebar overlaps B client; press and drag | captured owner and moved window are A |
| Resize margin | A invisible edge overlaps B client | captured owner and resized window are A; exact edge retained |
| Ordinary subsurface | A child overlaps A titlebar | `PointerSceneHit::Decoration(A)` |
| Popup | popup P overlaps parent SSD | `PointerSceneHit::Client(P)` |
| Buttons | B under A button cluster | minimize/maximize/close affect A only |
| Double click | B under A titlebar | maximize/restore A only |
| Capture | drag crosses B/background/another SSD | interaction owner remains A until release |
| Focus churn | 1000 client↔SSD transitions | focus generation stable; no B enter or activation churn |
| Protocol | A client→SSD→A client | one leave then one enter; no duplicate leaves |
| Layers/grabs | layer, popup, implicit, lock, confinement | higher-level routing remains authoritative |
| Modes | CSD, XWayland, fullscreen | no fake SSD for CSD/fullscreen; managed XWayland uses same authority |
| Resize/render | immediate resize and native scene regressions | latest visual geometry is hit-tested; ghosting tests remain green |

## Performance considerations

The pointer hot path uses cached group ordering and current geometry only. The
cache is invalidated by surface-tree/order, popup-topology, and scene changes.
Per-motion work is bounded traversal of compact groups and geometry checks. No
filesystem access, theme reload, SVG rasterization, font shaping, large frame
allocation, or full decoration render-plan construction is permitted.

## Rejected alternatives

- **Coordinate hacks:** rejected because popups, subsurfaces, layer surfaces,
  and XWayland override-redirect surfaces make raw titlebar coordinates an
  incomplete ownership model.
- **Always test SSD before every descendant:** rejected because a popup rendered
  above the SSD must win input.
- **Changing active decoration colors:** rejected because color state is a
  symptom; ownership is wrong.
- **Disabling pointer-enter focus:** rejected because it changes unrelated
  focus policy instead of fixing the scene owner.
- **Building render plans in input:** rejected because it violates the 1000 Hz
  pointer-path budget.
- **Porting KWin or Hyprland event machinery:** rejected; only the owning-window
  invariant is needed.

## Evidence classification

- **NATIVE-UNQUALIFIED:** residual titlebar inconsistency and micro-hitch symptom;
  native qualification was blocked before a usable rendered session.
- **CONFIRMED:** client-only interaction authority split; decoration re-hit;
  raw input order differing from renderer grouping.
- **STRONG HYPOTHESIS:** redundant same-window focus work contributes to the
  hitch.
- **UNPROVEN until verification:** native XWayland/titlebar/button stress,
  exact 1000-cycle protocol counts, and no regression in the full native
  presentation path. The deterministic 1000-iteration scene-hit stress test
  is covered in the unit suite; it is not a substitute for native protocol
  tracing.
