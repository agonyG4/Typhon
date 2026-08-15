# Typhon Atomic Rendering & Predictive Triple Buffering Performance Closure

## Status

Approved for implementation from the task specification. This design is scoped to
the current `main` checkout at `2498ae0` and preserves unrelated dirty-worktree
changes.

## Evidence and constraints

- The explicit Atomic EGL/GBM path owns compositor-rendered GBM framebuffer slots;
  it does not present through an EGLSurface swap.
- `PartialRepaintPlanner` currently gates partial repair on both buffer age and
  EGL swap-damage support, conflating render repair with presentation submission.
- `AtomicOutputSlot` records a per-slot presented serial, but its age query is
  currently invalidated by a global pending-presentation boolean.
- KMS jobs already capture a pacing frame ID at admission, while settlement
  currently resolves the mutable active/ready role again. This is the suspected
  trace assertion root cause and will be proven with a role-transition test.
- Existing `TimingSummary` and paint statistics are the bounded instrumentation
  foundation. No per-frame text logging or profiling framework will be added.
- Unknown lineage always falls back to a full repaint. A failed or unpresented
  render cannot advance presented damage history.

## Design

### Pacing identity

Treat worker admission as an immutable pacing reservation. The captured frame ID
is the identity settled by submit, rejection, cancellation, or stale-job checks.
Settlement removes that exact ID from whichever active/ready role currently owns
it; it does not infer identity from the role selected at settlement time. A role
transition may therefore change scheduling state without changing the job's
identity. Missing or mismatched IDs remain errors and must not mutate a newer
frame. Trace output observes the same transitions and cannot change them.

### Capability model

Represent these independently:

1. buffer-age/target-lineage availability;
2. partial render repair capability;
3. EGLSurface swap-damage submission capability.

The legacy EGLSurface backend maps render repair to the capabilities it can prove
for its swapchain and retains swap-damage submission separately. The explicit
Atomic FBO backend may advertise render repair without swap-damage submission only
when its persistent slot, exact identity, generation, geometry, and presented
history invariants hold.

### Slot-local age and damage lineage

An acquired slot's age derives only from that slot's last successfully presented
serial and the current presented serial. Another slot being pending does not
invalidate it. Presented damage history advances only at confirmed pageflip
settlement. Render-ahead damage is not committed as presented early.

The planner repairs an acquired slot with its current scene difference plus the
bounded history required by that slot's age. A slot with unknown, quarantined,
failed, stale-generation, mode-changed, resize-invalidated, or direct-scanout
transitioned lineage receives age zero/full repaint. This is conservative per
slot, not a global output-wide invalidation.

### Bounded observability

Extend existing aggregate timing/paint accounting with a focused diagnostic
snapshot. It will expose render timing percentiles, repaint mode/reason counts,
age buckets, repair/full pixel totals, buffering counters, and existing KMS
aggregate summaries without requiring `OBLIVION_ONE_PERF_LOG=1`.

### Predictive Triple

Do not tune the predictor before the renderer and lineage fixes are tested. Keep
ordinary VSync as the prerequisite for Reactive Double/Predictive Triple and
preserve async/tearing routing. Predictor policy remains unchanged unless a
repeatable post-fix benchmark shows a material regression and its reason counters
identify an evidence-backed policy defect.

## TDD and validation sequence

1. Add failing pacing tests for active-to-ready role transition, exact settlement,
   stale rejection, cancellation, and exact-once behavior; implement the minimum
   immutable-ID settlement change.
2. Add failing capability and buffer-age tests; split capability gates and remove
   only the global pending invalidation after slot-local tests prove the safe
   fallback behavior.
3. Add bounded telemetry tests and a control-snapshot path.
4. Add damage-lineage/model tests covering render-ahead, pageflip-only history
   commit, rejected frames, generation reset, direct-scanout transitions, and
   partial-vs-full repair coverage.
5. Run focused tests, then the full repository validation suite.
6. Run native Wayland qualification and the fixed A/B benchmark when the TTY
   session permits it. Record exact environment and any unavailable metrics as
   `N/A`; never substitute measurements from another compositor.

## Explicit non-goals

No XWayland/selection/Eclipse work, SHM clone refactor, scene-cache rewrite,
real-time scheduling change, VRR/tearing protocol change, multi-output work,
Direct Scanout eligibility change, or generic percentile micro-optimization.
