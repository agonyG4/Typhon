# Typhon Dynamic Per-Surface DMA-BUF Scanout Feedback

## Goal

Teach Typhon to publish a Direct Scanout preference for the exact Wayland
surface whose client-owned DMA-BUF allocation was proven incompatible with the
current KMS primary plane. The preference must let the client reallocate an
exactly supported format/modifier pair while preserving renderer fallback and
all KMS safety checks.

## Scope and invariants

- `zwp_linux_dmabuf_v1` remains at version 4.
- Default feedback is renderer/import feedback and never receives an
  unconditional scanout tranche.
- Surface feedback is bound to the exact `wl_surface` supplied to
  `get_surface_feedback`; the DMA-BUF source surface, not a logical/root
  owner, owns the preference.
- Scanout capabilities are exact FOURCC + modifier pairs for the proven
  opaque RGB8888 family (`XRGB8888`, `XBGR8888`). No modifier or format
  rewriting, conversion, KMS bypass, or unsupported TEST_ONLY attempt is
  introduced.
- Client Direct Scanout capability authority is the intersection of primary
  plane support, renderer/import support, and Typhon's structural format
  policy. GBM self-allocation is not part of that authority.
- Existing renderer/default feedback remains available as fallback after a
  scanout tranche.

## Architecture

### Comparable protocol description

Split the current feedback payload into two layers:

1. `DmabufFeedbackSnapshot`, a normalized `PartialEq + Eq` description of
   main device, immutable format-table entries, and ordered tranches.
2. A materialized Wayland payload containing the snapshot's format-table file
   and tranche data.

Every feedback resource retains its latest materialized payload through its
resource user data. A new snapshot creates a new immutable format-table file;
the old file is never mutated. Reconciliation compares snapshots before
materializing or sending, so identical effective feedback produces no Wayland
traffic.

### Feedback scopes and lifecycle

Each live `zwp_linux_dmabuf_feedback_v1` binding has one of these scopes:

- `Default`: renderer/import feedback only.
- `Surface(surface_id)`: renderer feedback, optionally prefixed by the
  surface's current scanout preference.
- `InertSurface`: the associated `wl_surface` was destroyed; no future update
  is sent.

`CompositorState` owns the surface-hint set and a registry of live feedback
bindings. The registry stores weak Wayland resources plus shared binding state,
and removes entries in the feedback resource `destroyed()` callback. Surface
teardown marks matching bindings inert and removes their active registry
entries without forcibly destroying the client's feedback object.

The surface request resolves `surface.data::<SurfaceData>()` immediately and
stores the exact compositor surface ID in the binding. Invalid/missing surface
metadata does not create a surface-scoped preference.

### State reconciliation

`set_dmabuf_feedback*` updates the authoritative renderer feedback, main-device
identity, scanout capability catalog, and target-device override, then
reconciles every live binding:

- default bindings use renderer fallback only;
- surface bindings use scanout-first feedback only when their surface hint is
  active;
- inert bindings are skipped and pruned if their weak resource is dead.

`activate_surface_scanout_hint(surface_id)` and
`clear_surface_scanout_hint(surface_id)` update only the relevant effective
feedback state, while still rebuilding from the current global capability
catalog. Repeated activation or clearing is a no-op when the effective
snapshot is unchanged. Bounded diagnostic counters record update sends and
duplicate suppression.

### Native direct-presentation integration

`discover_direct_scanout_capabilities` becomes a pure intersection helper over
the plane's exact pairs and `EglGlesDmabufFeedback`; it rejects invalid
modifiers, ARGB, and non-opaque formats, but does not invoke a GBM allocation
probe. GBM probing remains with Typhon-owned output/swapchain format selection.

At the existing pre-import check in `try_direct_scanout`, an unsupported exact
candidate pair activates a hint for `candidate.surface_id`, preserves the
normal composited fallback reason, and returns before import or TEST_ONLY.

The native presentation layer tracks the current direct candidate source. A
source transition clears the previous surface's hint before the new source can
become the active hint; candidate disappearance clears the last source. A
compatible replacement buffer keeps the same source hint active. Surface
destruction clears it through centralized compositor teardown.

### NVIDIA compatibility

The existing same-physical-GPU target-device normalization is preserved and is
applied to dynamically generated surface scanout tranches as well as the
renderer fallback. Startup diagnostics use a precise dynamic per-surface
policy label instead of `default-and-surface-same`.

## Error and safety behavior

- Unsupported client pairs are never imported or passed to Atomic TEST_ONLY.
- Import failures and generic TEST_ONLY failures do not activate scanout
  feedback in this task.
- Empty scanout capability sets omit the scanout tranche.
- Renderer feedback build failures leave existing bindings untouched and keep
  the fallback payload valid where possible.
- Dead weak resources are pruned during reconciliation; resource destruction
  also removes its registry entry directly.

## Tests

Add unit coverage for snapshot equality, exact pair preservation, capability
intersection without GBM self-allocation, and default-vs-surface tranche
construction. Extend protocol integration coverage to exercise two surfaces,
surface-local activation/removal, duplicate suppression, source-versus-root
selection, feedback resource destruction, inert surface destruction, and
global capability replacement. Extend native scanout coverage to assert the
unsupported-pair trigger happens before import and targets `surface_id`.

Run focused tests first, then `cargo fmt --check`, `cargo check`, `cargo test`,
`cargo clippy --all-targets --all-features -- -D warnings`, and `git diff --check`
through the repository's RTK wrappers where applicable. Hardware acceptance
must observe a changed per-surface feedback sequence followed by a client
reallocation; feedback alone is not evidence of successful direct scanout.
