#![allow(dead_code)] // The active executor consumes the cache at the effect backend boundary.

use std::{collections::HashMap, io};

use glow::HasContext;
use oblivion_one::effects::{EffectUniformBinding, EffectWorkingSpace};

use super::super::{GlProgram, RendererResult};
use super::blur;

pub const RESERVED_SHADER_NAMES: &[&str] = &[
    "u_typhon_primary",
    "u_typhon_texture_size",
    "u_typhon_content_rect",
    "u_typhon_output_size",
    "u_typhon_scale",
    "u_typhon_time",
    "u_typhon_delta",
    "v_uv",
    "out_color",
    "TyphonEffectContext",
];

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ShaderProgramKey {
    pub module: oblivion_one::effects::ShaderModuleId,
    pub variant_bits: u64,
    pub working_space: EffectWorkingSpace,
}

impl ShaderProgramKey {
    pub const fn new(
        module: oblivion_one::effects::ShaderModuleId,
        variant_bits: u64,
        working_space: EffectWorkingSpace,
    ) -> Self {
        Self {
            module,
            variant_bits,
            working_space,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShaderCacheError {
    InvalidCapacity,
    CacheFull,
    MissingProgram,
}

impl std::fmt::Display for ShaderCacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ShaderCacheError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShaderWrapperError {
    ReservedName(String),
    InvalidUniformName(String),
    MissingEntryPoint,
    OwnsMainFunction,
}

impl std::fmt::Display for ShaderWrapperError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ShaderWrapperError {}

struct CachedShaderProgram {
    source_hash: u64,
    last_used: u64,
    program: Option<GlProgram>,
    compile_log: Option<String>,
}

pub struct ShaderProgramCache {
    entries: HashMap<ShaderProgramKey, CachedShaderProgram>,
    capacity: usize,
    clock: u64,
    evicted: Vec<ShaderProgramKey>,
}

impl ShaderProgramCache {
    pub(crate) fn new(capacity: usize) -> Result<Self, ShaderCacheError> {
        if capacity == 0 {
            return Err(ShaderCacheError::InvalidCapacity);
        }
        Ok(Self {
            entries: HashMap::new(),
            capacity,
            clock: 0,
            evicted: Vec::new(),
        })
    }

    pub(crate) fn remember_source(
        &mut self,
        key: ShaderProgramKey,
        source: &str,
    ) -> Result<(), ShaderCacheError> {
        self.clock = self.clock.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.source_hash = shader_source_hash(source);
            entry.last_used = self.clock;
            return Ok(());
        }
        self.evict_until_room(None);
        if self.entries.len() >= self.capacity {
            return Err(ShaderCacheError::CacheFull);
        }
        self.entries.insert(
            key,
            CachedShaderProgram {
                source_hash: shader_source_hash(source),
                last_used: self.clock,
                program: None,
                compile_log: None,
            },
        );
        Ok(())
    }

    pub(crate) fn touch(&mut self, key: ShaderProgramKey) -> Result<(), ShaderCacheError> {
        self.clock = self.clock.saturating_add(1);
        let entry = self
            .entries
            .get_mut(&key)
            .ok_or(ShaderCacheError::CacheFull)?;
        entry.last_used = self.clock;
        Ok(())
    }

    pub(crate) fn evicted_keys(&self) -> Vec<ShaderProgramKey> {
        self.evicted.clone()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn get_or_compile(
        &mut self,
        gl: &glow::Context,
        key: ShaderProgramKey,
        vertex_source: &str,
        fragment_source: &str,
    ) -> RendererResult<GlProgram> {
        self.clock = self.clock.saturating_add(1);
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.last_used = self.clock;
            if let Some(program) = entry.program {
                return Ok(program);
            }
        }

        let program = match super::super::program::create_program_from_sources(
            gl,
            vertex_source,
            fragment_source,
        ) {
            Ok(program) => program,
            Err(error) => {
                if let Some(entry) = self.entries.get_mut(&key) {
                    entry.compile_log = Some(error.to_string());
                }
                return Err(error);
            }
        };
        self.evict_until_room(Some(gl));
        if self.entries.len() >= self.capacity && !self.entries.contains_key(&key) {
            unsafe { gl.delete_program(program) };
            return Err(io::Error::other("effect shader cache is full").into());
        }
        self.entries.insert(
            key,
            CachedShaderProgram {
                source_hash: shader_source_hash(fragment_source),
                last_used: self.clock,
                program: Some(program),
                compile_log: None,
            },
        );
        Ok(program)
    }

    pub(crate) fn lookup(&mut self, key: ShaderProgramKey) -> RendererResult<GlProgram> {
        self.clock = self.clock.saturating_add(1);
        let entry = self
            .entries
            .get_mut(&key)
            .ok_or_else(|| io::Error::other(ShaderCacheError::MissingProgram))?;
        entry.last_used = self.clock;
        entry
            .program
            .ok_or_else(|| io::Error::other("effect shader was not prewarmed").into())
    }

    pub(crate) fn prewarm(
        &mut self,
        gl: &glow::Context,
        key: ShaderProgramKey,
        vertex_source: &str,
        fragment_source: &str,
    ) -> RendererResult<GlProgram> {
        self.get_or_compile(gl, key, vertex_source, fragment_source)
    }

    pub(crate) fn prewarm_builtins(&mut self, gl: &glow::Context) -> RendererResult<()> {
        let vertex = blur::DUAL_KAWASE_VERTEX_SHADER;
        for (module, variant, fragment) in [
            (1001, 0, blur::DUAL_KAWASE_DOWNSAMPLE_SHADER),
            (1001, 1, blur::DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER),
            (1002, 0, blur::DUAL_KAWASE_UPSAMPLE_SHADER),
            (1003, 0, super::executor::COPY_FRAGMENT_SHADER),
            (1004, 0, super::executor::COMPOSITE_FRAGMENT_SHADER),
        ] {
            let key = ShaderProgramKey::new(
                oblivion_one::effects::ShaderModuleId::new(module)
                    .expect("builtin shader ids are non-zero"),
                variant,
                EffectWorkingSpace::LinearSrgb,
            );
            self.prewarm(gl, key, vertex, fragment)?;
        }
        Ok(())
    }

    pub(crate) fn last_compile_log(&self, key: ShaderProgramKey) -> Option<&str> {
        self.entries
            .get(&key)
            .and_then(|entry| entry.compile_log.as_deref())
    }

    pub(crate) fn clear(&mut self, gl: &glow::Context) {
        for (_, entry) in self.entries.drain() {
            if let Some(program) = entry.program {
                unsafe { gl.delete_program(program) };
            }
        }
    }

    fn evict_until_room(&mut self, gl: Option<&glow::Context>) {
        while self.entries.len() >= self.capacity {
            let Some(key) = self.oldest_key() else {
                break;
            };
            let Some(entry) = self.entries.remove(&key) else {
                break;
            };
            if let (Some(gl), Some(program)) = (gl, entry.program) {
                unsafe { gl.delete_program(program) };
            }
            self.evicted.push(key);
        }
    }

    fn oldest_key(&self) -> Option<ShaderProgramKey> {
        self.entries
            .iter()
            .min_by_key(|(key, entry)| {
                (
                    entry.last_used,
                    key.module.get(),
                    key.variant_bits,
                    working_space_rank(key.working_space),
                )
            })
            .map(|(key, _)| *key)
    }
}

fn working_space_rank(space: EffectWorkingSpace) -> u8 {
    match space {
        EffectWorkingSpace::OutputEncodedSrgb => 0,
        EffectWorkingSpace::LinearSrgb => 1,
    }
}

pub fn generate_fragment_wrapper(
    body: &str,
    uniforms: &[EffectUniformBinding],
) -> Result<String, ShaderWrapperError> {
    if !body.contains("typhon_effect_main") {
        return Err(ShaderWrapperError::MissingEntryPoint);
    }
    if body.contains("void main") {
        return Err(ShaderWrapperError::OwnsMainFunction);
    }
    for binding in uniforms {
        validate_uniform_name(binding)?;
    }
    for name in RESERVED_SHADER_NAMES {
        if *name == "TyphonEffectContext" {
            continue;
        }
        if body.contains(name) {
            return Err(ShaderWrapperError::ReservedName((*name).to_owned()));
        }
    }
    let mut source = String::from(
        r#"#version 300 es
precision mediump float;
uniform sampler2D u_typhon_primary;
uniform vec2 u_typhon_texture_size;
uniform vec4 u_typhon_content_rect;
uniform vec2 u_typhon_output_size;
uniform float u_typhon_scale;
uniform float u_typhon_time;
uniform float u_typhon_delta;
in vec2 v_uv;
out vec4 out_color;

struct TyphonEffectContext {
    sampler2D primary;
    vec2 texture_size;
    vec4 content_rect;
    vec2 output_size;
    float scale;
    float time;
    float delta;
    vec2 uv;
};

"#,
    );
    source.push_str(body);
    source.push_str(
        r#"

void main() {
    TyphonEffectContext ctx;
    ctx.primary = u_typhon_primary;
    ctx.texture_size = u_typhon_texture_size;
    ctx.content_rect = u_typhon_content_rect;
    ctx.output_size = u_typhon_output_size;
    ctx.scale = u_typhon_scale;
    ctx.time = u_typhon_time;
    ctx.delta = u_typhon_delta;
    ctx.uv = v_uv;
    out_color = typhon_effect_main(ctx);
}
"#,
    );
    Ok(source)
}

fn validate_uniform_name(binding: &EffectUniformBinding) -> Result<(), ShaderWrapperError> {
    if RESERVED_SHADER_NAMES.contains(&binding.shader_name.as_str()) {
        return Err(ShaderWrapperError::ReservedName(
            binding.shader_name.clone(),
        ));
    }
    if !is_glsl_identifier(&binding.shader_name) {
        return Err(ShaderWrapperError::InvalidUniformName(
            binding.shader_name.clone(),
        ));
    }
    Ok(())
}

fn is_glsl_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn shader_source_hash(source: &str) -> u64 {
    source
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oblivion_one::effects::{
        EffectParameterId, EffectUniformBinding, EffectWorkingSpace, ShaderModuleId,
    };

    fn key(module: u64, variant_bits: u64) -> ShaderProgramKey {
        ShaderProgramKey::new(
            ShaderModuleId::new(module).unwrap(),
            variant_bits,
            EffectWorkingSpace::LinearSrgb,
        )
    }

    #[test]
    fn structural_key_does_not_include_uniform_values() {
        let first = key(1, 0);
        let second = key(1, 0);
        assert_eq!(first, second);
        assert_ne!(first, key(1, 1));
        assert_ne!(first, key(2, 0));
    }

    #[test]
    fn cache_evicts_the_oldest_variant_deterministically() {
        let mut cache = ShaderProgramCache::new(2).unwrap();
        cache.remember_source(key(1, 0), "one").unwrap();
        cache.remember_source(key(2, 0), "two").unwrap();
        cache.touch(key(2, 0)).unwrap();
        cache.remember_source(key(3, 0), "three").unwrap();
        assert_eq!(cache.evicted_keys(), vec![key(1, 0)]);
    }

    #[test]
    fn wrapper_rejects_reserved_names_before_compile() {
        let bindings = [EffectUniformBinding {
            parameter: EffectParameterId::new(1).unwrap(),
            shader_name: "u_typhon_primary".to_owned(),
        }];
        assert!(matches!(
            generate_fragment_wrapper(
                "vec4 typhon_effect_main(TyphonEffectContext ctx) { return vec4(1.0); }",
                &bindings
            ),
            Err(ShaderWrapperError::ReservedName(_))
        ));
    }

    #[test]
    fn wrapper_owns_main_and_context_abi() {
        let source = generate_fragment_wrapper(
            "vec4 typhon_effect_main(TyphonEffectContext ctx) { return texture(ctx.primary, ctx.uv); }",
            &[],
        )
        .unwrap();
        assert!(source.contains("void main()"));
        assert!(source.contains("u_typhon_primary"));
        assert!(source.contains("struct TyphonEffectContext"));
    }

    #[test]
    fn render_lookup_never_compiles_or_inserts_a_program() {
        let mut cache = ShaderProgramCache::new(2).unwrap();
        let key = key(1, 0);
        assert!(cache.lookup(key).is_err());
        assert_eq!(cache.len(), 0);
    }
}
