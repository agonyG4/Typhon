//! Immutable trusted effect-registry generations.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, RwLock},
};

use super::{
    BUILTIN_EFFECT_PROGRAM_ID, EffectFrameDemand, EffectNodeKind, EffectParameterId,
    EffectParameterImpact, EffectParameterRange, EffectParameterType, EffectProgramId,
    EffectUniformValue, INTERNAL_EFFECT_SHADER_MODULE_IDS, MAX_EFFECT_PROGRAMS, ShaderModuleId,
    ValidatedEffectProgram,
    config::{
        EffectConfigError, EffectDefinition, EffectManifest, EffectParameterDefinition,
        load_manifest,
    },
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffectRegistry {
    programs: HashMap<EffectProgramId, ValidatedEffectProgram>,
}

impl EffectRegistry {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_builtin_background_blur() -> Self {
        let mut registry = Self::empty();
        registry
            .insert(super::render_graph::builtin_background_blur_program())
            .expect("builtin effect registry has capacity");
        registry
    }

    pub fn insert(&mut self, program: ValidatedEffectProgram) -> Result<(), EffectRegistryError> {
        if self.programs.len() >= MAX_EFFECT_PROGRAMS
            && !self.programs.contains_key(&program.program.id)
        {
            return Err(EffectRegistryError::Full);
        }
        self.programs.insert(program.program.id, program);
        Ok(())
    }

    pub fn get(&self, id: EffectProgramId) -> Option<&ValidatedEffectProgram> {
        self.programs.get(&id)
    }
    pub fn len(&self) -> usize {
        self.programs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.programs.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectRegistryError {
    Full,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredEffect {
    pub name: String,
    pub program: ValidatedEffectProgram,
    pub parameters: BTreeMap<String, super::config::EffectParameterDefinition>,
}

impl RegisteredEffect {
    /// Build the renderer parameter block from the validated manifest defaults.
    pub fn default_parameter_block(
        &self,
    ) -> Result<super::EffectParameterBlock, super::EffectValidationError> {
        super::EffectParameterBlock::from_values(
            self.parameters
                .values()
                .map(|parameter| (parameter.spec.id, parameter.default)),
        )
    }

    pub fn schema_signature(&self) -> u64 {
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        schema_mix(&mut signature, self.program.program.id.get());
        schema_mix(
            &mut signature,
            match self.program.program.frame_demand {
                EffectFrameDemand::OnDamage => 0,
                EffectFrameDemand::Continuous => 1,
                EffectFrameDemand::Manual => 2,
            },
        );
        for (name, parameter) in &self.parameters {
            for byte in name.bytes() {
                schema_mix(&mut signature, u64::from(byte));
            }
            schema_mix(&mut signature, 0xff);
            schema_mix(&mut signature, parameter.spec.id.get() as u64);
            schema_mix(
                &mut signature,
                match parameter.spec.ty {
                    EffectParameterType::Float => 0,
                    EffectParameterType::Vec2 => 1,
                    EffectParameterType::Vec3 => 2,
                    EffectParameterType::Vec4 => 3,
                    EffectParameterType::Int => 4,
                },
            );
            match parameter.spec.range {
                None => schema_mix(&mut signature, 0),
                Some(EffectParameterRange::Float { min, max }) => {
                    schema_mix(&mut signature, 1);
                    schema_mix(&mut signature, u64::from(min.to_bits()));
                    schema_mix(&mut signature, u64::from(max.to_bits()));
                }
                Some(EffectParameterRange::FloatComponents {
                    min,
                    max,
                    components,
                }) => {
                    schema_mix(&mut signature, 2);
                    schema_mix(&mut signature, u64::from(components));
                    for value in min.into_iter().chain(max) {
                        schema_mix(&mut signature, u64::from(value.to_bits()));
                    }
                }
                Some(EffectParameterRange::Int { min, max }) => {
                    schema_mix(&mut signature, 3);
                    schema_mix(&mut signature, min as u32 as u64);
                    schema_mix(&mut signature, max as u32 as u64);
                }
            }
            schema_mix(
                &mut signature,
                match parameter.spec.impact {
                    EffectParameterImpact::UniformOnly => 0,
                    EffectParameterImpact::Footprint => 1,
                    EffectParameterImpact::Structure => 2,
                },
            );
            match parameter.default {
                EffectUniformValue::Float(value) => {
                    schema_mix(&mut signature, 0);
                    schema_mix(&mut signature, u64::from(value.to_bits()));
                }
                EffectUniformValue::Vec2(values) => {
                    schema_mix(&mut signature, 1);
                    for value in values {
                        schema_mix(&mut signature, u64::from(value.to_bits()));
                    }
                }
                EffectUniformValue::Vec3(values) => {
                    schema_mix(&mut signature, 2);
                    for value in values {
                        schema_mix(&mut signature, u64::from(value.to_bits()));
                    }
                }
                EffectUniformValue::Vec4(values) => {
                    schema_mix(&mut signature, 3);
                    for value in values {
                        schema_mix(&mut signature, u64::from(value.to_bits()));
                    }
                }
                EffectUniformValue::Int(value) => {
                    schema_mix(&mut signature, 4);
                    schema_mix(&mut signature, value as u32 as u64);
                }
            }
        }
        signature
    }

    /// Stable semantic compatibility for persisted Material Program parameters.
    ///
    /// This deliberately excludes renderer identity and implementation details:
    /// program/parameter IDs, graph nodes, shader modules and sources, and frame
    /// demand do not affect whether named parameter overrides remain compatible.
    pub fn parameter_schema_signature(&self) -> u64 {
        let mut signature = 0xcbf2_9ce4_8422_2325_u64;
        for byte in self.name.bytes() {
            schema_mix(&mut signature, u64::from(byte));
        }
        schema_mix(&mut signature, 0xff);
        schema_mix(&mut signature, self.parameters.len() as u64);

        for (name, parameter) in self.parameters.iter().take(super::MAX_EFFECT_PARAMETERS) {
            for byte in name.bytes() {
                schema_mix(&mut signature, u64::from(byte));
            }
            schema_mix(&mut signature, 0xff);
            schema_mix(
                &mut signature,
                match parameter.spec.ty {
                    EffectParameterType::Float => 0,
                    EffectParameterType::Vec2 => 1,
                    EffectParameterType::Vec3 => 2,
                    EffectParameterType::Vec4 => 3,
                    EffectParameterType::Int => 4,
                },
            );
            match parameter.spec.range {
                None => schema_mix(&mut signature, 0),
                Some(EffectParameterRange::Float { min, max }) => {
                    schema_mix(&mut signature, 1);
                    schema_mix(&mut signature, u64::from(min.to_bits()));
                    schema_mix(&mut signature, u64::from(max.to_bits()));
                }
                Some(EffectParameterRange::FloatComponents {
                    min,
                    max,
                    components,
                }) => {
                    schema_mix(&mut signature, 2);
                    schema_mix(&mut signature, u64::from(components));
                    for value in min.into_iter().chain(max) {
                        schema_mix(&mut signature, u64::from(value.to_bits()));
                    }
                }
                Some(EffectParameterRange::Int { min, max }) => {
                    schema_mix(&mut signature, 3);
                    schema_mix(&mut signature, min as u32 as u64);
                    schema_mix(&mut signature, max as u32 as u64);
                }
            }
            schema_mix(
                &mut signature,
                match parameter.spec.impact {
                    EffectParameterImpact::UniformOnly => 0,
                    EffectParameterImpact::Footprint => 1,
                    EffectParameterImpact::Structure => 2,
                },
            );
            match parameter.default {
                EffectUniformValue::Float(value) => {
                    schema_mix(&mut signature, 0);
                    schema_mix(&mut signature, u64::from(value.to_bits()));
                }
                EffectUniformValue::Vec2(values) => {
                    schema_mix(&mut signature, 1);
                    for value in values {
                        schema_mix(&mut signature, u64::from(value.to_bits()));
                    }
                }
                EffectUniformValue::Vec3(values) => {
                    schema_mix(&mut signature, 2);
                    for value in values {
                        schema_mix(&mut signature, u64::from(value.to_bits()));
                    }
                }
                EffectUniformValue::Vec4(values) => {
                    schema_mix(&mut signature, 3);
                    for value in values {
                        schema_mix(&mut signature, u64::from(value.to_bits()));
                    }
                }
                EffectUniformValue::Int(value) => {
                    schema_mix(&mut signature, 4);
                    schema_mix(&mut signature, value as u32 as u64);
                }
            }
        }
        signature
    }
}

fn schema_mix(signature: &mut u64, value: u64) {
    *signature = signature.wrapping_mul(0x1000_0000_01b3) ^ value;
}

#[derive(Clone, Debug, PartialEq)]
pub struct TrustedShaderAsset {
    pub module: ShaderModuleId,
    pub relative_path: std::path::PathBuf,
    pub source: String,
    pub uniforms: Vec<super::EffectUniformBinding>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectRegistryGeneration {
    pub generation: u64,
    pub registry: EffectRegistry,
    pub effects: BTreeMap<String, RegisteredEffect>,
    pub shaders: BTreeMap<ShaderModuleId, TrustedShaderAsset>,
}

impl EffectRegistryGeneration {
    pub fn with_builtin_background_blur() -> Self {
        Self::with_builtin_background_material(
            crate::material::MaterialConfiguration::default()
                .effective()
                .expect("built-in material configuration validates"),
        )
    }

    pub fn with_builtin_background_material(material: crate::material::EffectiveMaterial) -> Self {
        let program = super::validate_effect_program(
            super::render_graph::builtin_background_material_program(material).program,
        )
        .expect("built-in material effect program must validate");
        let mut registry = EffectRegistry::empty();
        registry
            .insert(program.clone())
            .expect("builtin effect registry has capacity");
        let mut effects = BTreeMap::new();
        effects.insert(
            super::render_graph::BUILTIN_BACKGROUND_BLUR_NAME.to_string(),
            RegisteredEffect {
                name: super::render_graph::BUILTIN_BACKGROUND_BLUR_NAME.to_string(),
                program,
                parameters: BTreeMap::new(),
            },
        );
        Self {
            generation: 1,
            registry,
            effects,
            shaders: BTreeMap::new(),
        }
    }

    pub fn empty() -> Self {
        Self {
            generation: 0,
            registry: EffectRegistry::empty(),
            effects: BTreeMap::new(),
            shaders: BTreeMap::new(),
        }
    }

    pub fn program(&self, name: &str) -> Option<&ValidatedEffectProgram> {
        self.effects.get(name).map(|effect| &effect.program)
    }

    pub fn program_id(&self, name: &str) -> Option<EffectProgramId> {
        self.program(name).map(|program| program.program.id)
    }

    pub fn effect_for_program(&self, id: EffectProgramId) -> Option<&RegisteredEffect> {
        self.effects
            .values()
            .find(|effect| effect.program.program.id == id)
    }

    pub fn schema_signature(&self, id: EffectProgramId) -> Option<u64> {
        self.effect_for_program(id)
            .map(RegisteredEffect::schema_signature)
    }
}

#[derive(Debug)]
pub struct TrustedEffectRegistry {
    current: RwLock<Arc<EffectRegistryGeneration>>,
}

impl Default for TrustedEffectRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TrustedEffectRegistry {
    pub fn new() -> Self {
        Self {
            current: RwLock::new(Arc::new(EffectRegistryGeneration::empty())),
        }
    }

    pub fn with_builtin_background_blur() -> Self {
        Self {
            current: RwLock::new(Arc::new(
                EffectRegistryGeneration::with_builtin_background_blur(),
            )),
        }
    }

    pub fn with_builtin_background_material(material: crate::material::EffectiveMaterial) -> Self {
        Self {
            current: RwLock::new(Arc::new(
                EffectRegistryGeneration::with_builtin_background_material(material),
            )),
        }
    }

    pub fn current(&self) -> Arc<EffectRegistryGeneration> {
        self.current
            .read()
            .expect("effect registry lock is not poisoned")
            .clone()
    }

    /// Prepare a new immutable generation that changes only the canonical
    /// background material program and preserves all qualified effects.
    pub fn prepare_material_generation(
        &self,
        material: crate::material::EffectiveMaterial,
    ) -> Result<EffectRegistryGeneration, RegistryReloadError> {
        let previous = self.current();
        let program = super::validate_effect_program(
            super::render_graph::builtin_background_material_program(material).program,
        )
        .map_err(|error| RegistryReloadError::Config(EffectConfigError::Validation(error)))?;
        let name = super::render_graph::BUILTIN_BACKGROUND_BLUR_NAME.to_owned();
        let mut generation = (*previous).clone();
        generation.generation = previous.generation.saturating_add(1);
        generation.registry.insert(program.clone())?;
        generation.effects.insert(
            name.clone(),
            RegisteredEffect {
                name,
                program,
                parameters: BTreeMap::new(),
            },
        );
        Ok(generation)
    }

    /// Publish a material generation after the renderer has accepted the same
    /// candidate. All program validation is completed in preparation.
    pub fn publish_material_generation(
        &self,
        generation: EffectRegistryGeneration,
    ) -> Arc<EffectRegistryGeneration> {
        self.publish(generation)
    }

    // Kept private so callers cannot bypass `build_generation`'s v1 policy
    // checks (including StaticTexture rejection and reserved IDs).
    fn publish(&self, generation: EffectRegistryGeneration) -> Arc<EffectRegistryGeneration> {
        let generation = Arc::new(generation);
        *self
            .current
            .write()
            .expect("effect registry lock is not poisoned") = generation.clone();
        generation
    }

    /// Prepare, validate, and publish a complete generation.  The callback is
    /// the renderer's safe GL-boundary compile step; it runs before the
    /// generation becomes visible to frame execution.
    pub fn reload<F>(
        &self,
        manifest: EffectManifest,
        mut compile_shader: F,
    ) -> Result<Arc<EffectRegistryGeneration>, RegistryReloadError>
    where
        F: FnMut(&super::config::EffectShaderAsset) -> Result<(), String>,
    {
        let previous = self.current();
        let mut generation = build_generation(manifest, previous.generation.saturating_add(1))?;
        inherit_builtin_material(&mut generation, &previous)?;
        for shader in generation.shaders.values() {
            let definition = super::config::EffectShaderAsset {
                module: shader.module,
                relative_path: shader.relative_path.clone(),
                source: shader.source.clone(),
                uniforms: shader.uniforms.clone(),
            };
            if let Err(log) = compile_shader(&definition) {
                return Err(RegistryReloadError::ShaderCompile {
                    module: shader.module,
                    log,
                });
            }
        }
        Ok(self.publish(generation))
    }

    pub fn reload_from_path<F>(
        &self,
        path: &std::path::Path,
        root: &std::path::Path,
        compile_shader: F,
    ) -> Result<Arc<EffectRegistryGeneration>, RegistryReloadError>
    where
        F: FnMut(&super::config::EffectShaderAsset) -> Result<(), String>,
    {
        let manifest = load_manifest(path, root)?;
        self.reload(manifest, compile_shader)
    }
}

/// GL-backed owners implement this boundary to make a complete generation
/// executable before compositor name lookup can observe it.
pub trait EffectGenerationPublisher {
    fn publish_effect_generation(
        &mut self,
        generation: EffectRegistryGeneration,
    ) -> Result<(), RegistryReloadError>;
}

/// Publish one trusted generation as a single runtime transaction.
///
/// The renderer-side publisher owns the fallible GL work.  The compositor
/// registry is updated only after that work succeeds, so a failed candidate
/// cannot become protocol-resolvable or replace the previous generation.
pub fn reload_with_publisher<P>(
    registry: &TrustedEffectRegistry,
    manifest: EffectManifest,
    publisher: &mut P,
) -> Result<Arc<EffectRegistryGeneration>, RegistryReloadError>
where
    P: EffectGenerationPublisher,
{
    let previous = registry.current();
    let mut candidate = build_generation(manifest, previous.generation.saturating_add(1))?;
    inherit_builtin_material(&mut candidate, &previous)?;
    publisher.publish_effect_generation(candidate.clone())?;
    Ok(registry.publish(candidate))
}

fn inherit_builtin_material(
    generation: &mut EffectRegistryGeneration,
    previous: &EffectRegistryGeneration,
) -> Result<(), RegistryReloadError> {
    let name = super::render_graph::BUILTIN_BACKGROUND_BLUR_NAME;
    let Some(previous_program) = previous.program(name) else {
        return Ok(());
    };
    generation.registry.insert(previous_program.clone())?;
    if let Some(effect) = generation.effects.get_mut(name) {
        effect.program = previous_program.clone();
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub enum RegistryReloadError {
    Config(EffectConfigError),
    Registry(EffectRegistryError),
    ProgramIdCollision(EffectProgramId),
    ReservedProgramId(EffectProgramId),
    ReservedSystemEffectName(String),
    DuplicateParameterId(EffectParameterId),
    UndefinedUniformParameter(EffectParameterId),
    ShaderModuleCollision(ShaderModuleId),
    ReservedShaderModuleId(ShaderModuleId),
    ShaderCompile { module: ShaderModuleId, log: String },
}

impl std::fmt::Display for RegistryReloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for RegistryReloadError {}

impl From<EffectConfigError> for RegistryReloadError {
    fn from(error: EffectConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<EffectRegistryError> for RegistryReloadError {
    fn from(error: EffectRegistryError) -> Self {
        Self::Registry(error)
    }
}

pub fn build_generation(
    manifest: EffectManifest,
    generation: u64,
) -> Result<EffectRegistryGeneration, RegistryReloadError> {
    let builtin = super::render_graph::builtin_background_blur_program();
    let builtin_id = builtin.program.id;
    let mut registry = EffectRegistry::empty();
    registry.insert(builtin.clone())?;
    let mut effects = BTreeMap::new();
    effects.insert(
        super::render_graph::BUILTIN_BACKGROUND_BLUR_NAME.to_owned(),
        RegisteredEffect {
            name: super::render_graph::BUILTIN_BACKGROUND_BLUR_NAME.to_owned(),
            program: builtin,
            parameters: BTreeMap::new(),
        },
    );
    let mut shaders = BTreeMap::new();
    for (name, definition) in manifest.effects {
        let EffectDefinition {
            program,
            parameters,
            shader_assets,
            ..
        } = definition;
        validate_parameter_schema(&program, &parameters)?;
        if let Some(parameter) = parameters
            .values()
            .find(|parameter| parameter.spec.impact != EffectParameterImpact::UniformOnly)
        {
            return Err(RegistryReloadError::Config(
                EffectConfigError::UnsupportedParameterImpact(parameter.spec.impact),
            ));
        }
        if name.starts_with("system.") {
            return Err(RegistryReloadError::ReservedSystemEffectName(name));
        }
        if program.id.get() == BUILTIN_EFFECT_PROGRAM_ID {
            return Err(RegistryReloadError::ReservedProgramId(program.id));
        }
        if effects
            .values()
            .any(|effect: &RegisteredEffect| effect.program.program.id == program.id)
            || program.id == builtin_id
        {
            return Err(RegistryReloadError::ProgramIdCollision(program.id));
        }
        let validated = super::validate_effect_program(program)
            .map_err(|error| RegistryReloadError::Config(EffectConfigError::Validation(error)))?;
        let effect = RegisteredEffect {
            name: name.clone(),
            program: validated.clone(),
            parameters,
        };
        effect
            .default_parameter_block()
            .map_err(|error| RegistryReloadError::Config(EffectConfigError::Validation(error)))?;
        registry.insert(validated.clone())?;
        for asset in shader_assets {
            if INTERNAL_EFFECT_SHADER_MODULE_IDS.contains(&asset.module.get()) {
                return Err(RegistryReloadError::ReservedShaderModuleId(asset.module));
            }
            if shaders
                .insert(
                    asset.module,
                    TrustedShaderAsset {
                        module: asset.module,
                        relative_path: asset.relative_path,
                        source: asset.source,
                        uniforms: asset.uniforms,
                    },
                )
                .is_some()
            {
                return Err(RegistryReloadError::ShaderModuleCollision(asset.module));
            }
        }
        effects.insert(name.clone(), effect);
    }
    Ok(EffectRegistryGeneration {
        generation,
        registry,
        effects,
        shaders,
    })
}

fn validate_parameter_schema(
    program: &super::EffectProgram,
    parameters: &BTreeMap<String, EffectParameterDefinition>,
) -> Result<(), RegistryReloadError> {
    let mut parameter_ids = Vec::with_capacity(parameters.len());
    for parameter in parameters.values() {
        if parameter_ids.contains(&parameter.spec.id) {
            return Err(RegistryReloadError::DuplicateParameterId(parameter.spec.id));
        }
        parameter_ids.push(parameter.spec.id);
    }
    validate_custom_fragment_parameters(program, &parameter_ids)
}

fn validate_custom_fragment_parameters(
    program: &super::EffectProgram,
    parameter_ids: &[EffectParameterId],
) -> Result<(), RegistryReloadError> {
    for node in &program.nodes {
        if let EffectNodeKind::CustomFragment(spec) = &node.kind {
            for binding in &spec.uniforms {
                if !parameter_ids.contains(&binding.parameter) {
                    return Err(RegistryReloadError::UndefinedUniformParameter(
                        binding.parameter,
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn manifest() -> EffectManifest {
        let source = super::super::EffectNodeId::new(1).unwrap();
        let blur = super::super::EffectNodeId::new(2).unwrap();
        let program = super::super::EffectProgram {
            id: super::super::EffectProgramId::new(77).unwrap(),
            nodes: vec![
                super::super::EffectNode::source(source, super::super::EffectSource::Backdrop),
                super::super::EffectNode::dual_kawase(
                    blur,
                    source,
                    super::super::DualKawaseBlurSpec::new(4.0, 2, 1.0).unwrap(),
                ),
            ],
            output: blur,
            working_space: super::super::EffectWorkingSpace::LinearSrgb,
            alpha_mode: super::super::EffectAlphaMode::Opaque,
            outsets: super::super::EffectOutsets::ZERO,
            frame_demand: super::super::EffectFrameDemand::OnDamage,
            failure_policy: super::super::EffectFailurePolicy::Passthrough,
        };
        EffectManifest {
            version: 1,
            effects: [(
                "glass.panel".into(),
                EffectDefinition {
                    name: "glass.panel".into(),
                    program,
                    parameters: BTreeMap::new(),
                    shader_assets: vec![super::super::config::EffectShaderAsset {
                        module: super::super::ShaderModuleId::new(9).unwrap(),
                        relative_path: "panel.frag".into(),
                        source: "trusted".into(),
                        uniforms: Vec::new(),
                    }],
                },
            )]
            .into_iter()
            .collect(),
        }
    }

    #[test]
    fn material_generation_preserves_trusted_registry_and_replaces_canonical_program() {
        let registry = TrustedEffectRegistry::with_builtin_background_blur();
        registry.reload(manifest(), |_| Ok(())).unwrap();
        let previous = registry.current();
        let material = crate::material::MaterialConfiguration {
            position: 1.0,
            ..crate::material::MaterialConfiguration::default()
        }
        .effective()
        .unwrap();

        let candidate = registry.prepare_material_generation(material).unwrap();
        assert_eq!(candidate.generation, previous.generation + 1);
        assert_eq!(candidate.effects.len(), previous.effects.len());
        assert!(candidate.effects.contains_key("system.background_blur"));
        assert!(candidate.effects.contains_key("glass.panel"));
        let blur = candidate.program("system.background_blur").unwrap();
        assert!(blur.aggregate_footprint.sample_radius_x > 48);
    }

    #[test]
    fn registered_effect_default_parameter_block_uses_manifest_values() {
        let mut candidate = manifest();
        candidate
            .effects
            .get_mut("glass.panel")
            .unwrap()
            .parameters
            .insert(
                "intensity".to_owned(),
                super::super::config::EffectParameterDefinition {
                    spec: super::super::EffectParameterSpec {
                        id: super::super::EffectParameterId::new(1).unwrap(),
                        name: "intensity".to_owned(),
                        ty: super::super::EffectParameterType::Float,
                        range: None,
                        impact: EffectParameterImpact::UniformOnly,
                    },
                    default: EffectUniformValue::Float(0.65),
                },
            );
        let registry = TrustedEffectRegistry::new();
        registry.reload(candidate, |_| Ok(())).unwrap();

        let effect = registry.current().effects["glass.panel"].clone();
        let block = effect.default_parameter_block().unwrap();

        assert_eq!(block.values().len(), 1);
        assert_eq!(block.values()[0].id.get(), 1);
        assert_eq!(block.values()[0].value, EffectUniformValue::Float(0.65));
    }

    #[test]
    fn build_generation_rejects_duplicate_parameter_ids() {
        let mut candidate = manifest();
        candidate
            .effects
            .get_mut("glass.panel")
            .unwrap()
            .parameters
            .extend([
                (
                    "intensity".to_owned(),
                    super::super::config::EffectParameterDefinition {
                        spec: super::super::EffectParameterSpec {
                            id: super::super::EffectParameterId::new(1).unwrap(),
                            name: "intensity".to_owned(),
                            ty: super::super::EffectParameterType::Float,
                            range: None,
                            impact: EffectParameterImpact::UniformOnly,
                        },
                        default: EffectUniformValue::Float(0.65),
                    },
                ),
                (
                    "highlight".to_owned(),
                    super::super::config::EffectParameterDefinition {
                        spec: super::super::EffectParameterSpec {
                            id: super::super::EffectParameterId::new(1).unwrap(),
                            name: "highlight".to_owned(),
                            ty: super::super::EffectParameterType::Float,
                            range: None,
                            impact: EffectParameterImpact::UniformOnly,
                        },
                        default: EffectUniformValue::Float(0.2),
                    },
                ),
            ]);

        assert!(build_generation(candidate, 1).is_err());
    }

    #[test]
    fn reload_from_path_rejects_duplicate_parameter_ids_in_the_config_file() {
        let config_root = std::env::temp_dir().join(format!(
            "typhon-effect-schema-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&config_root).unwrap();
        std::fs::write(
            config_root.join("effects.json"),
            r#"{
                "version": 1,
                "effects": {
                    "glass.panel": {
                        "nodes": [{"id": 1, "kind": "backdrop"}],
                        "output": 1,
                        "parameters": {
                            "intensity": {"id": 1, "type": "float", "default": 0.65},
                            "highlight": {"id": 1, "type": "float", "default": 0.2}
                        }
                    }
                }
            }"#,
        )
        .unwrap();
        let registry = TrustedEffectRegistry::new();
        let previous = registry.current();

        let result = registry.reload_from_path(Path::new("effects.json"), &config_root, |_| {
            panic!("invalid parameter schema reached shader precompile")
        });

        assert!(result.is_err());
        assert!(Arc::ptr_eq(&previous, &registry.current()));
        std::fs::remove_dir_all(config_root).unwrap();
    }

    #[test]
    fn build_generation_rejects_undefined_custom_fragment_parameter_bindings() {
        let mut candidate = manifest();
        let definition = candidate.effects.get_mut("glass.panel").unwrap();
        definition.parameters.insert(
            "intensity".to_owned(),
            super::super::config::EffectParameterDefinition {
                spec: super::super::EffectParameterSpec {
                    id: super::super::EffectParameterId::new(1).unwrap(),
                    name: "intensity".to_owned(),
                    ty: super::super::EffectParameterType::Float,
                    range: None,
                    impact: EffectParameterImpact::UniformOnly,
                },
                default: EffectUniformValue::Float(0.65),
            },
        );
        let fragment = super::super::EffectNode::custom_fragment(
            super::super::EffectNodeId::new(3).unwrap(),
            definition.program.output,
            super::super::CustomFragmentSpec {
                shader: super::super::ShaderModuleId::new(9).unwrap(),
                declared_footprint: super::super::EffectFootprint {
                    sample_radius_x: 0,
                    sample_radius_y: 0,
                    output_outsets: super::super::EffectOutsets::ZERO,
                },
                uniforms: vec![super::super::EffectUniformBinding {
                    parameter: super::super::EffectParameterId::new(2).unwrap(),
                    shader_name: "u_intensity".to_owned(),
                }],
                auxiliary_inputs: Vec::new(),
            },
        )
        .unwrap();
        definition.program.output = fragment.id;
        definition.program.nodes.push(fragment);

        assert!(build_generation(candidate, 1).is_err());
    }

    #[test]
    fn invalid_parameter_schema_reload_retains_the_previous_generation() {
        let registry = TrustedEffectRegistry::new();
        let first = registry.reload(manifest(), |_| Ok(())).unwrap();
        let mut invalid = manifest();
        invalid
            .effects
            .get_mut("glass.panel")
            .unwrap()
            .parameters
            .extend([
                (
                    "intensity".to_owned(),
                    super::super::config::EffectParameterDefinition {
                        spec: super::super::EffectParameterSpec {
                            id: super::super::EffectParameterId::new(1).unwrap(),
                            name: "intensity".to_owned(),
                            ty: super::super::EffectParameterType::Float,
                            range: None,
                            impact: EffectParameterImpact::UniformOnly,
                        },
                        default: EffectUniformValue::Float(0.65),
                    },
                ),
                (
                    "highlight".to_owned(),
                    super::super::config::EffectParameterDefinition {
                        spec: super::super::EffectParameterSpec {
                            id: super::super::EffectParameterId::new(1).unwrap(),
                            name: "highlight".to_owned(),
                            ty: super::super::EffectParameterType::Float,
                            range: None,
                            impact: EffectParameterImpact::UniformOnly,
                        },
                        default: EffectUniformValue::Float(0.2),
                    },
                ),
            ]);

        let failed = registry.reload(invalid, |_| {
            panic!("invalid schema reached shader precompile")
        });

        assert!(failed.is_err());
        assert!(Arc::ptr_eq(&first, &registry.current()));
    }

    #[test]
    fn failed_reload_keeps_previous_generation() {
        let registry = TrustedEffectRegistry::new();
        let first = registry.reload(manifest(), |_| Ok(())).unwrap();
        let failed = registry.reload(manifest(), |_| Err("compile failed".into()));
        assert!(matches!(failed, Err(RegistryReloadError::ShaderCompile { .. })) || failed.is_ok());
        assert_eq!(registry.current().generation, first.generation);
    }

    #[test]
    fn successful_reload_publishes_one_complete_generation() {
        let registry = TrustedEffectRegistry::new();
        let generation = registry.reload(manifest(), |_| Ok(())).unwrap();
        assert_eq!(generation.generation, 1);
        assert_eq!(generation.program_id("glass.panel").unwrap().get(), 77);
        assert_eq!(
            generation.program_id("system.background_blur"),
            Some(super::super::render_graph::builtin_background_blur_program_id())
        );
        assert!(registry.current().program("glass.panel").is_some());
        let _ = Path::new(".");
    }

    #[test]
    fn v1_registry_rejects_static_texture_before_publication() {
        let registry = TrustedEffectRegistry::new();
        let previous = registry.current().generation;
        let static_id = super::super::StaticTextureId::new(9).unwrap();
        let mut candidate = manifest();
        let definition = candidate.effects.get_mut("glass.panel").unwrap();
        definition.program.nodes[0] = super::super::EffectNode::source(
            super::super::EffectNodeId::new(1).unwrap(),
            super::super::EffectSource::StaticTexture(static_id),
        );

        let result = registry.reload(candidate, |_| {
            panic!("static texture candidates must fail before shader prewarm")
        });

        assert_eq!(
            result,
            Err(RegistryReloadError::Config(EffectConfigError::Validation(
                super::super::EffectValidationError::UnsupportedStaticTexture(static_id)
            )))
        );
        assert_eq!(registry.current().generation, previous);
        assert!(registry.current().program("glass.panel").is_none());
    }

    #[test]
    fn v1_registry_rejects_non_uniform_parameter_impact_before_publication() {
        let mut candidate = manifest();
        candidate
            .effects
            .get_mut("glass.panel")
            .unwrap()
            .parameters
            .insert(
                "radius".into(),
                super::super::config::EffectParameterDefinition {
                    spec: super::super::EffectParameterSpec {
                        id: super::super::EffectParameterId::new(1).unwrap(),
                        name: "radius".into(),
                        ty: super::super::EffectParameterType::Float,
                        range: None,
                        impact: super::super::EffectParameterImpact::Footprint,
                    },
                    default: super::super::EffectUniformValue::Float(1.0),
                },
            );
        assert_eq!(
            build_generation(candidate, 1),
            Err(RegistryReloadError::Config(
                EffectConfigError::UnsupportedParameterImpact(
                    super::super::EffectParameterImpact::Footprint
                )
            ))
        );
    }

    #[derive(Default)]
    struct TestGenerationPublisher {
        published: Vec<u64>,
        fail_generation: Option<u64>,
    }

    impl EffectGenerationPublisher for TestGenerationPublisher {
        fn publish_effect_generation(
            &mut self,
            generation: EffectRegistryGeneration,
        ) -> Result<(), RegistryReloadError> {
            if self.fail_generation == Some(generation.generation) {
                return Err(RegistryReloadError::ShaderCompile {
                    module: ShaderModuleId::new(9).unwrap(),
                    log: "test compile failure".into(),
                });
            }
            self.published.push(generation.generation);
            Ok(())
        }
    }

    #[test]
    fn publication_transaction_keeps_generations_identical() {
        let registry = TrustedEffectRegistry::new();
        let mut publisher = TestGenerationPublisher::default();
        let generation = reload_with_publisher(&registry, manifest(), &mut publisher).unwrap();

        assert_eq!(generation.generation, 1);
        assert_eq!(publisher.published, vec![generation.generation]);
        assert_eq!(registry.current().generation, generation.generation);
    }

    #[test]
    fn publisher_reload_preserves_loaded_non_default_material_and_prior_state_on_failure() {
        let material = crate::material::MaterialConfiguration {
            position: 1.0,
            ..crate::material::MaterialConfiguration::default()
        }
        .effective()
        .unwrap();
        let registry = TrustedEffectRegistry::with_builtin_background_material(material);
        let material_program_before = registry
            .current()
            .program("system.background_blur")
            .expect("canonical material program")
            .clone();
        let initial_generation = registry.current().generation;
        assert!(material_program_before.aggregate_footprint.sample_radius_x > 48);
        let mut publisher = TestGenerationPublisher::default();
        publisher
            .publish_effect_generation((*registry.current()).clone())
            .unwrap();

        let reloaded = reload_with_publisher(&registry, manifest(), &mut publisher).unwrap();

        assert!(reloaded.effects.contains_key("glass.panel"));
        let canonical = reloaded
            .program("system.background_blur")
            .expect("canonical background material remains present");
        assert_eq!(canonical, &material_program_before);
        assert_eq!(
            publisher.published,
            vec![initial_generation, reloaded.generation]
        );

        let generation_before_failure = registry.current().generation;
        let material_before_failure = registry
            .current()
            .program("system.background_blur")
            .expect("canonical material program")
            .clone();
        let mut failing_publisher = TestGenerationPublisher {
            fail_generation: Some(generation_before_failure + 1),
            ..TestGenerationPublisher::default()
        };
        assert!(matches!(
            reload_with_publisher(&registry, manifest(), &mut failing_publisher),
            Err(RegistryReloadError::ShaderCompile { .. })
        ));
        let after_failure = registry.current();
        assert_eq!(after_failure.generation, generation_before_failure);
        assert_eq!(
            after_failure.program("system.background_blur"),
            Some(&material_before_failure)
        );
        assert!(after_failure.effects.contains_key("glass.panel"));
        assert!(failing_publisher.published.is_empty());
    }

    #[test]
    fn publication_transaction_retains_previous_generation_on_renderer_failure() {
        let registry = TrustedEffectRegistry::new();
        let first = reload_with_publisher(
            &registry,
            manifest(),
            &mut TestGenerationPublisher::default(),
        )
        .unwrap();
        let mut publisher = TestGenerationPublisher {
            fail_generation: Some(first.generation.saturating_add(1)),
            ..TestGenerationPublisher::default()
        };

        let result = reload_with_publisher(&registry, manifest(), &mut publisher);

        assert!(matches!(
            result,
            Err(RegistryReloadError::ShaderCompile { .. })
        ));
        assert_eq!(registry.current().generation, first.generation);
        assert!(publisher.published.is_empty());
    }

    #[test]
    fn schema_signature_covers_all_v1_parameter_compatibility_fields() {
        let mut generation = build_generation(manifest(), 1).unwrap();
        let parameter = super::super::config::EffectParameterDefinition {
            spec: super::super::EffectParameterSpec {
                id: super::super::EffectParameterId::new(1).unwrap(),
                name: "radius".into(),
                ty: super::super::EffectParameterType::Float,
                range: Some(super::super::EffectParameterRange::Float { min: 0.0, max: 1.0 }),
                impact: super::super::EffectParameterImpact::UniformOnly,
            },
            default: super::super::EffectUniformValue::Float(0.5),
        };
        let effect = generation.effects.get_mut("glass.panel").unwrap();
        effect.parameters.insert("radius".into(), parameter);
        let baseline = effect.schema_signature();

        let effect = generation.effects.get_mut("glass.panel").unwrap();
        effect.parameters.get_mut("radius").unwrap().spec.range =
            Some(super::super::EffectParameterRange::Float {
                min: 0.25,
                max: 1.0,
            });
        assert_ne!(effect.schema_signature(), baseline);
        let changed_range = effect.schema_signature();
        effect.parameters.get_mut("radius").unwrap().spec.range =
            Some(super::super::EffectParameterRange::Float { min: 0.0, max: 2.0 });
        assert_ne!(effect.schema_signature(), changed_range);
        effect.parameters.get_mut("radius").unwrap().default =
            super::super::EffectUniformValue::Float(0.75);
        assert_ne!(effect.schema_signature(), changed_range);
        effect.parameters.get_mut("radius").unwrap().spec.impact =
            super::super::EffectParameterImpact::Footprint;
        assert_ne!(effect.schema_signature(), changed_range);
        effect.parameters.get_mut("radius").unwrap().spec.ty =
            super::super::EffectParameterType::Int;
        assert_ne!(effect.schema_signature(), changed_range);
        effect.parameters.insert(
            "extra".into(),
            super::super::config::EffectParameterDefinition {
                spec: super::super::EffectParameterSpec {
                    id: super::super::EffectParameterId::new(2).unwrap(),
                    name: "extra".into(),
                    ty: super::super::EffectParameterType::Float,
                    range: None,
                    impact: super::super::EffectParameterImpact::UniformOnly,
                },
                default: super::super::EffectUniformValue::Float(0.0),
            },
        );
        let added = effect.schema_signature();
        assert_ne!(added, changed_range);
        effect.parameters.remove("extra");
        assert_ne!(effect.schema_signature(), added);
    }

    #[test]
    fn trusted_schema_reload_keeps_source_only_compatibility_schema() {
        let first = build_generation(manifest(), 1).unwrap();
        let mut second_manifest = manifest();
        second_manifest
            .effects
            .get_mut("glass.panel")
            .unwrap()
            .shader_assets[0]
            .source = "changed trusted shader".into();
        let second = build_generation(second_manifest, 2).unwrap();
        assert_eq!(
            first.schema_signature(first.program_id("glass.panel").unwrap()),
            second.schema_signature(second.program_id("glass.panel").unwrap())
        );
    }

    #[test]
    fn successful_reload_removes_old_programs_with_a_new_generation() {
        let registry = TrustedEffectRegistry::new();
        let first = reload_with_publisher(
            &registry,
            manifest(),
            &mut TestGenerationPublisher::default(),
        )
        .unwrap();
        let empty = EffectManifest {
            version: 1,
            effects: BTreeMap::new(),
        };
        let second =
            reload_with_publisher(&registry, empty, &mut TestGenerationPublisher::default())
                .unwrap();

        assert_eq!(second.generation, first.generation.saturating_add(1));
        assert!(registry.current().program("glass.panel").is_none());
        assert_eq!(registry.current().generation, second.generation);
    }

    #[test]
    fn v1_reserves_builtin_system_name_and_program_id() {
        let mut system_name = manifest();
        let definition = system_name.effects.remove("glass.panel").unwrap();
        system_name
            .effects
            .insert("system.background_blur".into(), definition);
        assert!(matches!(
            build_generation(system_name, 1),
            Err(RegistryReloadError::ReservedSystemEffectName(name))
                if name == "system.background_blur"
        ));

        let mut builtin_id = manifest();
        builtin_id
            .effects
            .get_mut("glass.panel")
            .unwrap()
            .program
            .id = super::super::EffectProgramId::new(BUILTIN_EFFECT_PROGRAM_ID).unwrap();
        assert!(matches!(
            build_generation(builtin_id, 1),
            Err(RegistryReloadError::ReservedProgramId(id))
                if id.get() == BUILTIN_EFFECT_PROGRAM_ID
        ));
    }

    #[test]
    fn v1_rejects_every_internal_shader_module_id_but_allows_external_ids() {
        for module in INTERNAL_EFFECT_SHADER_MODULE_IDS {
            let mut candidate = manifest();
            candidate
                .effects
                .get_mut("glass.panel")
                .unwrap()
                .shader_assets[0]
                .module = ShaderModuleId::new(*module).unwrap();
            assert!(matches!(
                build_generation(candidate, 1),
                Err(RegistryReloadError::ReservedShaderModuleId(id))
                    if id.get() == *module
            ));
        }

        let mut external = manifest();
        external
            .effects
            .get_mut("glass.panel")
            .unwrap()
            .shader_assets[0]
            .module = ShaderModuleId::new(9001).unwrap();
        assert!(build_generation(external, 1).is_ok());
    }
}
