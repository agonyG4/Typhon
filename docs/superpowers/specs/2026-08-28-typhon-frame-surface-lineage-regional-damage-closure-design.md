# Typhon Frame Surface Lineage and Regional Damage Closure

**Date:** 2026-08-28

## Current-source findings

The local checkout is authoritative and is eight commits ahead of its
`origin/main` (`a0d5b8a`). It already contains the ordered
`renderable_surfaces` vector plus a global `SurfaceId -> index` sidecar, a
separate `ActiveSceneView` index, final `ResolvedNativeFrameScene.surface_ids`,
keyed frame-batch damage ownership, and an integrated client/output swapchain
oracle. Those are retained.

The remaining defects are narrower:

* the compatibility `capture_surface_damage_presentation()` path still scans
  every renderable surface and uses a vector duplicate check;
* scene snapshot comparison promotes popup metadata and every order-signature
  change to `FullOutput`;
* native snapshots store only a rectangle vector, while compositor settlement
  collapses journal `HistoryLost` into ordinary `Full` damage;
* the no-visual-change terminal currently settles a sampled token even though
  only physical presentation may advance the presented damage lineage;
* a final scene is re-resolved after exact IDs are captured in the native render
  path, so capture and paint should share one resolved-scene value;
* one active-scene tree refresh still uses a vector search on a topology path.

The codebase-memory index reports a parser-missed line in
`presentation_cycle.rs:152`; that range was read directly from the checkout.
No skipped source file affects this design.

## Design

### Damage provenance

Preserve journal provenance through settlement by adding a
`HistoryLost` state to compositor damage. Native scene snapshots expose an
explicit evidence enum:

```text
AuthoritativeEmpty
Known(rectangles)
HistoryLost
```

An authoritative empty entry contributes no content repair. Known damage
contributes only its clipped rectangles. History loss contributes the current
surface footprint, without becoming output-global damage. Buffer identity
continues to be independent from logical damage; every new client buffer is
still imported and retained normally.

### Regional scene transitions

Content damage remains journal-owned. Geometry changes repair old and new
bounds. Mapping repairs current bounds; unmapping repairs old bounds. Popup and
subsurface membership changes are represented by those exact old/new surface
snapshots and no longer use popup metadata as an automatic full-output reason.

Ordered surface transitions use the common unchanged prefix and suffix. The
old and new bounds in the changed middle span are damaged. This covers
occlusion changes for additions, removals, and reorders while leaving unrelated
ordered surfaces untouched. Visibility changes and external-overlay membership
remain conservative `FullOutput` reasons because this closure does not prove
their affected region.

### Exact presentation lineage

The exact IDs from the one `ResolvedNativeFrameScene` used for painting create
the frame's keyed damage token. The token remains attached to that rendered
frame through READY, admission, kernel ownership, and pageflip. Only the
physical presentation terminal settles it. Render failures, replacement,
abandonment, and non-presented no-visual-change completion drop the token
without advancing the presented commit. Direct Scanout keeps its candidate
local framebuffer identity and ownership path.

The existing global renderable index remains the authority for compositor-owned
storage; the active-scene index remains the authority for the exact visible
ordered scene. Presentation settlement uses the global sidecar directly and
does not scan the renderable vector.

### Observability

Use bounded counters for exact sampled entries, settlement entries, global
scan attempts, evidence classes, regional popup/subsurface/order transitions,
full-output transitions, and rectangle/pixel totals. The normal exact-frame
path must record zero global renderable scans.

## Test strategy

RED tests cover popup and subsurface map/unmap/move/order locality, explicit
empty versus history loss, no-visual-change non-consumption, exact frame
sampling and settlement, READY lineage retention, index consistency, true
global invalidation, direct-scanout identity, scene-size counters, and a
deterministic overlapping-scene pixel oracle. The existing client-buffer
rotation/output-buffer-age oracle is extended with topology transitions and
compared against a full-reference renderer on every presented frame.

No O1 callback admission policy, SHM release timing, DMA-BUF release authority,
KMS admission timing, scheduler, resize, or Direct Scanout identity semantics
are changed.
