use super::*;
use crate::presentation_animation::{AnimationTime, PresentationSceneSample};
use crate::window_lifecycle_animation::LifecycleSceneSample;

impl CompositorState {
    #[cfg(test)]
    pub(in crate::compositor) fn resolved_effect_scene_for_composition_plan(
        &self,
        fullscreen_plan: &FullscreenCompositionPlan,
    ) -> ResolvedEffectScene {
        let lifecycle = self.lifecycle_scene_sample_at(
            AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0)),
        );
        self.resolved_effect_scene_for_composition_plan_with_lifecycle(fullscreen_plan, &lifecycle)
    }

    pub(in crate::compositor) fn resolved_effect_scene_for_composition_plan_with_lifecycle(
        &self,
        fullscreen_plan: &FullscreenCompositionPlan,
        lifecycle: &LifecycleSceneSample,
    ) -> ResolvedEffectScene {
        let scene = self.resolved_effect_scene();
        let instances = scene
            .instances
            .into_iter()
            .filter(|instance| {
                self.effect_instance_allows_presentation(instance, fullscreen_plan, lifecycle)
            })
            .collect();
        ResolvedEffectScene::new(scene.generation, instances)
    }

    pub(in crate::compositor) fn effect_instance_allows_presentation(
        &self,
        instance: &ResolvedEffectInstance,
        fullscreen_plan: &FullscreenCompositionPlan,
        lifecycle: &LifecycleSceneSample,
    ) -> bool {
        let root_surface_id = match instance.anchor {
            EffectAnchor::BeforeSurface(surface_id)
            | EffectAnchor::ReplaceSurface(surface_id)
            | EffectAnchor::AfterSurface(surface_id) => {
                Some(self.root_surface_id_for_surface(surface_id))
            }
            EffectAnchor::OutputPostProcess => None,
        };
        root_surface_id.is_none_or(|root| {
            !lifecycle.restore_suppresses_root(root)
                && fullscreen_plan.allows_presentation_root(root)
        })
    }

    pub(in crate::compositor) fn resolved_effect_scene_with_presentation(
        &self,
        presentation: &PresentationSceneSample,
        fullscreen_plan: &FullscreenCompositionPlan,
    ) -> ResolvedEffectScene {
        let lifecycle = self.lifecycle_scene_sample_at(
            AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0)),
        );
        self.resolved_effect_scene_with_presentation_and_lifecycle(
            presentation,
            fullscreen_plan,
            &lifecycle,
        )
    }

    pub(in crate::compositor) fn resolved_effect_scene_with_presentation_and_lifecycle(
        &self,
        presentation: &PresentationSceneSample,
        fullscreen_plan: &FullscreenCompositionPlan,
        lifecycle: &LifecycleSceneSample,
    ) -> ResolvedEffectScene {
        let scene = self
            .resolved_effect_scene_for_composition_plan_with_lifecycle(fullscreen_plan, lifecycle);
        let instances = scene
            .instances
            .into_iter()
            .map(|mut instance| {
                let surface_id = match instance.anchor {
                    EffectAnchor::BeforeSurface(surface_id)
                    | EffectAnchor::ReplaceSurface(surface_id)
                    | EffectAnchor::AfterSurface(surface_id) => Some(surface_id),
                    EffectAnchor::OutputPostProcess => None,
                };
                let Some(transform) = surface_id
                    .map(|surface_id| self.root_surface_id_for_surface(surface_id))
                    .and_then(|root| presentation.transform_for_root(root))
                else {
                    return instance;
                };

                instance.region = crate::compositor::effects::map_effect_region(
                    transform,
                    &instance.region,
                    instance.target_bounds,
                );
                if let Some(target_bounds) =
                    crate::compositor::effects::map_effect_rect(transform, instance.target_bounds)
                {
                    instance.target_bounds = target_bounds;
                }
                instance.signature =
                    instance.signature.wrapping_mul(0x0000_0100_0000_01b3) ^ transform.signature();
                instance
            })
            .collect();
        ResolvedEffectScene::new(scene.generation, instances)
    }
}
