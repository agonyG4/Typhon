use std::io;

use glow::HasContext;
use oblivion_one::effects::{
    CompiledFrameGraph, CompiledRenderPass, EffectColorConversion, EffectNodeKind, EffectRegion,
    GraphTextureId, GraphTextureSource, INTERNAL_EFFECT_SHADER_MODULE_BLEND,
    INTERNAL_EFFECT_SHADER_MODULE_FRAGMENT, INTERNAL_EFFECT_SHADER_MODULE_MASK, RenderPassKind,
    ShaderModuleId,
};

use super::super::geometry::EglDrawLayer;
use super::super::{GlesSceneRenderer, OutputFramebufferOrigin, OutputRect, RendererResult};
use super::{
    blur, capture,
    resources::{PooledEffectTexture, release_dead_graph_textures},
    shader_cache::{ShaderProgramCache, ShaderProgramKey},
};

pub(super) const COPY_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
in vec2 v_uv;
out vec4 out_color;

void main() {
    out_color = texture(u_effect_input, v_uv);
}
"#;

pub(crate) const NORMALIZE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec4 u_effect_input_domain;
uniform vec4 u_effect_output_domain;
uniform int u_effect_decode_srgb;
uniform int u_effect_encode_srgb;
in vec2 v_uv;
out vec4 out_color;

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
    vec2 input_uv = (output_position - u_effect_input_domain.xy) /
        u_effect_input_domain.zw;
    if (any(lessThan(input_uv, vec2(0.0))) || any(greaterThan(input_uv, vec2(1.0)))) {
        out_color = vec4(0.0);
        return;
    }
    vec4 result = texture(u_effect_input, input_uv);
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
in vec2 v_uv;
out vec4 out_color;

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
    vec2 input_uv = (output_position - u_effect_input_domain.xy) /
        u_effect_input_domain.zw;
    if (any(lessThan(input_uv, vec2(0.0))) || any(greaterThan(input_uv, vec2(1.0)))) {
        discard;
    }
    vec4 result = texture(u_effect_input, input_uv);
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
in vec2 v_uv;
out vec4 out_color;

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
    vec4 result = texture(u_effect_input, v_uv);
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
in vec2 v_uv;
out vec4 out_color;

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
    vec4 result = texture(u_effect_input, v_uv);
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
in vec2 v_uv;
out vec4 out_color;

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
    vec4 first = texture(u_effect_input, v_uv);
    vec4 second = texture(u_effect_input_secondary, v_uv);
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
    pub passes: usize,
    pub scene_captures: usize,
    pub blur_downsamples: usize,
    pub blur_upsamples: usize,
    pub composites: usize,
}

