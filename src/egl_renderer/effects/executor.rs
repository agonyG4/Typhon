use std::io;

use glow::HasContext;
use oblivion_one::effects::{
    CompiledFrameGraph, CompiledRenderPass, EffectAlphaMode, EffectColorConversion,
    EffectExecutionDemand, EffectNodeKind, EffectRegion, GraphPassId, GraphTextureId,
    GraphTextureSource, INTERNAL_EFFECT_SHADER_MODULE_BLEND,
    INTERNAL_EFFECT_SHADER_MODULE_FRAGMENT, INTERNAL_EFFECT_SHADER_MODULE_MASK,
    MAX_EFFECT_REGION_RECTS, RenderPassKind, ShaderModuleId, logical_rect_to_physical_coverage,
};

use super::super::damage::OutputDamage;
use super::super::geometry::{
    EglDrawCommand, EglDrawLayer, SurfaceConsumerPlan, add_surface_consumers_for_capture_indices,
    add_surface_consumers_for_command_range,
};
use super::super::{GlesSceneRenderer, OutputFramebufferOrigin, OutputRect, RendererResult};
use super::{
    EffectDebugCaptureMode, EffectDebugConfig, EffectDebugKawaseMode, FrameTraceSummary,
    PassTraceSummary, blur, capture, effect_debug_config,
    resources::{
        EffectTextureFilter, EffectTextureFormat, EffectTextureKey, PooledEffectTexture,
        release_dead_graph_textures,
    },
    shader_cache::{ShaderProgramCache, ShaderProgramKey},
};

pub(super) const COPY_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

void main() {
    out_color = texture(u_effect_input, typhon_effect_sample_uv(v_uv));
}
"#;

pub(crate) const NORMALIZE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec4 u_effect_input_domain;
uniform vec4 u_effect_output_domain;
uniform int u_effect_decode_srgb;
uniform int u_effect_encode_srgb;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

float typhon_decode_srgb_channel(float value);
float typhon_encode_srgb_channel(float value);
vec4 typhon_sanitize_premultiplied(vec4 value);

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

