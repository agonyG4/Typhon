//! Immutable trusted effect-registry generations.

use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, RwLock},
};

use super::{
    EffectProgramId, MAX_EFFECT_PROGRAMS, ShaderModuleId, ValidatedEffectProgram,
    config::{EffectConfigError, EffectDefinition, EffectManifest, load_manifest},
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

    pub fn current(&self) -> Arc<EffectRegistryGeneration> {
        self.current
            .read()
            .expect("effect registry lock is not poisoned")
            .clone()
    }

    pub fn publish(&self, generation: EffectRegistryGeneration) -> Arc<EffectRegistryGeneration> {
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
        let generation = build_generation(manifest, previous.generation.saturating_add(1))?;
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

#[derive(Clone, Debug, PartialEq)]
pub enum RegistryReloadError {
    Config(EffectConfigError),
    Registry(EffectRegistryError),
    ProgramIdCollision(EffectProgramId),
    ShaderModuleCollision(ShaderModuleId),
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
    let mut registry = EffectRegistry::empty();
    let mut effects = BTreeMap::new();
    let mut shaders = BTreeMap::new();
    for (name, definition) in manifest.effects {
        let EffectDefinition {
            program,
            parameters,
            shader_assets,
            ..
        } = definition;
        if effects
            .values()
            .any(|effect: &RegisteredEffect| effect.program.program.id == program.id)
        {
            return Err(RegistryReloadError::ProgramIdCollision(program.id));
        }
        let validated = super::validate_effect_program(program)
            .map_err(|error| RegistryReloadError::Config(EffectConfigError::Validation(error)))?;
        registry.insert(validated.clone())?;
        for asset in shader_assets {
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
        effects.insert(
            name.clone(),
            RegisteredEffect {
                name,
                program: validated,
                parameters,
            },
        );
    }
    Ok(EffectRegistryGeneration {
        generation,
        registry,
        effects,
        shaders,
    })
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
        assert!(registry.current().program("glass.panel").is_some());
        let _ = Path::new(".");
    }
}