pub(crate) fn execute_effect_graph(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: &super::super::damage::RepaintPlan,
) -> RendererResult<EffectExecutionStats> {
    let mut textures = std::collections::HashMap::new();
    let result = execute_graph_passes(
        renderer,
        graph,
        &mut textures,
        framebuffer_origin,
        repaint_plan,
    );
    if result.is_err() {
        renderer.establish_ordinary_scene_state();
        unsafe { renderer.gl.bind_texture(glow::TEXTURE_2D, None) };
    }
    let release_result = renderer.effect_resources.release_graph(textures);
    match (result, release_result) {
        (Ok(stats), Ok(())) => Ok(stats),
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
}

fn execute_graph_passes(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    textures: &mut std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    framebuffer_origin: OutputFramebufferOrigin,
    repaint_plan: &super::super::damage::RepaintPlan,
) -> RendererResult<EffectExecutionStats> {
    let mut stats = EffectExecutionStats::default();
    let repaint_rects = renderer.begin_effect_repaint(repaint_plan, framebuffer_origin)?;
    let mut scene_cursor = 0;
    for pass in &graph.passes {
        ensure_pass_textures(renderer, graph, pass, textures)?;
        if matches!(
            pass.kind,
            RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
        ) && !pass.checkpoint_dependencies.is_empty()
        {
            let (draw_end, _) = composition_range(
                &renderer.commands,
                pass.anchor,
                pass.visual_group,
                pass.anchor_scope,
            );
            if draw_end > scene_cursor {
                renderer.draw_effect_scene_range(
                    &repaint_rects,
                    scene_cursor,
                    draw_end,
                    framebuffer_origin,
                )?;
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
            renderer.draw_effect_scene_range(
                &repaint_rects,
                scene_cursor,
                draw_end,
                framebuffer_origin,
            )?;
            scene_cursor = next_cursor.max(scene_cursor);
        }
        execute_pass(
            renderer,
            graph,
            textures,
            pass,
            framebuffer_origin,
            &mut stats,
        )?;
        release_dead_graph_textures(&mut renderer.effect_resources, graph, pass.id, textures)?;
        stats.passes = stats.passes.saturating_add(1);
    }
    renderer.draw_effect_scene_range(
        &repaint_rects,
        scene_cursor,
        renderer.commands.len(),
        framebuffer_origin,
    )?;
    renderer.draw_effect_overlays(&repaint_rects, framebuffer_origin)?;
    renderer.establish_ordinary_scene_state();
    Ok(stats)
}

fn ensure_pass_textures(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
    textures: &mut std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
) -> RendererResult<()> {
    for texture_id in pass.inputs.iter().copied().chain(pass.output) {
        let plan = graph_texture(graph, texture_id)?;
        if plan.source == GraphTextureSource::Output || textures.contains_key(&texture_id) {
            continue;
        }
        let realized = renderer.effect_resources.acquire_plan(&renderer.gl, plan)?;
        textures.insert(texture_id, realized);
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

fn execute_pass(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    textures: &std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    pass: &CompiledRenderPass,
    framebuffer_origin: OutputFramebufferOrigin,
    stats: &mut EffectExecutionStats,
) -> RendererResult<()> {
    match pass.kind {
        RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture => {
            execute_capture(renderer, graph, textures, pass, framebuffer_origin)?;
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
    unsafe {
        renderer
            .gl
            .viewport(0, 0, output_plan.width as i32, output_plan.height as i32);
        renderer.gl.disable(glow::BLEND);
        renderer.gl.use_program(Some(program));
        if let Some(location) = uniform_location(
            &mut renderer.effect_shaders,
            &renderer.gl,
            shader_key,
            program,
            "u_effect_origin_bottom_left",
        ) {
            renderer.gl.uniform_1_i32(
                Some(&location),
                i32::from(matches!(
                    input_plan.origin,
                    oblivion_one::effects::GraphTextureOrigin::BottomLeft
                )),
            );
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
        draw_damage_scissors(&renderer.gl, &pass.damage, output_plan, framebuffer_origin);
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

fn execute_capture(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    textures: &std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    pass: &CompiledRenderPass,
    framebuffer_origin: OutputFramebufferOrigin,
) -> RendererResult<()> {
    let output = pass
        .output
        .ok_or_else(|| io::Error::other("capture pass has no output texture"))?;
    let target = textures
        .get(&output)
        .ok_or_else(|| io::Error::other("capture output texture is not allocated"))?;
    let target_plan = graph_texture(graph, output)?;
    if !pass.checkpoint_dependencies.is_empty() {
        let target_texture = renderer
            .effect_resources
            .texture(target)
            .ok_or_else(|| io::Error::other("checkpoint texture was not realized"))?;
        let source_x = target_plan.domain.x.max(0);
        let source_y = match framebuffer_origin {
            OutputFramebufferOrigin::BottomLeft => target_plan.domain.y.max(0),
            OutputFramebufferOrigin::TopLeftScanout => renderer
                .current_size
                .1
                .saturating_sub(target_plan.domain.bottom().max(0) as u32)
                as i32,
        };
        renderer.bind_active_output_framebuffer();
        unsafe {
            renderer
                .gl
                .bind_texture(glow::TEXTURE_2D, Some(target_texture));
            renderer.gl.copy_tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                0,
                0,
                source_x,
                source_y,
                target_plan.width as i32,
                target_plan.height as i32,
            );
            renderer.gl.bind_texture(glow::TEXTURE_2D, None);
        }
        restore_output_viewport(renderer);
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
        renderer.gl.enable(glow::BLEND);
        renderer.gl.blend_func_separate(
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
        );
        renderer.gl.clear_color(0.0, 0.0, 0.0, 0.0);
        renderer.gl.clear(glow::COLOR_BUFFER_BIT);
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
    let scissors = if pass.damage.is_empty() {
        vec![full_output_rect(renderer.current_size)]
    } else {
        pass.damage
            .rects()
            .iter()
            .map(|rect| effect_rect_to_output_rect(*rect))
            .collect::<Vec<_>>()
    };
    renderer.draw_capture_commands_for_regions(
        &indices,
        &scissors,
        target_plan.domain,
        (target_plan.width, target_plan.height),
    )?;
    renderer.effect_resources.unbind_render_target(&renderer.gl);
    renderer.bind_active_output_framebuffer();
    restore_output_viewport(renderer);
    Ok(())
}

fn execute_fullscreen_pass(
    renderer: &mut GlesSceneRenderer,
    graph: &CompiledFrameGraph,
    textures: &std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    pass: &CompiledRenderPass,
    fragment_shader: &str,
    framebuffer_origin: OutputFramebufferOrigin,
    blur_shader: bool,
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
    unsafe {
        renderer.gl.use_program(Some(program));
        if let Some(location) = uniform_location(
            &mut renderer.effect_shaders,
            &renderer.gl,
            shader_key,
            program,
            "u_effect_origin_bottom_left",
        ) {
            renderer.gl.uniform_1_i32(
                Some(&location),
                i32::from(
                    !output_is_framebuffer
                        || framebuffer_origin == OutputFramebufferOrigin::BottomLeft,
                ),
            );
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
        draw_damage_scissors(&renderer.gl, &pass.damage, output_plan, framebuffer_origin);
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

fn effect_rect_to_texture_rect(
    rect: oblivion_one::effects::EffectRect,
    target: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<OutputRect> {
    let domain = target.domain;
    let left = i64::from(rect.x).max(i64::from(domain.x));
    let top = i64::from(rect.y).max(i64::from(domain.y));
    let right = i64::from(rect.right()).min(i64::from(domain.right()));
    let bottom = i64::from(rect.bottom()).min(i64::from(domain.bottom()));
    if right <= left || bottom <= top {
        return None;
    }
    let domain_width = i64::from(domain.width).max(1);
    let domain_height = i64::from(domain.height).max(1);
    let physical_width = i64::from(target.width.max(1));
    let physical_height = i64::from(target.height.max(1));
    let local_left = ((left - i64::from(domain.x)) * physical_width) / domain_width;
    let local_top = ((top - i64::from(domain.y)) * physical_height) / domain_height;
    let local_right =
        ((right - i64::from(domain.x)) * physical_width + domain_width - 1) / domain_width;
    let local_bottom =
        ((bottom - i64::from(domain.y)) * physical_height + domain_height - 1) / domain_height;
    let width = local_right.saturating_sub(local_left).max(1);
    let height = local_bottom.saturating_sub(local_top).max(1);
    let y = match target.origin {
        oblivion_one::effects::GraphTextureOrigin::BottomLeft => match target.source {
            GraphTextureSource::Output => match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => physical_height.saturating_sub(local_bottom),
                OutputFramebufferOrigin::TopLeftScanout => local_top,
            },
            _ => physical_height.saturating_sub(local_bottom),
        },
    };
    Some(OutputRect::new(
        local_left as i32,
        y as i32,
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
        let rect = effect_rect_to_texture_rect(
            oblivion_one::effects::EffectRect::new(110, 60, 20, 20).unwrap(),
            &target,
            OutputFramebufferOrigin::TopLeftScanout,
        )
        .unwrap();
        assert_eq!(rect, OutputRect::new(5, 35, 10, 10));
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
