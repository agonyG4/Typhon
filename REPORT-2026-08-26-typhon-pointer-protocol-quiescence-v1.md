# Typhon Pointer Protocol Quiescence v1 — Closure Report

**Date:** 2026-08-26  
**Scope:** Native input, Wayland protocol scheduling, Astrea publication, and compositor-owned move/resize interaction hot paths  
**Baseline:** `ba67468 perf: make interaction visuals scheduler-admitted`

## Outcome

The approved Pointer Protocol Quiescence v1 design is implemented in the current dirty checkout. Native input readiness is now independent from Wayland read-side readiness, Astrea publication is independently timer-serviced and gated before reconciliation, compositor-owned interaction motion avoids generic scene-hover discovery, and native input write-side flushes are coalesced at the input-batch boundary.

The change does not redesign rendering, KMS scheduling, direct scanout, Dwindle authority, or scheduler-admitted interaction geometry.

## Implemented architecture

### Native runtime domains

- `wayland_dispatch` is driven only by Wayland listener/client readiness.
- `input` remains independently serviceable on native input readiness.
- `astrea_publication` is a protocol-only domain derived from the pending-publication timer state.
- Surface pacing retains its existing control/readiness progression without inheriting input-only wakes.
- Input-only cycles and actual Wayland read-dispatch cycles have separate resource-efficiency counters.
- The runtime re-arms the complete existing deadline tree after cycles that skip presentation, preserving immediate Astrea publication service after input dirties focus/toplevel state.

The native dispatch function accepts an explicit read-dispatch flag. It calls `OwnCompositorServer::tick()` only for the actual Wayland read-side path. Native-only key/button input retains the narrow existing pointer-constraint follow-up; pointer motion does not call `tick()`, and combined input plus Wayland readiness does not duplicate the full tick.

### Astrea publication

- `AstreaToplevelPublisher::should_reconcile()` is the central cheap gate.
- Clean publication calls return before collection construction, pruning, resource-metric scans, or `reconcile()`.
- `reconcile()` and pruning now expose internal counters used by the gate test.
- Pointer/input wrappers no longer reconcile Astrea as part of native input delivery.
- Focus/toplevel state changes still mark the publisher dirty; the pending publication is serviced by the independent native publication deadline/domain.
- Explicit manager, handle, and client-disconnect lifecycle cleanup remains intact.

### Input and interaction hot paths

- Native pointer, keyboard, and button delivery uses no-publication wrappers.
- Active compositor interaction motion updates exact pointer/cursor state through an interaction-local path, then updates latest desired interaction geometry and sends exact grabbed-target pointer motion.
- The interaction-local path does not call generic decoration hover or scene hit testing.
- Ordinary pointer motion, implicit grabs, popup grabs, locked pointers, confined pointers, and other non-interaction semantics retain their existing ordinary routing.
- Native input processing opens one batch boundary. Internal flush requests are deferred and coalesced into at most one real client flush at batch end.
- The existing immediate flush behavior remains available outside a native input batch.

## Deterministic evidence

- The domain stress seam classified 1,000 independent input wakes as 1,000 input cycles and **0** Wayland read-dispatch cycles.
- The Astrea clean-gate test observed one gate check and one clean-gate skip, with **0** reconcile calls and **0** prune passes; after marking state dirty, the gate admitted reconciliation.
- The native input batch test verified no flush with no pending output and one coalesced flush request after two internal flush calls.
- The interaction pointer test verified local pointer-state updates while generic hover hit testing remained at **0**.
- The active interaction ordering test verified geometry update before exact grabbed-client pointer delivery.

## Verification

Passed:

```text
rtk cargo test --locked work_domains -- --test-threads=1
rtk cargo test --locked clean_publication_gate_skips_reconcile_and_pruning -- --test-threads=1
rtk cargo test --locked interaction_pointer_motion_updates_local_state_without_generic_hover_hit_testing -- --test-threads=1
rtk cargo test --locked active_window_interaction_motion_updates_geometry_before_exact_client_dispatch -- --test-threads=1
rtk cargo test --locked native_input_batch_defers_and_coalesces_write_side_flushes -- --test-threads=1
rtk cargo test --locked performance_snapshot_round_trips_resource_efficiency_field -- --test-threads=1
rtk cargo fmt --all -- --check
git diff --check
rtk cargo check --locked
TMPDIR=/tmp rtk cargo test --locked -- --test-threads=1
```

The final serialized suite completed with:

```text
2,961 passed, 5 ignored, 40 filtered out (30 suites)
```

One earlier full-suite run had an unrelated SIGCHLD timing failure; the affected test passed when rerun in isolation. The final full-suite run against the post-rearm implementation completed successfully.

`rtk cargo clippy --all-targets --all-features -- -D warnings` remains blocked by 23 pre-existing warnings/errors in unrelated dirty workspace/layout/compositor code, plus one pre-existing test warning. The newly added task code did not contribute a known clippy diagnostic after the task test warning was corrected.

## Host qualification status

No real DRM/KMS host qualification was run here. No CPU, GPU, frame-time, or wake-rate numbers are fabricated.

The remaining qualification should be run on the target host with the real backend and representative heavy clients, comparing at least:

1. idle compositor;
2. ordinary pointer motion over one stable client;
3. pointer crossing between two clients;
4. compositor-owned floating move/resize;
5. tiled/Dwindle resize;
6. pointer motion with a pending Astrea transaction;
7. concurrent Wayland client readiness and native input readiness;
8. suspended/session-transition behavior;
9. hardware and software cursor modes.

The expected observation is that input-only wakes no longer execute read-side Wayland maintenance or repeatedly advance an older Astrea transaction, while actual pointer protocol output remains promptly flushed.

## Residual risks and scope notes

- Native-only key/button events may still perform the narrow pointer-constraint follow-up `server.tick()` when pointer-constraint state can change. Native pointer motion does not use that follow-up.
- Resource-efficiency snapshots include the four native domain/tick/flush counters. Astrea gate/reconcile/prune counters are currently internal publisher metrics and test evidence rather than control-snapshot fields.
- The 1,000-wake proof is a deterministic domain/cycle-decision seam, not a hardware-backed DRM/KMS measurement.
- Suspended-session publication is serviced on its timer path; output presentation remains suppressed according to the existing session lifecycle.
- Explicit lifecycle cleanup remains the ownership boundary for Astrea resource pruning; clean pointer motion is no longer used as garbage collection.
