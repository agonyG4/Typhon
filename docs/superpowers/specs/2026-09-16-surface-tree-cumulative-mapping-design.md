# SurfaceTree Cumulative Mapping Preflight

## Goal

Make SurfaceTree admission validate and prepare each exact same-surface Content Update against the effective state produced by all earlier updates in publication order, without mutating live `SurfaceData` before publication.

## Architecture

`submit_surface_tree_nodes_with_kind` will retain exact candidate nodes for a side-effect-free cumulative validation pass. A small per-surface scratch state will carry the committed viewport, buffer scale, buffer transform, effective content (`retained`, `attached`, or `empty`), and the viewport resource that authored the effective source. Each exact node applies its deltas to that scratch state, validates the resulting mapping, and derives attachment metadata without mutating the pending buffer.

After the complete exact candidate validates, coalescible nodes may pass through the existing destructive canonicalizer. The canonical form is then validated and prepared using the same scratch machinery so a surviving attachment receives the final merged mapping. Pacing boundaries and `merge_frozen` boundaries skip canonicalization and preserve exact nodes, but still use cumulative state progression. Mapping writes happen only after a whole pass succeeds.

Committed viewport source ownership is retained with the committed surface state. A source delta replaces that owner, while a destination-only delta preserves it. Errors from a cumulative state therefore target the resource that authored the source, even if a later update registered another viewport.

Publication will use the preflight-prepared mapping for retained content and treat a failed reconstruction as an internal invariant violation with a conservative cleanup path. Malformed client state must be rejected before admission and must not cause a production panic or mutate `SurfaceData` to an invalid state.

## Testing

Focused unit and lifecycle regressions will cover final-invalid and final-valid viewport compositions, destination/scale/transform followed by attachment, pacing-protected and real `merge_frozen` exact candidates, NULL and new-content transitions, source error ownership, and exactly-once release of rejected pending buffers and callbacks. Existing canonicalization, explicit-sync, F07, F08, and F09 coordinate/damage behavior will remain covered by the relevant suites.

