# Direct Scanout Effect Contribution Analysis

## Goal

Allow Direct Scanout when effects exist in generic compositor state but cannot alter the frame that the exact opaque scanout source would present. Preserve generic effect rendering semantics and every existing Direct Scanout safety authority.

## Architecture

`ResolvedEffectScene::new` remains the generic rendering summary: any visible instance keeps `requires_composition` true. The Direct Scanout path receives a separate typed `DirectScanoutEffectAnalysis` stored on `DirectScanoutSceneAnalysis`.

The renderer and Direct Scanout share one canonical effect-scene helper that applies lifecycle suppression and `FullscreenCompositionPlan::allows_presentation_root`. The renderer then applies presentation transforms. Direct Scanout uses the canonical, untransformed scene because it already rejects pending or non-identity presentation/lifecycle states and must not authorize scanout from an unproven transformed scene.

Direct Scanout first identifies the exact presentation-coverage source and retains the existing coverage and opaque-format proof. Only then does it classify effects relative to that source. The result is both the blocker authority and the doctor input; doctor does not reconstruct effect visibility or ordering.

## Typed contribution analysis

The aggregate contains raw visible count, presentation-visible count, presentation-culled count, outside-output count, occluded count, contributing count, and `requires_composition`. Each bounded instance has a typed disposition:

- `PresentationCulled`
- `OutsideOutput`
- `OccludedByOpaqueScanoutSource`
- `ContributingAboveSource`
- `ContributingAtSource`
- `OutputPostProcess`
- `UnknownOrder`

Zero raw visible effects return the default analysis without reconstructing a scene. Unknown order, missing source ordering, or missing opacity/coverage proof remains conservative and contributes to composition demand.

## Ordering rules

Ordering uses `EffectSceneOrder`, `VisualGroupId`, and active scene surface order, never raw surface IDs or creation order.

- An effect group below the source group is occluded only when the source is proven opaque and covers the output. A group above contributes.
- In the same group, surface-scoped effects compare anchored surface order with the source surface. Lower surfaces are occluded; higher surfaces contribute. At the source surface, `BeforeSurface` is occluded while `ReplaceSurface` and `AfterSurface` contribute.
- Visual-group-scoped effects use group semantics. In the source group, `BeforeSurface` is below group-visible content and may be occluded; `ReplaceSurface` and `AfterSurface` contribute. Surface order is not used as a substitute for group semantics.
- `OutputPostProcess` always contributes when its region intersects the output and is never occluded by the source.

For every presentation-visible instance, `region.intersect_rect(output_bounds)` determines output visibility. Conservative-full regions remain conservative and therefore intersect the output. Empty intersection is `OutsideOutput`.

## Integration and observability

The old raw summary-based effect blocker is removed from scene eligibility. The new analysis adds `EffectRequiresComposition` only when `contributing_instance_count > 0`; all unrelated blockers stay unchanged.

Doctor retains `effects_visible_instance_count` and adds presentation, culling, outside, occluded, contributing, and `effect_requires_composition` fields. Bounded effect entries include the typed disposition rendered as debug text. These values come from the exact analysis stored on the scene result.

## Tests

Update the existing resolved-blur Direct Scanout regression to expect one presentation-visible, occluded, non-contributing effect. Add coverage for fullscreen-plan culling, output post-process, Surface and VisualGroup anchor phases, below/above visual groups, outside and partial regions, conservative-full regions, unknown/translucent source coverage, zero-effect fast path, and doctor/eligibility consistency. Keep the existing renderer presentation-filter test and all DMA-BUF/KMS tests unchanged.

## Non-goals

No blur, Eclipse, fullscreen-application, or program whitelist; no partial scanout or hardware overlay work; no changes to DMA-BUF feedback, modifier filtering, GBM import, KMS validation, presentation coverage, opacity proof, or render-graph execution.
