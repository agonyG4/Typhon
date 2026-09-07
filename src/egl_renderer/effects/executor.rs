use std::io;

use glow::HasContext;
use oblivion_one::effects::{
    CompiledFrameGraph, CompiledRenderPass, EffectNodeKind, EffectRegion, GraphTextureId,
    GraphTextureSource, RenderPassKind, ShaderModuleId,
};

use super::super::geometry::EglDrawLayer;
use super::super::{GlesSceneRenderer, OutputFramebufferOrigin, OutputRect, RendererResult};
use super::{
    blur, capture,
    resources::{PooledEffectTexture, release_dead_graph_textures},
    shader_cache::ShaderProgramKey,
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

pub(super) const COMPOSITE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform vec4 u_effect_input_domain;
uniform vec2 u_effect_output_size;
in vec2 v_uv;
out vec4 out_color;

vec3 typhon_linear_to_srgb(vec3 value) {
    return mix(value * 12.92, 1.055 * pow(value, vec3(1.0 / 2.4)) - 0.055, step(vec3(0.0031308), value));
}

void main() {
    vec2 output_position = v_uv * u_effect_output_size;
    vec2 input_uv = (output_position - u_effect_input_domain.xy) /
        u_effect_input_domain.zw;
    if (any(lessThan(input_uv, vec2(0.0))) || any(greaterThan(input_uv, vec2(1.0)))) {
        discard;
    }
    vec4 result = texture(u_effect_input, input_uv);
    result.rgb = typhon_linear_to_srgb(result.rgb);
    out_color = result;
}
"#;

pub(super) const FRAGMENT_STAGE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform int u_effect_decode_srgb;
uniform mat4 u_effect_color_matrix;
uniform vec4 u_effect_color_bias;
uniform vec4 u_effect_tint_color;
uniform float u_effect_tint_amount;
uniform float u_effect_noise_amount;
in vec2 v_uv;
out vec4 out_color;

vec3 typhon_srgb_to_linear(vec3 value) {
    return mix(value / 12.92, pow((value + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), value));
}

void main() {
    vec4 result = texture(u_effect_input, v_uv);
    if (u_effect_decode_srgb != 0) result.rgb = typhon_srgb_to_linear(result.rgb);
    result = u_effect_color_matrix * result + u_effect_color_bias;
    result.rgb = mix(result.rgb, result.rgb * u_effect_tint_color.rgb, clamp(u_effect_tint_amount, 0.0, 1.0));
    float noise = fract(sin(dot(v_uv, vec2(12.9898, 78.233))) * 43758.5453) - 0.5;
    result.rgb += noise * u_effect_noise_amount;
    out_color = result;
}
"#;

pub(super) const MASK_STAGE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform int u_effect_decode_srgb;
uniform int u_effect_inverted;
in vec2 v_uv;
out vec4 out_color;

vec3 typhon_srgb_to_linear(vec3 value) {
    return mix(value / 12.92, pow((value + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), value));
}

void main() {
    vec4 result = texture(u_effect_input, v_uv);
    if (u_effect_decode_srgb != 0) result.rgb = typhon_srgb_to_linear(result.rgb);
    result.a = u_effect_inverted != 0 ? 1.0 - result.a : result.a;
    out_color = result;
}
"#;

pub(super) const BLEND_STAGE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
uniform sampler2D u_effect_input_secondary;
uniform int u_effect_decode_srgb;
uniform int u_effect_blend_mode;
uniform float u_effect_blend_opacity;
in vec2 v_uv;
out vec4 out_color;

vec3 typhon_srgb_to_linear(vec3 value) {
    return mix(value / 12.92, pow((value + 0.055) / 1.055, vec3(2.4)), step(vec3(0.04045), value));
}

void main() {
    vec4 first = texture(u_effect_input, v_uv);
    vec4 second = texture(u_effect_input_secondary, v_uv);
    if (u_effect_decode_srgb != 0) { first.rgb = typhon_srgb_to_linear(first.rgb); second.rgb = typhon_srgb_to_linear(second.rgb); }
    float opacity = clamp(u_effect_blend_opacity, 0.0, 1.0);
    float second_alpha = clamp(second.a * opacity, 0.0, 1.0);
    vec3 blended = second.rgb;
    if (u_effect_blend_mode == 1) blended = first.rgb + second.rgb;
    else if (u_effect_blend_mode == 2) blended = first.rgb * second.rgb;
    else if (u_effect_blend_mode == 3) blended = 1.0 - (1.0 - first.rgb) * (1.0 - second.rgb);
    vec3 rgb = u_effect_blend_mode == 0
        ? second.rgb * opacity + first.rgb * (1.0 - second_alpha)
        : first.rgb * (1.0 - second_alpha) + blended * second_alpha;
    float alpha = second_alpha + first.a * (1.0 - second_alpha);
    out_color = vec4(rgb, alpha);
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
    let mut composite_passes = Vec::new();
    for pass in &graph.passes {
        if matches!(
            pass.kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        ) {
            composite_passes.push(pass);
            continue;
        }
        ensure_pass_textures(renderer, graph, pass, textures)?;
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

    let layers = renderer
        .commands
        .iter()
        .map(|command| match command.layer {
            EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
            _ => capture::CaptureLayer::Other,
        })
        .collect::<Vec<_>>();
    composite_passes
        .sort_by_key(|pass| (composition_position(&layers, pass.anchor), pass.id.get()));
    let repaint_rects = renderer.begin_effect_repaint(repaint_plan, framebuffer_origin)?;
    let mut scene_cursor = 0;
    for pass in composite_passes {
        let position = composition_position(&layers, pass.anchor);
        let (draw_end, next_cursor) = match pass.anchor {
            oblivion_one::compositor::EffectAnchor::BeforeSurface(_) => (position, position),
            oblivion_one::compositor::EffectAnchor::ReplaceSurface(_) => {
                (position, position.saturating_add(1))
            }
            oblivion_one::compositor::EffectAnchor::AfterSurface(_) => (position, position),
            oblivion_one::compositor::EffectAnchor::OutputPostProcess => {
                (renderer.commands.len(), renderer.commands.len())
            }
        };
        renderer.draw_effect_scene_range(
            &repaint_rects,
            scene_cursor,
            draw_end,
            framebuffer_origin,
        )?;
        ensure_pass_textures(renderer, graph, pass, textures)?;
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
        scene_cursor = next_cursor.max(scene_cursor);
    }
    renderer.draw_effect_scene_range(
        &repaint_rects,
        scene_cursor,
        renderer.commands.len(),
        framebuffer_origin,
    )?;
    renderer.draw_effect_overlays(&repaint_rects, framebuffer_origin)?;
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
            let fragment = match pass.kind {
                RenderPassKind::DualKawaseDownsample if first_downsample => {
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
        RenderPassKind::Fragment | RenderPassKind::Blend | RenderPassKind::Mask => {
            let Some(stage) = pass.stage.as_ref() else {
                return Err(io::Error::other("effect stage has no lowering metadata").into());
            };
            let (module, fragment) = match stage {
                EffectNodeKind::Mask(_) => (1006, MASK_STAGE_FRAGMENT_SHADER),
                EffectNodeKind::Blend(_) => (1007, BLEND_STAGE_FRAGMENT_SHADER),
                EffectNodeKind::CustomFragment(spec) => (spec.shader.get(), ""),
                EffectNodeKind::ColorMatrix(_)
                | EffectNodeKind::Tint(_)
                | EffectNodeKind::Noise(_) => (1005, FRAGMENT_STAGE_FRAGMENT_SHADER),
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
        renderer.gl.use_program(Some(program));
        if let Some(location) = renderer
            .gl
            .get_uniform_location(program, "u_effect_origin_bottom_left")
        {
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
        if let Some(location) = renderer.gl.get_uniform_location(program, "u_effect_input") {
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
            if let Some(location) = renderer.gl.get_uniform_location(program, name) {
                renderer.gl.uniform_1_i32(Some(&location), unit as i32);
            }
        }
        if let Some(location) = renderer
            .gl
            .get_uniform_location(program, "u_effect_decode_srgb")
        {
            renderer.gl.uniform_1_i32(
                Some(&location),
                i32::from(
                    input_plan.working_space
                        == oblivion_one::effects::EffectWorkingSpace::OutputEncodedSrgb,
                ),
            );
        }
        set_stage_uniforms(
            &renderer.gl,
            program,
            stage,
            &pass.parameter_block,
            input_plan,
            &pass.fused_stages,
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
    }
    renderer.effect_resources.unbind_render_target(&renderer.gl);
    renderer.bind_active_output_framebuffer();
    restore_output_viewport(renderer);
    let _ = fragment_shader;
    Ok(())
}

fn set_stage_uniforms(
    gl: &glow::Context,
    program: glow::NativeProgram,
    stage: &EffectNodeKind,
    parameters: &oblivion_one::effects::EffectParameterBlock,
    input_plan: &oblivion_one::effects::GraphTexturePlan,
    fused_stages: &[EffectNodeKind],
) {
    unsafe {
        if matches!(
            stage,
            EffectNodeKind::ColorMatrix(_) | EffectNodeKind::Tint(_) | EffectNodeKind::Noise(_)
        ) {
            set_identity_color_matrix(gl, program);
            if let Some(location) = gl.get_uniform_location(program, "u_effect_tint_color") {
                gl.uniform_4_f32(Some(&location), 1.0, 1.0, 1.0, 1.0);
            }
            if let Some(location) = gl.get_uniform_location(program, "u_effect_tint_amount") {
                gl.uniform_1_f32(Some(&location), 0.0);
            }
            if let Some(location) = gl.get_uniform_location(program, "u_effect_noise_amount") {
                gl.uniform_1_f32(Some(&location), 0.0);
            }
            for stage in std::iter::once(stage).chain(fused_stages.iter()) {
                match stage {
                    EffectNodeKind::ColorMatrix(spec) => {
                        if let Some(location) =
                            gl.get_uniform_location(program, "u_effect_color_matrix")
                        {
                            gl.uniform_matrix_4_f32_slice(Some(&location), false, &spec.matrix);
                        }
                        if let Some(location) =
                            gl.get_uniform_location(program, "u_effect_color_bias")
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
                        if let Some(location) =
                            gl.get_uniform_location(program, "u_effect_tint_color")
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
                            gl.get_uniform_location(program, "u_effect_tint_amount")
                        {
                            gl.uniform_1_f32(Some(&location), spec.amount);
                        }
                    }
                    EffectNodeKind::Noise(spec) => {
                        if let Some(location) =
                            gl.get_uniform_location(program, "u_effect_noise_amount")
                        {
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
                if let Some(location) = gl.get_uniform_location(program, "u_effect_color_matrix") {
                    gl.uniform_matrix_4_f32_slice(Some(&location), false, &spec.matrix);
                }
                if let Some(location) = gl.get_uniform_location(program, "u_effect_color_bias") {
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
                set_identity_color_matrix(gl, program);
                if let Some(location) = gl.get_uniform_location(program, "u_effect_tint_color") {
                    gl.uniform_4_f32(
                        Some(&location),
                        spec.color[0],
                        spec.color[1],
                        spec.color[2],
                        spec.color[3],
                    );
                }
                if let Some(location) = gl.get_uniform_location(program, "u_effect_tint_amount") {
                    gl.uniform_1_f32(Some(&location), spec.amount);
                }
            }
            EffectNodeKind::Noise(spec) => {
                set_identity_color_matrix(gl, program);
                if let Some(location) = gl.get_uniform_location(program, "u_effect_noise_amount") {
                    gl.uniform_1_f32(Some(&location), spec.amount);
                }
            }
            EffectNodeKind::Mask(spec) => {
                if let Some(location) = gl.get_uniform_location(program, "u_effect_inverted") {
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
                if let Some(location) = gl.get_uniform_location(program, "u_effect_blend_mode") {
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
                if let Some(location) = gl.get_uniform_location(program, "u_effect_blend_opacity") {
                    gl.uniform_1_f32(Some(&location), spec.opacity);
                }
            }
            EffectNodeKind::CustomFragment(spec) => {
                if let Some(location) = gl.get_uniform_location(program, "u_typhon_primary") {
                    gl.uniform_1_i32(Some(&location), 0);
                }
                if let Some(location) = gl.get_uniform_location(program, "u_typhon_texture_size") {
                    gl.uniform_2_f32(
                        Some(&location),
                        input_plan.width.max(1) as f32,
                        input_plan.height.max(1) as f32,
                    );
                }
                if let Some(location) = gl.get_uniform_location(program, "u_typhon_content_rect") {
                    gl.uniform_4_f32(
                        Some(&location),
                        input_plan.domain.x as f32,
                        input_plan.domain.y as f32,
                        input_plan.domain.width.max(1) as f32,
                        input_plan.domain.height.max(1) as f32,
                    );
                }
                if let Some(location) = gl.get_uniform_location(program, "u_typhon_output_size") {
                    gl.uniform_2_f32(
                        Some(&location),
                        input_plan.domain.width.max(1) as f32,
                        input_plan.domain.height.max(1) as f32,
                    );
                }
                if let Some(location) = gl.get_uniform_location(program, "u_typhon_scale") {
                    gl.uniform_1_f32(Some(&location), 1.0);
                }
                if let Some(location) = gl.get_uniform_location(program, "u_typhon_time") {
                    gl.uniform_1_f32(Some(&location), 0.0);
                }
                if let Some(location) = gl.get_uniform_location(program, "u_typhon_delta") {
                    gl.uniform_1_f32(Some(&location), 0.0);
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
                    let Some(location) = gl.get_uniform_location(program, &binding.shader_name)
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

fn set_identity_color_matrix(gl: &glow::Context, program: glow::NativeProgram) {
    unsafe {
        if let Some(location) = gl.get_uniform_location(program, "u_effect_color_matrix") {
            gl.uniform_matrix_4_f32_slice(
                Some(&location),
                false,
                &[
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                ],
            );
        }
        if let Some(location) = gl.get_uniform_location(program, "u_effect_color_bias") {
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
    renderer
        .effect_resources
        .bind_render_target(&renderer.gl, target)?;
    unsafe {
        renderer
            .gl
            .viewport(0, 0, target_plan.width as i32, target_plan.height as i32);
        renderer.gl.disable(glow::SCISSOR_TEST);
        renderer.gl.clear_color(0.0, 0.0, 0.0, 0.0);
        renderer.gl.clear(glow::COLOR_BUFFER_BIT);
        renderer.gl.use_program(Some(renderer.capture_program));
        if let Some(location) = renderer
            .gl
            .get_uniform_location(renderer.capture_program, "u_capture_output_size")
        {
            renderer.gl.uniform_2_f32(
                Some(&location),
                renderer.current_size.0.max(1) as f32,
                renderer.current_size.1.max(1) as f32,
            );
        }
        if let Some(location) = renderer
            .gl
            .get_uniform_location(renderer.capture_program, "u_capture_domain")
        {
            renderer.gl.uniform_4_f32(
                Some(&location),
                target_plan.domain.x as f32,
                target_plan.domain.y as f32,
                target_plan.domain.width.max(1) as f32,
                target_plan.domain.height.max(1) as f32,
            );
        }
        if let Some(location) = renderer
            .gl
            .get_uniform_location(renderer.capture_program, "u_capture_origin_bottom_left")
        {
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
    let indices = capture::indices_for_capture(
        &layers,
        pass.anchor,
        pass.kind == RenderPassKind::SurfaceCapture,
    );
    let scissor = pass.damage.bounding_rect().map_or_else(
        || full_output_rect(renderer.current_size),
        effect_rect_to_output_rect,
    );
    renderer.draw_capture_commands(&indices, scissor)?;
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
            RenderPassKind::DualKawaseDownsample => 1001,
            RenderPassKind::DualKawaseUpsample => 1002,
            _ => 1000,
        }
    } else if output_is_framebuffer {
        1004
    } else {
        1003
    };
    let shader_key = ShaderProgramKey::new(
        ShaderModuleId::new(module).expect("static effect shader ids are non-zero"),
        if pass.kind == RenderPassKind::DualKawaseDownsample
            && fragment_shader == blur::DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER
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
        if let Some(location) = renderer
            .gl
            .get_uniform_location(program, "u_effect_origin_bottom_left")
        {
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
        if let Some(location) = renderer.gl.get_uniform_location(program, "u_effect_input") {
            renderer.gl.uniform_1_i32(Some(&location), 0);
        }
        if blur_shader
            && let Some(location) = renderer
                .gl
                .get_uniform_location(program, "u_effect_texel_size")
        {
            renderer.gl.uniform_2_f32(
                Some(&location),
                1.0 / input_plan.width.max(1) as f32,
                1.0 / input_plan.height.max(1) as f32,
            );
        }
        if blur_shader
            && let Some(location) = renderer
                .gl
                .get_uniform_location(program, "u_effect_blur_radius")
        {
            renderer
                .gl
                .uniform_1_f32(Some(&location), pass.blur_radius.unwrap_or(1.0));
        }
        if !blur_shader
            && output_is_framebuffer
            && let Some(location) = renderer
                .gl
                .get_uniform_location(program, "u_effect_input_domain")
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
            && output_is_framebuffer
            && let Some(location) = renderer
                .gl
                .get_uniform_location(program, "u_effect_output_size")
        {
            renderer.gl.uniform_2_f32(
                Some(&location),
                output_plan.domain.width.max(1) as f32,
                output_plan.domain.height.max(1) as f32,
            );
        }
        renderer.gl.bind_vertex_array(Some(vertex_array));
        draw_damage_scissors(&renderer.gl, &pass.damage, output_plan, framebuffer_origin);
        renderer.gl.bind_vertex_array(None);
        renderer.gl.bind_texture(glow::TEXTURE_2D, None);
        renderer.gl.disable(glow::SCISSOR_TEST);
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
    fn full_output_fallback_is_bounded_to_renderer_size() {
        assert_eq!(
            full_output_rect((1920, 1080)),
            OutputRect::new(0, 0, 1920, 1080)
        );
    }
}
