# Typhon Locked Cursor Visual Reveal Qualification v1.2 Report

## Result

v1.2 Gate 1 observability closure is implemented from starting `HEAD`
`d15470e7c3a2f6921e3c257b19d655c59e4636f`. The final implementation HEAD before
this report-only commit is `493708087f72ddae06aa430bec3b1a22fc2933cc`; the report is the superseding qualification
record for that implementation.

v1.1 closed pageflip-time attribution with the exact physical identity
`(output_generation, crtc_id, PageFlipToken)`. v1.2 closes the remaining gap at
freeze time: reveal ownership, cursor revision, and cursor source are frozen at
the same semantic boundary as the cursor presentation state, then carried
through render-ahead, ready-frame delay, worker admission, sidecars,
synchronous submission, KMS evidence, pageflip, and presented-cursor matching.

No Gate 2 semantic cursor correction was implemented. Cursor placement,
scheduling, visibility policy, pointer coordinates, worker selection, sidecar
replacement policy, KMS ordering, and input behavior remain outside this
qualification. The underlying cursor teleport is therefore not claimed fixed.

## Delivered

- Kept the v1.1 trace-only bounded ledger and exact key
  `(output_generation, crtc_id, PageFlipToken)`.
- Made `CursorRevealTraceSnapshot` a presentation-plane carrier and stored it on
  `RenderedOutputFrame` at render freeze. Ready-frame submission no longer
  reconstructs reveal identity from mutable current server/cursor state.
- Carried the frozen snapshot into `KmsBundleOwners`, retaining it on the
  primary owner when no cursor-plane owner exists. Worker pageflip binding uses
  the owner snapshot; a replacement sidecar replaces its cursor state and trace
  snapshot together.
- Changed presented-state snapshot construction so `PresentedCursorState.source`
  is authoritative. A later mutable `cursor.client_source_key()` cannot
  override a source already frozen into the presentation.
- Preserved frozen epoch and revision for synchronous primary KMS evidence, and
  used frozen owner metadata for worker primary and direct-primary evidence.
- Captured immediate cursor-only and direct-worker snapshots before their
  physical submission boundary; token binding only adds the physical identity.
- Added `cursor_submission_bound` evidence containing the frozen reveal/source
  fields and physical output/CRTC/token identity.
- Kept no-visible reveals terminal: `no_visible_cursor_requested` is emitted
  when tracing is enabled and the active trace authority is retired, so a later
  visible reveal cannot inherit its first-visible slot.
- Made trace-disabled construction neutral: no trace-only reveal authority,
  snapshot, hidden-state trace clone, owner propagation, or sidecar trace is
  created when `TYPHON_CURSOR_PRESENTATION_TRACE` is not enabled.
- Preserved exact atomic request writes while extending the canonical KMS
  vocabulary. Human geometry now uses `pointer_position`, `hotspot`, and
  `plane_origin_signed`; raw DRM values are labeled `CRTC_X_RAW` and
  `CRTC_Y_RAW`.

The required closure claims are explicit:

> Reveal ownership is frozen at the same boundary as the cursor presentation state it describes.

> Physical token binding never consults mutable current reveal or current cursor-source state for an already-frozen cursor presentation.

> With cursor presentation tracing disabled, no trace-only reveal authority or reveal snapshot is created or propagated through cursor/KMS ownership.

> `pointer_position` denotes the cursor hotspot position; `plane_origin_signed` denotes the cursor-plane top-left; raw `CRTC_X/Y` remain the exact DRM atomic property representation.

> No Gate 2 semantic cursor correction was implemented.

## KMS geometry qualification

The assignment helper continues to compute the signed plane origin as pointer
position minus hotspot, then preserves the exact raw representation submitted
to DRM as `i64::from(origin) as u64`.

- `(100,80)` with hotspot `(10,5)` reports
  `pointer_position=(100,80)`, `hotspot=(10,5)`,
  `plane_origin_signed=(90,75)`, `CRTC_X_RAW=90`, and `CRTC_Y_RAW=75`.
- `(2,3)` with hotspot `(8,9)` reports
  `plane_origin_signed=(-6,-6)` while raw values remain `(-6i64) as u64`.
- Disabled and unavailable assignments report unknown geometry rather than
  inventing pointer or plane coordinates.

## Causal reconstruction boundary

The trace chain is now attributable as:

```text
cursor state/reveal freeze
  -> frozen frame or immediate cursor snapshot
  -> cursor_submission_bound + cursor_kms_submit
  -> pageflip identity lookup
  -> presented cursor comparison / first-visible attribution
```

For delayed primary submission, the frame snapshot is the source of reveal,
epoch, revision, delivery, visual state, and source. The physical token is
attached only after KMS submission returns or the worker reports its immutable
job ownership. For cursor-only paths, the same fields are captured before the
submission call. The four overlapping A/B orderings are represented by the
bounded identity ledger and cannot claim a newer reveal merely because it is
current when an older token is allocated or completed.

## Verification

All commands ran in the checkout's normal local target directory through
`rtk`.

RED proof before the v1.2 production changes:

```text
rtk cargo test --locked cursor_trace
failed: 10 compile errors
```

The failures were the intentionally changed freeze-safe presented snapshot
constructor and the new raw/signed coordinate fields.

Focused GREEN evidence:

```text
rtk cargo test --locked cursor_trace -- --test-threads=1
10 passed

rtk cargo test --locked ready_frame_carries_frozen_cursor_reveal_through_worker_queue -- --test-threads=1
1 passed

rtk cargo test --locked cursor_plane_assignment -- --test-threads=1
2 passed

rtk cargo test --locked scanout -- --test-threads=1
240 passed

rtk cargo test --locked worker -- --test-threads=1
187 passed

rtk cargo test --locked sidecar -- --test-threads=1
17 passed

rtk cargo test --locked disabled -- --test-threads=1
23 passed

rtk cargo test --locked no_visible -- --test-threads=1
2 passed

rtk cargo test --locked cursor_reveal -- --test-threads=1
2 passed

rtk cargo test --locked synchronous -- --test-threads=1
4 passed

rtk cargo test --locked synchronous_kms_payload_uses_canonical_exact_fields -- --test-threads=1
1 passed

rtk cargo test --locked visible_cursor_geometry_preserves_negative_partially_offscreen_coordinates -- --test-threads=1
1 passed

rtk cargo test --locked plane_delta_pageflip_ack_releases_worker_inflight -- --test-threads=1
1 passed
```

Fresh full verification:

```text
rtk cargo fmt --check
passed

rtk cargo check --locked --all-targets
passed

rtk cargo clippy --locked --all-targets -- -D warnings
passed

rtk cargo test --locked
3404 passed, 5 ignored

rtk git diff --check
passed
```

## Qualification boundary and next step

This report qualifies the causal observability and attribution closure only.
Native A/B capture with `OBLIVION_ONE_CURSOR=hardware` and software cursor
presentation, both with `TYPHON_CURSOR_PRESENTATION_TRACE=1`, remains the next
step. That capture may determine whether the observed teleport is a real
semantic cursor-placement defect, a presentation-path mismatch, or a trace
interpretation issue; v1.2 does not claim that result in advance.
