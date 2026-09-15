# Phase 1A: Logical Output Identity Foundation

## Goal

Introduce an explicit, typed logical `OutputId` and qualify the native-output boundaries where output identity is required for correctness, while preserving Typhon’s current single-output visual, scheduling, KMS, rendering, pacing, cursor, and Direct Scanout behavior.

## Current repository truth

- Starting HEAD: `b7de3ff3b1a4872a2b76f2296ab5c96ab985d50c`
- Starting worktree: three unrelated pre-existing modifications in
  `src/compositor/state/subsurfaces.rs`, `src/compositor/subsurface.rs`, and
  `src/compositor/tests/surface_frames.rs`; all are outside this phase’s edit
  set and must remain untouched.
- The current native product has one implicit `PhysicalOutputId(0)` in surface membership.
- `drm_file_generation` is already distinct from logical output identity and changes during session recovery.
- `NativeSceneHistory` is the existing physical scene authority for ready → submitted → presented transitions.
- `OutputTransactionLedger` is the existing transaction authority; transaction semantics and admission policy must remain unchanged.
- The source-layout check currently reports pre-existing oversized-file violations; this phase must not add new violations.

## Selected approach

Use a cheap scalar identity modeled on `WindowId`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OutputId(NonZeroU64);
```

`OutputIdAllocator` owns monotonic allocation, rejects exhaustion, and never recycles IDs. The current compositor state creates the one logical native output identity during server/native-output bootstrap, and `NativeRuntime` stores the allocated value for all native-output work. Backend/session recovery updates only `drm_file_generation`; it does not allocate another `OutputId`.

The output membership state will store `HashSet<OutputId>`. The single-output reconciliation path will insert/remove the real native ID, while `entered_resources: HashSet<u32>` remains the separate per-client `wl_output` resource bookkeeping.

`NativeFrameSceneSnapshot` will carry `output_id`. `NativeSceneHistory` will retain one bound output ID and reject a ready snapshot from another output. Rejection is deterministic and leaves existing physical history untouched. Pageflip promotion continues to be the only path that advances presented scene authority.

`OutputTransactionLedger` will be bound to one `OutputId`. Physical identity records that cross the KMS/output boundary will carry `OutputId` alongside their existing `PageFlipToken`, KMS bundle ID, DRM generation, CRTC ID, and transaction IDs. Direct Scanout candidate/validation keys and cursor capability keys will likewise include `OutputId`; their eligibility and hardware policy remain unchanged.

## Alternatives considered

1. Keep the singleton and add comments: rejected because it leaves output-zero aliasing in correctness boundaries.
2. Use `{ index, generation }` slot identities: rejected because the current product has no output reuse requirement and this would conflate logical identity with lifecycle generations.
3. Add a full per-output runtime container: deferred to Phase 1B because it would create broad mechanical churn and risk behavior changes unrelated to identity.

## Boundaries intentionally unchanged

- `drm_file_generation`: active DRM/backend binding lifetime; still invalidates backend resources on recovery.
- `pool_generation`: scanout/swapchain resource ownership.
- `render_generation`: renderable scene/content generation.
- `frame_id`: produced compositor frame identity/order.
- `OutputTransactionId`, `PageFlipToken`, KMS bundle ID, CRTC, and surface generations: their existing lifecycle domains.
- `ResolvedNativeFrameScene`: remains output-agnostic because resolution is already confined to the native output operation.
- Native scene ordering, damage, rendering, scheduling, pacing, VRR, FIFO, tearing, KMS worker policy, Direct Scanout eligibility, and cursor policy.

## Testing strategy

Tests will be written before production changes in focused cycles. Coverage will include:

- `OutputId` zero rejection, monotonic allocation, ordering/hash behavior, and separation from Wayland resource IDs.
- Surface enter/leave with real synthetic `OutputId` values while preserving `wl_output` resource bookkeeping.
- Snapshot identity preservation through scene history and rejection of wrong-output ready/pageflip paths.
- Session recovery retaining `OutputId` while changing DRM generation and continuing to invalidate stale backend state.
- Output-bound transaction/KMS/pageflip identity rejection across synthetic outputs with equal backend generations.
- Direct Scanout and cursor cache keys not aliasing across logical outputs, with existing eligibility/policy tests unchanged.

## Scope

This phase does not implement multi-monitor product behavior, hotplug lifecycle, a per-output runtime container, generic scene nodes, scene projections, a generalized Presentation Engine, new animation/effect behavior, or any new scheduling/KMS/rendering policy.
