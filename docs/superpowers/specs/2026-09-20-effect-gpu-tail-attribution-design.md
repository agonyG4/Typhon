# Typhon Effect GPU Tail Attribution v1

## Goal

Extend the existing one-record-per-graph GPU timing telemetry so it identifies
the slowest valid individual render pass and its bounded execution workload,
without changing rendering or issuing additional GPU timing queries.

## Design

Each timed pass carries an immutable `PassTimingWork` value containing the
existing effect-space demanded pixels, execution damage rectangle count,
execution damage bounding-box area, and the dimensions of its planned output
texture. The executor constructs this value only inside the existing active
graph-timing branch. Passes without an output texture use zero dimensions.

When an existing timestamp pair resolves validly, `TimingState::finish_pass`
updates category totals and considers the pass for an independent
`MaxEffectPassTiming` candidate before applying capture-only attribution. A
strict longer-than comparison preserves first-winner behavior on ties. The
candidate carries all workload fields from that exact span. Capture mode comes
from that span's already-bound `CaptureTimingMetadata`; the existing
`MaxCapturePassTiming` and Replay Attribution v2 details remain unchanged.

The graph record exposes the sum of valid individual pass-category durations
as `pass_timed_ns`. `graph_unattributed_ns` is the saturating difference from
the total span and has no subsystem-level causal interpretation. The canonical
GPU timing line appends stable kind and capture-mode values plus the max-pass
identity and workload scalars. Empty max-pass attribution emits zeros and
`none` values.

## Constraints and verification

No execution damage, graph selection, pass ordering, capture, allocation,
shader, draw, EGL priority, scheduling, or resource-pool behavior changes.
Timestamp query call sites and the 2048-span/4096-query capacities remain
fixed. Tests cover independent max authorities, workload ownership, invalid
spans, scope ownership, remainder saturation, formatter uniqueness, and the
disabled-timing path. `docs/EFFECTS_QUALIFICATION.md` documents the units and
limits of the new fields. Native qualification is diagnostic only and does not
authorize an optimization.
