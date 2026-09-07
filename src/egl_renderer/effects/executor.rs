use std::io;

use glow::HasContext;
use oblivion_one::effects::{
    CompiledFrameGraph, CompiledRenderPass, EffectRegion, GraphTextureId, GraphTextureSource,
    RenderPassKind, ShaderModuleId,
};

use super::super::geometry::EglDrawLayer;
use super::super::{GlesSceneRenderer, OutputFramebufferOrigin, OutputRect, RendererResult};
use super::{blur, capture, resources::PooledEffectTexture, shader_cache::ShaderProgramKey};

const COPY_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
in vec2 v_uv;
out vec4 out_color;

void main() {
    out_color = texture(u_effect_input, v_uv);
}
"#;

const COMPOSITE_FRAGMENT_SHADER: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_effect_input;
in vec2 v_uv;
out vec4 out_color;

vec3 typhon_linear_to_srgb(vec3 value) {
    return mix(value * 12.92, 1.055 * pow(value, vec3(1.0 / 2.4)) - 0.055, step(vec3(0.0031308), value));
}

void main() {
    vec4 result = texture(u_effect_input, v_uv);
    result.rgb = typhon_linear_to_srgb(result.rgb);
    out_color = result;
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
) -> RendererResult<EffectExecutionStats> {
    let textures = renderer
        .effect_resources
        .acquire_graph(&renderer.gl, graph)?;
    let result = execute_graph_passes(renderer, graph, &textures, framebuffer_origin);
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
    textures: &std::collections::HashMap<GraphTextureId, PooledEffectTexture>,
    framebuffer_origin: OutputFramebufferOrigin,
) -> RendererResult<EffectExecutionStats> {
    let mut stats = EffectExecutionStats::default();
    for pass in &graph.passes {
        execute_pass(
            renderer,
            graph,
            textures,
            pass,
            framebuffer_origin,
            &mut stats,
        )?;
        stats.passes = stats.passes.saturating_add(1);
    }
    Ok(stats)
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
            let fragment = match pass.kind {
                RenderPassKind::DualKawaseDownsample => blur::DUAL_KAWASE_DOWNSAMPLE_SHADER,
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
            execute_fullscreen_pass(
                renderer,
                graph,
                textures,
                pass,
                COPY_FRAGMENT_SHADER,
                framebuffer_origin,
                false,
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
        renderer.gl.use_program(Some(renderer.program));
    }
    let layers = renderer
        .commands
        .iter()
        .map(|command| match command.layer {
            EglDrawLayer::Surface(id) => capture::CaptureLayer::Surface(id),
            _ => capture::CaptureLayer::Other,
        })
        .collect::<Vec<_>>();
    let indices = capture::indices_below_anchor(&layers, pass.anchor);
    let scissor = pass.damage.bounding_rect().map_or_else(
        || full_output_rect(renderer.current_size),
        effect_rect_to_output_rect,
    );
    renderer.draw_capture_commands(&indices, scissor)?;
    renderer.effect_resources.unbind_render_target(&renderer.gl);
    restore_output_viewport(renderer);
    let _ = framebuffer_origin;
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
        0,
        oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
    );
    let program = renderer.effect_shaders.get_or_compile(
        &renderer.gl,
        shader_key,
        blur::DUAL_KAWASE_VERTEX_SHADER,
        fragment_shader,
    )?;
    let (vertex_array, _) = renderer.ensure_effect_quad()?;
    unsafe {
        renderer.gl.use_program(Some(program));
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
                1.0 / output_plan.width.max(1) as f32,
                1.0 / output_plan.height.max(1) as f32,
            );
        }
        renderer.gl.bind_vertex_array(Some(vertex_array));
        draw_damage_scissors(
            &renderer.gl,
            &pass.damage,
            renderer.current_size.1,
            framebuffer_origin,
            output_plan.width,
            output_plan.height,
        );
        renderer.gl.bind_vertex_array(None);
        renderer.gl.bind_texture(glow::TEXTURE_2D, None);
        renderer.gl.disable(glow::SCISSOR_TEST);
    }
    if output_texture.is_some() {
        renderer.effect_resources.unbind_render_target(&renderer.gl);
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
    output_height: u32,
    framebuffer_origin: OutputFramebufferOrigin,
    target_width: u32,
    target_height: u32,
) {
    let rects = damage
        .bounding_rect()
        .map(|rect| vec![effect_rect_to_output_rect(rect)])
        .unwrap_or_else(|| vec![full_output_rect((target_width, target_height))]);
    unsafe {
        gl.enable(glow::SCISSOR_TEST);
        for rect in rects {
            let y = match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => {
                    output_height.saturating_sub(rect.y.max(0) as u32 + rect.height) as i32
                }
                OutputFramebufferOrigin::TopLeftScanout => rect.y,
            };
            gl.scissor(rect.x.max(0), y, rect.width as i32, rect.height as i32);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
        }
    }
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
