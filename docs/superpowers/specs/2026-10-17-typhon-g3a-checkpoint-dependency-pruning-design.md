# Typhon G3a: Sample-Support Checkpoint Dependency Pruning

## Goal

Add an opt-in exact-graph optimization that removes an existing checkpoint dependency only when G2's `derive_effect_sample_support()` proves that the producer's `output_influence_region` is disjoint from the consumer's exact `backdrop_region`. Keep the runtime default off.

## Design

The frame compiler first builds the existing conservative exact graph. It uses that graph for all current precomputed-backdrop eligibility and graph-construction decisions. After those decisions finish, an enabled G3a pass analyzes only consumers with dependencies that have exactly one `SceneCapture`, no `SurfaceCapture`, and non-Continuous demand. Analysis creates an immutable edge decision plan, validates dependency/pass correspondence and ordering, then applies only fully proven per-consumer changes to both the instance and SceneCapture metadata. Any missing or ambiguous metadata, unsupported topology, or sample-support error retains the original edges and is recorded as ineligible or unresolved.

`FrameExecutionCompileOptions` exposes a deterministic switch. The direct `compile_frame_execution_plan()` API remains conservative; only the ordinary EGL frame planner reads `TYPHON_EFFECT_CHECKPOINT_DEPENDENCY_PRUNING` (`off` by default, `on` opt-in). Unknown values warn once and fail closed. The precomputed graph is never pruned.

No capture layout, texture, pass, damage, sampling, terminal fusion, cache identity, or physical damage behavior changes. Cache identity continues to include ordered dependency semantic identities, so a pruned dependency-free key must populate through the existing full-domain replay path before a later stable frame can reuse it.

## Validation

Test the exact 111 px aggregate versus 67 px sampled-support false edge, true overlap, terminal-fusion parity, unsupported topologies, malformed dependency/pass metadata, precomputed eligibility isolation, and graph identity outside dependency vectors and G3a accounting. Add real GLES cold, warm, source-change, partial-repair, and cache-transition comparisons. Run existing G2 and dependency-free replay/checkpoint causal suites, then applicable repository gates with Cargo output under `/mnt/Aether/Desktop/GitHub/Typhon-target`. Keep native A/B qualification separate for cold, warm, source churn, and topology churn; leave rollout default-off unless the evidence supports a later change.

## Scope exclusions

Do not shrink capture domains or dimensions, modify G2 sampling math, damage planning, Kawase semantics, precomputed eligibility, dependency-free replay semantics, capture/downsample fusion, physical effect damage, or unrelated dirty work.
