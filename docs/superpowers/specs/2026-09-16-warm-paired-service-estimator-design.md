# Ready-Time Warm Paired Service Estimator

## Goal

Promote the existing exact same-frame paired service history from telemetry into
the pacing authority used by ordinary target planning and Predictive O1, while
keeping the independent estimator conservative during cold start and after a
proven deadline miss.

## Constraints and preserved contracts

- `AdaptiveRenderJournal::record_render_sample` continues to measure GPU-visible
  render service from `composite_started_at` to an exact sync-file fence signal.
- Paired service continues to require exact sync-file fence quality plus exact
  submit start/return timestamps, and remains render service plus same-frame
  submit ioctl service only.
- Ready-Time Opportunity Pull-In, Adaptive KMS Dispatch Tail Guard, their
  attribution rules, and all Predictive O1 physical identity/lifecycle rules
  remain unchanged.
- Worker queue residency is diagnostic only and is not added to paired service
  or predicted service cost.
- The independent estimator formula is unchanged and remains authoritative in
  `ColdStart` and `MissRecovery`.

## Estimator selection

`AdaptiveRenderJournal` will centralize:

```text
WARM_PAIRED_MIN_SAMPLES = 20
MISS_RECOVERY_PAIRED_SUCCESSES = 20
```

An exact paired observation decrements recovery by one. An approximate or
incomplete observation does nothing. A proven miss resets recovery to the full
configured horizon, so repeated misses do not accumulate an unbounded debt.
Warm mode is available only when recovery is zero and the paired sample count
meets the warm threshold.

The independent path remains:

```text
independent_total = render_risk
                 + main_event_loop_wake_guard
                 + kms_dispatch_budget
                 + kms_apply_guard
```

The warm path uses the paired P95 and subtracts the already-accounted P95
atomic ioctl from the current exported KMS dispatch budget:

```text
worker_non_ioctl_lead = kms_dispatch_budget.saturating_sub(p95_atomic_ioctl)
warm_paired_total = paired_service_p95
                  + main_event_loop_wake_guard
                  + worker_non_ioctl_lead
                  + kms_apply_guard
```

Its conservative floor is:

```text
independent_p90_floor = p90_recent_render
                      + main_event_loop_wake_guard
                      + kms_dispatch_budget
                      + kms_apply_guard
selected_warm_total = max(warm_paired_total, independent_p90_floor)
```

Selection happens before the existing idle guard. The idle guard then applies
the current `refresh_interval - 100 us` lower bound for every mode.

The independent `render_risk` and deviation remain intact as diagnostics. The
prediction will additionally expose independent total, warm total, P90 floor,
worker non-ioctl lead, and recovery remaining so logs can explain why the
selected total has its value.

## Single end-to-end authority for O1

`PipelineServiceEstimate` retains its component fields for diagnostics and
existing simulator contracts, but gains an explicit selected end-to-end total.
The normal constructor populates that total from the component sum. The native
presentation cycle uses a selected-total constructor/override populated from
`RenderPrediction::total_cost_ns`.

`end_to_end_service_ns`, `latest_successor_render_start`, and therefore
`overlap_required_ns` use the explicit selected total. This means paired render
plus submit service is subtracted once for O1; O1 does not reconstruct a second
independent render-risk total.

The planner continues to receive `prediction.total_cost_ns`, so its immediate
physical-successor target identity, unreachable-opportunity rejection, and
explicit `ProvenReadinessMiss`/`ForcedValidation` exceptions are unaffected.

## Diagnostics and snapshot semantics

Pacing trace fields retain exact paired history, independent render risk, and
mode, and add the estimator arithmetic fields. Performance snapshot fields that
describe the old component decomposition will be explicitly named as
independent diagnostics; the existing selected total field remains the
unambiguous selected authority.

## Test strategy

Tests will be added or updated in the existing native adaptive-buffering,
buffering, presentation O1, presentation deadline, and runtime integration
modules. They will cover:

1. a RED regression proving warm mode was previously telemetry-only;
2. warm arithmetic and saturating ioctl subtraction;
3. exactly-once ioctl accounting;
4. sticky independent deviation remaining diagnostic while warm total is used;
5. the independent P90 floor;
6. bounded 20-success miss recovery with reset and approximate-observation
   exclusion;
7. unchanged cold-start and idle behavior;
8. the real presentation-cycle-to-O1 integration using the selected total; and
9. unchanged immediate-successor identity and explicit planner behavior.

No native acceptance claim will be made from automated tests. Native
qualification remains a separate workload step and will be reported only if it
is actually run.
