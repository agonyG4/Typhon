# Render-readiness evidence consistency closure

## Scope

This qualification follow-up closes a diagnostics consistency gap at the
native pageflip policy boundary. It does not alter pacing, target selection,
scheduler wake ownership, KMS adaptation, or recovery policy.

## Design

Pageflip policy constructs one typed `RenderReadinessObservation` from a
presented frame's fence timing and `rendered_at` timestamp. Exact sync-file
and guarded approximate fence observations retain their fence timestamp;
frames without fence timing use `rendered_at` as an explicitly named
`RenderedAtFallback` observation. The observation is the single source for
physical KMS classification, local deadline assessment, and readiness
diagnostics.

`rendered_at` is a compositor-side observation after render-command/fence
export handling. It is a conservative lower bound on GPU readiness, not GPU
payload-completion proof. Therefore fallback observations are passed to the
physical classifier as the timestamp it already consumed, but
`payload_ready_at_ns` remains `none` without fence evidence. If that lower
bound itself is later than the commit-complete deadline, the already-proven
physical `RenderReadinessMiss` maps to `ExactRender`: the compositor did not
reach its render/fence-export completion point in time, so the true GPU-ready
timestamp cannot make the miss earlier.

Diagnostics retain the existing fence-derived fields and add
`render_readiness_observed_at_ns` and
`render_readiness_observation_lateness_ns` for the source-independent
timestamp used by classification. Pending exact or guarded render evidence
continues to take precedence over pageflip-local reconstruction.

## Verification intent

Typed tests cover exact, approximate, and fallback mappings, fallback
non-fabrication of payload-ready timestamps, fallback observation lateness,
on-time fallback classification, pending-evidence precedence, and the
existing advisory ReactiveDouble dispatch contract. The implementation is
behavior-preserving: only the evidence authority and diagnostic vocabulary
change.
