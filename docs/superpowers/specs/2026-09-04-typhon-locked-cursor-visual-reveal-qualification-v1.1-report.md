# Typhon Locked Cursor Visual Reveal Qualification v1.1 Report

## Result

v1.1 Gate 1 observability closure is implemented in commit `b067ff4` on top of
the v1 report state `cafd18e901b564b3d3cee650c1506a9f91d49dab`.

The implementation freezes reveal ownership against the exact physical
submission identity `(output_generation, crtc_id, PageFlipToken)` before an
asynchronous boundary is crossed. Pageflip processing consumes that identity
from a trace-only bounded ledger; it never infers ownership from the mutable
current compositor reveal. A no-visibility reveal is terminally recorded and
cannot claim an unrelated visible cursor.

No Gate 2 semantic cursor correction was implemented. Cursor placement,
scheduling, visibility policy, pointer coordinates, worker selection, sidecar
replacement, and KMS ordering remain outside this qualification.

## Delivered

- Added one shared atomic cursor-plane assignment description, generated from
  the same helper that writes the real cursor properties. It preserves exact
  source geometry, hotspot-adjusted CRTC position, dimensions, framebuffer,
  disable, unavailable-plane, and unsupported-visible semantics.
- Added canonical `cursor_kms_submit` evidence for worker and synchronous
  cursor-only and primary-plus-cursor submissions. The payload reports the
  physical token, output/CRTC identity, transaction when available, epoch and
  revision when available, transport, delivery, and exact known KMS fields.
- Added a trace-only `CursorRevealTraceLedger` with FIFO capacity 32 for both
  physical submission entries and reveal lifecycle slots. Overflow emits an
  explicit diagnostic and retires only trace state.
- Carried frozen reveal snapshots through worker cursor owners and sidecars,
  binding them when the main runtime observes the submitted physical identity.
  Synchronous paths bind after the backend submission returns its token.
- Replaced the compositor-global first-visible slot with per-reveal state.
  First-visible evidence compares position, revision, delivery, hotspot,
  framebuffer, image generation, source, visual state, and overall state using
  `true`, `false`, or `unknown`.
- Added explicit `superseded_by_new_reveal` and
  `no_visible_cursor_requested` terminal evidence.
- Constructed all new ledger/snapshot state only when
  `TYPHON_CURSOR_PRESENTATION_TRACE=1`; the disabled path has no new ledger
  allocation or formatting work.

## Verification

The implementation was compiled and tested in the checkout's normal local
target directory.

RED proof before the shared assignment helper existed:

```text
rtk cargo test --locked cursor_plane_assignment_describes_the_exact_atomic_geometry
failed: 7 compile errors
```

GREEN and regression evidence:

```text
rtk cargo test --locked cursor_plane_assignment
2 passed

rtk cargo test --locked cursor_trace
9 passed

rtk cargo test --locked overlapping_reveals_keep_submission_identity
1 passed

rtk cargo test --locked cursor_trace_snapshot_exposes_submitted_and_queued_state_identity
1 passed

rtk cargo test --locked v11_client_warp_after_backend_ack_settles_unlock
1 passed

rtk cargo test --locked cursor_job_keeps_immutable_presented_or_predecessor_validation_base
1 passed

rtk cargo test --locked sidecar_offered_before_freeze_is_attached_to_exact_primary_bundle
1 passed

rtk cargo test --locked plane_delta_pageflip_ack_releases_worker_inflight
1 passed

rtk cargo check --locked --all-targets
passed

rtk cargo clippy --locked --all-targets -- -D warnings
passed

rtk cargo test --locked
3400 passed, 5 ignored

rtk git diff --check
passed
```

The focused tests cover overlapping reveal identity, independent first-visible
slots, no-visible terminal neutrality, stale same-position visual mismatch,
exact known visual matching, disabled state neutrality, bounded overflow,
canonical synchronous payload fields, exact cursor assignment semantics,
worker ownership, sidecar handoff, pageflip acknowledgement, and unlock
settlement.

## Remaining qualification

Native A/B capture with `OBLIVION_ONE_CURSOR=hardware` and software cursor
presentation, both with `TYPHON_CURSOR_PRESENTATION_TRACE=1`, remains the next
step. This report therefore qualifies the causal observability and attribution
closure only; it does not claim that the underlying cursor teleport is fixed.
