# Composite Scene Replay Attribution v1

## Goal

Measure the ordinary scene replay performed immediately before Composite passes so
native qualification can compare its GPU and host CPU durations with the existing
`kawase_up -> composite` graph gap.

## Design

`EffectGpuProfiler` keeps one shared timestamp-pair pool. `TimingSpanMetadata`
gains an explicit `TimingSpanPurpose` with `Total`, `Pass`, and
`CompositeSceneReplay` variants. Replay spans are resolved by their own path and
never enter `finish_pass()`, pass category totals, `TimedPassBoundary`, or graph
gap interval selection.

The executor opens a replay span only for a live graph scope, a `Composite` pass,
`draw_end > scene_cursor`, non-empty `SceneReplayWorkState::active_work()`, and
no capture in progress. Immutable work metadata is captured from that exact
active region and command range. Host CPU timing and `GlesSceneFrameStats`
before/after deltas are taken only after span allocation. The draw result is
preserved while the span is closed on both success and error paths; the GPU END
timestamp is issued before detail construction.

Replay aggregates retain saturating totals and one strict-`>` longest valid span.
Availability starts true for no-work graphs, becomes false for dropped, invalid,
or missing-detail eligible spans, and is emitted with expected/resolved counts.
Graph `total_ns`, pass timing, pass boundaries, and graph-gap remainder semantics
remain unchanged.

## Tests and documentation

Focused unit tests cover isolation, exact static and execution ownership, slot
generation reuse, same-frame scope separation, dropped/invalid/missing-detail
availability, no-work graphs, aggregation, max ties, disabled profiling, final
GPU-END ordering, formatter uniqueness, and the unchanged 2048-span/4096-query
capacity proof. `docs/EFFECTS_QUALIFICATION.md` documents the measurement units,
overlap semantics, eligibility, and bounded pool cost. Native qualification is
performed only if the current environment can provide the established
1920x1080@165 Hz production workload.
