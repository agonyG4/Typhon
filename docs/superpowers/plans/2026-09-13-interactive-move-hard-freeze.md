# Interactive move hard-freeze investigation

## Scope

Trace the post-cursor-authority interactive-move path through cursor planning,
predictive primary ownership, worker transport, pageflip settlement, and the
effect graph. Preserve raw-input coalescing and avoid policy workarounds.

## Evidence-led findings

1. A worker-queued Atomic primary keeps `atomic_commit_pending()` true. The
   cursor classifier previously treated that lane state as an unchanged-base
   failure, so an x/y-only hardware cursor move was promoted from
   `PositionOnly` to `Visual` while the primary was still attachable.
2. The predictive swapchain revalidation check compared the physical ACK and
   future-owner claims in the wrong direction. When frame N was worker-queued
   and frame N+1 was ready, the valid ACK for N was rejected as overtaking the
   future owner.

## Verification plan

- Keep a focused classifier regression for an attachable worker primary.
- Exercise hundreds of predictive worker move iterations with primary frames,
  frozen cursor owners, and cursor framebuffer pins.
- Exercise scheduler arbitration with continuous primary and cursor work.
- Run existing worker, sidecar, pageflip, swapchain, cursor, compositor, and
  effect/resource suites, followed by the required repository checks.
- Qualify native A/B only when the reproducible launch path and machine are
  available; do not claim hardware freeze closure from unit tests alone.
