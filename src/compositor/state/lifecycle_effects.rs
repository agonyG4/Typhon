use super::*;
use crate::effects::{EffectRect, EffectRegion};
use crate::presentation_animation::{AnimationTime, PresentationClipRect, PresentationSceneSample};
use crate::window_lifecycle_animation::LifecycleSceneSample;
use std::collections::HashMap;

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
                && fullscreen_plan.allows_composition_root(root)
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

    pub(in crate::compositor) fn resolved_effect_scene_with_frame_presentation(
        &self,
        frame_source: &FrameEffectSceneInput<'_>,
        presentation: &PresentationSceneSample,
        fullscreen_plan: &FullscreenCompositionPlan,
        lifecycle: &LifecycleSceneSample,
    ) -> ResolvedEffectScene {
        assert_eq!(
            frame_source.surfaces.len(),
            frame_source.presentation_owner_roots.len(),
            "frame effect owner roots must remain aligned with frame surfaces"
        );
        let owner_root_by_surface = frame_source
            .surfaces
            .iter()
            .zip(frame_source.presentation_owner_roots.iter().copied())
            .map(|(surface, owner_root)| (surface.surface_id, owner_root))
            .collect::<HashMap<_, _>>();
        let scene = self.resolved_effect_scene_for_frame_surfaces(
            frame_source.surfaces,
            frame_source.popup_surface_ids,
        );
        let mut instances = Vec::with_capacity(scene.instances.len());
        for mut instance in scene.instances {
            if !self.effect_instance_allows_presentation(&instance, fullscreen_plan, lifecycle) {
                continue;
            }
            let surface_id = match instance.anchor {
                EffectAnchor::BeforeSurface(surface_id)
                | EffectAnchor::ReplaceSurface(surface_id)
                | EffectAnchor::AfterSurface(surface_id) => Some(surface_id),
                EffectAnchor::OutputPostProcess => None,
            };
            let Some(surface_id) = surface_id else {
                instances.push(instance);
                continue;
            };
            let Some(owner_root) = owner_root_by_surface.get(&surface_id).copied() else {
                instances.push(instance);
                continue;
            };

            if let Some(transform) = presentation.transform_for_root(owner_root) {
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
            }

            if let Some(workspace) = frame_source.workspace_by_owner_root.get(&owner_root)
                && let Some(participant) = frame_source
                    .workspace_sample
                    .participants
                    .iter()
                    .find(|participant| participant.workspace == *workspace)
            {
                let Some(workspace_clip) = effect_rect_from_logical_clip(
                    participant.logical_cell_clip.x,
                    participant.logical_cell_clip.y,
                    participant.logical_cell_clip.width,
                    participant.logical_cell_clip.height,
                ) else {
                    continue;
                };
                let existing_clip = presentation
                    .clips
                    .iter()
                    .find(|clip| clip.root_surface_id == owner_root)
                    .and_then(|clip| clip.presented_clip);
                let clip = if let Some(existing_clip) = existing_clip {
                    let Some(existing_clip) = effect_rect_from_presentation_clip(existing_clip)
                    else {
                        continue;
                    };
                    let Some(clip) = workspace_clip.intersect(existing_clip) else {
                        continue;
                    };
                    clip
                } else {
                    workspace_clip
                };
                let translation_x = participant.translation_x.round() as i32;
                let translation_y = participant.translation_y.round() as i32;
                instance.region = translate_and_clip_effect_region(
                    &instance.region,
                    translation_x,
                    translation_y,
                    clip,
                );
                let Some(target_bounds) =
                    translate_effect_rect(instance.target_bounds, translation_x, translation_y)
                        .and_then(|bounds| bounds.intersect(clip))
                else {
                    continue;
                };
                instance.target_bounds = target_bounds;
                if let Some(coverage) = instance.coverage.as_ref() {
                    instance.coverage =
                        coverage.translated(f64::from(translation_x), f64::from(translation_y));
                }
                instance.signature = instance.signature.wrapping_mul(0x0000_0100_0000_01b3)
                    ^ frame_source.workspace_sample.signature
                    ^ participant.workspace_index as u64;
            }
            instances.push(instance);
        }

        let mut resolved = ResolvedEffectScene::new(scene.generation, instances);
        resolved.signature = resolved.signature.wrapping_mul(0x0000_0100_0000_01b3)
            ^ frame_source.workspace_sample.signature;
        resolved
    }
}

fn effect_rect_from_logical_clip(x: f64, y: f64, width: f64, height: f64) -> Option<EffectRect> {
    effect_rect_from_float_bounds(x, y, width, height, true)
}

fn effect_rect_from_presentation_clip(clip: PresentationClipRect) -> Option<EffectRect> {
    effect_rect_from_float_bounds(clip.x(), clip.y(), clip.width(), clip.height(), true)
}

fn effect_rect_from_float_bounds(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    inward: bool,
) -> Option<EffectRect> {
    if !x.is_finite() || !y.is_finite() || !width.is_finite() || !height.is_finite() {
        return None;
    }
    let (left, top, right, bottom) = if inward {
        (
            x.ceil(),
            y.ceil(),
            (x + width).floor(),
            (y + height).floor(),
        )
    } else {
        (
            x.floor(),
            y.floor(),
            (x + width).ceil(),
            (y + height).ceil(),
        )
    };
    let left = left.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let top = top.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let right = right.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    let bottom = bottom.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    EffectRect::new(
        left,
        top,
        u32::try_from(i64::from(right) - i64::from(left)).ok()?,
        u32::try_from(i64::from(bottom) - i64::from(top)).ok()?,
    )
}

fn translate_effect_rect(rect: EffectRect, dx: i32, dy: i32) -> Option<EffectRect> {
    let left = i64::from(rect.x) + i64::from(dx);
    let top = i64::from(rect.y) + i64::from(dy);
    let right = left + i64::from(rect.width);
    let bottom = top + i64::from(rect.height);
    let left = left.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    let top = top.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    let right = right.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    let bottom = bottom.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    EffectRect::new(
        left as i32,
        top as i32,
        u32::try_from(right - left).ok()?,
        u32::try_from(bottom - top).ok()?,
    )
}

fn translate_and_clip_effect_region(
    region: &EffectRegion,
    dx: i32,
    dy: i32,
    clip: EffectRect,
) -> EffectRegion {
    if region.rects().is_empty() && !region.is_empty() {
        return EffectRegion::from_rect(clip);
    }
    let mut translated = EffectRegion::empty();
    for rect in region.rects() {
        if let Some(rect) =
            translate_effect_rect(*rect, dx, dy).and_then(|rect| rect.intersect(clip))
        {
            translated.push(rect);
        }
    }
    translated
}
