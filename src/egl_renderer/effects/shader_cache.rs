#![allow(dead_code)] // The active executor consumes the cache at the effect backend boundary.

use std::{collections::HashMap, io};

use glow::HasContext;
use oblivion_one::effects::{
    EffectUniformBinding, EffectWorkingSpace, INTERNAL_EFFECT_SHADER_MODULE_BLEND,
    INTERNAL_EFFECT_SHADER_MODULE_COMPOSITE, INTERNAL_EFFECT_SHADER_MODULE_COPY,
    INTERNAL_EFFECT_SHADER_MODULE_DOWNSAMPLE, INTERNAL_EFFECT_SHADER_MODULE_FRAGMENT,
    INTERNAL_EFFECT_SHADER_MODULE_MASK, INTERNAL_EFFECT_SHADER_MODULE_UPSAMPLE,
};

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
    "u_typhon_decode_srgb",
    "u_typhon_encode_srgb",
    "u_typhon_aux0",
    "u_typhon_aux1",
    "u_typhon_aux2",
    "u_typhon_aux3",
    "u_typhon_aux4",
    "u_typhon_aux5",
    "u_typhon_aux6",
    "u_typhon_aux7",
    "u_typhon_aux_count",
    "typhon_sample_primary",
    "typhon_sample_aux",
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
    uniform_locations: HashMap<String, Option<glow::UniformLocation>>,
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
                uniform_locations: HashMap::new(),
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
                uniform_locations: HashMap::new(),
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

    pub(crate) fn uniform_location(
        &mut self,
        gl: &glow::Context,
        key: ShaderProgramKey,
        program: GlProgram,
        name: &str,
    ) -> Option<glow::UniformLocation> {
        let entry = self.entries.get_mut(&key)?;
        if let Some(location) = entry.uniform_locations.get(name) {
            return *location;
        }
        let location = unsafe { gl.get_uniform_location(program, name) };
        entry.uniform_locations.insert(name.to_owned(), location);
        location
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

    pub(crate) fn prewarm_trusted_custom(
        &mut self,
        gl: &glow::Context,
        asset: &oblivion_one::effects::TrustedShaderAsset,
    ) -> RendererResult<()> {
        let fragment =
            generate_fragment_wrapper(&asset.source, &asset.uniforms).map_err(io::Error::other)?;
        let key = ShaderProgramKey::new(asset.module, 0, EffectWorkingSpace::LinearSrgb);
        self.prewarm(gl, key, blur::DUAL_KAWASE_VERTEX_SHADER, &fragment)?;
        Ok(())
    }

    pub(crate) fn prewarm_builtins(&mut self, gl: &glow::Context) -> RendererResult<()> {
        let vertex = blur::DUAL_KAWASE_VERTEX_SHADER;
        for (module, variant, fragment) in [
            (
                INTERNAL_EFFECT_SHADER_MODULE_DOWNSAMPLE,
                0,
                blur::DUAL_KAWASE_DOWNSAMPLE_SHADER,
            ),
            (
                INTERNAL_EFFECT_SHADER_MODULE_DOWNSAMPLE,
                1,
                blur::DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER,
            ),
            (
                INTERNAL_EFFECT_SHADER_MODULE_UPSAMPLE,
                0,
                blur::DUAL_KAWASE_UPSAMPLE_SHADER,
            ),
            (
                INTERNAL_EFFECT_SHADER_MODULE_COPY,
                0,
                super::executor::COPY_FRAGMENT_SHADER,
            ),
            (
                INTERNAL_EFFECT_SHADER_MODULE_COPY,
                1,
                super::executor::NORMALIZE_FRAGMENT_SHADER,
            ),
            (
                INTERNAL_EFFECT_SHADER_MODULE_COMPOSITE,
                0,
                super::executor::COMPOSITE_FRAGMENT_SHADER,
            ),
            (
                INTERNAL_EFFECT_SHADER_MODULE_FRAGMENT,
                0,
                super::executor::FRAGMENT_STAGE_FRAGMENT_SHADER,
            ),
            (
                INTERNAL_EFFECT_SHADER_MODULE_MASK,
                0,
                super::executor::MASK_STAGE_FRAGMENT_SHADER,
            ),
            (
                INTERNAL_EFFECT_SHADER_MODULE_BLEND,
                0,
                super::executor::BLEND_STAGE_FRAGMENT_SHADER,
            ),
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
        if matches!(
            *name,
            "TyphonEffectContext" | "typhon_sample_primary" | "typhon_sample_aux"
        ) {
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
uniform int u_typhon_decode_srgb;
uniform int u_typhon_encode_srgb;
uniform sampler2D u_typhon_aux0;
uniform sampler2D u_typhon_aux1;
uniform sampler2D u_typhon_aux2;
uniform sampler2D u_typhon_aux3;
uniform sampler2D u_typhon_aux4;
uniform sampler2D u_typhon_aux5;
uniform sampler2D u_typhon_aux6;
uniform sampler2D u_typhon_aux7;
uniform int u_typhon_aux_count;
in vec2 v_uv;
out vec4 out_color;

struct TyphonEffectContext {
    vec2 texture_size;
    vec4 content_rect;
    vec2 output_size;
    float scale;
    float time;
    float delta;
    int aux_count;
    vec2 uv;
};

vec4 typhon_decode_premultiplied_srgb(vec4 value);
vec4 typhon_encode_premultiplied_srgb(vec4 value);

vec4 typhon_sample_primary(vec2 uv) {
    vec4 result = texture(u_typhon_primary, uv);
    if (u_typhon_decode_srgb != 0) result = typhon_decode_premultiplied_srgb(result);
    return result;
}

vec4 typhon_sample_aux(int index, vec2 uv) {
    vec4 result = vec4(0.0);
    if (index == 0) result = texture(u_typhon_aux0, uv);
    if (index == 1) result = texture(u_typhon_aux1, uv);
    if (index == 2) result = texture(u_typhon_aux2, uv);
    if (index == 3) result = texture(u_typhon_aux3, uv);
    if (index == 4) result = texture(u_typhon_aux4, uv);
    if (index == 5) result = texture(u_typhon_aux5, uv);
    if (index == 6) result = texture(u_typhon_aux6, uv);
    if (index == 7) result = texture(u_typhon_aux7, uv);
    if (u_typhon_decode_srgb != 0) result = typhon_decode_premultiplied_srgb(result);
    return result;
}

float typhon_decode_srgb_channel(float value) {
    value = max(value, 0.0);
    if (value <= 0.04045) return value / 12.92;
    return pow((value + 0.055) / 1.055, 2.4);
}

float typhon_encode_srgb_channel(float value) {
    value = max(value, 0.0);
    if (value <= 0.0031308) return value * 12.92;
    return 1.055 * pow(value, 1.0 / 2.4) - 0.055;
}

vec4 typhon_sanitize_premultiplied(vec4 value) {
    if (any(isnan(value)) || any(isinf(value))) return vec4(0.0);
    float alpha = clamp(value.a, 0.0, 1.0);
    return vec4(clamp(value.rgb, vec3(0.0), vec3(alpha)), alpha);
}

vec4 typhon_decode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 linear = vec3(typhon_decode_srgb_channel(straight.r), typhon_decode_srgb_channel(straight.g), typhon_decode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(linear * value.a, value.a));
}

vec4 typhon_encode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 encoded = vec3(typhon_encode_srgb_channel(straight.r), typhon_encode_srgb_channel(straight.g), typhon_encode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(encoded * value.a, value.a));
}

"#,
    );
    source.push_str(body);
    source.push_str(
        r#"

void main() {
    TyphonEffectContext ctx;
    ctx.texture_size = u_typhon_texture_size;
    ctx.content_rect = u_typhon_content_rect;
    ctx.output_size = u_typhon_output_size;
    ctx.scale = u_typhon_scale;
    ctx.time = u_typhon_time;
    ctx.delta = u_typhon_delta;
    ctx.aux_count = clamp(u_typhon_aux_count, 0, 8);
    ctx.uv = v_uv;
    out_color = typhon_effect_main(ctx);
    if (u_typhon_encode_srgb != 0) out_color = typhon_encode_premultiplied_srgb(out_color);
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
            "vec4 typhon_effect_main(TyphonEffectContext ctx) { return typhon_sample_primary(ctx.uv); }",
            &[],
        )
        .unwrap();
        assert!(source.contains("void main()"));
        assert!(source.contains("u_typhon_primary"));
        assert!(source.contains("struct TyphonEffectContext"));
        assert!(!source.contains("sampler2D primary"));
    }

    #[test]
    fn wrapper_exposes_bounded_auxiliary_texture_abi() {
        let source = generate_fragment_wrapper(
            "vec4 typhon_effect_main(TyphonEffectContext ctx) { return typhon_sample_aux(0, ctx.uv); }",
            &[],
        )
        .unwrap();
        assert!(source.contains("u_typhon_aux0"));
        assert!(source.contains("u_typhon_aux7"));
        assert!(source.contains("u_typhon_aux_count"));
        assert!(source.contains("ctx.aux_count"));
        assert!(source.contains("if (index == 7) result = texture(u_typhon_aux7, uv);"));
        for index in 0..8 {
            assert!(source.contains(&format!("u_typhon_aux{index}")));
        }
        assert!(source.contains("ctx.time"));
        assert!(source.contains("ctx.delta"));
        assert!(!source.contains("sampler2D aux"));
    }

    #[test]
    fn wrapper_preserves_premultiplied_srgb_conversion_contract() {
        let source = generate_fragment_wrapper(
            "vec4 typhon_effect_main(TyphonEffectContext ctx) { return typhon_sample_primary(ctx.uv); }",
            &[],
        )
        .unwrap();
        assert!(source.contains("typhon_decode_premultiplied_srgb"));
        assert!(source.contains("typhon_encode_premultiplied_srgb"));
        assert!(!source.contains("result.rgb = typhon_srgb_to_linear"));
    }

    #[test]
    fn render_lookup_never_compiles_or_inserts_a_program() {
        let mut cache = ShaderProgramCache::new(2).unwrap();
        let key = key(1, 0);
        assert!(cache.lookup(key).is_err());
        assert_eq!(cache.len(), 0);
    }
}
