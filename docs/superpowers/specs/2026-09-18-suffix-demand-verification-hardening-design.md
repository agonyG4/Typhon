# Suffix-Demand Verification Hardening Design

## Goal

Harden execution-level verification of the existing suffix-demand scene replay architecture without changing its work-plan geometry, checkpoint domains, Kawase behavior, scene-work preservation, or render-graph dependency semantics.

## Current architecture and authority

`SceneReplayWorkPlan` defines presentation and checkpoint work. `SceneReplayWorkState` is the shared authority used by both `plan_effect_surface_consumers_with_debug_config` and `execute_graph_passes_inner`. A direct framebuffer capture retires its requirement only after its `execute_pass` succeeds. `draw_effect_scene_range` consumes the current `active_work`, and checkpoint validity is checked by `checkpoint_source_semantic_validity`.

The verification changes will preserve this pass-based lifetime and will not introduce a second state machine or duplicate suffix geometry.

## Test design

- Keep the pure `SceneReplayWorkState` tests unchanged.
- Reuse the existing native-faithful A/B helper for both `native_dock_fixture()` and `native_topbar_fixture()`. Each run compares final framebuffer bytes within the existing tolerance, requires zero missing checkpoint pixels, verifies baseline replay intervals save zero pixels, and verifies suffix replay saves pixels when removable work exists.
- Use the existing compiler-backed three-effect fixture with independent background, A, C, and B surfaces. Assert three distinct composition ranges, naturally compiled checkpoint dependencies, progressive pending counts, shrinking active work, zero missing pixels for every capture, and final presentation-only replay.
- Use the same fixture with C and B sharing an anchor. Assert executor trace evidence that C retires while B remains pending, then B retires; compare GlobalBaseline and SuffixDemand pixels and require zero missing pixels for the later capture.
- Add RED checks by applying temporary local mutations to requirement retirement and same-anchor behavior, recording the expected failing evidence, and restoring the production implementation.

## Planner finalization parity

Retain the executor’s existing pending-requirement invariant and release fallback. Ensure trailing surface-consumer planning checks the same `SceneReplayWorkState` before consuming trailing work. In debug/test, the inconsistent state remains an invariant failure; in release-style semantics, the state is forced to baseline before the trailing range is planned. The focused planner regression will prove that pending requirements cannot cause under-planning of baseline-required surfaces.

## Scope and non-goals

Expected production changes are limited to the existing executor/planner path and, only if needed, bounded trace assertions. No changes will be made to `SceneReplayWorkPlan` geometry, `SceneReplayWorkState` lifetime rules, checkpoint capture domains, render-graph dependency compilation, Kawase, damage planning, resource pools, or scene-work preservation.

## Verification

Run the new focused GLES and planner tests first, then the existing suffix-demand, checkpoint validity, scene-work preservation, dependency, and framebuffer-origin tests. Finish with `rtk cargo fmt --check`, workspace check, workspace clippy with warnings denied, and the requested native workload if hardware is available. Report clippy failures by total, modified-file failures, and pre-existing out-of-scope failures. Report native trace work-pixel totals and GPU timing separately.
