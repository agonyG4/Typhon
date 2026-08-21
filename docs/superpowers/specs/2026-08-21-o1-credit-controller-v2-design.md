# Typhon O1 Credit Controller v2 Design

## Goal

Complete O1 opportunity-locked buffering by separating policy demand, physical
future-primary ownership, admission budget, and per-frame presentation timing.
The implementation must make render-side pressure the only direct source of
extra credit, allow policy revocation while depth two is still physically
owned, and validate lifecycle behavior with deterministic virtual time.

## Architecture

`O1CreditDemandController` is a pure bounded policy object. It owns only the
desired credit (1 or 2), hysteresis, the last observed opportunity identity,
typed demand reason, and aggregate grant/revoke counters. It never receives
physical ownership as an input. Render-readiness misses and unique positive
overlap observations may raise demand; KMS dispatch/apply misses update their
existing timing models only. Force and Off remain explicit policy ceilings.

The native output pipeline remains the source of physical truth through
`future_primary_depth()`. Pipeline admission receives an explicit future
primary limit, so `desired_credit == 1 && depth == 2` preserves existing
ownership while suppressing any refill. `AdaptiveBufferingMode` and
`NativeOutputPacingMode` remain compatibility/diagnostic values where needed,
but fixed-VSync admission and target creation use the explicit limit and the
immutable target role rather than a global mode transition.

The simulator becomes an integrated deterministic event model. A priority
queue drives visual work, render, fence, worker, submit, pageflip, generation,
failure, and timing-constraint events. The model tracks armed target identity,
desired credit, physical owner depth, each pipeline queue/slot, worker
transport, and bounded useful-credit outcome counters. It reuses the production
demand controller and service-estimate calculations.

## File boundaries

- `src/native/buffering/credit.rs`: pure O1 demand controller and typed demand
  reasons; no runtime ownership or scheduler mode dependencies.
- `src/native/buffering/simulator.rs`: deterministic virtual-time events,
  ownership state, bounded sweep/scenario results, and simulator tests.
- `src/native/buffering/mod.rs`: opportunity contracts, service estimate,
  public re-exports, and small contract tests.
- `src/native/adaptive_buffering.rs`: compatibility wrapper around demand
  policy, render-readiness attribution, capability ceilings, and aggregate
  telemetry accessors.
- `src/native/scheduler/pipeline.rs`: scheduler admission based on explicit
  future-primary budget and physical depth, with mode-independent fixed-VSync
  decisions.
- `src/native_output/presentation/pipeline.rs`: snapshot future-primary limit,
  depth-based validation, and admission helpers; retain slot, generation,
  target-order, and queue-capacity invariants.
- `src/native_output/runtime/planner.rs` and
  `src/native_output/runtime/presentation_cycle.rs`: create/render/submit
  targets from the admission budget and immutable target role; never mutate an
  armed target because demand changed.
- `src/control_snapshots.rs`, `src/native_output/pacing.rs`, and runtime
  metrics: expose bounded O1 demand, drain, refill-suppression, and useful
  credit classifications alongside existing KMS miss counters.

## Error handling and invariants

- Physical future-primary depth is always at most two.
- Desired credit is clamped to one or two and is independent of ownership.
- A repeated observation for one predecessor opportunity is ignored.
- An armed opportunity's sequence and timestamp never change; invalid leases
  terminate explicitly before a successor is allocated.
- A demand revoke never cancels a valid rendered/owned frame.
- KMS dispatch/apply misses never call the demand grant path directly.
- Worker queue residency is not included in render service estimation.
- Existing slot alias, output-generation, monotonic-target, prepared-capacity,
  and worker/kernel queue invariants remain enforced.

## Validation

TDD starts with regressions for KMS-only non-grants, revocation at owned depth
two, drain-without-refill, mode-independent admission, and target immutability.
The integrated simulator covers low load, sustained overlap, one-frame spikes,
KMS-only misses, worker on/off equivalence, generation changes, and useful,
unnecessary, ineffective, and granted-not-consumed credit. Bounded refresh,
service, dispatch, apply-guard, and spike-position sweeps verify the physical
depth and opportunity invariants. Focused Rust tests run before the final
format/check/full-test/source-layout validation. No long benchmark campaign is
part of this change.

## Scope

This work does not redesign KMS Timing v2, the worker transport, VRR, direct
scanout, damage, scene rendering, frame callbacks, commit coalescing, or other
systems listed as out of scope in the supplied task specification.
