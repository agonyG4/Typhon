# Scene Replay Suffix-Demand Design

## Goal

Reduce ordinary scene replay work for replay-mode checkpoint effects after a direct framebuffer checkpoint capture has successfully executed. Presentation work remains active for the whole graph. Internal checkpoint work remains active until the specific capture pass that owns it succeeds; cursor position never expires a requirement.

## Chosen architecture

`scene_replay_work_plan` will build a pure, frame-static `SceneReplayWorkPlan` containing:

- `presentation_work`: the repaint rectangles;
- `baseline_work`: the existing exact `scene_work_rects` result, formed from presentation work plus every selected direct framebuffer capture domain using `is_direct_framebuffer_capture`, `clipped_output_rect`, `OutputDamage::rects`, and `disjoint_output_rects`;
- `extra_scene_work`: the existing exact difference between baseline work and presentation work, retained unchanged for clear and preservation;
- one `SceneCheckpointRequirement` per selected direct framebuffer capture, keyed by `GraphPassId` and carrying its clipped output region.

`SceneReplayWorkState` will borrow the static plan and track pending capture pass IDs. It starts with every requirement pending. In suffix-demand mode it computes presentation work plus all pending requirement regions through the existing damage normalization path. In global-baseline mode it always exposes `baseline_work`. A requirement is retired only after its direct capture's `execute_pass` returns `Ok(())`.

The state will assert exact region monotonicity in debug/test builds by subtracting the next `EffectRegion` from the previous one; bounding-box containment is not used. Unexpected pending requirements at graph completion fail an invariant in debug/test builds. Production falls back to baseline work for the final replay so an inconsistent plan cannot trade correctness for savings.

## Execution and planner parity

The executor will use one `SceneReplayWorkState` for checkpoint-dependency advances, framebuffer-capture advances, composite advances, and final replay. After every successful non-empty scene advance, `scene_valid_region` is replaced with the current active work region, preventing expired internal regions from remaining semantically valid.

`plan_effect_surface_consumers_with_debug_config` will instantiate the same static plan and state, process passes in graph order, use the state’s active work for each command range, and retire requirements at the same direct-capture boundary. Capture-index consumer planning remains unchanged.

Replay mode uses suffix demand. Debug framebuffer capture and lifecycle backdrop execution use the global baseline policy to preserve diagnostic behavior and keep this change scoped to production replay checkpoint captures. No new environment variable or user-facing legacy mode is added.

## Diagnostics

Scene replay trace events will include bounded work metrics for each ordinary and final replay:

- active and baseline rectangle counts;
- exact active and baseline pixel totals from disjoint rectangles;
- saved pixels as the saturated difference;
- pending checkpoint requirement count.

Rectangle lists and per-pixel data are not emitted. These are work-quantity metrics only, not timing claims.

## Verification strategy

Tests will first prove the pure state transitions for one checkpoint, disjoint and overlapping checkpoints, same-anchor captures, presentation overlap, no checkpoints, exact-region monotonicity, and strict reduction in a representative multi-checkpoint plan. Planner/executor parity will be asserted at equivalent pass boundaries.

The existing native-faithful stacked Dock and TopBar fixtures will run once with an internal global-baseline strategy and once with suffix demand, requiring pixel equivalence within the existing GLES tolerance and `missing_pixels=0`. A multi-checkpoint execution test will prove B remains active after A succeeds, including the same-anchor case. Existing semantic-validity, preservation, coordinate, fullscreen, and capture regressions remain unchanged.

No blur algorithm, Kawase implementation, capture domain, preservation storage, render-graph dependency semantics, or capture-index consumer policy changes are included.
