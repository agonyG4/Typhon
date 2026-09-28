use crate::effects::{
    EffectFrameDemand, EffectNodeKind, EffectParameterImpact, EffectRegistryGeneration,
    EffectSource, RegisteredEffect,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialProgramCatalogSnapshot {
    pub registry_generation: u64,
    pub rendering_available: bool,
    pub programs: Vec<MaterialProgramCatalogEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaterialProgramCatalogEntry {
    pub name: String,
    pub origin: MaterialProgramOrigin,
    pub schema_signature: u64,
    pub parameter_count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum MaterialProgramOrigin {
    System,
    TrustedLocal,
}

impl MaterialProgramCatalogSnapshot {
    pub fn from_registry_generation(
        generation: &EffectRegistryGeneration,
        rendering_available: bool,
    ) -> Self {
        let builtin_name = crate::effects::BUILTIN_BACKGROUND_BLUR_NAME;
        let builtin = generation
            .effects
            .get(builtin_name)
            .cloned()
            .unwrap_or_else(|| RegisteredEffect {
                name: builtin_name.to_owned(),
                program: crate::effects::render_graph::builtin_background_blur_program(),
                parameters: Default::default(),
            });
        let mut programs = vec![catalog_entry(&builtin, MaterialProgramOrigin::System)];
        programs.extend(
            generation
                .effects
                .values()
                .filter(|effect| effect.name != builtin_name)
                .filter(|effect| qualifies_registered_effect(effect))
                .map(|effect| catalog_entry(effect, MaterialProgramOrigin::TrustedLocal)),
        );
        Self {
            registry_generation: generation.generation,
            rendering_available,
            programs,
        }
    }
}

pub fn qualifies_registered_effect(effect: &RegisteredEffect) -> bool {
    if effect.program.program.frame_demand != EffectFrameDemand::OnDamage
        || effect.default_parameter_block().is_err()
        || effect
            .parameters
            .values()
            .any(|parameter| parameter.spec.impact != EffectParameterImpact::UniformOnly)
    {
        return false;
    }

    let mut uses_backdrop = false;
    for node in &effect.program.program.nodes {
        if let EffectNodeKind::Source(source) = &node.kind {
            match source {
                EffectSource::Backdrop => uses_backdrop = true,
                EffectSource::TargetContent | EffectSource::StaticTexture(_) => return false,
            }
        }
    }
    uses_backdrop
}

fn catalog_entry(
    effect: &RegisteredEffect,
    origin: MaterialProgramOrigin,
) -> MaterialProgramCatalogEntry {
    MaterialProgramCatalogEntry {
        name: effect.name.clone(),
        origin,
        schema_signature: effect.parameter_schema_signature(),
        parameter_count: u16::try_from(effect.parameters.len())
            .expect("validated effect parameter count fits catalog wire type"),
    }
}