void main() {
    vec2 output_position = u_effect_output_domain.xy + v_uv * u_effect_output_domain.zw;
    vec2 logical_input_uv = (output_position - u_effect_input_domain.xy) /
        u_effect_input_domain.zw;
    if (any(lessThan(logical_input_uv, vec2(0.0))) || any(greaterThan(logical_input_uv, vec2(1.0)))) {
        out_color = vec4(0.0);
        return;
    }
    vec4 result = texture(u_effect_input, typhon_effect_sample_uv(logical_input_uv));
    if (u_effect_decode_srgb != 0) result = typhon_decode_premultiplied_srgb(result);
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

pub(super) const COMPOSITE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec4 u_effect_input_domain;
uniform vec2 u_effect_output_size;
uniform int u_effect_encode_srgb;
uniform int u_effect_force_opaque;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
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

vec4 typhon_encode_premultiplied_srgb(vec4 value) {
    value = typhon_sanitize_premultiplied(value);
    if (value.a <= 0.00001) return vec4(0.0);
    vec3 straight = clamp(value.rgb / value.a, vec3(0.0), vec3(1.0));
    vec3 encoded = vec3(typhon_encode_srgb_channel(straight.r), typhon_encode_srgb_channel(straight.g), typhon_encode_srgb_channel(straight.b));
    return typhon_sanitize_premultiplied(vec4(encoded * value.a, value.a));
}

void main() {
    vec2 output_position = v_uv * u_effect_output_size;
    vec2 logical_input_uv = (output_position - u_effect_input_domain.xy) /
        u_effect_input_domain.zw;
    if (any(lessThan(logical_input_uv, vec2(0.0))) || any(greaterThan(logical_input_uv, vec2(1.0)))) {
        discard;
    }
    vec4 result = texture(u_effect_input, typhon_effect_sample_uv(logical_input_uv));
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    if (u_effect_force_opaque != 0) result.a = 1.0;
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

pub(super) const FRAGMENT_STAGE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform int u_effect_decode_srgb;
uniform int u_effect_encode_srgb;
uniform mat4 u_effect_color_matrix;
uniform vec4 u_effect_color_bias;
uniform vec4 u_effect_tint_color;
uniform float u_effect_tint_amount;
uniform float u_effect_noise_amount;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

float typhon_decode_srgb_channel(float value);
float typhon_encode_srgb_channel(float value);
vec4 typhon_sanitize_premultiplied(vec4 value);

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

void main() {
    vec4 result = texture(u_effect_input, typhon_effect_sample_uv(v_uv));
    if (u_effect_decode_srgb != 0) result = typhon_decode_premultiplied_srgb(result);
    result = u_effect_color_matrix * result + u_effect_color_bias;
    result.rgb = mix(result.rgb, result.rgb * u_effect_tint_color.rgb, clamp(u_effect_tint_amount, 0.0, 1.0));
    float noise = fract(sin(dot(v_uv, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
    result.rgb += noise * u_effect_noise_amount;
    result = typhon_sanitize_premultiplied(result);
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

pub(crate) const MASK_STAGE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform int u_effect_decode_srgb;
uniform int u_effect_encode_srgb;
uniform int u_effect_inverted;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

float typhon_decode_srgb_channel(float value);
float typhon_encode_srgb_channel(float value);
vec4 typhon_sanitize_premultiplied(vec4 value);

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

void main() {
    vec4 result = texture(u_effect_input, typhon_effect_sample_uv(v_uv));
    if (u_effect_decode_srgb != 0) result = typhon_decode_premultiplied_srgb(result);
    float coverage = u_effect_inverted != 0 ? 1.0 - result.a : result.a;
    result *= clamp(coverage, 0.0, 1.0);
    result = typhon_sanitize_premultiplied(result);
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

pub(crate) const BLEND_STAGE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform sampler2D u_effect_input_secondary;
uniform int u_effect_decode_srgb;
uniform int u_effect_encode_srgb;
uniform int u_effect_blend_mode;
uniform float u_effect_blend_opacity;
uniform int u_effect_input_flip_y;
in vec2 v_uv;
out vec4 out_color;

vec2 typhon_effect_sample_uv(vec2 logical_uv) {
    return u_effect_input_flip_y != 0
        ? vec2(logical_uv.x, 1.0 - logical_uv.y)
        : logical_uv;
}

float typhon_decode_srgb_channel(float value);
float typhon_encode_srgb_channel(float value);
vec4 typhon_sanitize_premultiplied(vec4 value);

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

void main() {
    vec2 sample_uv = typhon_effect_sample_uv(v_uv);
    vec4 first = texture(u_effect_input, sample_uv);
    vec4 second = texture(u_effect_input_secondary, sample_uv);
    if (u_effect_decode_srgb != 0) {
        first = typhon_decode_premultiplied_srgb(first);
        second = typhon_decode_premultiplied_srgb(second);
    }
    float opacity = clamp(u_effect_blend_opacity, 0.0, 1.0);
    float second_alpha = clamp(second.a * opacity, 0.0, 1.0);
    vec3 second_premultiplied = second.rgb * opacity;
    vec3 blended = second.rgb;
    if (u_effect_blend_mode == 1) {
        blended = first.rgb + second.rgb;
    } else if (u_effect_blend_mode == 2 || u_effect_blend_mode == 3) {
        vec3 first_straight = first.a > 0.00001 ? first.rgb / first.a : vec3(0.0);
        vec3 second_straight = second.a > 0.00001 ? second.rgb / second.a : vec3(0.0);
        blended = u_effect_blend_mode == 2
            ? first_straight * second_straight
            : 1.0 - (1.0 - first_straight) * (1.0 - second_straight);
    }
    vec3 rgb = u_effect_blend_mode == 0
        ? second.rgb * opacity + first.rgb * (1.0 - second_alpha)
        : u_effect_blend_mode == 1
            ? first.rgb + second.rgb * opacity
            : first.rgb * (1.0 - second_alpha) + second_premultiplied * (1.0 - first.a) + blended * first.a * second_alpha;
    float alpha = second_alpha + first.a * (1.0 - second_alpha);
    vec4 result = typhon_sanitize_premultiplied(vec4(rgb, alpha));
    if (u_effect_encode_srgb != 0) result = typhon_encode_premultiplied_srgb(result);
    out_color = typhon_sanitize_premultiplied(result);
}
"#;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct EffectExecutionStats {
    pub instances: usize,
    pub passes: usize,
    pub scene_captures: usize,
    /// Physical target-texture pixels covered by executed capture regions.
    /// This is execution work, not the allocated texture area.
    pub capture_execution_pixels: u64,
    pub blur_downsamples: usize,
    pub blur_upsamples: usize,
    pub composites: usize,
    pub resource_acquisitions: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EffectPassBlendMode {
    Replace,
    PremultipliedSourceOver,
}

fn effect_pass_blend_mode(
    kind: RenderPassKind,
    output_is_framebuffer: bool,
    alpha_mode: EffectAlphaMode,
) -> EffectPassBlendMode {
    if output_is_framebuffer
        && matches!(
            kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        )
        && alpha_mode == EffectAlphaMode::Preserve
    {
        EffectPassBlendMode::PremultipliedSourceOver
    } else {
        EffectPassBlendMode::Replace
    }
}

pub(crate) fn establish_effect_pass_blend_state(gl: &glow::Context, mode: EffectPassBlendMode) {
    unsafe {
        match mode {
            EffectPassBlendMode::Replace => gl.disable(glow::BLEND),
            EffectPassBlendMode::PremultipliedSourceOver => {
                gl.enable(glow::BLEND);
                gl.blend_func_separate(
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                    glow::ONE,
                    glow::ONE_MINUS_SRC_ALPHA,
                );
            }
        }
    }
}

fn capture_blend_mode() -> EffectPassBlendMode {
    EffectPassBlendMode::PremultipliedSourceOver
}

#[derive(Debug)]
struct PreparedEffectRegion {
    region: EffectRegion,
    input_rect_count: usize,
    duplicate_rects_removed: usize,
    overlap_fragments_generated: usize,
    fallback: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EffectExecutionInvariantError {
    MissingPassOutput(GraphPassId),
    UnknownTexture(GraphTextureId),
    UninitializedInputRegion {
        consumer: GraphPassId,
        input: GraphTextureId,
        missing: EffectRegion,
    },
    InvalidCheckpointSource {
        pass: GraphPassId,
        missing: EffectRegion,
    },
    SampledOutputTexture(GraphTextureId),
    MissingTextureResource(GraphTextureId),
    FeedbackTextureAlias {
        input: u64,
        output: u64,
    },
    InvalidTextureDimensions(GraphTextureId),
    InvalidTextureDomain(GraphTextureId),
    CaptureDomainOutsideOutput(GraphTextureId),
    InvalidScissor(GraphPassId),
    InvalidDomainMapping(GraphTextureId),
    InvalidFramebufferBlitTargets,
}

impl std::fmt::Display for EffectExecutionInvariantError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for EffectExecutionInvariantError {}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct EffectExecutionSelection {
    pub(crate) executed_passes: Vec<GraphPassId>,
    pub(crate) executed_instances: Vec<oblivion_one::effects::EffectInstanceId>,
    pub(crate) acquired_texture_ids: Vec<GraphTextureId>,
}

pub(crate) fn select_effect_execution(
    graph: &CompiledFrameGraph,
    demand: &EffectExecutionDemand,
) -> EffectExecutionSelection {
    let mut selection = EffectExecutionSelection::default();
    for pass in &graph.passes {
        if !demand.is_conservative_full() && !demand.contains(pass.instance) {
            continue;
        }
        if demand.has_pass_plan()
            && demand
                .pass_output_region(pass.id)
                .is_none_or(EffectRegion::is_empty)
        {
            continue;
        }
        selection.executed_passes.push(pass.id);
        if !selection.executed_instances.contains(&pass.instance) {
            selection.executed_instances.push(pass.instance);
        }
        for texture_id in pass.inputs.iter().copied().chain(pass.output) {
            let is_output = graph
                .textures
                .iter()
                .find(|texture| texture.id == texture_id)
                .is_some_and(|texture| texture.source == GraphTextureSource::Output);
            if !is_output && !selection.acquired_texture_ids.contains(&texture_id) {
                selection.acquired_texture_ids.push(texture_id);
            }
        }
    }
    selection
}

pub(crate) fn plan_effect_surface_consumers(
    graph: &CompiledFrameGraph,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
    commands: &[EglDrawCommand],
    repaint_rects: &[OutputRect],
    output_size: (u32, u32),
) -> SurfaceConsumerPlan {
    plan_effect_surface_consumers_with_debug_config(
        graph,
        demand,
        selection,
        commands,
        repaint_rects,
        output_size,
        *effect_debug_config(),
    )
}

pub(crate) fn plan_effect_surface_consumers_with_debug_config(
    graph: &CompiledFrameGraph,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
    commands: &[EglDrawCommand],
    repaint_rects: &[OutputRect],
    output_size: (u32, u32),
    debug_config: EffectDebugConfig,
) -> SurfaceConsumerPlan {
    let mut plan = SurfaceConsumerPlan::default();
    let scene_work = scene_work_regions(
        repaint_rects,
        graph,
        selection,
        output_size,
        false,
        debug_config,
    );
    let mut scene_cursor = 0;
    let layers = commands
        .iter()
        .map(|command| match command.layer {
            EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
            _ => capture::CaptureLayer::Other,
        })
        .collect::<Vec<_>>();
    let visual_groups = commands
        .iter()
        .map(|command| command.visual_group)
        .collect::<Vec<_>>();

    for pass in &graph.passes {
        if !selection.executed_passes.contains(&pass.id) {
            continue;
        }
        if matches!(
            pass.kind,
            RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
        ) && !pass.checkpoint_dependencies.is_empty()
        {
            let (draw_end, _) =
                composition_range(commands, pass.anchor, pass.visual_group, pass.anchor_scope);
            add_surface_consumers_for_command_range(
                &mut plan,
                commands,
                scene_cursor,
                draw_end,
                &scene_work.scene_work_rects,
            );
            scene_cursor = scene_cursor.max(draw_end.min(commands.len()));
        }
        if matches!(
            pass.kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        ) {
            let (draw_end, next_cursor) =
                composition_range(commands, pass.anchor, pass.visual_group, pass.anchor_scope);
            add_surface_consumers_for_command_range(
                &mut plan,
                commands,
                scene_cursor,
                draw_end,
                &scene_work.scene_work_rects,
            );
            scene_cursor = scene_cursor.max(next_cursor.min(commands.len()));
        }
        if matches!(
            pass.kind,
            RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
        ) {
            let indices = capture::indices_for_capture(
                &layers,
                &visual_groups,
                pass.anchor,
                pass.kind == RenderPassKind::SurfaceCapture,
                pass.visual_group,
                pass.anchor_scope,
            );
            let execution_damage = prepare_effect_execution_region(
                graph,
                pass,
                capture_execution_damage(graph, demand, pass, false, debug_config),
            );
            let target_domain = pass
                .output
                .and_then(|output| graph.textures.iter().find(|texture| texture.id == output))
                .map(|texture| texture.domain);
            let capture_rects =
                capture_materialization_plan(&execution_damage.region, target_domain, output_size)
                    .output_rects;
            add_surface_consumers_for_capture_indices(
                &mut plan,
                commands,
                &indices,
                &capture_rects,
            );
        }
    }
    add_surface_consumers_for_command_range(
        &mut plan,
        commands,
        scene_cursor,
        commands.len(),
        &scene_work.scene_work_rects,
    );
    plan.finish();
    plan
}

pub(crate) fn execute_effect_graph(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: &super::super::damage::RepaintPlan,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
) -> RendererResult<EffectExecutionStats> {
    execute_effect_graph_with_debug_config(
        renderer,
        graph,
        framebuffer_origin,
        repaint_plan,
        demand,
        selection,
        *effect_debug_config(),
    )
}

pub(crate) fn execute_effect_graph_with_debug_config(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: &super::super::damage::RepaintPlan,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
    debug_config: EffectDebugConfig,
) -> RendererResult<EffectExecutionStats> {
    let mut textures = std::collections::HashMap::new();
    let trace_summary = effect_trace_summary(renderer, graph, Some(repaint_plan), selection);
    renderer
        .effect_trace
        .frame_boundary("effect_resource_sync", "begin", trace_summary);
    renderer
        .effect_trace
        .frame_boundary("effect_graph_execute", "begin", trace_summary);
    let result = execute_graph_passes(
        renderer,
        graph,
        &mut textures,
        framebuffer_origin,
        Some(repaint_plan),
        None,
        demand,
        selection,
        debug_config,
        false,
        true,
    );
    renderer.effect_trace.frame_boundary(
        "effect_graph_execute",
        "end",
        effect_trace_summary(renderer, graph, Some(repaint_plan), selection),
    );
    renderer.effect_trace.frame_boundary(
        "effect_resource_sync",
        "end",
        effect_trace_summary(renderer, graph, Some(repaint_plan), selection),
    );
    if result.is_err() {
        renderer.establish_ordinary_scene_state();
        unsafe { renderer.gl.bind_texture(glow::TEXTURE_2D, None) };
    }
    renderer.effect_trace.frame_boundary(
        "effect_graph_release",
        "begin",
        effect_trace_summary(renderer, graph, Some(repaint_plan), selection),
    );
    let release_result = renderer.effect_resources.release_graph(textures);
    renderer.effect_trace.frame_boundary(
        "effect_graph_release",
        "end",
        effect_trace_summary(renderer, graph, Some(repaint_plan), selection),
    );
    match (result, release_result) {
        (Ok(stats), Ok(())) => Ok(stats),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

pub(crate) fn execute_effect_graph_for_lifecycle(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_rects: &[OutputRect],
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
) -> RendererResult<EffectExecutionStats> {
    let mut textures = std::collections::HashMap::new();
    let trace_summary = FrameTraceSummary {
        selected_effect_count: Some(selection.executed_instances.len()),
        graph_pass_count: Some(graph.stats.passes),
        graph_texture_count: Some(graph.stats.textures),
        peak_live_intermediate_count: Some(graph.stats.peak_live_intermediates),
        ..FrameTraceSummary::default()
    };
    renderer
        .effect_trace
        .frame_boundary("effect_resource_sync", "begin", trace_summary);
    renderer
        .effect_trace
        .frame_boundary("effect_graph_execute", "begin", trace_summary);
    let result = execute_graph_passes(
        renderer,
        graph,
        &mut textures,
        framebuffer_origin,
        None,
        Some(repaint_rects),
        demand,
        selection,
        *effect_debug_config(),
        true,
        false,
    );
    renderer
        .effect_trace
        .frame_boundary("effect_graph_execute", "end", trace_summary);
    renderer
        .effect_trace
        .frame_boundary("effect_resource_sync", "end", trace_summary);
    if result.is_err() {
        renderer.establish_ordinary_scene_state();
        unsafe { renderer.gl.bind_texture(glow::TEXTURE_2D, None) };
    }
    renderer
        .effect_trace
        .frame_boundary("effect_graph_release", "begin", trace_summary);
    let release_result = renderer.effect_resources.release_graph(textures);
    renderer
        .effect_trace
        .frame_boundary("effect_graph_release", "end", trace_summary);
    match (result, release_result) {
        (Ok(stats), Ok(())) => Ok(stats),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_graph_passes(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    textures: &mut std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: Option<&super::super::damage::RepaintPlan>,
    explicit_repaint_rects: Option<&[OutputRect]>,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
    debug_config: EffectDebugConfig,
    lifecycle_backdrop: bool,
    draw_overlays: bool,
) -> RendererResult<EffectExecutionStats> {
    let graph_scope = (!renderer.capture_in_progress)
        .then(|| {
            renderer
                .effect_gpu_profiler
                .begin_graph(&renderer.gl, renderer.effect_trace.frame_id())
        })
        .flatten();
    let result = execute_graph_passes_inner(
        renderer,
        graph,
        textures,
        framebuffer_origin,
        repaint_plan,
        explicit_repaint_rects,
        demand,
        selection,
        debug_config,
        lifecycle_backdrop,
        draw_overlays,
        graph_scope,
    );
    renderer
        .effect_gpu_profiler
        .end_graph(&renderer.gl, graph_scope);
    result
}

fn scene_advance_reason(
    pass: &CompiledRenderPass,
    framebuffer_capture: bool,
) -> Option<&'static str> {
    if !pass.checkpoint_dependencies.is_empty()
        && matches!(
            pass.kind,
            RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
        )
    {
        return Some("checkpoint_dependency");
    }
    (pass.kind == RenderPassKind::SceneCapture && framebuffer_capture)
        .then_some("framebuffer_capture")
}

#[allow(clippy::too_many_arguments)]
fn execute_graph_passes_inner(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    textures: &mut std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: Option<&super::super::damage::RepaintPlan>,
    explicit_repaint_rects: Option<&[OutputRect]>,
    demand: &EffectExecutionDemand,
    selection: &EffectExecutionSelection,
    debug_config: EffectDebugConfig,
    lifecycle_backdrop: bool,
    draw_overlays: bool,
    graph_scope: Option<super::gpu_timing::GraphTimingScope>,
) -> RendererResult<EffectExecutionStats> {
    let mut stats = EffectExecutionStats::default();
    #[cfg(any(debug_assertions, test))]
    // Validity belongs to this graph execution and logical texture ID. A
    // pooled physical allocation never carries validity into this map.
    let mut valid_regions = std::collections::HashMap::<GraphTextureId, EffectRegion>::new();
    let framebuffer_capture = !lifecycle_backdrop
        && repaint_plan.is_some()
        && debug_config.capture_mode() == EffectDebugCaptureMode::Framebuffer;
    let repaint_rects = if let Some(rects) = explicit_repaint_rects {
        rects.to_vec()
    } else {
        renderer.begin_effect_repaint(
            repaint_plan.expect("ordinary effect execution needs a repaint plan"),
            framebuffer_origin,
        )?
    };
    let scene_work = scene_work_regions(
        &repaint_rects,
        graph,
        selection,
        renderer.current_size,
        lifecycle_backdrop,
        debug_config,
    );
    let scene_work_rects = &scene_work.scene_work_rects;
    let output_size = renderer.current_size;
    let reconstruct_internal_scene_work =
        framebuffer_capture || !scene_work.extra_scene_work.is_empty();
    let mut scene_valid_region = EffectRegion::empty();
    let mut effect_valid_regions =
        std::collections::HashMap::<oblivion_one::effects::EffectInstanceId, EffectRegion>::new();
    let mut scene_work_preservation =
        if reconstruct_internal_scene_work && !scene_work.extra_scene_work.is_empty() {
            Some(capture_scene_work_preservation(
                renderer,
                output_size,
                framebuffer_origin,
            )?)
        } else {
            None
        };
    let execution_result = (|| -> RendererResult<EffectExecutionStats> {
        if reconstruct_internal_scene_work {
            renderer.clear_effect_scene_work(scene_work_rects, framebuffer_origin)?;
        }
        let mut scene_cursor = 0;
        for pass in &graph.passes {
            if !selection.executed_passes.contains(&pass.id) {
                continue;
            }
            let execution_damage = prepare_effect_execution_region(
                graph,
                pass,
                capture_execution_damage(graph, demand, pass, lifecycle_backdrop, debug_config),
            );
            if renderer.effect_trace.enabled() {
                renderer.effect_trace.execution_region(
                    pass,
                    execution_damage.input_rect_count,
                    execution_damage.region.rects().len(),
                    execution_damage.duplicate_rects_removed,
                    execution_damage.overlap_fragments_generated,
                    execution_damage.fallback,
                );
                if let Some((input_rect_count, clip_rect_count)) = pass.visible_clip_fallback {
                    renderer.effect_trace.visible_clip_fallback(
                        pass,
                        input_rect_count,
                        clip_rect_count,
                    );
                }
                let is_final_output_pass = matches!(
                    pass.kind,
                    RenderPassKind::Composite | RenderPassKind::OutputPostProcess
                );
                for fallback in demand.visible_clip_fallbacks().iter().filter(|fallback| {
                    fallback.instance == pass.instance
                        && (fallback.pass == Some(pass.id)
                            || (fallback.pass.is_none() && is_final_output_pass))
                }) {
                    renderer.effect_trace.visible_clip_fallback(
                        pass,
                        fallback.input_rect_count,
                        fallback.clip_rect_count,
                    );
                }
                renderer.effect_trace.pass_boundary(
                    "begin",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        lifecycle_backdrop,
                        debug_config,
                    ),
                );
            }
            if renderer.effect_trace.enabled() {
                renderer.effect_trace.pass_boundary(
                    "resources_begin",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        lifecycle_backdrop,
                        debug_config,
                    ),
                );
            }
            let resource_result = ensure_pass_textures(renderer, graph, pass, textures, &mut stats);
            if renderer.effect_trace.enabled() {
                renderer.effect_trace.pass_boundary(
                    "resources_end",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        lifecycle_backdrop,
                        debug_config,
                    ),
                );
            }
            resource_result?;
            if let Some(scene_advance_reason) = scene_advance_reason(pass, framebuffer_capture) {
                let (draw_end, _) = composition_range(
                    &renderer.commands,
                    pass.anchor,
                    pass.visual_group,
                    pass.anchor_scope,
                );
                if draw_end > scene_cursor {
                    if renderer.effect_trace.enabled() {
                        renderer.effect_trace.scene_replay_boundary(
                            "begin",
                            pass,
                            scene_advance_reason,
                            scene_cursor,
                            draw_end,
                        );
                    }
                    renderer.draw_effect_scene_range(
                        scene_work_rects,
                        scene_cursor,
                        draw_end,
                        framebuffer_origin,
                    )?;
                    scene_valid_region =
                        scene_valid_region.union(&output_rects_to_effect_region(scene_work_rects));
                    if renderer.effect_trace.enabled() {
                        renderer.effect_trace.scene_replay_boundary(
                            "end",
                            pass,
                            scene_advance_reason,
                            scene_cursor,
                            draw_end,
                        );
                    }
                    scene_cursor = draw_end;
                }
            }
            if matches!(
                pass.kind,
                RenderPassKind::Composite | RenderPassKind::OutputPostProcess
            ) {
                let (draw_end, next_cursor) = composition_range(
                    &renderer.commands,
                    pass.anchor,
                    pass.visual_group,
                    pass.anchor_scope,
                );
                if renderer.effect_trace.enabled() {
                    renderer.effect_trace.scene_replay_boundary(
                        "begin",
                        pass,
                        "composite_advance",
                        scene_cursor,
                        draw_end,
                    );
                }
                renderer.draw_effect_scene_range(
                    scene_work_rects,
                    scene_cursor,
                    draw_end,
                    framebuffer_origin,
                )?;
                if draw_end > scene_cursor {
                    scene_valid_region =
                        scene_valid_region.union(&output_rects_to_effect_region(scene_work_rects));
                }
                if renderer.effect_trace.enabled() {
                    renderer.effect_trace.scene_replay_boundary(
                        "end",
                        pass,
                        "composite_advance",
                        scene_cursor,
                        draw_end,
                    );
                }
                scene_cursor = next_cursor.max(scene_cursor);
            }
            if is_direct_framebuffer_capture(pass, lifecycle_backdrop, debug_config) {
                let required = pass_output_texture_domain(graph, pass);
                let required_effect_region =
                    checkpoint_dependency_influence_region(graph, pass, &required);
                let mut dependency_validity = Vec::new();
                for dependency in &pass.checkpoint_dependencies {
                    let Some(dependency_pass) = graph
                        .passes
                        .iter()
                        .find(|candidate| candidate.id == *dependency)
                    else {
                        continue;
                    };
                    let dependency_influence = graph
                        .instances
                        .iter()
                        .find(|instance| instance.id == dependency_pass.instance)
                        .map(|instance| instance.output_influence_region.intersect(&required))
                        .unwrap_or_else(EffectRegion::empty);
                    let dependency_valid = effect_valid_regions
                        .get(&dependency_pass.instance)
                        .cloned()
                        .unwrap_or_else(EffectRegion::empty);
                    dependency_validity.push((dependency_influence, dependency_valid));
                }
                let validity = checkpoint_source_semantic_validity(
                    &required,
                    &scene_valid_region,
                    &required_effect_region,
                    &dependency_validity,
                );
                if renderer.effect_trace.enabled() {
                    renderer.effect_trace.checkpoint_source_validity(
                        pass,
                        validity.required_rect_count,
                        validity.required_bounding_box,
                        validity.valid_rect_count,
                        validity.valid_bounding_box,
                        validity.missing.rects().len(),
                        validity
                            .missing
                            .bounding_rect()
                            .map(|rect| (rect.x, rect.y, rect.width, rect.height)),
                        effect_region_pixels(&validity.missing),
                    );
                }
                #[cfg(any(debug_assertions, test))]
                if !validity.missing.is_empty() {
                    let error = EffectExecutionInvariantError::InvalidCheckpointSource {
                        pass: pass.id,
                        missing: validity.missing,
                    };
                    renderer.effect_trace.invariant_failure(&error);
                    return Err(Box::new(error));
                }
            }
            if renderer.effect_trace.enabled() {
                renderer.effect_trace.pass_boundary(
                    "validate_begin",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        lifecycle_backdrop,
                        debug_config,
                    ),
                );
            }
            let validation_result = validate_effect_pass_resources(
                renderer,
                graph,
                pass,
                textures,
                &execution_damage.region,
                framebuffer_origin,
            );
            if renderer.effect_trace.enabled() {
                renderer.effect_trace.pass_boundary(
                    "validate_end",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        lifecycle_backdrop,
                        debug_config,
                    ),
                );
            }
            if let Err(error) = validation_result {
                renderer.effect_trace.invariant_failure(&error);
                return Err(Box::new(error));
            }
            #[cfg(any(debug_assertions, test))]
            if let Err(error) = validate_current_frame_input_regions(
                graph,
                pass,
                &execution_damage.region,
                &valid_regions,
            ) {
                renderer.effect_trace.invariant_failure(&error);
                return Err(Box::new(error));
            }
            if renderer.effect_trace.enabled() {
                renderer.effect_trace.pass_boundary(
                    "execute_begin",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        lifecycle_backdrop,
                        debug_config,
                    ),
                );
            }
            let pass_timing = graph_scope.and_then(|scope| {
                if renderer.capture_in_progress {
                    return None;
                }
                renderer.effect_gpu_profiler.begin_pass(
                    &renderer.gl,
                    scope,
                    u64::from(pass.id.get()),
                    pass.instance.get(),
                    pass.kind,
                    effect_region_pixels(&execution_damage.region),
                )
            });
            let execute_result = execute_pass(
                renderer,
                graph,
                textures,
                pass,
                framebuffer_origin,
                &execution_damage.region,
                lifecycle_backdrop,
                debug_config,
                &mut stats,
            );
            renderer
                .effect_gpu_profiler
                .end_pass(&renderer.gl, pass_timing);
            if renderer.effect_trace.enabled() {
                renderer.effect_trace.pass_boundary(
                    "execute_end",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        lifecycle_backdrop,
                        debug_config,
                    ),
                );
            }
            if let Err(error) = execute_result {
                if let Some(invariant) = error.downcast_ref::<EffectExecutionInvariantError>() {
                    renderer.effect_trace.invariant_failure(invariant);
                }
                return Err(error);
            }
            if matches!(
                pass.kind,
                RenderPassKind::Composite | RenderPassKind::OutputPostProcess
            ) {
                effect_valid_regions
                    .entry(pass.instance)
                    .and_modify(|valid| *valid = valid.union(&execution_damage.region))
                    .or_insert_with(|| execution_damage.region.clone());
            }
            #[cfg(any(debug_assertions, test))]
            {
                let output_region = record_current_frame_output_region(
                    renderer,
                    graph,
                    pass,
                    &execution_damage.region,
                    lifecycle_backdrop,
                    debug_config,
                )
                .map_err(|error| {
                    renderer.effect_trace.invariant_failure(&error);
                    Box::new(error) as Box<dyn std::error::Error>
                })?;
                valid_regions.insert(pass.output.expect("validated pass output"), output_region);
            }
            if renderer.effect_trace.enabled() {
                renderer.effect_trace.pass_boundary(
                    "end",
                    pass,
                    graph,
                    textures,
                    pass_trace_summary(
                        renderer,
                        graph,
                        pass,
                        demand,
                        &execution_damage.region,
                        scene_work_rects,
                        framebuffer_origin,
                        lifecycle_backdrop,
                        debug_config,
                    ),
                );
            }
            release_dead_graph_textures(&mut renderer.effect_resources, graph, pass.id, textures)?;
            stats.passes = stats.passes.saturating_add(1);
        }
        let final_scene_cursor_end = renderer.commands.len();
        if renderer.effect_trace.enabled() {
            renderer.effect_trace.final_scene_replay_boundary(
                "begin",
                scene_cursor,
                final_scene_cursor_end,
            );
        }
        renderer.draw_effect_scene_range(
            scene_work_rects,
            scene_cursor,
            final_scene_cursor_end,
            framebuffer_origin,
        )?;
        if renderer.effect_trace.enabled() {
            renderer.effect_trace.final_scene_replay_boundary(
                "end",
                scene_cursor,
                final_scene_cursor_end,
            );
        }
        if let Some(preservation) = scene_work_preservation.take() {
            let restore_result = restore_scene_work_preservation(
                renderer,
                &preservation,
                &scene_work.extra_scene_work,
                framebuffer_origin,
            );
            let release_result = renderer.effect_resources.release(preservation.texture);
            restore_result?;
            release_result?;
        }
        if draw_overlays {
            if renderer.effect_trace.enabled() {
                renderer.effect_trace.overlay_boundary("begin");
            }
            renderer.draw_lifecycle_overlays(
                &repaint_rects,
                framebuffer_origin,
                repaint_plan.expect("ordinary effect execution needs a repaint plan"),
            )?;
            renderer.draw_effect_overlays(&repaint_rects, framebuffer_origin)?;
            if renderer.effect_trace.enabled() {
                renderer.effect_trace.overlay_boundary("end");
            }
        }
        renderer.establish_ordinary_scene_state();
        stats.instances = selection.executed_instances.len();
        Ok(stats)
    })();
    let cleanup_result = if let Some(preservation) = scene_work_preservation.take() {
        let restore_result = restore_scene_work_preservation(
            renderer,
            &preservation,
            &scene_work.extra_scene_work,
            framebuffer_origin,
        );
        let release_result = renderer.effect_resources.release(preservation.texture);
        restore_result.and(release_result.map_err(Into::into))
    } else {
        Ok(())
    };
    match (execution_result, cleanup_result) {
        (Ok(stats), Ok(())) => Ok(stats),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn effect_region_pixels(region: &EffectRegion) -> u64 {
    region.rects().iter().fold(0u64, |total, rect| {
        total.saturating_add(u64::from(rect.width).saturating_mul(u64::from(rect.height)))
    })
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CheckpointSourceValidity {
    required_rect_count: usize,
    required_bounding_box: Option<(i32, i32, u32, u32)>,
    valid_rect_count: usize,
    valid_bounding_box: Option<(i32, i32, u32, u32)>,
    missing: EffectRegion,
}

fn checkpoint_source_validity(
    required: &EffectRegion,
    valid: &EffectRegion,
) -> CheckpointSourceValidity {
    CheckpointSourceValidity {
        required_rect_count: required.rects().len(),
        required_bounding_box: required
            .bounding_rect()
            .map(|rect| (rect.x, rect.y, rect.width, rect.height)),
        valid_rect_count: valid.rects().len(),
        valid_bounding_box: valid
            .bounding_rect()
            .map(|rect| (rect.x, rect.y, rect.width, rect.height)),
        missing: required.subtract(valid),
    }
}

fn checkpoint_source_semantic_validity(
    required: &EffectRegion,
    scene_valid: &EffectRegion,
    dependency_influence: &EffectRegion,
    dependencies: &[(EffectRegion, EffectRegion)],
) -> CheckpointSourceValidity {
    let mut missing = required
        .subtract(dependency_influence)
        .subtract(scene_valid);
    for (influence, valid) in dependencies {
        let required_from_dependency = required.intersect(influence);
        missing = missing.union(&required_from_dependency.subtract(valid));
    }
    let semantic_valid = required.subtract(&missing);
    checkpoint_source_validity(required, &semantic_valid)
}

fn output_rects_to_effect_region(rects: &[OutputRect]) -> EffectRegion {
    let mut region = EffectRegion::empty();
    for rect in rects {
        if let Some(effect_rect) =
            oblivion_one::effects::EffectRect::new(rect.x, rect.y, rect.width, rect.height)
        {
            region.push(effect_rect);
        }
    }
    region
}

fn prepare_effect_execution_region(
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    input: EffectRegion,
) -> PreparedEffectRegion {
    let input_rect_count = input.rects().len();
    let final_output = matches!(
        pass.kind,
        RenderPassKind::Composite | RenderPassKind::OutputPostProcess
    );
    let authoritative_clip = final_output.then(|| {
        graph
            .instances
            .iter()
            .find(|instance| instance.id == pass.instance)
            .map(|instance| instance.output_influence_region.clone())
    });
    let mut duplicate_rects_removed = 0;

    let (candidate, mut fallback) = if let Some(Some(clip)) = authoritative_clip.as_ref() {
        let bounded = input.intersect_bounded_within_result(clip);
        duplicate_rects_removed = bounded.duplicate_rects_removed;
        if bounded.overflowed {
            (clip.clone(), Some("output_influence"))
        } else {
            (bounded.region, None)
        }
    } else if final_output {
        (EffectRegion::empty(), None)
    } else {
        (input, None)
    };
    let disjoint = candidate.disjoint_bounded();
    let region = if disjoint.overflowed {
        fallback = Some(if final_output {
            "output_influence"
        } else {
            "work_region_bbox_coalesce"
        });
        if final_output {
            authoritative_clip
                .flatten()
                .unwrap_or_else(EffectRegion::empty)
        } else {
            candidate
                .bounding_rect()
                .map(EffectRegion::from_rect)
                .unwrap_or_else(|| pass_output_texture_domain(graph, pass))
        }
    } else {
        disjoint.region
    };

    PreparedEffectRegion {
        region,
        input_rect_count,
        duplicate_rects_removed: duplicate_rects_removed
            .saturating_add(disjoint.duplicate_rects_removed),
        overlap_fragments_generated: disjoint.overlap_fragments_generated,
        fallback,
    }
}

fn output_rect_pixels(rects: &[OutputRect]) -> u64 {
    rects.iter().fold(0u64, |total, rect| {
        total.saturating_add(u64::from(rect.width).saturating_mul(u64::from(rect.height)))
    })
}

fn effective_pass_damage(
    graph: &CompiledFrameGraph,
    demand: &EffectExecutionDemand,
    pass: &CompiledRenderPass,
) -> EffectRegion {
    let authoritative_output = graph
        .instances
        .iter()
        .find(|instance| instance.id == pass.instance)
        .map(|instance| &instance.output_influence_region);
    if demand.has_pass_plan() {
        let planned = demand
            .pass_output_region(pass.id)
            .cloned()
            .unwrap_or_else(|| {
                if demand.is_conservative_full()
                    || demand.instance_is_conservative_full(pass.instance)
                {
                    pass_output_texture_domain(graph, pass)
                } else {
                    EffectRegion::empty()
                }
            });
        return if matches!(
            pass.kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        ) {
            authoritative_output.map_or_else(EffectRegion::empty, |clip| {
                planned.intersect_bounded_within(clip)
            })
        } else {
            planned
        };
    }
    let Some(output_region) = demand.output_region(pass.instance) else {
        return if matches!(
            pass.kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        ) {
            authoritative_output
                .cloned()
                .unwrap_or_else(EffectRegion::empty)
        } else if demand.is_conservative_full() {
            pass.output
                .and_then(|output| graph.textures.iter().find(|texture| texture.id == output))
                .map_or_else(EffectRegion::empty, |texture| {
                    EffectRegion::from_rect(texture.domain)
                })
        } else {
            EffectRegion::empty()
        };
    };
    if matches!(
        pass.kind,
        RenderPassKind::Composite | RenderPassKind::OutputPostProcess
    ) {
        return authoritative_output.map_or_else(EffectRegion::empty, |clip| {
            pass.damage
                .union(output_region)
                .intersect_bounded_within(clip)
        });
    }
    pass.output
        .and_then(|output| graph.textures.iter().find(|texture| texture.id == output))
        .map_or_else(
            || output_region.clone(),
            |texture| EffectRegion::from_rect(texture.domain),
        )
}

fn pass_output_texture_domain(
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
) -> EffectRegion {
    pass.output
        .and_then(|output| graph.textures.iter().find(|texture| texture.id == output))
        .map_or_else(EffectRegion::empty, |texture| {
            EffectRegion::from_rect(texture.domain)
        })
}

fn checkpoint_dependency_influence_region(
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    required: &EffectRegion,
) -> EffectRegion {
    let mut influence = EffectRegion::empty();
    for dependency in &pass.checkpoint_dependencies {
        let Some(dependency_pass) = graph
            .passes
            .iter()
            .find(|candidate| candidate.id == *dependency)
        else {
            continue;
        };
        let Some(dependency_instance) = graph
            .instances
            .iter()
            .find(|instance| instance.id == dependency_pass.instance)
        else {
            continue;
        };
        influence = influence.union(
            &dependency_instance
                .output_influence_region
                .intersect(required),
        );
    }
    influence
}

fn is_direct_framebuffer_capture(
    pass: &CompiledRenderPass,
    lifecycle_backdrop: bool,
    debug_config: EffectDebugConfig,
) -> bool {
    match pass.kind {
        RenderPassKind::SceneCapture => {
            lifecycle_backdrop
                || !pass.checkpoint_dependencies.is_empty()
                || debug_config.capture_mode() == EffectDebugCaptureMode::Framebuffer
        }
        RenderPassKind::SurfaceCapture => !pass.checkpoint_dependencies.is_empty(),
        _ => false,
    }
}

fn capture_execution_damage(
    graph: &CompiledFrameGraph,
    demand: &EffectExecutionDemand,
    pass: &CompiledRenderPass,
    lifecycle_backdrop: bool,
    debug_config: EffectDebugConfig,
) -> EffectRegion {
    if is_direct_framebuffer_capture(pass, lifecycle_backdrop, debug_config) {
        pass_output_texture_domain(graph, pass)
    } else {
        effective_pass_damage(graph, demand, pass)
    }
}

fn validate_effect_pass_resources(
    renderer: &GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    textures: &std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    execution_damage: &EffectRegion,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Result<(), EffectExecutionInvariantError> {
    let output = pass
        .output
        .ok_or(EffectExecutionInvariantError::MissingPassOutput(pass.id))?;
    let output_plan = graph
        .textures
        .iter()
        .find(|texture| texture.id == output)
        .ok_or(EffectExecutionInvariantError::UnknownTexture(output))?;
    validate_graph_texture_plan(renderer, output_plan)?;
    validate_domain_mapping(output_plan)?;

    let output_physical =
        if output_plan.source == GraphTextureSource::Output {
            None
        } else {
            let texture = textures.get(&output).ok_or(
                EffectExecutionInvariantError::MissingTextureResource(output),
            )?;
            Some(
                renderer
                    .effect_resources
                    .physical_texture_id(texture)
                    .ok_or(EffectExecutionInvariantError::MissingTextureResource(
                        output,
                    ))?,
            )
        };

    for input in &pass.inputs {
        let input_plan = graph
            .textures
            .iter()
            .find(|texture| texture.id == *input)
            .ok_or(EffectExecutionInvariantError::UnknownTexture(*input))?;
        validate_graph_texture_plan(renderer, input_plan)?;
        validate_domain_mapping(input_plan)?;
        if input_plan.source == GraphTextureSource::Output {
            return Err(EffectExecutionInvariantError::SampledOutputTexture(*input));
        }
        let input_texture =
            textures
                .get(input)
                .ok_or(EffectExecutionInvariantError::MissingTextureResource(
                    *input,
                ))?;
        let input_physical = renderer
            .effect_resources
            .physical_texture_id(input_texture)
            .ok_or(EffectExecutionInvariantError::MissingTextureResource(
                *input,
            ))?;
        validate_no_texture_feedback(output_physical, std::iter::once(input_physical))?;
    }

    if matches!(
        pass.kind,
        RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample
    ) {
        let input = pass
            .inputs
            .first()
            .and_then(|input| graph.textures.iter().find(|texture| texture.id == *input))
            .ok_or(EffectExecutionInvariantError::UnknownTexture(
                pass.inputs.first().copied().unwrap_or(output),
            ))?;
        if input.width == 0
            || input.height == 0
            || output_plan.width == 0
            || output_plan.height == 0
        {
            return Err(EffectExecutionInvariantError::InvalidTextureDimensions(
                output,
            ));
        }
    }

    if !execution_damage.is_empty() {
        if let Some(bounding_box) = execution_damage.bounding_rect()
            && effect_rect_to_texture_rect(bounding_box, output_plan, framebuffer_origin).is_none()
        {
            return Err(EffectExecutionInvariantError::InvalidScissor(pass.id));
        }
        for rect in execution_damage.rects() {
            let Some(scissor) = effect_rect_to_texture_rect(*rect, output_plan, framebuffer_origin)
            else {
                continue;
            };
            let right = i64::from(scissor.x).saturating_add(i64::from(scissor.width));
            let bottom = i64::from(scissor.y).saturating_add(i64::from(scissor.height));
            if scissor.x < 0
                || scissor.y < 0
                || scissor.width == 0
                || scissor.height == 0
                || right > i64::from(output_plan.width)
                || bottom > i64::from(output_plan.height)
            {
                return Err(EffectExecutionInvariantError::InvalidScissor(pass.id));
            }
        }
    }

    Ok(())
}

#[cfg(any(debug_assertions, test))]
fn validate_current_frame_input_regions(
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    execution_damage: &EffectRegion,
    valid_regions: &std::collections::HashMap<GraphTextureId, EffectRegion>,
) -> Result<(), EffectExecutionInvariantError> {
    let output = pass
        .output
        .ok_or(EffectExecutionInvariantError::MissingPassOutput(pass.id))?;
    let output_plan = graph
        .textures
        .iter()
        .find(|texture| texture.id == output)
        .ok_or(EffectExecutionInvariantError::UnknownTexture(output))?;

    for input_id in &pass.inputs {
        let input_plan = graph
            .textures
            .iter()
            .find(|texture| texture.id == *input_id)
            .ok_or(EffectExecutionInvariantError::UnknownTexture(*input_id))?;
        if matches!(input_plan.source, GraphTextureSource::Static(_)) {
            continue;
        }
        let required = oblivion_one::effects::required_input_region(
            pass,
            execution_damage,
            output_plan,
            input_plan,
        )
        .ok_or(EffectExecutionInvariantError::UninitializedInputRegion {
            consumer: pass.id,
            input: *input_id,
            missing: EffectRegion::from_rect(input_plan.domain),
        })?;
        let valid = valid_regions
            .get(input_id)
            .cloned()
            .unwrap_or_else(EffectRegion::empty);
        let missing = required.subtract(&valid);
        if !missing.is_empty() {
            return Err(EffectExecutionInvariantError::UninitializedInputRegion {
                consumer: pass.id,
                input: *input_id,
                missing,
            });
        }
    }
    Ok(())
}

#[cfg(any(debug_assertions, test))]
fn record_current_frame_output_region(
    renderer: &GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    execution_damage: &EffectRegion,
    lifecycle_backdrop: bool,
    debug_config: EffectDebugConfig,
) -> Result<EffectRegion, EffectExecutionInvariantError> {
    let output = pass
        .output
        .ok_or(EffectExecutionInvariantError::MissingPassOutput(pass.id))?;
    let output_plan = graph
        .textures
        .iter()
        .find(|texture| texture.id == output)
        .ok_or(EffectExecutionInvariantError::UnknownTexture(output))?;
    if matches!(
        pass.kind,
        RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
    ) {
        if is_direct_framebuffer_capture(pass, lifecycle_backdrop, debug_config) {
            return Ok(EffectRegion::from_rect(output_plan.domain));
        }
        return Ok(capture_materialization_plan(
            execution_damage,
            Some(output_plan.domain),
            renderer.current_size,
        )
        .region);
    }
    Ok(execution_damage.intersect_rect(output_plan.domain))
}

fn validate_domain_mapping(
    texture: &oblivion_one::effects::GraphTexturePlan,
) -> Result<(), EffectExecutionInvariantError> {
    let domain_width = i128::from(texture.domain.width);
    let domain_height = i128::from(texture.domain.height);
    if domain_width
        .checked_mul(i128::from(texture.width))
        .is_none()
        || domain_height
            .checked_mul(i128::from(texture.height))
            .is_none()
        || domain_width * i128::from(texture.width) > i128::from(i64::MAX)
        || domain_height * i128::from(texture.height) > i128::from(i64::MAX)
    {
        return Err(EffectExecutionInvariantError::InvalidDomainMapping(
            texture.id,
        ));
    }
    Ok(())
}

fn validate_no_texture_feedback(
    output_physical: Option<u64>,
    input_physical_ids: impl Iterator<Item = u64>,
) -> Result<(), EffectExecutionInvariantError> {
    let Some(output_physical) = output_physical else {
        return Ok(());
    };
    for input_physical in input_physical_ids {
        if input_physical == output_physical {
            return Err(EffectExecutionInvariantError::FeedbackTextureAlias {
                input: input_physical,
                output: output_physical,
            });
        }
    }
    Ok(())
}

fn validate_graph_texture_plan(
    renderer: &GlesSceneRenderer,
    texture: &oblivion_one::effects::GraphTexturePlan,
) -> Result<(), EffectExecutionInvariantError> {
    let domain = texture.domain;
    if texture.width == 0 || texture.height == 0 || domain.width == 0 || domain.height == 0 {
        return Err(EffectExecutionInvariantError::InvalidTextureDimensions(
            texture.id,
        ));
    }
    let Some(domain_right) = i64::from(domain.x).checked_add(i64::from(domain.width)) else {
        return Err(EffectExecutionInvariantError::InvalidTextureDomain(
            texture.id,
        ));
    };
    let Some(domain_bottom) = i64::from(domain.y).checked_add(i64::from(domain.height)) else {
        return Err(EffectExecutionInvariantError::InvalidTextureDomain(
            texture.id,
        ));
    };
    if domain_right > i64::from(i32::MAX) || domain_bottom > i64::from(i32::MAX) {
        return Err(EffectExecutionInvariantError::InvalidTextureDomain(
            texture.id,
        ));
    }
    if matches!(
        texture.source,
        GraphTextureSource::CapturedScene
            | GraphTextureSource::CapturedTarget
            | GraphTextureSource::Output
    ) && (domain.x < 0
        || domain.y < 0
        || i64::from(domain.right()) > i64::from(renderer.current_size.0)
        || i64::from(domain.bottom()) > i64::from(renderer.current_size.1))
    {
        return Err(EffectExecutionInvariantError::CaptureDomainOutsideOutput(
            texture.id,
        ));
    }
    Ok(())
}

fn effect_trace_summary(
    renderer: &GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    repaint_plan: Option<&super::super::damage::RepaintPlan>,
    selection: &EffectExecutionSelection,
) -> FrameTraceSummary {
    FrameTraceSummary {
        repaint_mode: repaint_plan.map(|plan| plan.mode.as_str()),
        render_damage_signature: repaint_plan.map(|plan| plan.render_damage.identity_signature()),
        repair_damage_signature: repaint_plan.map(|plan| plan.repair_damage.identity_signature()),
        visible_effect_count: Some(renderer.frame_stats.effect_instances_visible),
        selected_effect_count: Some(selection.executed_instances.len()),
        graph_pass_count: Some(graph.stats.passes),
        graph_texture_count: Some(graph.stats.textures),
        peak_live_intermediate_count: Some(graph.stats.peak_live_intermediates),
        ..FrameTraceSummary::default()
    }
}

#[allow(clippy::too_many_arguments)]
fn pass_trace_summary(
    renderer: &GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    demand: &EffectExecutionDemand,
    execution_damage: &EffectRegion,
    scene_work_rects: &[OutputRect],
    framebuffer_origin: OutputFramebufferOrigin,
    lifecycle_backdrop: bool,
    debug_config: EffectDebugConfig,
) -> PassTraceSummary {
    if !renderer.effect_trace.enabled() {
        return PassTraceSummary::default();
    }

    let input_flip_y = pass
        .inputs
        .first()
        .and_then(|input| graph.textures.iter().find(|texture| texture.id == *input))
        .is_some_and(|texture| effect_input_requires_sample_y_flip(texture.origin));
    let output_plan = pass
        .output
        .and_then(|output| graph.textures.iter().find(|texture| texture.id == output));
    let output_is_framebuffer =
        output_plan.is_some_and(|texture| texture.source == GraphTextureSource::Output);
    let target_flip_y =
        effect_target_requires_logical_y_flip(output_is_framebuffer, framebuffer_origin);
    let direct_capture = is_direct_framebuffer_capture(pass, lifecycle_backdrop, debug_config);
    let conservative_pass_demand = direct_capture
        || demand.is_conservative_full()
        || demand.instance_is_conservative_full(pass.instance);
    let debug_full_kawase = debug_config.kawase_mode() == EffectDebugKawaseMode::Full
        && matches!(
            pass.kind,
            RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample
        );
    let conservative_pass_demand_kind = if direct_capture {
        "direct_framebuffer_capture"
    } else if debug_full_kawase {
        "debug_full_kawase"
    } else if conservative_pass_demand
        && matches!(
            pass.kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        )
    {
        "final_output_constrained"
    } else if conservative_pass_demand {
        "internal_full_domain"
    } else {
        "precise"
    };
    let capture_mode = matches!(
        pass.kind,
        RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
    )
    .then_some(if direct_capture {
        "framebuffer_blit"
    } else {
        "replay"
    });
    let capture_command_count = if capture_mode == Some("replay") {
        let layers = renderer
            .commands
            .iter()
            .map(|command| match command.layer {
                EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
                _ => capture::CaptureLayer::Other,
            })
            .collect::<Vec<_>>();
        let visual_groups = renderer
            .commands
            .iter()
            .map(|command| command.visual_group)
            .collect::<Vec<_>>();
        Some(
            capture::indices_for_capture(
                &layers,
                &visual_groups,
                pass.anchor,
                pass.kind == RenderPassKind::SurfaceCapture,
                pass.visual_group,
                pass.anchor_scope,
            )
            .len(),
        )
    } else {
        None
    };
    let damage_bounding_box = execution_damage
        .bounding_rect()
        .map(|rect| (rect.x, rect.y, rect.width, rect.height));
    PassTraceSummary {
        target_flip_y,
        input_flip_y,
        framebuffer_origin: Some(match framebuffer_origin {
            OutputFramebufferOrigin::BottomLeft => "bottom_left",
            OutputFramebufferOrigin::TopLeftScanout => "top_left_scanout",
        }),
        damage_rect_count: execution_damage.rects().len(),
        damage_bounding_box,
        capture_mode,
        capture_command_count,
        read_framebuffer: direct_capture.then(|| {
            renderer.active_output_framebuffer.map_or_else(
                || "default".to_owned(),
                |framebuffer| format!("{framebuffer:?}"),
            )
        }),
        draw_framebuffer: direct_capture
            .then(|| renderer.effect_resources.scratch_framebuffer_identity())
            .flatten(),
        scratch_fbo_present: direct_capture.then(|| {
            renderer
                .effect_resources
                .scratch_framebuffer_identity()
                .is_some()
        }),
        conservative_pass_demand,
        conservative_pass_demand_kind,
        backdrop_capture_policy: Some(debug_config.capture_mode().as_str()),
        kawase_execution_policy: Some(debug_config.kawase_mode().as_str()),
        scene_work_damage_rects: scene_work_rects.len(),
        scene_work_damage_bounding_box: output_rects_bounding_box(scene_work_rects),
    }
}

fn ensure_pass_textures(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    textures: &mut std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    stats: &mut EffectExecutionStats,
) -> RendererResult<()> {
    for texture_id in pass.inputs.iter().copied().chain(pass.output) {
        let plan = graph_texture(graph, texture_id)?;
        if plan.source == GraphTextureSource::Output || textures.contains_key(&texture_id) {
            continue;
        }
        let realized = renderer.effect_resources.acquire_plan(&renderer.gl, plan)?;
        textures.insert(texture_id, realized);
        stats.resource_acquisitions = stats.resource_acquisitions.saturating_add(1);
    }
    Ok(())
}

fn composition_position(
    layers: &[capture::CaptureLayer],
    anchor: oblivion_one::compositor::EffectAnchor,
) -> usize {
    match anchor {
        oblivion_one::compositor::EffectAnchor::BeforeSurface(surface_id)
        | oblivion_one::compositor::EffectAnchor::ReplaceSurface(surface_id) => layers
            .iter()
            .position(|layer| *layer == capture::CaptureLayer::Surface(surface_id))
            .unwrap_or(layers.len()),
        oblivion_one::compositor::EffectAnchor::AfterSurface(surface_id) => layers
            .iter()
            .rposition(|layer| *layer == capture::CaptureLayer::Surface(surface_id))
            .map_or(layers.len(), |index| index.saturating_add(1)),
        oblivion_one::compositor::EffectAnchor::OutputPostProcess => layers.len(),
    }
}

fn composition_range(
    commands: &[super::super::geometry::EglDrawCommand],
    anchor: oblivion_one::compositor::EffectAnchor,
    visual_group: Option<oblivion_one::compositor::VisualGroupId>,
    anchor_scope: oblivion_one::compositor::EffectAnchorScope,
) -> (usize, usize) {
    if anchor_scope == oblivion_one::compositor::EffectAnchorScope::VisualGroup {
        let Some(visual_group) = visual_group else {
            return (commands.len(), commands.len());
        };
        let Some(start) = commands
            .iter()
            .position(|command| command.visual_group == Some(visual_group))
        else {
            return (commands.len(), commands.len());
        };
        let end = commands
            .iter()
            .rposition(|command| command.visual_group == Some(visual_group))
            .map_or(start, |index| index.saturating_add(1));
        return match anchor {
            oblivion_one::compositor::EffectAnchor::BeforeSurface(_) => (start, start),
            oblivion_one::compositor::EffectAnchor::ReplaceSurface(_) => (start, end),
            oblivion_one::compositor::EffectAnchor::AfterSurface(_) => (end, end),
            oblivion_one::compositor::EffectAnchor::OutputPostProcess => {
                (commands.len(), commands.len())
            }
        };
    }
    let position = composition_position(
        &commands
            .iter()
            .map(|command| match command.layer {
                EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
                _ => capture::CaptureLayer::Other,
            })
            .collect::<Vec<_>>(),
        anchor,
    );
    match anchor {
        oblivion_one::compositor::EffectAnchor::BeforeSurface(_) => (position, position),
        oblivion_one::compositor::EffectAnchor::ReplaceSurface(_) => {
            (position, position.saturating_add(1))
        }
        oblivion_one::compositor::EffectAnchor::AfterSurface(_) => (position, position),
        oblivion_one::compositor::EffectAnchor::OutputPostProcess => {
            (commands.len(), commands.len())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_pass(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    textures: &std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    pass: &CompiledRenderPass,
    framebuffer_origin: OutputFramebufferOrigin,
    execution_damage: &EffectRegion,
    lifecycle_backdrop: bool,
    debug_config: EffectDebugConfig,
    stats: &mut EffectExecutionStats,
) -> RendererResult<()> {
    match pass.kind {
        RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture => {
            execute_capture(
                renderer,
                graph,
                textures,
                pass,
                framebuffer_origin,
                execution_damage,
                lifecycle_backdrop,
                debug_config,
                stats,
            )?;
            stats.scene_captures = stats.scene_captures.saturating_add(1);
        }
        RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample => {
            let first_downsample = pass.kind == RenderPassKind::DualKawaseDownsample
                && graph
                    .passes
                    .iter()
                    .find(|candidate| {
                        candidate.kind == RenderPassKind::DualKawaseDownsample
                            && candidate.instance == pass.instance
                    })
                    .is_some_and(|candidate| candidate.id == pass.id);
            let input_is_linear = pass
                .inputs
                .first()
                .and_then(|input| graph.textures.iter().find(|texture| texture.id == *input))
                .is_some_and(|texture| {
                    texture.working_space == oblivion_one::effects::EffectWorkingSpace::LinearSrgb
                });
            let fragment = match pass.kind {
                RenderPassKind::DualKawaseDownsample if first_downsample && !input_is_linear => {
                    blur::DUAL_KAWASE_DOWNSAMPLE_SHADER
                }
                RenderPassKind::DualKawaseDownsample => blur::DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER,
                RenderPassKind::DualKawaseUpsample => blur::DUAL_KAWASE_UPSAMPLE_SHADER,
                _ => unreachable!(),
            };
            execute_fullscreen_pass(
                renderer,
                graph,
                textures,
                pass,
                fragment,
                framebuffer_origin,
                true,
                execution_damage,
            )?;
            match pass.kind {
                RenderPassKind::DualKawaseDownsample => {
                    stats.blur_downsamples = stats.blur_downsamples.saturating_add(1)
                }
                RenderPassKind::DualKawaseUpsample => {
                    stats.blur_upsamples = stats.blur_upsamples.saturating_add(1)
                }
                _ => unreachable!(),
            }
        }
        RenderPassKind::NormalizeInput => {
            execute_fullscreen_pass(
                renderer,
                graph,
                textures,
                pass,
                NORMALIZE_FRAGMENT_SHADER,
                framebuffer_origin,
                false,
                execution_damage,
            )?;
        }
        RenderPassKind::Fragment | RenderPassKind::Blend | RenderPassKind::Mask => {
            let Some(stage) = pass.stage.as_ref() else {
                return Err(io::Error::other("effect stage has no lowering metadata").into());
            };
            let (module, fragment) = match stage {
                EffectNodeKind::Mask(_) => (
                    INTERNAL_EFFECT_SHADER_MODULE_MASK,
                    MASK_STAGE_FRAGMENT_SHADER,
                ),
                EffectNodeKind::Blend(_) => (
                    INTERNAL_EFFECT_SHADER_MODULE_BLEND,
                    BLEND_STAGE_FRAGMENT_SHADER,
                ),
                EffectNodeKind::CustomFragment(spec) => (spec.shader.get(), ""),
                EffectNodeKind::ColorMatrix(_)
                | EffectNodeKind::Tint(_)
                | EffectNodeKind::Noise(_) => (
                    INTERNAL_EFFECT_SHADER_MODULE_FRAGMENT,
                    FRAGMENT_STAGE_FRAGMENT_SHADER,
                ),
                EffectNodeKind::Source(_) | EffectNodeKind::DualKawaseBlur(_) => {
                    return Err(io::Error::other("invalid effect stage kind").into());
                }
            };
            execute_fullscreen_stage(
                renderer,
                graph,
                textures,
                pass,
                stage,
                fragment,
                module,
                framebuffer_origin,
                execution_damage,
            )?;
        }
        RenderPassKind::Composite | RenderPassKind::OutputPostProcess => {
            execute_fullscreen_pass(
                renderer,
                graph,
                textures,
                pass,
                COMPOSITE_FRAGMENT_SHADER,
                framebuffer_origin,
                false,
                execution_damage,
            )?;
            stats.composites = stats.composites.saturating_add(1);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn execute_fullscreen_stage(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    textures: &std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    pass: &CompiledRenderPass,
    stage: &EffectNodeKind,
    fragment_shader: &str,
    module: u64,
    framebuffer_origin: OutputFramebufferOrigin,
    execution_damage: &EffectRegion,
) -> RendererResult<()> {
    let input = pass
        .inputs
        .first()
        .copied()
        .ok_or_else(|| io::Error::other("effect stage has no input"))?;
    let input_texture = textures
        .get(&input)
        .and_then(|texture| renderer.effect_resources.texture(texture))
        .ok_or_else(|| io::Error::other("effect stage input is not realized"))?;
    let output = pass
        .output
        .ok_or_else(|| io::Error::other("effect stage has no output"))?;
    let input_plan = graph_texture(graph, input)?;
    let output_plan = graph_texture(graph, output)?;
    let output_texture = textures
        .get(&output)
        .ok_or_else(|| io::Error::other("effect stage output is not realized"))?;
    renderer
        .effect_resources
        .bind_render_target(&renderer.gl, output_texture)?;
    let module = ShaderModuleId::new(module)
        .ok_or_else(|| io::Error::other("effect stage shader id is zero"))?;
    let shader_key = ShaderProgramKey::new(
        module,
        0,
        oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
    );
    let program = renderer.effect_shaders.lookup(shader_key)?;
    let (vertex_array, _) = renderer.ensure_effect_quad()?;
    let target_flip_y = effect_target_requires_logical_y_flip(false, framebuffer_origin);
    let input_flip_y = effect_input_requires_sample_y_flip(input_plan.origin);
    establish_effect_pass_blend_state(&renderer.gl, EffectPassBlendMode::Replace);
    unsafe {
        renderer
            .gl
            .viewport(0, 0, output_plan.width as i32, output_plan.height as i32);
        renderer.gl.use_program(Some(program));
        if let Some(location) = uniform_location(
            &mut renderer.effect_shaders,
            &renderer.gl,
            shader_key,
            program,
            "u_effect_target_flip_y",
        ) {
            renderer
                .gl
                .uniform_1_i32(Some(&location), i32::from(target_flip_y));
        }
        if matches!(stage, EffectNodeKind::CustomFragment(_))
            && let Some(location) = uniform_location(
                &mut renderer.effect_shaders,
                &renderer.gl,
                shader_key,
                program,
                "u_typhon_input_flip_y",
            )
        {
            renderer
                .gl
                .uniform_1_i32(Some(&location), i32::from(input_flip_y));
        }
        if let Some(location) = uniform_location(
            &mut renderer.effect_shaders,
            &renderer.gl,
            shader_key,
            program,
            "u_effect_input_flip_y",
        ) {
            renderer
                .gl
                .uniform_1_i32(Some(&location), i32::from(input_flip_y));
        }
        renderer.gl.active_texture(glow::TEXTURE0);
        renderer
            .gl
            .bind_texture(glow::TEXTURE_2D, Some(input_texture));
        if let Some(location) = uniform_location(
            &mut renderer.effect_shaders,
            &renderer.gl,
            shader_key,
            program,
            "u_effect_input",
        ) {
            renderer.gl.uniform_1_i32(Some(&location), 0);
        }
        for (unit, input_id) in pass.inputs.iter().copied().enumerate().skip(1) {
            let texture = textures
                .get(&input_id)
                .and_then(|texture| renderer.effect_resources.texture(texture))
                .ok_or_else(|| io::Error::other("secondary effect input is not realized"))?;
            let unit =
                u32::try_from(unit).map_err(|_| io::Error::other("too many effect inputs"))?;
            renderer.gl.active_texture(glow::TEXTURE0 + unit);
            renderer.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            let name = if unit == 1 {
                "u_effect_input_secondary"
            } else {
                "u_effect_input_aux"
            };
            if let Some(location) = uniform_location(
                &mut renderer.effect_shaders,
                &renderer.gl,
                shader_key,
                program,
                name,
            ) {
                renderer.gl.uniform_1_i32(Some(&location), unit as i32);
            }
            if matches!(stage, EffectNodeKind::CustomFragment(_)) {
                let aux_index = unit.saturating_sub(1);
                let aux_name = format!("u_typhon_aux{aux_index}");
                if let Some(location) = uniform_location(
                    &mut renderer.effect_shaders,
                    &renderer.gl,
                    shader_key,
                    program,
                    &aux_name,
                ) {
                    renderer.gl.uniform_1_i32(Some(&location), unit as i32);
                }
            }
        }
        if matches!(stage, EffectNodeKind::CustomFragment(_))
            && let Some(location) = uniform_location(
                &mut renderer.effect_shaders,
                &renderer.gl,
                shader_key,
                program,
                "u_typhon_aux_count",
            )
        {
            renderer.gl.uniform_1_i32(
                Some(&location),
                pass.inputs.len().saturating_sub(1).min(8) as i32,
            );
        }
        if let Some(location) = uniform_location(
            &mut renderer.effect_shaders,
            &renderer.gl,
            shader_key,
            program,
            "u_effect_decode_srgb",
        ) {
            renderer.gl.uniform_1_i32(
                Some(&location),
                i32::from(pass.color_conversion == EffectColorConversion::DecodeSrgbToLinear),
            );
        }
        if let Some(location) = uniform_location(
            &mut renderer.effect_shaders,
            &renderer.gl,
            shader_key,
            program,
            "u_effect_encode_srgb",
        ) {
            renderer.gl.uniform_1_i32(
                Some(&location),
                i32::from(pass.color_conversion == EffectColorConversion::EncodeLinearToSrgb),
            );
        }
        set_stage_uniforms(
            &mut renderer.effect_shaders,
            &renderer.gl,
            shader_key,
            program,
            stage,
            StageUniformContext {
                parameters: &pass.parameter_block,
                input_plan,
                output_size: trusted_effect_output_size(
                    renderer.current_size,
                    (output_plan.width, output_plan.height),
                ),
                output_scale: renderer.effect_output_scale,
                effect_time_seconds: renderer.effect_time_seconds,
                effect_delta_seconds: renderer.effect_delta_seconds,
                color_conversion: pass.color_conversion,
                fused_stages: &pass.fused_stages,
            },
        );
        renderer.gl.bind_vertex_array(Some(vertex_array));
        draw_damage_scissors(
            &renderer.gl,
            execution_damage,
            output_plan,
            framebuffer_origin,
        );
        renderer.gl.bind_vertex_array(None);
        for unit in 0..=pass.inputs.len() {
            let unit = u32::try_from(unit).unwrap_or(u32::MAX);
            if unit == u32::MAX {
                break;
            }
            renderer.gl.active_texture(glow::TEXTURE0 + unit);
            renderer.gl.bind_texture(glow::TEXTURE_2D, None);
        }
        renderer.gl.disable(glow::SCISSOR_TEST);
        renderer.gl.disable(glow::BLEND);
    }
    renderer.effect_resources.unbind_render_target(&renderer.gl);
    renderer.bind_active_output_framebuffer();
    restore_output_viewport(renderer);
    let _ = fragment_shader;
    Ok(())
}

fn uniform_location(
    shaders: &mut ShaderProgramCache,
    gl: &glow::Context,
    key: ShaderProgramKey,
    program: glow::NativeProgram,
    name: &str,
) -> Option<glow::UniformLocation> {
    shaders.uniform_location(gl, key, program, name)
}

struct StageUniformContext<'a> {
    parameters: &'a oblivion_one::effects::EffectParameterBlock,
    input_plan: &'a oblivion_one::effects::GraphTexturePlan,
    output_size: (f32, f32),
    output_scale: f32,
    effect_time_seconds: f32,
    effect_delta_seconds: f32,
    color_conversion: EffectColorConversion,
    fused_stages: &'a [EffectNodeKind],
}

fn set_stage_uniforms(
    shaders: &mut ShaderProgramCache,
    gl: &glow::Context,
    shader_key: ShaderProgramKey,
    program: glow::NativeProgram,
    stage: &EffectNodeKind,
    context: StageUniformContext<'_>,
) {
    let StageUniformContext {
        parameters,
        input_plan,
        output_size,
        output_scale,
        effect_time_seconds,
        effect_delta_seconds,
        color_conversion,
        fused_stages,
    } = context;
    unsafe {
        if matches!(
            stage,
            EffectNodeKind::ColorMatrix(_) | EffectNodeKind::Tint(_) | EffectNodeKind::Noise(_)
        ) {
            set_identity_color_matrix(shaders, gl, shader_key, program);
            if let Some(location) =
                uniform_location(shaders, gl, shader_key, program, "u_effect_tint_color")
            {
                gl.uniform_4_f32(Some(&location), 1.0, 1.0, 1.0, 1.0);
            }
            if let Some(location) =
                uniform_location(shaders, gl, shader_key, program, "u_effect_tint_amount")
            {
                gl.uniform_1_f32(Some(&location), 0.0);
            }
            if let Some(location) =
                uniform_location(shaders, gl, shader_key, program, "u_effect_noise_amount")
            {
                gl.uniform_1_f32(Some(&location), 0.0);
            }
            for stage in std::iter::once(stage).chain(fused_stages.iter()) {
                match stage {
                    EffectNodeKind::ColorMatrix(spec) => {
                        if let Some(location) = uniform_location(
                            shaders,
                            gl,
                            shader_key,
                            program,
                            "u_effect_color_matrix",
                        ) {
                            gl.uniform_matrix_4_f32_slice(Some(&location), false, &spec.matrix);
                        }
                        if let Some(location) = uniform_location(
                            shaders,
                            gl,
                            shader_key,
                            program,
                            "u_effect_color_bias",
                        ) {
                            gl.uniform_4_f32(
                                Some(&location),
                                spec.bias[0],
                                spec.bias[1],
                                spec.bias[2],
                                spec.bias[3],
                            );
                        }
                    }
                    EffectNodeKind::Tint(spec) => {
                        if let Some(location) = uniform_location(
                            shaders,
                            gl,
                            shader_key,
                            program,
                            "u_effect_tint_color",
                        ) {
                            gl.uniform_4_f32(
                                Some(&location),
                                spec.color[0],
                                spec.color[1],
                                spec.color[2],
                                spec.color[3],
                            );
                        }
                        if let Some(location) = uniform_location(
                            shaders,
                            gl,
                            shader_key,
                            program,
                            "u_effect_tint_amount",
                        ) {
                            gl.uniform_1_f32(Some(&location), spec.amount);
                        }
                    }
                    EffectNodeKind::Noise(spec) => {
                        if let Some(location) = uniform_location(
                            shaders,
                            gl,
                            shader_key,
                            program,
                            "u_effect_noise_amount",
                        ) {
                            gl.uniform_1_f32(Some(&location), spec.amount);
                        }
                    }
                    _ => {}
                }
            }
            return;
        }
        match stage {
            EffectNodeKind::ColorMatrix(spec) => {
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_effect_color_matrix")
                {
                    gl.uniform_matrix_4_f32_slice(Some(&location), false, &spec.matrix);
                }
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_effect_color_bias")
                {
                    gl.uniform_4_f32(
                        Some(&location),
                        spec.bias[0],
                        spec.bias[1],
                        spec.bias[2],
                        spec.bias[3],
                    );
                }
            }
            EffectNodeKind::Tint(spec) => {
                set_identity_color_matrix(shaders, gl, shader_key, program);
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_effect_tint_color")
                {
                    gl.uniform_4_f32(
                        Some(&location),
                        spec.color[0],
                        spec.color[1],
                        spec.color[2],
                        spec.color[3],
                    );
                }
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_effect_tint_amount")
                {
                    gl.uniform_1_f32(Some(&location), spec.amount);
                }
            }
            EffectNodeKind::Noise(spec) => {
                set_identity_color_matrix(shaders, gl, shader_key, program);
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_effect_noise_amount")
                {
                    gl.uniform_1_f32(Some(&location), spec.amount);
                }
            }
            EffectNodeKind::Mask(spec) => {
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_effect_inverted")
                {
                    gl.uniform_1_i32(
                        Some(&location),
                        i32::from(matches!(
                            spec.mode,
                            oblivion_one::effects::MaskMode::InvertedAlpha
                        )),
                    );
                }
            }
            EffectNodeKind::Blend(spec) => {
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_effect_blend_mode")
                {
                    gl.uniform_1_i32(
                        Some(&location),
                        match spec.mode {
                            oblivion_one::effects::BlendMode::SourceOver => 0,
                            oblivion_one::effects::BlendMode::Add => 1,
                            oblivion_one::effects::BlendMode::Multiply => 2,
                            oblivion_one::effects::BlendMode::Screen => 3,
                        },
                    );
                }
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_effect_blend_opacity")
                {
                    gl.uniform_1_f32(Some(&location), spec.opacity);
                }
            }
            EffectNodeKind::CustomFragment(spec) => {
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_typhon_decode_srgb")
                {
                    gl.uniform_1_i32(
                        Some(&location),
                        i32::from(color_conversion == EffectColorConversion::DecodeSrgbToLinear),
                    );
                }
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_typhon_encode_srgb")
                {
                    gl.uniform_1_i32(
                        Some(&location),
                        i32::from(color_conversion == EffectColorConversion::EncodeLinearToSrgb),
                    );
                }
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_typhon_primary")
                {
                    gl.uniform_1_i32(Some(&location), 0);
                }
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_typhon_texture_size")
                {
                    gl.uniform_2_f32(
                        Some(&location),
                        input_plan.width.max(1) as f32,
                        input_plan.height.max(1) as f32,
                    );
                }
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_typhon_content_rect")
                {
                    gl.uniform_4_f32(
                        Some(&location),
                        input_plan.domain.x as f32,
                        input_plan.domain.y as f32,
                        input_plan.domain.width.max(1) as f32,
                        input_plan.domain.height.max(1) as f32,
                    );
                }
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_typhon_output_size")
                {
                    gl.uniform_2_f32(Some(&location), output_size.0, output_size.1);
                }
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_typhon_scale")
                {
                    gl.uniform_1_f32(Some(&location), output_scale);
                }
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_typhon_time")
                {
                    gl.uniform_1_f32(Some(&location), effect_time_seconds);
                }
                if let Some(location) =
                    uniform_location(shaders, gl, shader_key, program, "u_typhon_delta")
                {
                    gl.uniform_1_f32(Some(&location), effect_delta_seconds);
                }
                for binding in &spec.uniforms {
                    let Some(value) = parameters
                        .values()
                        .iter()
                        .find(|value| value.id == binding.parameter)
                        .map(|value| value.value)
                    else {
                        continue;
                    };
                    let Some(location) =
                        uniform_location(shaders, gl, shader_key, program, &binding.shader_name)
                    else {
                        continue;
                    };
                    match value {
                        oblivion_one::effects::EffectUniformValue::Float(value) => {
                            gl.uniform_1_f32(Some(&location), value);
                        }
                        oblivion_one::effects::EffectUniformValue::Vec2(value) => {
                            gl.uniform_2_f32(Some(&location), value[0], value[1]);
                        }
                        oblivion_one::effects::EffectUniformValue::Vec3(value) => {
                            gl.uniform_3_f32(Some(&location), value[0], value[1], value[2]);
                        }
                        oblivion_one::effects::EffectUniformValue::Vec4(value) => {
                            gl.uniform_4_f32(
                                Some(&location),
                                value[0],
                                value[1],
                                value[2],
                                value[3],
                            );
                        }
                        oblivion_one::effects::EffectUniformValue::Int(value) => {
                            gl.uniform_1_i32(Some(&location), value);
                        }
                    }
                }
            }
            EffectNodeKind::Source(_) | EffectNodeKind::DualKawaseBlur(_) => {}
        }
    }
}

fn set_identity_color_matrix(
    shaders: &mut ShaderProgramCache,
    gl: &glow::Context,
    shader_key: ShaderProgramKey,
    program: glow::NativeProgram,
) {
    unsafe {
        if let Some(location) =
            uniform_location(shaders, gl, shader_key, program, "u_effect_color_matrix")
        {
            gl.uniform_matrix_4_f32_slice(
                Some(&location),
                false,
                &[
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                ],
            );
        }
        if let Some(location) =
            uniform_location(shaders, gl, shader_key, program, "u_effect_color_bias")
        {
            gl.uniform_4_f32(Some(&location), 0.0, 0.0, 0.0, 0.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_capture(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    textures: &std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    pass: &CompiledRenderPass,
    framebuffer_origin: OutputFramebufferOrigin,
    execution_damage: &EffectRegion,
    lifecycle_backdrop: bool,
    debug_config: EffectDebugConfig,
    stats: &mut EffectExecutionStats,
) -> RendererResult<()> {
    let output = pass
        .output
        .ok_or_else(|| io::Error::other("capture pass has no output texture"))?;
    let target = textures
        .get(&output)
        .ok_or_else(|| io::Error::other("capture output texture is not allocated"))?;
    let target_plan = graph_texture(graph, output)?;
    let direct_capture = is_direct_framebuffer_capture(pass, lifecycle_backdrop, debug_config);
    let materialization = if direct_capture {
        None
    } else {
        Some(capture_materialization_plan(
            execution_damage,
            Some(target_plan.domain),
            renderer.current_size,
        ))
    };
    let capture_rects = if direct_capture {
        vec![full_output_rect((target_plan.width, target_plan.height))]
    } else {
        materialization
            .as_ref()
            .expect("replay capture has a materialization plan")
            .output_rects
            .clone()
    };
    let capture_texture_rects = materialization.as_ref().map_or_else(
        || capture_rects.clone(),
        |plan| plan.texture_rects(target_plan),
    );
    stats.capture_execution_pixels = stats
        .capture_execution_pixels
        .saturating_add(output_rect_pixels(&capture_rects));
    if let Some(materialization) = materialization.as_ref()
        && renderer.effect_trace.enabled()
    {
        renderer.effect_trace.capture_materialization(
            pass,
            materialization.region.rects().len(),
            materialization
                .region
                .bounding_rect()
                .map(|rect| (rect.x, rect.y, rect.width, rect.height)),
            materialization.output_rects.len(),
            output_rect_pixels(&capture_rects),
        );
    }
    if direct_capture {
        capture_output_region_to_graph_texture(renderer, target, target_plan, framebuffer_origin)?;
        return Ok(());
    }
    renderer
        .effect_resources
        .bind_render_target(&renderer.gl, target)?;
    unsafe {
        renderer
            .gl
            .viewport(0, 0, target_plan.width as i32, target_plan.height as i32);
        renderer.gl.disable(glow::SCISSOR_TEST);
        renderer.gl.disable(glow::BLEND);
        renderer.gl.clear_color(0.0, 0.0, 0.0, 0.0);
        for rect in &capture_texture_rects {
            renderer.gl.enable(glow::SCISSOR_TEST);
            renderer
                .gl
                .scissor(rect.x, rect.y, rect.width as i32, rect.height as i32);
            renderer.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        renderer.gl.disable(glow::SCISSOR_TEST);
        establish_effect_pass_blend_state(&renderer.gl, capture_blend_mode());
        renderer.gl.use_program(Some(renderer.capture_program));
        if let Some(location) = renderer.capture_uniform_location("u_capture_output_size") {
            renderer.gl.uniform_2_f32(
                Some(&location),
                renderer.current_size.0.max(1) as f32,
                renderer.current_size.1.max(1) as f32,
            );
        }
        if let Some(location) = renderer.capture_uniform_location("u_capture_domain") {
            renderer.gl.uniform_4_f32(
                Some(&location),
                target_plan.domain.x as f32,
                target_plan.domain.y as f32,
                target_plan.domain.width.max(1) as f32,
                target_plan.domain.height.max(1) as f32,
            );
        }
        if let Some(location) = renderer.capture_uniform_location("u_capture_origin_bottom_left") {
            renderer.gl.uniform_1_i32(
                Some(&location),
                i32::from(matches!(
                    framebuffer_origin,
                    OutputFramebufferOrigin::BottomLeft
                )),
            );
        }
    }
    let layers = renderer
        .commands
        .iter()
        .map(|command| match command.layer {
            EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
            _ => capture::CaptureLayer::Other,
        })
        .collect::<Vec<_>>();
    let visual_groups = renderer
        .commands
        .iter()
        .map(|command| command.visual_group)
        .collect::<Vec<_>>();
    let indices = capture::indices_for_capture(
        &layers,
        &visual_groups,
        pass.anchor,
        pass.kind == RenderPassKind::SurfaceCapture,
        pass.visual_group,
        pass.anchor_scope,
    );
    let scissors = materialization
        .as_ref()
        .expect("replay capture has a materialization plan")
        .output_rects
        .clone();
    renderer.draw_capture_commands_for_regions(
        &indices,
        &scissors,
        target_plan.domain,
        (target_plan.width, target_plan.height),
    )?;
    renderer.effect_resources.unbind_render_target(&renderer.gl);
    renderer.bind_active_output_framebuffer();
    restore_output_viewport(renderer);
    establish_effect_pass_blend_state(&renderer.gl, EffectPassBlendMode::Replace);
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GlBlitRect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl GlBlitRect {
    const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GraphTextureCaptureBlit {
    source: GlBlitRect,
    destination: GlBlitRect,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SceneWorkPreservationPlan {
    transfers: Vec<GraphTextureCaptureBlit>,
    pixels: u64,
}

impl GraphTextureCaptureBlit {
    fn pixels(self) -> u64 {
        let width =
            (i64::from(self.destination.x1) - i64::from(self.destination.x0)).unsigned_abs();
        let height =
            (i64::from(self.destination.y1) - i64::from(self.destination.y0)).unsigned_abs();
        width.saturating_mul(height)
    }
}

impl SceneWorkPreservationPlan {
    fn from_extra_scene_work(
        extra_scene_work: &[OutputRect],
        output_size: (u32, u32),
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Self {
        let transfers = extra_scene_work
            .iter()
            .filter_map(|rect| {
                scene_work_preservation_blit_rects(*rect, output_size, framebuffer_origin)
            })
            .collect::<Vec<_>>();
        let pixels = transfers.iter().copied().fold(0u64, |total, transfer| {
            total.saturating_add(transfer.pixels())
        });
        Self { transfers, pixels }
    }
}

fn scene_work_preservation_blit_rects(
    rect: OutputRect,
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<GraphTextureCaptureBlit> {
    let left = i64::from(rect.x).clamp(0, i64::from(output_size.0));
    let top = i64::from(rect.y).clamp(0, i64::from(output_size.1));
    let right = (i64::from(rect.x) + i64::from(rect.width)).clamp(0, i64::from(output_size.0));
    let bottom = (i64::from(rect.y) + i64::from(rect.height)).clamp(0, i64::from(output_size.1));
    if right <= left || bottom <= top {
        return None;
    }
    let left = i32::try_from(left).ok()?;
    let top = i32::try_from(top).ok()?;
    let right = i32::try_from(right).ok()?;
    let bottom = i32::try_from(bottom).ok()?;
    let height = i32::try_from(output_size.1).ok()?;
    let source = GlBlitRect::new(left, height - bottom, right, height - top);
    let destination = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => source,
        OutputFramebufferOrigin::TopLeftScanout => GlBlitRect::new(left, bottom, right, top),
    };
    Some(GraphTextureCaptureBlit {
        source,
        destination,
    })
}

struct SceneWorkPreservation {
    texture: PooledEffectTexture,
    plan: SceneWorkPreservationPlan,
}

fn capture_scene_work_preservation(
    renderer: &mut GlesSceneRenderer,
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) -> RendererResult<SceneWorkPreservation> {
    let key = EffectTextureKey::new(
        output_size.0,
        output_size.1,
        EffectTextureFormat::Rgba8,
        EffectTextureFilter::Nearest,
        oblivion_one::effects::EffectWorkingSpace::OutputEncodedSrgb,
    );
    let texture = renderer.effect_resources.acquire(&renderer.gl, key)?;
    let plan = SceneWorkPreservationPlan::from_extra_scene_work(
        &[full_output_rect(output_size)],
        output_size,
        framebuffer_origin,
    );
    let result = (|| {
        let transfer = scene_work_preservation_blit_rects(
            full_output_rect(output_size),
            output_size,
            framebuffer_origin,
        )
        .ok_or_else(|| io::Error::other("scene-work preservation output is empty"))?;
        let output_framebuffer = renderer.active_output_framebuffer;
        renderer.bind_active_output_framebuffer();
        let draw_framebuffer = renderer
            .effect_resources
            .bind_draw_target(&renderer.gl, &texture)?;
        unsafe {
            renderer.gl.disable(glow::SCISSOR_TEST);
            renderer
                .gl
                .bind_framebuffer(glow::READ_FRAMEBUFFER, output_framebuffer);
            renderer
                .gl
                .bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(draw_framebuffer));
            renderer.gl.blit_framebuffer(
                transfer.source.x0,
                transfer.source.y0,
                transfer.source.x1,
                transfer.source.y1,
                transfer.destination.x0,
                transfer.destination.y0,
                transfer.destination.x1,
                transfer.destination.y1,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    renderer.establish_ordinary_scene_state();
    match result {
        Ok(()) => Ok(SceneWorkPreservation { texture, plan }),
        Err(error) => {
            let _ = renderer.effect_resources.release(texture);
            Err(error)
        }
    }
}

fn restore_scene_work_preservation(
    renderer: &mut GlesSceneRenderer,
    preservation: &SceneWorkPreservation,
    extra_scene_work: &[OutputRect],
    framebuffer_origin: OutputFramebufferOrigin,
) -> RendererResult<()> {
    let output_framebuffer = renderer.active_output_framebuffer;
    let read_framebuffer = renderer
        .effect_resources
        .bind_read_target(&renderer.gl, &preservation.texture)?;
    unsafe {
        renderer.gl.disable(glow::SCISSOR_TEST);
        renderer
            .gl
            .bind_framebuffer(glow::READ_FRAMEBUFFER, Some(read_framebuffer));
        renderer
            .gl
            .bind_framebuffer(glow::DRAW_FRAMEBUFFER, output_framebuffer);
        for rect in extra_scene_work {
            let Some(transfer) = scene_work_preservation_blit_rects(
                *rect,
                renderer.current_size,
                framebuffer_origin,
            ) else {
                continue;
            };
            renderer.gl.blit_framebuffer(
                transfer.source.x0,
                transfer.source.y0,
                transfer.source.x1,
                transfer.source.y1,
                transfer.destination.x0,
                transfer.destination.y0,
                transfer.destination.x1,
                transfer.destination.y1,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        }
    }
    renderer.establish_ordinary_scene_state();
    Ok(())
}

/// Effect coordinate contract:
///
/// * logical domains are top-left, integer output-space rectangles;
/// * graph textures use bottom-left physical storage;
/// * the framebuffer origin describes the active output image only;
/// * fullscreen vertex UVs describe the logical destination and are flipped
///   only when drawing directly to a top-left scanout framebuffer;
/// * sampled graph inputs are converted independently using their storage
///   origin; and
/// * translating a logical domain must never change either orientation flag.
///
/// Scene replay uses `u_capture_domain` plus
/// `u_capture_origin_bottom_left`. Direct framebuffer capture uses the
/// explicit READ/DRAW blit mapping. Kawase, normalization, and final
/// composite passes use the same independent target/input rules here.
fn effect_target_requires_logical_y_flip(
    output_is_framebuffer: bool,
    framebuffer_origin: OutputFramebufferOrigin,
) -> bool {
    output_is_framebuffer && framebuffer_origin == OutputFramebufferOrigin::TopLeftScanout
}

/// Returns whether logical UVs must be converted to physical sampling UVs for
/// the graph texture's canonical storage origin.
fn effect_input_requires_sample_y_flip(
    input_origin: oblivion_one::effects::GraphTextureOrigin,
) -> bool {
    matches!(
        input_origin,
        oblivion_one::effects::GraphTextureOrigin::BottomLeft
    )
}

fn plan_graph_texture_capture(
    output_size: (u32, u32),
    domain: oblivion_one::effects::EffectRect,
    target_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<GraphTextureCaptureBlit> {
    if target_size.0 == 0 || target_size.1 == 0 || domain.x < 0 || domain.y < 0 {
        return None;
    }
    let domain_right = i64::from(domain.x).checked_add(i64::from(domain.width))?;
    let domain_bottom = i64::from(domain.y).checked_add(i64::from(domain.height))?;
    if domain_right > i64::from(output_size.0) || domain_bottom > i64::from(output_size.1) {
        return None;
    }
    let source_y = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => {
            i64::from(output_size.1).checked_sub(domain_bottom)?
        }
        OutputFramebufferOrigin::TopLeftScanout => i64::from(domain.y),
    };
    let source = GlBlitRect {
        x0: domain.x,
        y0: i32::try_from(source_y).ok()?,
        x1: i32::try_from(domain_right).ok()?,
        y1: i32::try_from(source_y.checked_add(i64::from(domain.height))?).ok()?,
    };
    let target_width = i32::try_from(target_size.0).ok()?;
    let target_height = i32::try_from(target_size.1).ok()?;
    let destination = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => GlBlitRect {
            x0: 0,
            y0: 0,
            x1: target_width,
            y1: target_height,
        },
        OutputFramebufferOrigin::TopLeftScanout => GlBlitRect {
            x0: 0,
            y0: target_height,
            x1: target_width,
            y1: 0,
        },
    };
    Some(GraphTextureCaptureBlit {
        source,
        destination,
    })
}

/// Capture a logical output domain into a graph texture with the canonical
/// `GraphTextureOrigin::BottomLeft` orientation.
fn capture_output_region_to_graph_texture(
    renderer: &mut GlesSceneRenderer,
    target: &PooledEffectTexture,
    target_plan: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
) -> RendererResult<()> {
    if target_plan.origin != oblivion_one::effects::GraphTextureOrigin::BottomLeft {
        return Err(io::Error::other("direct capture target is not bottom-left oriented").into());
    }
    let transfer = plan_graph_texture_capture(
        renderer.current_size,
        target_plan.domain,
        (target_plan.width, target_plan.height),
        framebuffer_origin,
    )
    .ok_or_else(|| io::Error::other("direct capture domain is outside the output"))?;
    let result = (|| {
        renderer.bind_active_output_framebuffer();
        let draw_framebuffer = renderer
            .effect_resources
            .bind_draw_target(&renderer.gl, target)?;
        if renderer.active_output_framebuffer == Some(draw_framebuffer)
            || target_plan.source == GraphTextureSource::Output
        {
            return Err(
                Box::new(EffectExecutionInvariantError::InvalidFramebufferBlitTargets)
                    as Box<dyn std::error::Error>,
            );
        }
        unsafe {
            renderer.gl.disable(glow::SCISSOR_TEST);
            renderer
                .gl
                .bind_framebuffer(glow::READ_FRAMEBUFFER, renderer.active_output_framebuffer);
            renderer
                .gl
                .bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(draw_framebuffer));
            renderer.gl.blit_framebuffer(
                transfer.source.x0,
                transfer.source.y0,
                transfer.source.x1,
                transfer.source.y1,
                transfer.destination.x0,
                transfer.destination.y0,
                transfer.destination.x1,
                transfer.destination.y1,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        }
        Ok(())
    })();
    renderer.establish_ordinary_scene_state();
    result
}

#[allow(clippy::too_many_arguments)]
fn execute_fullscreen_pass(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    textures: &std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    pass: &CompiledRenderPass,
    fragment_shader: &str,
    framebuffer_origin: OutputFramebufferOrigin,
    blur_shader: bool,
    execution_damage: &EffectRegion,
) -> RendererResult<()> {
    let input = pass
        .inputs
        .first()
        .copied()
        .ok_or_else(|| io::Error::other("effect pass has no input texture"))?;
    let input_texture = textures
        .get(&input)
        .and_then(|texture| renderer.effect_resources.texture(texture))
        .ok_or_else(|| io::Error::other("effect input texture is not realized"))?;
    let output = pass
        .output
        .ok_or_else(|| io::Error::other("effect pass has no output target"))?;
    let input_plan = graph_texture(graph, input)?;
    let output_plan = graph_texture(graph, output)?;
    let output_is_framebuffer = output_plan.source == GraphTextureSource::Output;
    let output_texture = if output_is_framebuffer {
        None
    } else {
        Some(
            textures
                .get(&output)
                .ok_or_else(|| io::Error::other("effect output texture is not allocated"))?,
        )
    };
    if let Some(texture) = output_texture {
        renderer
            .effect_resources
            .bind_render_target(&renderer.gl, texture)?;
    }
    let blend_mode = effect_pass_blend_mode(pass.kind, output_is_framebuffer, pass.alpha_mode);
    establish_effect_pass_blend_state(&renderer.gl, blend_mode);
    unsafe {
        renderer
            .gl
            .viewport(0, 0, output_plan.width as i32, output_plan.height as i32);
    }
    let module = if blur_shader {
        match pass.kind {
            RenderPassKind::DualKawaseDownsample => {
                oblivion_one::effects::INTERNAL_EFFECT_SHADER_MODULE_DOWNSAMPLE
            }
            RenderPassKind::DualKawaseUpsample => {
                oblivion_one::effects::INTERNAL_EFFECT_SHADER_MODULE_UPSAMPLE
            }
            _ => 1000,
        }
    } else if output_is_framebuffer {
        oblivion_one::effects::INTERNAL_EFFECT_SHADER_MODULE_COMPOSITE
    } else {
        oblivion_one::effects::INTERNAL_EFFECT_SHADER_MODULE_COPY
    };
    let shader_key = ShaderProgramKey::new(
        ShaderModuleId::new(module).expect("static effect shader ids are non-zero"),
        if pass.kind == RenderPassKind::NormalizeInput
            || (pass.kind == RenderPassKind::DualKawaseDownsample
                && fragment_shader == blur::DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER)
        {
            1
        } else {
            0
        },
        oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
    );
    let program = renderer.effect_shaders.lookup(shader_key)?;
    let (vertex_array, _) = renderer.ensure_effect_quad()?;
    let target_flip_y =
        effect_target_requires_logical_y_flip(output_is_framebuffer, framebuffer_origin);
    let input_flip_y = effect_input_requires_sample_y_flip(input_plan.origin);
    unsafe {
        renderer.gl.use_program(Some(program));
        if let Some(location) = uniform_location(
            &mut renderer.effect_shaders,
            &renderer.gl,
            shader_key,
            program,
            "u_effect_target_flip_y",
        ) {
            renderer
                .gl
                .uniform_1_i32(Some(&location), i32::from(target_flip_y));
        }
        if let Some(location) = uniform_location(
            &mut renderer.effect_shaders,
            &renderer.gl,
            shader_key,
            program,
            "u_effect_input_flip_y",
        ) {
            renderer
                .gl
                .uniform_1_i32(Some(&location), i32::from(input_flip_y));
        }
        renderer.gl.active_texture(glow::TEXTURE0);
        renderer
            .gl
            .bind_texture(glow::TEXTURE_2D, Some(input_texture));
        if let Some(location) = uniform_location(
            &mut renderer.effect_shaders,
            &renderer.gl,
            shader_key,
            program,
            "u_effect_input",
        ) {
            renderer.gl.uniform_1_i32(Some(&location), 0);
        }
        if blur_shader
            && let Some(location) = uniform_location(
                &mut renderer.effect_shaders,
                &renderer.gl,
                shader_key,
                program,
                "u_effect_texel_size",
            )
        {
            renderer.gl.uniform_2_f32(
                Some(&location),
                1.0 / input_plan.width.max(1) as f32,
                1.0 / input_plan.height.max(1) as f32,
            );
        }
        if blur_shader
            && let Some(location) = uniform_location(
                &mut renderer.effect_shaders,
                &renderer.gl,
                shader_key,
                program,
                "u_effect_blur_radius",
            )
        {
            renderer
                .gl
                .uniform_1_f32(Some(&location), pass.blur_radius.unwrap_or(1.0));
        }
        if !blur_shader
            && let Some(location) = uniform_location(
                &mut renderer.effect_shaders,
                &renderer.gl,
                shader_key,
                program,
                "u_effect_input_domain",
            )
        {
            renderer.gl.uniform_4_f32(
                Some(&location),
                input_plan.domain.x as f32,
                input_plan.domain.y as f32,
                input_plan.domain.width.max(1) as f32,
                input_plan.domain.height.max(1) as f32,
            );
        }
        if !blur_shader
            && let Some(location) = uniform_location(
                &mut renderer.effect_shaders,
                &renderer.gl,
                shader_key,
                program,
                "u_effect_decode_srgb",
            )
        {
            renderer.gl.uniform_1_i32(
                Some(&location),
                i32::from(pass.color_conversion == EffectColorConversion::DecodeSrgbToLinear),
            );
        }
        if !blur_shader
            && let Some(location) = uniform_location(
                &mut renderer.effect_shaders,
                &renderer.gl,
                shader_key,
                program,
                "u_effect_encode_srgb",
            )
        {
            renderer.gl.uniform_1_i32(
                Some(&location),
                i32::from(
                    pass.encode_output
                        || pass.color_conversion == EffectColorConversion::EncodeLinearToSrgb,
                ),
            );
        }
        if output_is_framebuffer {
            if let Some(location) = uniform_location(
                &mut renderer.effect_shaders,
                &renderer.gl,
                shader_key,
                program,
                "u_effect_encode_srgb",
            ) {
                renderer
                    .gl
                    .uniform_1_i32(Some(&location), i32::from(pass.encode_output));
            }
            if let Some(location) = uniform_location(
                &mut renderer.effect_shaders,
                &renderer.gl,
                shader_key,
                program,
                "u_effect_force_opaque",
            ) {
                renderer.gl.uniform_1_i32(
                    Some(&location),
                    i32::from(pass.alpha_mode == oblivion_one::effects::EffectAlphaMode::Opaque),
                );
            }
            if pass.alpha_mode == oblivion_one::effects::EffectAlphaMode::Preserve {
                renderer.gl.enable(glow::BLEND);
                renderer.gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            }
        }
        if !blur_shader {
            if pass.kind == RenderPassKind::NormalizeInput {
                if let Some(location) = uniform_location(
                    &mut renderer.effect_shaders,
                    &renderer.gl,
                    shader_key,
                    program,
                    "u_effect_output_domain",
                ) {
                    renderer.gl.uniform_4_f32(
                        Some(&location),
                        output_plan.domain.x as f32,
                        output_plan.domain.y as f32,
                        output_plan.domain.width.max(1) as f32,
                        output_plan.domain.height.max(1) as f32,
                    );
                }
            } else if let Some(location) = uniform_location(
                &mut renderer.effect_shaders,
                &renderer.gl,
                shader_key,
                program,
                "u_effect_output_size",
            ) {
                renderer.gl.uniform_2_f32(
                    Some(&location),
                    output_plan.domain.width.max(1) as f32,
                    output_plan.domain.height.max(1) as f32,
                );
            }
        }
        renderer.gl.bind_vertex_array(Some(vertex_array));
        draw_damage_scissors(
            &renderer.gl,
            execution_damage,
            output_plan,
            framebuffer_origin,
        );
        renderer.gl.bind_vertex_array(None);
        renderer.gl.bind_texture(glow::TEXTURE_2D, None);
        renderer.gl.disable(glow::SCISSOR_TEST);
        renderer.gl.disable(glow::BLEND);
    }
    if output_texture.is_some() {
        renderer.effect_resources.unbind_render_target(&renderer.gl);
        renderer.bind_active_output_framebuffer();
        restore_output_viewport(renderer);
    }
    Ok(())
}

fn graph_texture(
    graph: &CompiledFrameGraph,
    id: GraphTextureId,
) -> RendererResult<&oblivion_one::effects::GraphTexturePlan> {
    graph
        .textures
        .iter()
        .find(|texture| texture.id == id)
        .ok_or_else(|| io::Error::other("effect graph references an unknown texture").into())
}

fn full_output_rect(size: (u32, u32)) -> OutputRect {
    OutputRect::new(0, 0, size.0, size.1)
}

#[derive(Debug, Default, PartialEq, Eq)]
struct SceneWorkRegions {
    scene_work_rects: Vec<OutputRect>,
    extra_scene_work: Vec<OutputRect>,
}

fn scene_work_regions(
    repaint_rects: &[OutputRect],
    graph: &CompiledFrameGraph,
    selection: &EffectExecutionSelection,
    output_size: (u32, u32),
    lifecycle_backdrop: bool,
    debug_config: EffectDebugConfig,
) -> SceneWorkRegions {
    let presentation_work = repaint_rects.to_vec();
    let mut checkpoint_work = Vec::new();
    for pass in &graph.passes {
        if !selection.executed_passes.contains(&pass.id)
            || !is_direct_framebuffer_capture(pass, lifecycle_backdrop, debug_config)
            || !matches!(
                pass.kind,
                RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
            )
        {
            continue;
        }
        let Some(output) = pass.output else {
            continue;
        };
        let Some(texture) = graph.textures.iter().find(|texture| texture.id == output) else {
            continue;
        };
        let Some(rect) = clipped_output_rect(texture.domain, output_size) else {
            continue;
        };
        checkpoint_work.push(rect);
    }

    let mut rects = presentation_work.clone();
    rects.extend(checkpoint_work.iter().copied());
    let coalesced = OutputDamage::rects(output_size.0, output_size.1, rects);
    let scene_work_rects = match coalesced {
        OutputDamage::Empty => Vec::new(),
        OutputDamage::Full => vec![full_output_rect(output_size)],
        OutputDamage::Rects(rects) => rects,
    };
    let scene_work_rects = disjoint_output_rects(scene_work_rects, output_size);
    let mut extra_scene_work = Vec::new();
    for scene_rect in &scene_work_rects {
        let mut fragments = vec![*scene_rect];
        for repaint_rect in repaint_rects {
            let mut next = Vec::new();
            for fragment in fragments {
                next.extend(subtract_output_rect(fragment, *repaint_rect));
            }
            fragments = next;
            if fragments.is_empty() {
                break;
            }
        }
        extra_scene_work.extend(fragments);
    }
    SceneWorkRegions {
        scene_work_rects,
        extra_scene_work,
    }
}

fn disjoint_output_rects(rects: Vec<OutputRect>, output_size: (u32, u32)) -> Vec<OutputRect> {
    let mut disjoint = Vec::new();
    for source in rects {
        let mut fragments = vec![source];
        for represented in &disjoint {
            let mut next = Vec::new();
            for fragment in fragments {
                next.extend(subtract_output_rect(fragment, *represented));
            }
            fragments = next;
            if fragments.is_empty() {
                break;
            }
        }
        disjoint.extend(fragments);
        if disjoint.len() > MAX_EFFECT_REGION_RECTS {
            return vec![full_output_rect(output_size)];
        }
    }
    disjoint
}

fn subtract_output_rect(source: OutputRect, excluded: OutputRect) -> Vec<OutputRect> {
    let left = i64::from(source.x).max(i64::from(excluded.x));
    let top = i64::from(source.y).max(i64::from(excluded.y));
    let right = (i64::from(source.x) + i64::from(source.width))
        .min(i64::from(excluded.x) + i64::from(excluded.width));
    let bottom = (i64::from(source.y) + i64::from(source.height))
        .min(i64::from(excluded.y) + i64::from(excluded.height));
    if left >= right || top >= bottom {
        return vec![source];
    }

    let mut result = Vec::with_capacity(4);
    let push = |result: &mut Vec<OutputRect>, x: i64, y: i64, right: i64, bottom: i64| {
        if right > x && bottom > y {
            result.push(OutputRect::new(
                i32::try_from(x).expect("scene work x fits i32"),
                i32::try_from(y).expect("scene work y fits i32"),
                u32::try_from(right - x).expect("scene work width fits u32"),
                u32::try_from(bottom - y).expect("scene work height fits u32"),
            ));
        }
    };
    let source_right = i64::from(source.x) + i64::from(source.width);
    let source_bottom = i64::from(source.y) + i64::from(source.height);
    push(
        &mut result,
        i64::from(source.x),
        i64::from(source.y),
        source_right,
        top,
    );
    push(
        &mut result,
        i64::from(source.x),
        bottom,
        source_right,
        source_bottom,
    );
    push(&mut result, i64::from(source.x), top, left, bottom);
    push(&mut result, right, top, source_right, bottom);
    result
}

fn clipped_output_rect(
    rect: oblivion_one::effects::EffectRect,
    output_size: (u32, u32),
) -> Option<OutputRect> {
    let left = i64::from(rect.x).clamp(0, i64::from(output_size.0));
    let top = i64::from(rect.y).clamp(0, i64::from(output_size.1));
    let right = i64::from(rect.right()).clamp(0, i64::from(output_size.0));
    let bottom = i64::from(rect.bottom()).clamp(0, i64::from(output_size.1));
    (right > left && bottom > top).then(|| {
        OutputRect::new(
            i32::try_from(left).expect("output width fits i32"),
            i32::try_from(top).expect("output height fits i32"),
            u32::try_from(right - left).expect("output rect width fits u32"),
            u32::try_from(bottom - top).expect("output rect height fits u32"),
        )
    })
}

fn output_rects_bounding_box(rects: &[OutputRect]) -> Option<(i32, i32, u32, u32)> {
    let first = rects.first().copied()?;
    let (left, top, right, bottom) = rects.iter().skip(1).fold(
        (
            i64::from(first.x),
            i64::from(first.y),
            i64::from(first.x) + i64::from(first.width),
            i64::from(first.y) + i64::from(first.height),
        ),
        |(left, top, right, bottom), rect| {
            (
                left.min(i64::from(rect.x)),
                top.min(i64::from(rect.y)),
                right.max(i64::from(rect.x) + i64::from(rect.width)),
                bottom.max(i64::from(rect.y) + i64::from(rect.height)),
            )
        },
    );
    Some((
        i32::try_from(left).ok()?,
        i32::try_from(top).ok()?,
        u32::try_from(right - left).ok()?,
        u32::try_from(bottom - top).ok()?,
    ))
}

fn effect_capture_output_rects(
    damage: &EffectRegion,
    target_domain: Option<oblivion_one::effects::EffectRect>,
    output_size: (u32, u32),
) -> Vec<OutputRect> {
    let full_output = oblivion_one::effects::EffectRect::new(0, 0, output_size.0, output_size.1)
        .expect("renderer dimensions are valid");
    let rects = if damage.is_empty() || damage.bounding_rect().is_none() {
        std::slice::from_ref(&full_output)
    } else {
        damage.rects()
    };
    rects
        .iter()
        .filter_map(|rect| target_domain.map_or(Some(*rect), |domain| rect.intersect(domain)))
        .map(effect_rect_to_output_rect)
        .collect()
}

fn effect_rect_to_output_rect(rect: oblivion_one::effects::EffectRect) -> OutputRect {
    OutputRect::new(rect.x.max(0), rect.y.max(0), rect.width, rect.height)
}

fn draw_damage_scissors(
    gl: &glow::Context,
    damage: &EffectRegion,
    target: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let rects = effect_damage_to_texture_rects(damage, target, framebuffer_origin);
    unsafe {
        gl.enable(glow::SCISSOR_TEST);
        for rect in rects {
            gl.scissor(rect.x, rect.y, rect.width as i32, rect.height as i32);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
        }
    }
}

fn effect_damage_to_texture_rects(
    damage: &EffectRegion,
    target: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Vec<OutputRect> {
    if damage.is_empty() {
        return Vec::new();
    }
    let Some(_) = damage.bounding_rect() else {
        return vec![full_output_rect((target.width, target.height))];
    };
    damage
        .rects()
        .iter()
        .filter_map(|rect| effect_rect_to_texture_rect(*rect, target, framebuffer_origin))
        .collect()
}

#[cfg(test)]
fn capture_clear_rects(
    damage: &EffectRegion,
    target: &oblivion_one::effects::GraphTexturePlan,
) -> Vec<OutputRect> {
    effect_damage_to_texture_rects(damage, target, OutputFramebufferOrigin::BottomLeft)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CaptureMaterializationPlan {
    region: EffectRegion,
    output_rects: Vec<OutputRect>,
}

impl CaptureMaterializationPlan {
    fn texture_rects(&self, target: &oblivion_one::effects::GraphTexturePlan) -> Vec<OutputRect> {
        effect_damage_to_texture_rects(&self.region, target, OutputFramebufferOrigin::BottomLeft)
    }
}

fn capture_materialization_plan(
    execution_damage: &EffectRegion,
    target_domain: Option<oblivion_one::effects::EffectRect>,
    output_size: (u32, u32),
) -> CaptureMaterializationPlan {
    let region = target_domain.map_or_else(
        || execution_damage.clone(),
        |domain| execution_damage.intersect_rect(domain),
    );
    let output_rects = effect_capture_output_rects(&region, target_domain, output_size);
    CaptureMaterializationPlan {
        region,
        output_rects,
    }
}

fn effect_rect_to_texture_rect(
    rect: oblivion_one::effects::EffectRect,
    target: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<OutputRect> {
    let coverage = logical_rect_to_physical_coverage(rect, target)?;
    let width = coverage.width();
    let height = coverage.height();
    let y = match target.origin {
        oblivion_one::effects::GraphTextureOrigin::BottomLeft => match target.source {
            GraphTextureSource::Output => match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => {
                    target.height.saturating_sub(coverage.bottom)
                }
                OutputFramebufferOrigin::TopLeftScanout => coverage.top,
            },
            _ => target.height.saturating_sub(coverage.bottom),
        },
    };
    Some(OutputRect::new(
        i32::try_from(coverage.left).ok()?,
        i32::try_from(y).ok()?,
        width as u32,
        height as u32,
    ))
}

fn restore_output_viewport(renderer: &GlesSceneRenderer) {
    unsafe {
        renderer.gl.viewport(
            0,
            0,
            renderer.current_size.0 as i32,
            renderer.current_size.1 as i32,
        );
    }
}

fn trusted_effect_output_size(
    renderer_size: (u32, u32),
    _stage_texture_size: (u32, u32),
) -> (f32, f32) {
    (renderer_size.0 as f32, renderer_size.1 as f32)
}

#[cfg(test)]
mod coordinate_tests {
    use super::*;
    use crate::egl_renderer::{EglRect, SurfaceSampling};

    fn target(
        source: GraphTextureSource,
        domain: oblivion_one::effects::EffectRect,
        width: u32,
        height: u32,
    ) -> oblivion_one::effects::GraphTexturePlan {
        oblivion_one::effects::GraphTexturePlan {
            id: GraphTextureId::new(1).unwrap(),
            source,
            width,
            height,
            domain,
            working_space: oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
            origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
            first_use: None,
            last_use: None,
        }
    }

    #[test]
    fn local_damage_translates_and_scales_into_an_offset_domain() {
        let target = target(
            GraphTextureSource::Intermediate,
            oblivion_one::effects::EffectRect::new(100, 50, 100, 100).unwrap(),
            50,
            50,
        );
        let logical_rect = oblivion_one::effects::EffectRect::new(110, 60, 20, 20).unwrap();
        let coverage = logical_rect_to_physical_coverage(logical_rect, &target).unwrap();
        assert_eq!(
            coverage,
            oblivion_one::effects::GraphTexturePhysicalRect {
                left: 5,
                top: 5,
                right: 15,
                bottom: 15,
            }
        );
        let rect = effect_rect_to_texture_rect(
            logical_rect,
            &target,
            OutputFramebufferOrigin::TopLeftScanout,
        )
        .unwrap();
        assert_eq!(rect, OutputRect::new(5, 35, 10, 10));
    }

    #[test]
    fn executor_scissor_preserves_shared_coverage_for_framebuffer_y_origins() {
        let target = target(
            GraphTextureSource::Output,
            oblivion_one::effects::EffectRect::new(145, 148, 1112, 873).unwrap(),
            556,
            437,
        );
        let logical_rect = oblivion_one::effects::EffectRect::new(647, 901, 610, 120).unwrap();
        let coverage = logical_rect_to_physical_coverage(logical_rect, &target).unwrap();
        assert_eq!(
            coverage,
            oblivion_one::effects::GraphTexturePhysicalRect {
                left: 251,
                top: 376,
                right: 556,
                bottom: 437,
            }
        );
        assert_eq!(
            effect_rect_to_texture_rect(
                logical_rect,
                &target,
                OutputFramebufferOrigin::BottomLeft,
            )
            .unwrap(),
            OutputRect::new(251, 0, 305, 61)
        );
        assert_eq!(
            effect_rect_to_texture_rect(
                logical_rect,
                &target,
                OutputFramebufferOrigin::TopLeftScanout,
            )
            .unwrap(),
            OutputRect::new(251, 376, 305, 61)
        );
    }

    #[test]
    fn effect_target_flip_y_matches_logical_destination_contract() {
        let cases = [
            (false, OutputFramebufferOrigin::BottomLeft, 0.0),
            (true, OutputFramebufferOrigin::BottomLeft, 0.0),
            (true, OutputFramebufferOrigin::TopLeftScanout, 1.0),
        ];
        for (output_is_framebuffer, origin, logical_top_a_uv_y) in cases {
            let flip = effect_target_requires_logical_y_flip(output_is_framebuffer, origin);
            let logical_top_v_uv_y = if flip {
                1.0 - logical_top_a_uv_y
            } else {
                logical_top_a_uv_y
            };
            assert_eq!(logical_top_v_uv_y, 0.0);
        }
    }

    #[test]
    fn effect_input_flip_y_matches_graph_texture_sample_contract() {
        assert!(effect_input_requires_sample_y_flip(
            oblivion_one::effects::GraphTextureOrigin::BottomLeft,
        ));
        let sample_y = |logical_y: f32| {
            if effect_input_requires_sample_y_flip(
                oblivion_one::effects::GraphTextureOrigin::BottomLeft,
            ) {
                1.0 - logical_y
            } else {
                logical_y
            }
        };
        assert_eq!(sample_y(0.0), 1.0);
        assert_eq!(sample_y(1.0), 0.0);
    }

    #[test]
    fn texture_feedback_alias_is_rejected_even_when_pool_keys_match() {
        let error = validate_no_texture_feedback(Some(17), [17].into_iter()).unwrap_err();
        assert_eq!(
            error,
            EffectExecutionInvariantError::FeedbackTextureAlias {
                input: 17,
                output: 17,
            }
        );

        validate_no_texture_feedback(Some(17), [18].into_iter())
            .expect("different physical resources are safe to sample");
    }

    #[test]
    fn built_in_effect_shaders_keep_logical_and_sample_uv_spaces_separate() {
        for shader in [
            COPY_FRAGMENT_SHADER,
            NORMALIZE_FRAGMENT_SHADER,
            COMPOSITE_FRAGMENT_SHADER,
            FRAGMENT_STAGE_FRAGMENT_SHADER,
            MASK_STAGE_FRAGMENT_SHADER,
            BLEND_STAGE_FRAGMENT_SHADER,
        ] {
            assert!(shader.contains("uniform int u_effect_input_flip_y;"));
            assert!(shader.contains("vec2 typhon_effect_sample_uv(vec2 logical_uv)"));
        }
        assert!(
            NORMALIZE_FRAGMENT_SHADER
                .contains("output_position = u_effect_output_domain.xy + v_uv")
        );
        assert!(
            COMPOSITE_FRAGMENT_SHADER.contains("output_position = v_uv * u_effect_output_size")
        );
        assert!(!NORMALIZE_FRAGMENT_SHADER.contains("texture(u_effect_input, input_uv)"));
        assert!(!COMPOSITE_FRAGMENT_SHADER.contains("texture(u_effect_input, input_uv)"));
    }

    #[test]
    fn every_fullscreen_pass_family_uses_the_shared_sample_orientation_contract() {
        for shader in [
            blur::DUAL_KAWASE_DOWNSAMPLE_SHADER,
            blur::DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER,
            blur::DUAL_KAWASE_UPSAMPLE_SHADER,
        ] {
            assert!(shader.contains("uniform int u_effect_input_flip_y;"));
            assert!(shader.contains("typhon_effect_sample_uv(v_uv)"));
        }
    }

    #[test]
    fn translating_a_domain_does_not_change_storage_orientation() {
        let first = target(
            GraphTextureSource::Intermediate,
            oblivion_one::effects::EffectRect::new(20, 30, 80, 40).unwrap(),
            40,
            20,
        );
        let moved = target(
            GraphTextureSource::Intermediate,
            oblivion_one::effects::EffectRect::new(700, 500, 80, 40).unwrap(),
            40,
            20,
        );
        assert_eq!(first.origin, moved.origin);
        assert_eq!(
            effect_input_requires_sample_y_flip(first.origin),
            effect_input_requires_sample_y_flip(moved.origin)
        );
    }

    #[test]
    fn direct_capture_transfer_selects_and_normalizes_each_output_origin() {
        let domain = oblivion_one::effects::EffectRect::new(8, 10, 20, 20).unwrap();

        let bottom_left = plan_graph_texture_capture(
            (100, 100),
            domain,
            (20, 20),
            OutputFramebufferOrigin::BottomLeft,
        )
        .expect("bottom-left capture domain should be valid");
        assert_eq!(
            bottom_left.source,
            GlBlitRect {
                x0: 8,
                y0: 70,
                x1: 28,
                y1: 90,
            }
        );
        assert_eq!(
            bottom_left.destination,
            GlBlitRect {
                x0: 0,
                y0: 0,
                x1: 20,
                y1: 20,
            }
        );

        let top_left = plan_graph_texture_capture(
            (100, 100),
            domain,
            (20, 20),
            OutputFramebufferOrigin::TopLeftScanout,
        )
        .expect("top-left capture domain should be valid");
        assert_eq!(
            top_left.source,
            GlBlitRect {
                x0: 8,
                y0: 10,
                x1: 28,
                y1: 30,
            }
        );
        assert_eq!(
            top_left.destination,
            GlBlitRect {
                x0: 0,
                y0: 20,
                x1: 20,
                y1: 0,
            }
        );
    }

    #[test]
    fn disjoint_damage_rectangles_preserve_their_hole() {
        let target = target(
            GraphTextureSource::Intermediate,
            oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap(),
            25,
            25,
        );
        let mut damage = EffectRegion::empty();
        damage.push(oblivion_one::effects::EffectRect::new(0, 0, 20, 20).unwrap());
        damage.push(oblivion_one::effects::EffectRect::new(80, 80, 20, 20).unwrap());
        let rects = effect_damage_to_texture_rects(
            &damage,
            &target,
            OutputFramebufferOrigin::TopLeftScanout,
        );
        assert_eq!(rects.len(), 2);
    }

    #[test]
    fn composition_position_keeps_before_target_effects_below_the_target() {
        let layers = [
            capture::CaptureLayer::Other,
            capture::CaptureLayer::Surface(10),
            capture::CaptureLayer::Surface(20),
            capture::CaptureLayer::Other,
        ];
        assert_eq!(
            composition_position(
                &layers,
                oblivion_one::compositor::EffectAnchor::BeforeSurface(20),
            ),
            2
        );
        assert_eq!(
            composition_position(
                &layers,
                oblivion_one::compositor::EffectAnchor::AfterSurface(20),
            ),
            3
        );
    }

    #[test]
    fn checkpoint_backdrop_capture_preserves_before_surface_exclusion() {
        let command = |layer| EglDrawCommand {
            layer,
            visual_group: None,
            bounds: EglRect::new(0.0, 0.0, 80.0, 60.0),
            opaque_regions: Vec::new(),
            vertex_start: 0,
            vertex_count: 6,
            sampling: SurfaceSampling::ExactNearest,
        };
        let commands = vec![
            command(EglDrawLayer::SolidRgba(0xff22_2222)),
            command(EglDrawLayer::Surface(10)),
            command(EglDrawLayer::Surface(20)),
        ];
        let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(20);
        let (draw_end, _) = composition_range(
            &commands,
            anchor,
            None,
            oblivion_one::compositor::EffectAnchorScope::Surface,
        );
        assert_eq!(draw_end, 2, "checkpoint scene must stop before target");

        let layers = commands
            .iter()
            .map(|command| match command.layer {
                EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
                _ => capture::CaptureLayer::Other,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            capture::indices_for_capture(
                &layers,
                &[None; 3],
                anchor,
                false,
                None,
                oblivion_one::compositor::EffectAnchorScope::Surface,
            ),
            vec![0, 1],
            "direct checkpoint capture must not include target surface"
        );
    }

    #[test]
    fn effect_anchor_scope_selects_exact_surface_or_complete_visual_group_range() {
        let group = oblivion_one::compositor::VisualGroupId::new(7).unwrap();
        let command = |layer| EglDrawCommand {
            layer,
            visual_group: Some(group),
            bounds: EglRect::new(0.0, 0.0, 80.0, 60.0),
            opaque_regions: Vec::new(),
            vertex_start: 0,
            vertex_count: 6,
            sampling: SurfaceSampling::ExactNearest,
        };
        let commands = vec![
            command(EglDrawLayer::Surface(1)),
            command(EglDrawLayer::Surface(10)),
            command(EglDrawLayer::Surface(20)),
        ];
        let child_anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(20);

        assert_eq!(
            composition_range(
                &commands,
                child_anchor,
                Some(group),
                oblivion_one::compositor::EffectAnchorScope::Surface,
            ),
            (2, 2)
        );
        assert_eq!(
            composition_range(
                &commands,
                child_anchor,
                Some(group),
                oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            ),
            (0, 0)
        );
        assert_eq!(
            composition_range(
                &commands,
                oblivion_one::compositor::EffectAnchor::ReplaceSurface(20),
                Some(group),
                oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            ),
            (0, 3)
        );
        assert_eq!(
            composition_range(
                &commands,
                oblivion_one::compositor::EffectAnchor::AfterSurface(20),
                Some(group),
                oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            ),
            (3, 3)
        );
        assert_eq!(
            capture::indices_for_capture(
                &commands
                    .iter()
                    .map(|command| match command.layer {
                        EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
                        _ => capture::CaptureLayer::Other,
                    })
                    .collect::<Vec<_>>(),
                &commands
                    .iter()
                    .map(|command| command.visual_group)
                    .collect::<Vec<_>>(),
                child_anchor,
                false,
                Some(group),
                oblivion_one::compositor::EffectAnchorScope::Surface,
            ),
            vec![0, 1]
        );
    }

    #[test]
    fn before_surface_visual_group_execution_keeps_root_and_decoration_commands() {
        let group = oblivion_one::compositor::VisualGroupId::new(8).unwrap();
        let command = |layer| EglDrawCommand {
            layer,
            visual_group: Some(group),
            bounds: EglRect::new(0.0, 0.0, 80.0, 60.0),
            opaque_regions: Vec::new(),
            vertex_start: 0,
            vertex_count: 6,
            sampling: SurfaceSampling::ExactNearest,
        };
        let commands = vec![
            command(EglDrawLayer::SolidRgba(0xff00_0000)),
            command(EglDrawLayer::Surface(42)),
            command(EglDrawLayer::SolidRgba(0xff33_3333)),
        ];
        let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);

        assert_eq!(
            composition_range(
                &commands,
                anchor,
                Some(group),
                oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            ),
            (0, 0)
        );
        assert_eq!(
            composition_range(
                &commands,
                oblivion_one::compositor::EffectAnchor::ReplaceSurface(42),
                Some(group),
                oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            ),
            (0, 3)
        );
        assert_eq!(
            composition_range(
                &commands,
                oblivion_one::compositor::EffectAnchor::AfterSurface(42),
                Some(group),
                oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            ),
            (3, 3)
        );
        assert_eq!(
            capture::indices_for_capture(
                &commands
                    .iter()
                    .map(|command| match command.layer {
                        EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
                        _ => capture::CaptureLayer::Other,
                    })
                    .collect::<Vec<_>>(),
                &commands
                    .iter()
                    .map(|command| command.visual_group)
                    .collect::<Vec<_>>(),
                anchor,
                true,
                Some(group),
                oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            ),
            vec![0, 1, 2]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egl_renderer::{EglRect, SurfaceSampling};

    fn test_texture(
        id: u16,
        source: GraphTextureSource,
        domain: oblivion_one::effects::EffectRect,
    ) -> oblivion_one::effects::GraphTexturePlan {
        oblivion_one::effects::GraphTexturePlan {
            id: GraphTextureId::new(id).unwrap(),
            source,
            width: domain.width,
            height: domain.height,
            domain,
            working_space: oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
            origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
            first_use: None,
            last_use: None,
        }
    }

    fn test_pass(
        id: u16,
        kind: RenderPassKind,
        instance: oblivion_one::effects::EffectInstanceId,
        inputs: Vec<GraphTextureId>,
        output: GraphTextureId,
        checkpoint_dependencies: Vec<GraphPassId>,
    ) -> CompiledRenderPass {
        CompiledRenderPass {
            id: GraphPassId::new(id).unwrap(),
            kind,
            inputs,
            output: Some(output),
            damage: EffectRegion::empty(),
            instance,
            anchor: oblivion_one::compositor::EffectAnchor::OutputPostProcess,
            blur_radius: None,
            stage: None,
            fused_stages: Vec::new(),
            parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
            alpha_mode: oblivion_one::effects::EffectAlphaMode::Preserve,
            encode_output: false,
            color_conversion: EffectColorConversion::None,
            checkpoint_dependencies,
            visual_group: None,
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            visible_clip_fallback: None,
        }
    }

    fn planned_demand(
        instance: oblivion_one::effects::EffectInstanceId,
        output_region: EffectRegion,
        passes: Vec<(GraphPassId, EffectRegion)>,
    ) -> EffectExecutionDemand {
        let mut demand = EffectExecutionDemand::new(
            vec![oblivion_one::effects::EffectInstanceExecutionDemand {
                id: instance,
                output_region: output_region.clone(),
            }],
            output_region,
        );
        demand.passes = passes
            .into_iter()
            .map(
                |(id, output_region)| oblivion_one::effects::EffectPassExecutionDemand {
                    id,
                    output_region,
                },
            )
            .collect();
        demand
    }

    #[test]
    fn framebuffer_capture_orders_scene_advance_by_capture_policy() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let output = GraphTextureId::new(2).unwrap();
        let ordinary_capture = test_pass(
            1,
            RenderPassKind::SceneCapture,
            instance,
            Vec::new(),
            output,
            Vec::new(),
        );
        assert_eq!(
            scene_advance_reason(&ordinary_capture, true),
            Some("framebuffer_capture")
        );
        assert_eq!(scene_advance_reason(&ordinary_capture, false), None);

        let checkpoint_capture = test_pass(
            3,
            RenderPassKind::SceneCapture,
            instance,
            Vec::new(),
            output,
            vec![GraphPassId::new(2).unwrap()],
        );
        assert_eq!(
            scene_advance_reason(&checkpoint_capture, false),
            Some("checkpoint_dependency")
        );

        let surface_capture = test_pass(
            4,
            RenderPassKind::SurfaceCapture,
            instance,
            Vec::new(),
            output,
            Vec::new(),
        );
        assert_eq!(scene_advance_reason(&surface_capture, true), None);
    }

    #[test]
    fn framebuffer_scene_work_includes_selected_backdrop_capture_domains() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let capture_id = GraphTextureId::new(1).unwrap();
        let capture_domain = oblivion_one::effects::EffectRect::new(40, 30, 60, 50).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::SceneCapture,
            instance,
            Vec::new(),
            capture_id,
            Vec::new(),
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![test_texture(
                1,
                GraphTextureSource::CapturedScene,
                capture_domain,
            )],
            instances: Vec::new(),
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let selection = EffectExecutionSelection {
            executed_passes: vec![pass.id],
            ..EffectExecutionSelection::default()
        };
        let config = EffectDebugConfig::new(
            EffectDebugCaptureMode::Framebuffer,
            EffectDebugKawaseMode::Partial,
        );
        let regions = scene_work_regions(
            &[OutputRect::new(5, 6, 7, 8)],
            &graph,
            &selection,
            (200, 150),
            false,
            config,
        );
        let work = &regions.scene_work_rects;

        assert!(work.contains(&OutputRect::new(5, 6, 7, 8)));
        assert!(work.contains(&OutputRect::new(40, 30, 60, 50)));
        assert!(!regions.extra_scene_work.is_empty());
        assert!(
            regions.extra_scene_work.iter().all(|rect| {
                subtract_output_rect(*rect, OutputRect::new(5, 6, 7, 8)).len() == 1
            })
        );
        assert!(work.len() <= MAX_EFFECT_REGION_RECTS);

        let coalesced = scene_work_regions(
            &[OutputRect::new(5, 6, 50, 40)],
            &graph,
            &selection,
            (200, 150),
            false,
            config,
        )
        .scene_work_rects;
        assert_eq!(coalesced.len(), 3);
        for (index, first) in coalesced.iter().enumerate() {
            for second in coalesced.iter().skip(index + 1) {
                assert!(
                    subtract_output_rect(*first, *second).len() == 1,
                    "scene-work scissors must not overlap: {first:?} and {second:?}"
                );
            }
        }
    }

    #[test]
    fn replay_scene_work_includes_selected_checkpoint_framebuffer_domains() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let capture_id = GraphTextureId::new(1).unwrap();
        let checkpoint_dependency = GraphPassId::new(9).unwrap();
        let capture_domain = oblivion_one::effects::EffectRect::new(762, 976, 396, 104).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::SceneCapture,
            instance,
            Vec::new(),
            capture_id,
            vec![checkpoint_dependency],
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![test_texture(
                1,
                GraphTextureSource::CapturedScene,
                capture_domain,
            )],
            instances: Vec::new(),
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let selection = EffectExecutionSelection {
            executed_passes: vec![pass.id],
            ..EffectExecutionSelection::default()
        };
        let config = EffectDebugConfig::new(
            EffectDebugCaptureMode::Replay,
            EffectDebugKawaseMode::Partial,
        );

        let regions = scene_work_regions(
            &[OutputRect::new(24, 20, 8, 8)],
            &graph,
            &selection,
            (1920, 1080),
            false,
            config,
        );

        assert!(
            regions
                .scene_work_rects
                .iter()
                .any(|rect| rect.x <= capture_domain.x
                    && rect.y <= capture_domain.y
                    && rect.x + rect.width as i32 >= capture_domain.right()
                    && rect.y + rect.height as i32 >= capture_domain.bottom()),
            "replay checkpoint capture must expand internal scene work to its exact source domain: {:?}",
            regions.scene_work_rects
        );
        assert!(!regions.extra_scene_work.is_empty());
    }

    #[test]
    fn replay_scene_work_includes_topbar_checkpoint_framebuffer_domain() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let capture_id = GraphTextureId::new(1).unwrap();
        let checkpoint_dependency = GraphPassId::new(9).unwrap();
        let capture_domain = oblivion_one::effects::EffectRect::new(0, 0, 120, 65).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::SceneCapture,
            instance,
            Vec::new(),
            capture_id,
            vec![checkpoint_dependency],
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![test_texture(
                1,
                GraphTextureSource::CapturedScene,
                capture_domain,
            )],
            instances: Vec::new(),
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let selection = EffectExecutionSelection {
            executed_passes: vec![pass.id],
            ..EffectExecutionSelection::default()
        };
        let regions = scene_work_regions(
            &[OutputRect::new(10, 10, 4, 4)],
            &graph,
            &selection,
            (1920, 1080),
            false,
            EffectDebugConfig::new(
                EffectDebugCaptureMode::Replay,
                EffectDebugKawaseMode::Partial,
            ),
        );

        assert!(regions.scene_work_rects.iter().any(|rect| {
            rect.x <= capture_domain.x
                && rect.y <= capture_domain.y
                && rect.x + rect.width as i32 >= capture_domain.right()
                && rect.y + rect.height as i32 >= capture_domain.bottom()
        }));
        assert!(!regions.extra_scene_work.is_empty());
    }

    #[test]
    fn checkpoint_source_validity_uses_exact_regions_not_bounding_boxes() {
        let required = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(100, 100, 20, 20).unwrap(),
        );
        let valid = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(100, 100, 20, 8).unwrap(),
        )
        .union(&EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(100, 112, 20, 8).unwrap(),
        ));

        let validity = checkpoint_source_validity(&required, &valid);

        assert_eq!(validity.required_rect_count, 1);
        assert_eq!(validity.valid_rect_count, 2);
        assert_eq!(validity.required_bounding_box, Some((100, 100, 20, 20)));
        assert_eq!(validity.valid_bounding_box, Some((100, 100, 20, 20)));
        assert_eq!(
            validity.missing.rects(),
            &[oblivion_one::effects::EffectRect::new(100, 108, 20, 4).unwrap()]
        );
        assert_eq!(effect_region_pixels(&validity.missing), 80);
    }

    #[test]
    fn checkpoint_semantic_validity_does_not_mask_dependency_gaps_with_base_scene() {
        let required = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(762, 976, 396, 104).unwrap(),
        );
        let dependency_influence = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(786, 1000, 348, 56).unwrap(),
        );
        let partial_dependency_output = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(900, 1010, 8, 8).unwrap(),
        );
        let validity = checkpoint_source_semantic_validity(
            &required,
            &required.subtract(&dependency_influence),
            &dependency_influence,
            &[(dependency_influence.clone(), partial_dependency_output)],
        );

        assert!(!validity.missing.is_empty());
        assert!(validity.missing.contains_point(800, 1004));
        assert!(!validity.missing.contains_point(902, 1012));
    }

    #[test]
    fn checkpoint_semantic_validity_does_not_mask_missing_overlapping_dependency() {
        let required = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(100, 100, 20, 20).unwrap(),
        );
        let dependency_a_influence = required.clone();
        let dependency_c_influence = required.clone();
        let dependency_a_valid = required.clone();
        let dependency_c_valid = EffectRegion::empty();
        let dependency_influence = dependency_a_influence
            .union(&dependency_c_influence)
            .intersect(&required);
        let validity = checkpoint_source_semantic_validity(
            &required,
            &EffectRegion::empty(),
            &dependency_influence,
            &[
                (dependency_a_influence, dependency_a_valid),
                (dependency_c_influence, dependency_c_valid),
            ],
        );

        assert!(validity.missing.contains_point(110, 110));
        assert_eq!(effect_region_pixels(&validity.missing), 400);
    }

    #[test]
    fn checkpoint_semantic_validity_accepts_fully_valid_overlapping_dependencies() {
        let required = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(100, 100, 20, 20).unwrap(),
        );
        let dependency_a_influence = required.clone();
        let dependency_c_influence = required.clone();
        let dependency_a_valid = required.clone();
        let dependency_c_valid = required.clone();
        let dependency_influence = dependency_a_influence
            .union(&dependency_c_influence)
            .intersect(&required);
        let validity = checkpoint_source_semantic_validity(
            &required,
            &EffectRegion::empty(),
            &dependency_influence,
            &[
                (dependency_a_influence, dependency_a_valid),
                (dependency_c_influence, dependency_c_valid),
            ],
        );

        assert!(validity.missing.is_empty());
    }

    #[test]
    fn checkpoint_semantic_validity_accepts_disjoint_dependency_coverage() {
        let required = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(100, 100, 40, 20).unwrap(),
        );
        let dependency_a_influence = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(100, 100, 20, 20).unwrap(),
        );
        let dependency_c_influence = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(120, 100, 20, 20).unwrap(),
        );
        let dependency_influence = dependency_a_influence
            .union(&dependency_c_influence)
            .intersect(&required);
        let validity = checkpoint_source_semantic_validity(
            &required,
            &EffectRegion::empty(),
            &dependency_influence,
            &[
                (dependency_a_influence.clone(), dependency_a_influence),
                (dependency_c_influence.clone(), dependency_c_influence),
            ],
        );

        assert!(validity.missing.is_empty());
    }

    #[test]
    fn checkpoint_dependency_region_requires_prior_effect_influence_coverage() {
        let earlier = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let later = oblivion_one::effects::EffectInstanceId::new(2).unwrap();
        let earlier_output = GraphTextureId::new(1).unwrap();
        let later_output = GraphTextureId::new(2).unwrap();
        let earlier_pass = test_pass(
            9,
            RenderPassKind::SceneCapture,
            earlier,
            Vec::new(),
            earlier_output,
            Vec::new(),
        );
        let later_pass = test_pass(
            10,
            RenderPassKind::SceneCapture,
            later,
            Vec::new(),
            later_output,
            vec![earlier_pass.id],
        );
        let earlier_influence = oblivion_one::effects::EffectRect::new(786, 1000, 348, 56).unwrap();
        let later_capture = oblivion_one::effects::EffectRect::new(762, 976, 396, 104).unwrap();
        let graph = CompiledFrameGraph {
            passes: vec![earlier_pass, later_pass.clone()],
            textures: vec![
                test_texture(1, GraphTextureSource::CapturedScene, earlier_influence),
                test_texture(2, GraphTextureSource::CapturedScene, later_capture),
            ],
            instances: vec![
                oblivion_one::effects::CompiledEffectInstance {
                    id: earlier,
                    output_influence_region: EffectRegion::from_rect(earlier_influence),
                    capture_region: EffectRegion::from_rect(earlier_influence),
                    dependencies: Vec::new(),
                },
                oblivion_one::effects::CompiledEffectInstance {
                    id: later,
                    output_influence_region: EffectRegion::from_rect(later_capture),
                    capture_region: EffectRegion::from_rect(later_capture),
                    dependencies: vec![earlier],
                },
            ],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };

        assert_eq!(
            checkpoint_dependency_influence_region(
                &graph,
                &graph.passes[1],
                &EffectRegion::from_rect(later_capture),
            ),
            EffectRegion::from_rect(earlier_influence)
        );
    }

    #[test]
    fn effect_pass_blend_modes_are_explicit_for_each_pass_family() {
        for kind in [
            RenderPassKind::NormalizeInput,
            RenderPassKind::DualKawaseDownsample,
            RenderPassKind::DualKawaseUpsample,
            RenderPassKind::Fragment,
            RenderPassKind::Blend,
            RenderPassKind::Mask,
        ] {
            assert_eq!(
                effect_pass_blend_mode(
                    kind,
                    false,
                    oblivion_one::effects::EffectAlphaMode::Preserve,
                ),
                EffectPassBlendMode::Replace,
                "internal pass {kind:?} must replace its target"
            );
        }
        assert_eq!(
            capture_blend_mode(),
            EffectPassBlendMode::PremultipliedSourceOver
        );
        assert_eq!(
            effect_pass_blend_mode(
                RenderPassKind::Composite,
                true,
                oblivion_one::effects::EffectAlphaMode::Opaque,
            ),
            EffectPassBlendMode::Replace
        );
        assert_eq!(
            effect_pass_blend_mode(
                RenderPassKind::OutputPostProcess,
                true,
                oblivion_one::effects::EffectAlphaMode::Preserve,
            ),
            EffectPassBlendMode::PremultipliedSourceOver
        );
    }

    #[test]
    fn capture_materialization_plan_keeps_region_and_raster_rects_authoritative() {
        let domain = oblivion_one::effects::EffectRect::new(100, 80, 120, 90).unwrap();
        let mut execution = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(108, 88, 12, 10).unwrap(),
        );
        execution.push(oblivion_one::effects::EffectRect::new(180, 132, 8, 7).unwrap());

        let exact = capture_materialization_plan(&execution, Some(domain), (320, 240));
        assert_eq!(exact.region, execution);
        assert_eq!(exact.output_rects.len(), 2);
        let target = test_texture(1, GraphTextureSource::CapturedScene, domain);
        assert_eq!(
            exact.texture_rects(&target),
            vec![
                OutputRect::new(8, 72, 12, 10),
                OutputRect::new(80, 31, 8, 7)
            ]
        );
    }

    #[test]
    fn current_frame_validity_guard_rejects_unproduced_input_texels() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let input = GraphTextureId::new(1).unwrap();
        let output = GraphTextureId::new(2).unwrap();
        let domain = oblivion_one::effects::EffectRect::new(0, 0, 64, 48).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::Fragment,
            instance,
            vec![input],
            output,
            Vec::new(),
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![
                test_texture(1, GraphTextureSource::Intermediate, domain),
                test_texture(2, GraphTextureSource::Intermediate, domain),
            ],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(domain),
                capture_region: EffectRegion::from_rect(domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let execution =
            EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(12, 14, 9, 7).unwrap());
        let error = validate_current_frame_input_regions(
            &graph,
            &pass,
            &execution,
            &std::collections::HashMap::new(),
        )
        .expect_err("an unproduced input must be rejected before sampling");
        assert!(matches!(
            error,
            EffectExecutionInvariantError::UninitializedInputRegion {
                consumer,
                input: actual_input,
                missing,
            } if consumer == pass.id && actual_input == input && !missing.is_empty()
        ));
    }

    #[test]
    fn blend_sensitive_execution_regions_have_single_coverage() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let input = GraphTextureId::new(1).unwrap();
        let output = GraphTextureId::new(2).unwrap();
        let domain = oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::Fragment,
            instance,
            vec![input],
            output,
            vec![],
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![
                test_texture(1, GraphTextureSource::Intermediate, domain),
                test_texture(2, GraphTextureSource::Intermediate, domain),
            ],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(domain),
                capture_region: EffectRegion::from_rect(domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let mut requested =
            EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(0, 0, 10, 10).unwrap());
        requested.push(oblivion_one::effects::EffectRect::new(5, 0, 10, 10).unwrap());

        let prepared = prepare_effect_execution_region(&graph, &pass, requested);
        let rects = effect_capture_output_rects(&prepared.region, Some(domain), (100, 100));

        assert_eq!(prepared.fallback, None);
        assert_eq!(output_rect_pixels(&rects), 150);
        for (index, first) in prepared.region.rects().iter().enumerate() {
            for second in prepared.region.rects().iter().skip(index + 1) {
                assert!(first.intersect(*second).is_none());
            }
        }
    }

    #[test]
    fn preserve_composite_execution_regions_have_single_coverage() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let input = GraphTextureId::new(1).unwrap();
        let output = GraphTextureId::new(2).unwrap();
        let domain = oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap();
        let mut pass = test_pass(
            1,
            RenderPassKind::Composite,
            instance,
            vec![input],
            output,
            vec![],
        );
        pass.alpha_mode = oblivion_one::effects::EffectAlphaMode::Preserve;
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![
                test_texture(1, GraphTextureSource::Intermediate, domain),
                test_texture(2, GraphTextureSource::Output, domain),
            ],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(domain),
                capture_region: EffectRegion::from_rect(domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let mut requested =
            EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(0, 0, 10, 10).unwrap());
        requested.push(oblivion_one::effects::EffectRect::new(5, 0, 10, 10).unwrap());

        let prepared = prepare_effect_execution_region(&graph, &pass, requested);

        assert_eq!(
            effect_pass_blend_mode(
                RenderPassKind::Composite,
                true,
                oblivion_one::effects::EffectAlphaMode::Preserve,
            ),
            EffectPassBlendMode::PremultipliedSourceOver
        );
        assert_eq!(prepared.fallback, None);
        assert_eq!(effect_region_pixels(&prepared.region), 150);
        for (index, first) in prepared.region.rects().iter().enumerate() {
            for second in prepared.region.rects().iter().skip(index + 1) {
                assert!(first.intersect(*second).is_none());
            }
        }
    }

    #[test]
    fn internal_single_coverage_overflow_uses_bounded_work_fallback() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let input = GraphTextureId::new(1).unwrap();
        let output = GraphTextureId::new(2).unwrap();
        let domain = oblivion_one::effects::EffectRect::new(0, 0, 128, 128).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::Fragment,
            instance,
            vec![input],
            output,
            vec![],
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![
                test_texture(1, GraphTextureSource::Intermediate, domain),
                test_texture(2, GraphTextureSource::Intermediate, domain),
            ],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(domain),
                capture_region: EffectRegion::from_rect(domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let mut requested = EffectRegion::empty();
        for index in 0..64 {
            requested.push(oblivion_one::effects::EffectRect::new(index * 2, 0, 1, 128).unwrap());
        }
        for index in 0..64 {
            requested
                .push(oblivion_one::effects::EffectRect::new(0, index * 2 + 1, 128, 1).unwrap());
        }

        let prepared = prepare_effect_execution_region(&graph, &pass, requested);

        assert_eq!(prepared.fallback, Some("work_region_bbox_coalesce"));
        assert_eq!(prepared.region, EffectRegion::from_rect(domain));
    }

    #[test]
    fn final_composite_overflow_uses_the_authoritative_disconnected_clip() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let input = GraphTextureId::new(1).unwrap();
        let output = GraphTextureId::new(2).unwrap();
        let domain = oblivion_one::effects::EffectRect::new(0, 0, 128, 128).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::Composite,
            instance,
            vec![input],
            output,
            vec![],
        );
        let mut visible =
            EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(0, 0, 1, 128).unwrap());
        visible.push(oblivion_one::effects::EffectRect::new(127, 0, 1, 128).unwrap());
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![
                test_texture(1, GraphTextureSource::Intermediate, domain),
                test_texture(2, GraphTextureSource::Output, domain),
            ],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: visible.clone(),
                capture_region: visible.clone(),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let mut requested = EffectRegion::empty();
        for index in 0..64 {
            requested.push(oblivion_one::effects::EffectRect::new(index * 2, 0, 1, 128).unwrap());
        }
        for index in 0..64 {
            requested
                .push(oblivion_one::effects::EffectRect::new(0, index * 2 + 1, 128, 1).unwrap());
        }

        let prepared = prepare_effect_execution_region(&graph, &pass, requested);
        let scissors = effect_damage_to_texture_rects(
            &prepared.region,
            &graph.textures[1],
            OutputFramebufferOrigin::BottomLeft,
        );

        assert_eq!(prepared.fallback, Some("output_influence"));
        assert_eq!(prepared.region, visible);
        assert_eq!(
            scissors,
            vec![
                OutputRect::new(0, 0, 1, 128),
                OutputRect::new(127, 0, 1, 128)
            ]
        );
        assert!(!prepared.region.contains_point(64, 64));
    }

    #[test]
    fn trusted_output_size_uses_the_compositor_output_not_the_stage_texture() {
        assert_eq!(
            trusted_effect_output_size((1920, 1080), (300, 200)),
            (1920.0, 1080.0)
        );
    }

    #[test]
    fn full_output_fallback_is_bounded_to_renderer_size() {
        assert_eq!(
            full_output_rect((1920, 1080)),
            OutputRect::new(0, 0, 1920, 1080)
        );
    }

    #[test]
    fn capture_source_over_contract_matches_two_translucent_layers() {
        let background = oblivion_one::effects::PremultipliedRgba::new(0.2, 0.1, 0.05, 0.5);
        let foreground = oblivion_one::effects::PremultipliedRgba::new(0.4, 0.2, 0.1, 0.5);
        let captured = background.blend(
            foreground,
            oblivion_one::effects::BlendMode::SourceOver,
            1.0,
        );
        assert_eq!(captured.a, 0.75);
        assert_eq!(captured.r, 0.5);
        assert_eq!(captured.g, 0.25);
        assert_eq!(captured.b, 0.125);
    }

    #[test]
    fn built_in_shader_contract_contains_mask_parity_and_safe_srgb_guards() {
        assert!(MASK_STAGE_FRAGMENT_SHADER.contains("result *= clamp(coverage"));
        assert!(NORMALIZE_FRAGMENT_SHADER.contains("u_effect_output_domain"));
        assert!(NORMALIZE_FRAGMENT_SHADER.contains("out_color = vec4(0.0)"));
        for shader in [
            NORMALIZE_FRAGMENT_SHADER,
            COMPOSITE_FRAGMENT_SHADER,
            FRAGMENT_STAGE_FRAGMENT_SHADER,
            MASK_STAGE_FRAGMENT_SHADER,
            BLEND_STAGE_FRAGMENT_SHADER,
        ] {
            assert!(shader.contains("isnan"));
            assert!(shader.contains("isinf"));
            assert!(shader.contains("if (value <= 0.0031308)"));
        }
        for shader in [
            NORMALIZE_FRAGMENT_SHADER,
            FRAGMENT_STAGE_FRAGMENT_SHADER,
            MASK_STAGE_FRAGMENT_SHADER,
            BLEND_STAGE_FRAGMENT_SHADER,
        ] {
            assert!(shader.contains("if (value <= 0.04045)"));
        }
    }

    #[test]
    fn normalize_nonzero_domain_maps_global_position() {
        assert!(
            NORMALIZE_FRAGMENT_SHADER
                .contains("u_effect_output_domain.xy + v_uv * u_effect_output_domain.zw")
        );
        assert!(
            NORMALIZE_FRAGMENT_SHADER.contains("(output_position - u_effect_input_domain.xy) /")
        );
    }

    #[test]
    fn normalize_pooled_clear_writes_transparent_black() {
        assert!(NORMALIZE_FRAGMENT_SHADER.contains("out_color = vec4(0.0);"));
        assert!(NORMALIZE_FRAGMENT_SHADER.contains("return;"));
        assert!(!NORMALIZE_FRAGMENT_SHADER.contains("discard;"));
    }

    #[test]
    fn pruned_instance_does_not_realize_textures() {
        let first = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let second = oblivion_one::effects::EffectInstanceId::new(2).unwrap();
        let first_input = GraphTextureId::new(1).unwrap();
        let first_output = GraphTextureId::new(2).unwrap();
        let second_input = GraphTextureId::new(3).unwrap();
        let second_output = GraphTextureId::new(4).unwrap();
        let pass = |id, instance, input, output| CompiledRenderPass {
            id: GraphPassId::new(id).unwrap(),
            kind: RenderPassKind::Fragment,
            inputs: vec![input],
            output: Some(output),
            damage: EffectRegion::empty(),
            instance,
            anchor: oblivion_one::compositor::EffectAnchor::OutputPostProcess,
            blur_radius: None,
            stage: None,
            fused_stages: Vec::new(),
            parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
            alpha_mode: oblivion_one::effects::EffectAlphaMode::Preserve,
            encode_output: false,
            color_conversion: EffectColorConversion::None,
            checkpoint_dependencies: Vec::new(),
            visual_group: None,
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            visible_clip_fallback: None,
        };
        let graph = CompiledFrameGraph {
            passes: vec![
                pass(1, first, first_input, first_output),
                pass(2, second, second_input, second_output),
            ],
            textures: Vec::new(),
            instances: vec![
                oblivion_one::effects::CompiledEffectInstance {
                    id: first,
                    output_influence_region: EffectRegion::from_rect(
                        oblivion_one::effects::EffectRect::new(0, 0, 10, 10).unwrap(),
                    ),
                    capture_region: EffectRegion::from_rect(
                        oblivion_one::effects::EffectRect::new(0, 0, 10, 10).unwrap(),
                    ),
                    dependencies: Vec::new(),
                },
                oblivion_one::effects::CompiledEffectInstance {
                    id: second,
                    output_influence_region: EffectRegion::from_rect(
                        oblivion_one::effects::EffectRect::new(20, 0, 10, 10).unwrap(),
                    ),
                    capture_region: EffectRegion::from_rect(
                        oblivion_one::effects::EffectRect::new(20, 0, 10, 10).unwrap(),
                    ),
                    dependencies: Vec::new(),
                },
            ],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let demand = oblivion_one::effects::EffectExecutionDemand::new(
            vec![oblivion_one::effects::EffectInstanceExecutionDemand {
                id: first,
                output_region: EffectRegion::from_rect(
                    oblivion_one::effects::EffectRect::new(0, 0, 10, 10).unwrap(),
                ),
            }],
            EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(0, 0, 10, 10).unwrap()),
        );

        let selection = select_effect_execution(&graph, &demand);

        assert_eq!(selection.executed_instances, vec![first]);
        assert_eq!(
            selection.executed_passes,
            vec![GraphPassId::new(1).unwrap()]
        );
        assert_eq!(
            selection.acquired_texture_ids,
            vec![first_input, first_output]
        );
    }

    #[test]
    fn precise_pass_demand_replaces_full_texture_domain_damage() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let input = GraphTextureId::new(1).unwrap();
        let output = GraphTextureId::new(2).unwrap();
        let domain = oblivion_one::effects::EffectRect::new(0, 0, 100, 80).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::Fragment,
            instance,
            vec![input],
            output,
            Vec::new(),
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![
                test_texture(1, GraphTextureSource::Intermediate, domain),
                test_texture(2, GraphTextureSource::Intermediate, domain),
            ],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(domain),
                capture_region: EffectRegion::from_rect(domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let demanded =
            EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(12, 14, 9, 7).unwrap());
        let demand = planned_demand(
            instance,
            demanded.clone(),
            vec![(pass.id, demanded.clone())],
        );

        assert_eq!(effective_pass_damage(&graph, &demand, &pass), demanded);
    }

    #[test]
    fn conservative_composite_scissor_excludes_capture_padding() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let input = GraphTextureId::new(1).unwrap();
        let output = GraphTextureId::new(2).unwrap();
        let output_domain = oblivion_one::effects::EffectRect::new(0, 0, 1920, 1080).unwrap();
        let visible = oblivion_one::effects::EffectRect::new(500, 250, 800, 500).unwrap();
        let capture_domain = oblivion_one::effects::EffectRect::new(476, 226, 848, 548).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::Composite,
            instance,
            vec![input],
            output,
            Vec::new(),
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![
                test_texture(1, GraphTextureSource::Intermediate, capture_domain),
                test_texture(2, GraphTextureSource::Output, output_domain),
            ],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(visible),
                capture_region: EffectRegion::from_rect(capture_domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let demand = oblivion_one::effects::plan_effect_execution_demand(
            &graph,
            &EffectRegion::empty(),
            true,
        );

        let execution_damage = effective_pass_damage(&graph, &demand, &pass);

        assert_eq!(
            execution_damage,
            EffectRegion::from_rect(
                oblivion_one::effects::EffectRect::new(500, 250, 800, 500).unwrap()
            )
        );
        assert!(!execution_damage.contains_point(476, 226));
        assert!(!execution_damage.contains_point(499, 400));
        assert!(!execution_damage.contains_point(1300, 400));
    }

    #[test]
    fn final_composite_scissors_stay_within_fragmented_output_clip() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let input = GraphTextureId::new(1).unwrap();
        let output = GraphTextureId::new(2).unwrap();
        let output_domain = oblivion_one::effects::EffectRect::new(0, 0, 500, 10).unwrap();
        let mut visible =
            EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(0, 0, 200, 10).unwrap());
        visible.push(oblivion_one::effects::EffectRect::new(300, 0, 200, 10).unwrap());
        let mut repair = EffectRegion::empty();
        for index in 0..128 {
            repair.push(
                oblivion_one::effects::EffectRect::new(index, 0, 500 - index as u32, 10).unwrap(),
            );
        }
        let pass = test_pass(
            1,
            RenderPassKind::Composite,
            instance,
            vec![input],
            output,
            Vec::new(),
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![
                test_texture(1, GraphTextureSource::Intermediate, output_domain),
                test_texture(2, GraphTextureSource::Output, output_domain),
            ],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: visible.clone(),
                capture_region: visible.clone(),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let demand = oblivion_one::effects::plan_effect_execution_demand(&graph, &repair, false);
        assert_eq!(demand.plan_stats().visible_clip_fallbacks, 1);

        let execution_damage = effective_pass_damage(&graph, &demand, &pass);
        let scissors = effect_damage_to_texture_rects(
            &execution_damage,
            &graph.textures[1],
            OutputFramebufferOrigin::BottomLeft,
        );

        assert_eq!(
            scissors,
            vec![
                OutputRect::new(0, 0, 200, 10),
                OutputRect::new(300, 0, 200, 10)
            ]
        );
        assert!(scissors.iter().all(|rect| {
            (rect.x..rect.x + rect.width as i32).all(|x| {
                (rect.y..rect.y + rect.height as i32).all(|y| visible.contains_point(x, y))
            })
        }));
        assert!(
            !scissors
                .iter()
                .any(|rect| rect.x <= 200 && 200 < rect.x + rect.width as i32)
        );
    }

    #[test]
    fn empty_pass_demand_skips_execution_and_resource_acquisition() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let first_input = GraphTextureId::new(1).unwrap();
        let first_output = GraphTextureId::new(2).unwrap();
        let second_input = GraphTextureId::new(3).unwrap();
        let second_output = GraphTextureId::new(4).unwrap();
        let domain = oblivion_one::effects::EffectRect::new(0, 0, 100, 80).unwrap();
        let first = test_pass(
            1,
            RenderPassKind::Fragment,
            instance,
            vec![first_input],
            first_output,
            Vec::new(),
        );
        let second = test_pass(
            2,
            RenderPassKind::Fragment,
            instance,
            vec![second_input],
            second_output,
            Vec::new(),
        );
        let graph = CompiledFrameGraph {
            passes: vec![first.clone(), second.clone()],
            textures: vec![
                test_texture(1, GraphTextureSource::Intermediate, domain),
                test_texture(2, GraphTextureSource::Intermediate, domain),
                test_texture(3, GraphTextureSource::Intermediate, domain),
                test_texture(4, GraphTextureSource::Intermediate, domain),
            ],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(domain),
                capture_region: EffectRegion::from_rect(domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let demanded =
            EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(12, 14, 9, 7).unwrap());
        let demand = planned_demand(
            instance,
            demanded.clone(),
            vec![(first.id, EffectRegion::empty()), (second.id, demanded)],
        );

        let selection = select_effect_execution(&graph, &demand);

        assert_eq!(selection.executed_passes, vec![second.id]);
        assert_eq!(
            selection.acquired_texture_ids,
            vec![second_input, second_output]
        );
    }

    #[test]
    fn malformed_producer_metadata_uses_full_domain_for_affected_instance() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let input = GraphTextureId::new(1).unwrap();
        let output = GraphTextureId::new(2).unwrap();
        let domain = oblivion_one::effects::EffectRect::new(0, 0, 1920, 1080).unwrap();
        let visible = oblivion_one::effects::EffectRect::new(500, 250, 800, 500).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::Composite,
            instance,
            vec![input],
            output,
            Vec::new(),
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![
                test_texture(1, GraphTextureSource::Intermediate, domain),
                test_texture(2, GraphTextureSource::Intermediate, domain),
                test_texture(2, GraphTextureSource::Intermediate, domain),
            ],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(visible),
                capture_region: EffectRegion::from_rect(domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let repair = EffectRegion::from_rect(visible);
        let demand = oblivion_one::effects::plan_effect_execution_demand(&graph, &repair, false);

        assert!(demand.instance_is_conservative_full(instance));
        assert_eq!(
            effective_pass_damage(&graph, &demand, &pass),
            EffectRegion::from_rect(visible)
        );
        assert_eq!(demand.plan_stats().pass_conservative_fallbacks, 1);
    }

    #[test]
    fn direct_framebuffer_capture_is_intentionally_conservative() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let output = GraphTextureId::new(1).unwrap();
        let domain = oblivion_one::effects::EffectRect::new(20, 30, 100, 80).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::SceneCapture,
            instance,
            Vec::new(),
            output,
            vec![GraphPassId::new(9).unwrap()],
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![test_texture(1, GraphTextureSource::CapturedScene, domain)],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(domain),
                capture_region: EffectRegion::from_rect(domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let demanded =
            EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(40, 45, 8, 6).unwrap());
        let demand = planned_demand(instance, demanded.clone(), vec![(pass.id, demanded)]);

        assert_eq!(
            capture_execution_damage(&graph, &demand, &pass, false, *effect_debug_config()),
            EffectRegion::from_rect(domain)
        );
    }

    #[test]
    fn replay_capture_clear_plan_covers_only_demanded_target_rectangles() {
        let target = test_texture(
            1,
            GraphTextureSource::CapturedScene,
            oblivion_one::effects::EffectRect::new(100, 50, 100, 100).unwrap(),
        );
        let damage = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(110, 60, 20, 20).unwrap(),
        );

        assert_eq!(
            capture_clear_rects(&damage, &target),
            vec![OutputRect::new(10, 70, 20, 20)]
        );
    }

    #[test]
    fn capture_execution_pixels_are_physical_demanded_area() {
        let target = test_texture(
            1,
            GraphTextureSource::CapturedScene,
            oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap(),
        );
        let damage = EffectRegion::from_rect(
            oblivion_one::effects::EffectRect::new(10, 20, 30, 40).unwrap(),
        );
        let rects = capture_clear_rects(&damage, &target);

        assert_eq!(output_rect_pixels(&rects), 1200);
        assert_ne!(output_rect_pixels(&rects), 10000);
    }

    #[test]
    fn capture_execution_pixels_do_not_double_count_duplicate_demand() {
        let target = test_texture(
            1,
            GraphTextureSource::CapturedScene,
            oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap(),
        );
        let rect = oblivion_one::effects::EffectRect::new(10, 20, 30, 40).unwrap();
        let demand = EffectRegion::from_rect(rect).union(&EffectRegion::from_rect(rect));
        let prepared = demand.disjoint_bounded();
        let rects = capture_clear_rects(&prepared.region, &target);

        assert_eq!(demand.rects().len(), 1);
        assert_eq!(output_rect_pixels(&rects), 1200);
    }

    #[test]
    fn effect_surface_consumer_plan_keeps_capture_only_source() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let pass_id = GraphPassId::new(1).unwrap();
        let output = GraphTextureId::new(1).unwrap();
        let full = oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap();
        let pass = CompiledRenderPass {
            id: pass_id,
            kind: RenderPassKind::SceneCapture,
            inputs: Vec::new(),
            output: Some(output),
            damage: EffectRegion::empty(),
            instance,
            anchor: oblivion_one::compositor::EffectAnchor::BeforeSurface(2),
            blur_radius: None,
            stage: None,
            fused_stages: Vec::new(),
            parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
            alpha_mode: oblivion_one::effects::EffectAlphaMode::Preserve,
            encode_output: false,
            color_conversion: EffectColorConversion::None,
            checkpoint_dependencies: vec![pass_id],
            visual_group: None,
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::Surface,
            visible_clip_fallback: None,
        };
        let graph = CompiledFrameGraph {
            passes: vec![pass],
            textures: vec![oblivion_one::effects::GraphTexturePlan {
                id: output,
                source: GraphTextureSource::Intermediate,
                width: 100,
                height: 100,
                domain: full,
                working_space: oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
                origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
                first_use: None,
                last_use: None,
            }],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(full),
                capture_region: EffectRegion::from_rect(full),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let demand = EffectExecutionDemand::new(
            vec![oblivion_one::effects::EffectInstanceExecutionDemand {
                id: instance,
                output_region: EffectRegion::from_rect(full),
            }],
            EffectRegion::from_rect(full),
        );
        let selection = select_effect_execution(&graph, &demand);
        let command = |layer| EglDrawCommand {
            layer,
            visual_group: None,
            bounds: EglRect::new(0.0, 0.0, 100.0, 100.0),
            opaque_regions: Vec::new(),
            vertex_start: 0,
            vertex_count: 6,
            sampling: SurfaceSampling::ExactNearest,
        };
        let commands = vec![
            command(EglDrawLayer::Surface(1)),
            EglDrawCommand {
                opaque_regions: vec![EglRect::new(0.0, 0.0, 100.0, 100.0)],
                ..command(EglDrawLayer::Surface(2))
            },
        ];

        let plan = plan_effect_surface_consumers(
            &graph,
            &demand,
            &selection,
            &commands,
            &[OutputRect::new(0, 0, 100, 100)],
            (100, 100),
        );

        assert!(plan.surface_ids().contains(&1));
    }

    #[test]
    fn effect_surface_consumer_plan_uses_precise_capture_demand() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let output = GraphTextureId::new(1).unwrap();
        let domain = oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::SceneCapture,
            instance,
            Vec::new(),
            output,
            Vec::new(),
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![test_texture(1, GraphTextureSource::CapturedScene, domain)],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(domain),
                capture_region: EffectRegion::from_rect(domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let demanded =
            EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(0, 0, 20, 20).unwrap());
        let demand = planned_demand(instance, demanded.clone(), vec![(pass.id, demanded)]);
        let selection = select_effect_execution(&graph, &demand);
        let command = |layer, x| EglDrawCommand {
            layer,
            visual_group: None,
            bounds: EglRect::new(x, 0.0, 20.0, 20.0),
            opaque_regions: Vec::new(),
            vertex_start: 0,
            vertex_count: 6,
            sampling: SurfaceSampling::ExactNearest,
        };
        let commands = vec![
            command(EglDrawLayer::Surface(1), 0.0),
            command(EglDrawLayer::Surface(2), 60.0),
        ];

        let plan = plan_effect_surface_consumers(
            &graph,
            &demand,
            &selection,
            &commands,
            &[OutputRect::new(0, 0, 20, 20)],
            (100, 100),
        );

        assert!(plan.surface_ids().contains(&1));
        assert!(!plan.surface_ids().contains(&2));
    }

    #[test]
    fn effect_surface_consumer_plan_framebuffer_scene_work_includes_upper_surface() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let output = GraphTextureId::new(1).unwrap();
        let capture_domain = oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap();
        let mut pass = test_pass(
            1,
            RenderPassKind::SceneCapture,
            instance,
            Vec::new(),
            output,
            Vec::new(),
        );
        pass.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(2);
        pass.anchor_scope = oblivion_one::compositor::EffectAnchorScope::Surface;
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![test_texture(
                1,
                GraphTextureSource::CapturedScene,
                capture_domain,
            )],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(capture_domain),
                capture_region: EffectRegion::from_rect(capture_domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let demand = EffectExecutionDemand::new(
            vec![oblivion_one::effects::EffectInstanceExecutionDemand {
                id: instance,
                output_region: EffectRegion::from_rect(capture_domain),
            }],
            EffectRegion::from_rect(capture_domain),
        );
        let selection = select_effect_execution(&graph, &demand);
        let command = |layer, x, width| EglDrawCommand {
            layer,
            visual_group: None,
            bounds: EglRect::new(x, 0.0, width, 20.0),
            opaque_regions: Vec::new(),
            vertex_start: 0,
            vertex_count: 6,
            sampling: SurfaceSampling::ExactNearest,
        };
        let commands = vec![
            command(EglDrawLayer::Surface(1), 0.0, 20.0),
            command(EglDrawLayer::Surface(2), 60.0, 20.0),
        ];
        let config = super::super::trace::EffectDebugConfig::new(
            super::super::trace::EffectDebugCaptureMode::Framebuffer,
            super::super::trace::EffectDebugKawaseMode::Partial,
        );

        let plan = plan_effect_surface_consumers_with_debug_config(
            &graph,
            &demand,
            &selection,
            &commands,
            &[OutputRect::new(0, 0, 4, 4)],
            (100, 100),
            config,
        );

        assert!(plan.surface_ids().contains(&1));
        assert!(plan.surface_ids().contains(&2));
    }

    #[test]
    fn effect_surface_consumer_plan_replay_checkpoint_uses_checkpoint_scene_work() {
        let instance = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let output = GraphTextureId::new(1).unwrap();
        let capture_domain = oblivion_one::effects::EffectRect::new(0, 0, 100, 100).unwrap();
        let pass = test_pass(
            1,
            RenderPassKind::SceneCapture,
            instance,
            Vec::new(),
            output,
            vec![GraphPassId::new(9).unwrap()],
        );
        let graph = CompiledFrameGraph {
            passes: vec![pass.clone()],
            textures: vec![test_texture(
                1,
                GraphTextureSource::CapturedScene,
                capture_domain,
            )],
            instances: vec![oblivion_one::effects::CompiledEffectInstance {
                id: instance,
                output_influence_region: EffectRegion::from_rect(capture_domain),
                capture_region: EffectRegion::from_rect(capture_domain),
                dependencies: Vec::new(),
            }],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let demand = EffectExecutionDemand::new(
            vec![oblivion_one::effects::EffectInstanceExecutionDemand {
                id: instance,
                output_region: EffectRegion::from_rect(capture_domain),
            }],
            EffectRegion::from_rect(capture_domain),
        );
        let selection = select_effect_execution(&graph, &demand);
        let command = |layer, x| EglDrawCommand {
            layer,
            visual_group: None,
            bounds: EglRect::new(x, 0.0, 20.0, 20.0),
            opaque_regions: Vec::new(),
            vertex_start: 0,
            vertex_count: 6,
            sampling: SurfaceSampling::ExactNearest,
        };
        let commands = vec![
            command(EglDrawLayer::Surface(1), 0.0),
            command(EglDrawLayer::Surface(2), 60.0),
        ];
        let plan = plan_effect_surface_consumers_with_debug_config(
            &graph,
            &demand,
            &selection,
            &commands,
            &[OutputRect::new(0, 0, 4, 4)],
            (100, 100),
            EffectDebugConfig::new(
                EffectDebugCaptureMode::Replay,
                EffectDebugKawaseMode::Partial,
            ),
        );

        assert!(plan.surface_ids().contains(&1));
        assert!(plan.surface_ids().contains(&2));
    }

    #[test]
    fn scene_work_preservation_plan_counts_dock_pixels_for_both_origins() {
        let extra_scene_work = [OutputRect::new(762, 976, 396, 104)];

        for framebuffer_origin in [
            OutputFramebufferOrigin::BottomLeft,
            OutputFramebufferOrigin::TopLeftScanout,
        ] {
            let plan = SceneWorkPreservationPlan::from_extra_scene_work(
                &extra_scene_work,
                (1920, 1080),
                framebuffer_origin,
            );

            assert_eq!(plan.transfers.len(), 1);
            assert_eq!(plan.pixels, 41_184);
        }
    }

    #[test]
    fn scene_work_preservation_plan_counts_topbar_pixels() {
        let plan = SceneWorkPreservationPlan::from_extra_scene_work(
            &[OutputRect::new(0, 0, 120, 65)],
            (1920, 1080),
            OutputFramebufferOrigin::TopLeftScanout,
        );

        assert_eq!(plan.transfers.len(), 1);
        assert_eq!(plan.pixels, 7_800);
    }

    #[test]
    fn scene_work_preservation_plan_keeps_disjoint_rectangles_separate() {
        let plan = SceneWorkPreservationPlan::from_extra_scene_work(
            &[
                OutputRect::new(762, 976, 396, 104),
                OutputRect::new(0, 0, 120, 65),
            ],
            (1920, 1080),
            OutputFramebufferOrigin::BottomLeft,
        );

        assert_eq!(plan.transfers.len(), 2);
        assert_eq!(plan.pixels, 48_984);
    }

    #[test]
    fn scene_work_preservation_plan_counts_only_clipped_pixels() {
        let plan = SceneWorkPreservationPlan::from_extra_scene_work(
            &[OutputRect::new(-10, -5, 30, 20)],
            (100, 80),
            OutputFramebufferOrigin::BottomLeft,
        );

        assert_eq!(plan.transfers.len(), 1);
        assert_eq!(plan.pixels, 300);
    }

    #[test]
    fn scene_work_preservation_plan_represents_empty_work_without_transfers() {
        let plan = SceneWorkPreservationPlan::from_extra_scene_work(
            &[],
            (1920, 1080),
            OutputFramebufferOrigin::BottomLeft,
        );

        assert!(plan.transfers.is_empty());
        assert_eq!(plan.pixels, 0);
    }

    #[test]
    fn scene_work_preservation_maps_framebuffer_origins() {
        let rect = OutputRect::new(10, 20, 30, 40);
        let bottom_left = scene_work_preservation_blit_rects(
            rect,
            (100, 80),
            OutputFramebufferOrigin::BottomLeft,
        )
        .expect("bottom-left preservation rects");
        assert_eq!(bottom_left.source, GlBlitRect::new(10, 20, 40, 60));
        assert_eq!(bottom_left.destination, GlBlitRect::new(10, 20, 40, 60));

        let top_left = scene_work_preservation_blit_rects(
            rect,
            (100, 80),
            OutputFramebufferOrigin::TopLeftScanout,
        )
        .expect("top-left preservation rects");
        assert_eq!(top_left.source, GlBlitRect::new(10, 20, 40, 60));
        assert_eq!(top_left.destination, GlBlitRect::new(10, 60, 40, 20));
    }

    #[test]
    fn effect_selection_matches_the_final_converged_repair() {
        use crate::egl_renderer::damage::{
            EglPartialRepaintCapabilities, OutputDamage, PartialRepaintPlanner, RepaintMode,
            RepaintPlan, resolve_effect_execution_for_repaint_plan,
        };

        let first = oblivion_one::effects::EffectInstanceId::new(1).unwrap();
        let second = oblivion_one::effects::EffectInstanceId::new(2).unwrap();
        let third = oblivion_one::effects::EffectInstanceId::new(3).unwrap();
        let unrelated = oblivion_one::effects::EffectInstanceId::new(4).unwrap();
        let pass = |id, instance, input, output| CompiledRenderPass {
            id: GraphPassId::new(id).unwrap(),
            kind: RenderPassKind::Fragment,
            inputs: vec![input],
            output: Some(output),
            damage: EffectRegion::empty(),
            instance,
            anchor: oblivion_one::compositor::EffectAnchor::OutputPostProcess,
            blur_radius: None,
            stage: None,
            fused_stages: Vec::new(),
            parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
            alpha_mode: oblivion_one::effects::EffectAlphaMode::Preserve,
            encode_output: false,
            color_conversion: EffectColorConversion::None,
            checkpoint_dependencies: Vec::new(),
            visual_group: None,
            anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
            visible_clip_fallback: None,
        };
        let texture = |id| oblivion_one::effects::GraphTexturePlan {
            id: GraphTextureId::new(id).unwrap(),
            source: GraphTextureSource::Intermediate,
            width: 10,
            height: 10,
            domain: oblivion_one::effects::EffectRect::new(0, 0, 10, 10).unwrap(),
            working_space: oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
            origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
            first_use: None,
            last_use: None,
        };
        let instance = |id, output_x, capture_x, capture_width, dependencies| {
            oblivion_one::effects::CompiledEffectInstance {
                id,
                output_influence_region: EffectRegion::from_rect(
                    oblivion_one::effects::EffectRect::new(output_x, 0, 10, 10).unwrap(),
                ),
                capture_region: EffectRegion::from_rect(
                    oblivion_one::effects::EffectRect::new(capture_x, 0, capture_width, 10)
                        .unwrap(),
                ),
                dependencies,
            }
        };
        let graph = CompiledFrameGraph {
            passes: vec![
                pass(
                    1,
                    first,
                    GraphTextureId::new(1).unwrap(),
                    GraphTextureId::new(2).unwrap(),
                ),
                pass(
                    2,
                    second,
                    GraphTextureId::new(3).unwrap(),
                    GraphTextureId::new(4).unwrap(),
                ),
                pass(
                    3,
                    third,
                    GraphTextureId::new(5).unwrap(),
                    GraphTextureId::new(6).unwrap(),
                ),
                pass(
                    4,
                    unrelated,
                    GraphTextureId::new(7).unwrap(),
                    GraphTextureId::new(8).unwrap(),
                ),
            ],
            textures: (1..=8).map(texture).collect(),
            instances: vec![
                instance(first, 10, 10, 10, Vec::new()),
                instance(second, 30, 10, 40, vec![first]),
                instance(third, 45, 45, 25, vec![second]),
                instance(unrelated, 80, 80, 10, Vec::new()),
            ],
            final_damage: EffectRegion::empty(),
            stats: Default::default(),
        };
        let planner = PartialRepaintPlanner::new(
            (100, 80),
            EglPartialRepaintCapabilities {
                buffer_age: true,
                partial_render_repair: true,
                swap_buffers_with_damage: true,
            },
        );
        let initial_damage = OutputDamage::rects(100, 80, [OutputRect::new(30, 0, 10, 10)]);
        let mut repaint_plan = RepaintPlan {
            render_damage: initial_damage.clone(),
            repair_damage: initial_damage,
            buffer_age: Some(2),
            mode: RepaintMode::Partial,
            fallback_reason: None,
        };
        let demand =
            resolve_effect_execution_for_repaint_plan(&planner, &graph, &mut repaint_plan, 100, 80);

        let selection = select_effect_execution(&graph, &demand);

        assert_eq!(selection.executed_instances, vec![first, second, third]);
        assert_eq!(
            selection.executed_passes,
            vec![
                GraphPassId::new(1).unwrap(),
                GraphPassId::new(2).unwrap(),
                GraphPassId::new(3).unwrap(),
            ]
        );
        assert!(!selection.executed_instances.contains(&unrelated));
        assert_eq!(repaint_plan.mode, RepaintMode::Partial);
    }
}
