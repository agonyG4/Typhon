use super::{EffectAnchor, effects::ResolvedEffectScene};
use crate::effects::{EffectRegistryGeneration, builtin_background_blur_program_id};

pub(super) fn effect_details(
    scene: &ResolvedEffectScene,
    trusted_effect_registry: &EffectRegistryGeneration,
) -> (u32, Vec<String>, bool) {
    const MAX_EFFECTS: usize = 32;
    let details = scene
        .instances
        .iter()
        .take(MAX_EFFECTS)
        .map(|instance| {
            let (anchor, target_surface) = match instance.anchor {
                EffectAnchor::BeforeSurface(surface_id) => (
                    format!("before_surface:{surface_id}"),
                    Some(surface_id),
                ),
                EffectAnchor::ReplaceSurface(surface_id) => (
                    format!("replace_surface:{surface_id}"),
                    Some(surface_id),
                ),
                EffectAnchor::AfterSurface(surface_id) => (
                    format!("after_surface:{surface_id}"),
                    Some(surface_id),
                ),
                EffectAnchor::OutputPostProcess => ("output_post_process".to_string(), None),
            };
            let region = if instance.region.rects().is_empty() {
                "full".to_string()
            } else {
                instance
                    .region
                    .rects()
                    .iter()
                    .map(|rect| format!("{},{},{},{}", rect.x, rect.y, rect.width, rect.height))
                    .collect::<Vec<_>>()
                    .join(";")
            };
            let program_name = trusted_effect_registry
                .effect_for_program(instance.program)
                .map(|effect| effect.name.as_str())
                .or_else(|| {
                    (instance.program == builtin_background_blur_program_id())
                        .then_some("background_blur")
                })
                .unwrap_or("unknown");
            format!(
                "{{id:{} program:{} program_id:{} anchor:{} region:{} target_surface:{} requires_composition:{}}}",
                instance.id.get(),
                program_name,
                instance.program.get(),
                anchor,
                region,
                target_surface.map_or_else(|| "none".to_string(), |id| id.to_string()),
                scene.summary.requires_composition,
            )
        })
        .collect::<Vec<_>>();
    (
        scene.summary.visible_instance_count,
        details,
        scene.instances.len() > MAX_EFFECTS,
    )
}
