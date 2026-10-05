use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EffectPassBlendMode {
    Replace,
    PremultipliedSourceOver,
}

pub(super) fn effect_pass_blend_mode(
    kind: RenderPassKind,
    output_is_framebuffer: bool,
    alpha_mode: EffectAlphaMode,
    presentation_opacity: f32,
) -> EffectPassBlendMode {
    if output_is_framebuffer
        && matches!(
            kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        )
        && (alpha_mode == EffectAlphaMode::Preserve || presentation_opacity < 1.0)
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

pub(super) fn capture_blend_mode() -> EffectPassBlendMode {
    EffectPassBlendMode::PremultipliedSourceOver
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_pass(
    renderer: &mut EffectExecutionContext<'_>,
    graph: &CompiledFrameGraph,
    textures: &std::collections::HashMap<GraphTextureId, GraphTextureBinding>,
    pass: &CompiledRenderPass,
    targets: EffectExecutionTargets,
    framebuffer_origin: OutputFramebufferOrigin,
    execution_damage: &EffectRegion,
    scene_baseline_authority: SceneBaselineAuthority,
    debug_config: EffectDebugConfig,
    capture_plan: CheckpointCaptureExecutionPlan,
    causal_stability: &CheckpointCausalStabilityPlan,
    host_timing_enabled: bool,
    capture_downsample_fusion: Option<CaptureDownsampleFusion>,
    stats: &mut EffectExecutionStats,
) -> RendererResult<Option<ReplayCaptureExecutionDetail>> {
    match pass.kind {
        RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture => {
            let replay_execution = execute_capture(
                renderer,
                graph,
                textures,
                pass,
                targets,
                framebuffer_origin,
                execution_damage,
                scene_baseline_authority,
                debug_config,
                capture_plan,
                causal_stability,
                host_timing_enabled,
                capture_downsample_fusion.filter(|fusion| fusion.capture_pass == pass.id),
                stats,
            )?;
            stats.scene_captures = stats.scene_captures.saturating_add(1);
            return Ok(replay_execution);
        }
        RenderPassKind::DualKawaseDownsample | RenderPassKind::DualKawaseUpsample => {
            if capture_downsample_fusion.is_some_and(|fusion| {
                fusion.consumer_pass == pass.id
                    && pass.inputs.as_slice() != [fusion.capture_texture]
            }) {
                return Err(
                    io::Error::other("fused downsample input changed after planning").into(),
                );
            }
            let fused_downsample = capture_downsample_fusion.filter(|fusion| {
                fusion.consumer_pass == pass.id
                    && pass.inputs.as_slice() == [fusion.capture_texture]
            });
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
                RenderPassKind::DualKawaseDownsample if fused_downsample.is_some() => {
                    blur::DUAL_KAWASE_DOWNSAMPLE_FUSED_CAPTURE_SHADER
                }
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
                targets,
                fragment,
                framebuffer_origin,
                true,
                execution_damage,
                fused_downsample,
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
                targets,
                NORMALIZE_FRAGMENT_SHADER,
                framebuffer_origin,
                false,
                execution_damage,
                None,
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
                targets,
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
                targets,
                COMPOSITE_FRAGMENT_SHADER,
                framebuffer_origin,
                false,
                execution_damage,
                None,
            )?;
            stats.composites = stats.composites.saturating_add(1);
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_fullscreen_stage(
    renderer: &mut EffectExecutionContext<'_>,
    graph: &CompiledFrameGraph,
    textures: &std::collections::HashMap<GraphTextureId, GraphTextureBinding>,
    pass: &CompiledRenderPass,
    targets: EffectExecutionTargets,
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
        .and_then(|texture| renderer.runtime.effect_resources.texture(texture))
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
        .runtime
        .effect_resources
        .bind_render_target(renderer.gl, output_texture)?;
    let module = ShaderModuleId::new(module)
        .ok_or_else(|| io::Error::other("effect stage shader id is zero"))?;
    let shader_key = ShaderProgramKey::new(
        module,
        0,
        oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
    );
    let program = renderer.runtime.effect_shaders.lookup(shader_key)?;
    let (vertex_array, _) = renderer.ensure_effect_quad()?;
    let target_flip_y = effect_target_requires_logical_y_flip(false, framebuffer_origin);
    let input_flip_y = effect_input_requires_sample_y_flip(input_plan.origin);
    establish_effect_pass_blend_state(renderer.gl, EffectPassBlendMode::Replace);
    unsafe {
        renderer
            .gl
            .viewport(0, 0, output_plan.width as i32, output_plan.height as i32);
        renderer.gl.use_program(Some(program));
        if let Some(location) = uniform_location(
            &mut renderer.runtime.effect_shaders,
            renderer.gl,
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
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
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
            &mut renderer.runtime.effect_shaders,
            renderer.gl,
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
            &mut renderer.runtime.effect_shaders,
            renderer.gl,
            shader_key,
            program,
            "u_effect_input",
        ) {
            renderer.gl.uniform_1_i32(Some(&location), 0);
        }
        for (unit, input_id) in pass.inputs.iter().copied().enumerate().skip(1) {
            let texture = textures
                .get(&input_id)
                .and_then(|texture| renderer.runtime.effect_resources.texture(texture))
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
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
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
                    &mut renderer.runtime.effect_shaders,
                    renderer.gl,
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
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
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
            &mut renderer.runtime.effect_shaders,
            renderer.gl,
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
            &mut renderer.runtime.effect_shaders,
            renderer.gl,
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
            &mut renderer.runtime.effect_shaders,
            renderer.gl,
            shader_key,
            program,
            stage,
            StageUniformContext {
                parameters: &pass.parameter_block,
                input_plan,
                output_size: trusted_effect_output_size(
                    renderer.scene.current_size,
                    (output_plan.width, output_plan.height),
                ),
                output_scale: renderer.runtime.effect_output_scale,
                effect_time_seconds: renderer.runtime.effect_time_seconds,
                effect_delta_seconds: renderer.runtime.effect_delta_seconds,
                color_conversion: pass.color_conversion,
                fused_stages: &pass.fused_stages,
            },
        );
        renderer.gl.bind_vertex_array(Some(vertex_array));
        draw_damage_scissors(
            renderer.gl,
            execution_damage,
            output_plan,
            framebuffer_origin,
            None,
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
    renderer
        .runtime
        .effect_resources
        .unbind_render_target(renderer.gl);
    renderer.establish_effect_composition_state(targets.composition_draw);
    restore_output_viewport(renderer.gl, renderer.scene.current_size);
    let _ = fragment_shader;
    Ok(())
}

pub(super) fn uniform_location(
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

pub(super) fn set_identity_color_matrix(
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
pub(super) fn execute_fullscreen_pass(
    renderer: &mut EffectExecutionContext<'_>,
    graph: &CompiledFrameGraph,
    textures: &std::collections::HashMap<GraphTextureId, GraphTextureBinding>,
    pass: &CompiledRenderPass,
    targets: EffectExecutionTargets,
    fragment_shader: &str,
    framebuffer_origin: OutputFramebufferOrigin,
    blur_shader: bool,
    execution_damage: &EffectRegion,
    capture_fusion: Option<CaptureDownsampleFusion>,
) -> RendererResult<()> {
    let input = pass
        .inputs
        .first()
        .copied()
        .ok_or_else(|| io::Error::other("effect pass has no input texture"))?;
    let input_texture = if let Some(fusion) = capture_fusion {
        if fusion.capture_texture != input {
            return Err(
                io::Error::other("fused downsample input does not match its capture").into(),
            );
        }
        renderer
            .scene
            .active_output_texture
            .ok_or_else(|| io::Error::other("fused downsample has no active output texture"))?
    } else {
        textures
            .get(&input)
            .and_then(|texture| renderer.runtime.effect_resources.texture(texture))
            .ok_or_else(|| io::Error::other("effect input texture is not realized"))?
    };
    let output = pass
        .output
        .ok_or_else(|| io::Error::other("effect pass has no output target"))?;
    let input_plan = graph_texture(graph, input)?;
    let output_plan = graph_texture(graph, output)?;
    let output_is_framebuffer = output_plan.source == GraphTextureSource::Output;
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
        if capture_fusion.is_some() {
            2
        } else if pass.kind == RenderPassKind::NormalizeInput
            || (pass.kind == RenderPassKind::DualKawaseDownsample
                && fragment_shader == blur::DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER)
        {
            1
        } else {
            0
        },
        oblivion_one::effects::EffectWorkingSpace::LinearSrgb,
    );
    let program = renderer.runtime.effect_shaders.lookup(shader_key)?;
    let (vertex_array, _) = renderer.ensure_effect_quad()?;
    if capture_fusion.is_some() {
        for name in [
            "u_effect_input",
            "u_effect_target_flip_y",
            "u_effect_input_flip_y",
            "u_effect_texel_size",
            "u_effect_blur_radius",
            "u_effect_output_size",
            "u_effect_capture_domain",
            "u_effect_capture_size",
            "u_effect_capture_origin_bottom_left",
        ] {
            if uniform_location(
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
                shader_key,
                program,
                name,
            )
            .is_none()
            {
                return Err(io::Error::other(format!(
                    "fused downsample shader is missing uniform {name}"
                ))
                .into());
            }
        }
    }
    let output_texture = if output_is_framebuffer {
        None
    } else {
        Some(
            textures
                .get(&output)
                .ok_or_else(|| io::Error::other("effect output texture is not allocated"))?,
        )
    };
    if output_is_framebuffer {
        renderer.establish_effect_composition_state(targets.composition_draw);
    }
    if let Some(texture) = output_texture {
        renderer
            .runtime
            .effect_resources
            .bind_render_target(renderer.gl, texture)?;
    }
    let presentation_opacity = if output_is_framebuffer
        && matches!(
            pass.kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        ) {
        renderer.presentation_opacity_for_visual_group(pass.visual_group)
    } else {
        1.0
    };
    let presentation_clip = if output_is_framebuffer && pass.kind == RenderPassKind::Composite {
        renderer.presentation_clip_for_visual_group(pass.visual_group)
    } else {
        None
    };
    let blend_mode = effect_pass_blend_mode(
        pass.kind,
        output_is_framebuffer,
        pass.alpha_mode,
        presentation_opacity,
    );
    establish_effect_pass_blend_state(renderer.gl, blend_mode);
    unsafe {
        renderer
            .gl
            .viewport(0, 0, output_plan.width as i32, output_plan.height as i32);
    }
    let target_flip_y =
        effect_target_requires_logical_y_flip(output_is_framebuffer, framebuffer_origin);
    let input_flip_y = effect_input_requires_sample_y_flip(input_plan.origin);
    unsafe {
        renderer.gl.use_program(Some(program));
        if matches!(
            pass.kind,
            RenderPassKind::Composite | RenderPassKind::OutputPostProcess
        ) && let Some(location) = uniform_location(
            &mut renderer.runtime.effect_shaders,
            renderer.gl,
            shader_key,
            program,
            "u_presentation_opacity",
        ) {
            renderer
                .gl
                .uniform_1_f32(Some(&location), presentation_opacity);
        }
        if let Some(location) = uniform_location(
            &mut renderer.runtime.effect_shaders,
            renderer.gl,
            shader_key,
            program,
            "u_effect_target_flip_y",
        ) {
            renderer
                .gl
                .uniform_1_i32(Some(&location), i32::from(target_flip_y));
        }
        if let Some(location) = uniform_location(
            &mut renderer.runtime.effect_shaders,
            renderer.gl,
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
            &mut renderer.runtime.effect_shaders,
            renderer.gl,
            shader_key,
            program,
            "u_effect_input",
        ) {
            renderer.gl.uniform_1_i32(Some(&location), 0);
        }
        if blur_shader
            && let Some(location) = uniform_location(
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
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
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
                shader_key,
                program,
                "u_effect_blur_radius",
            )
        {
            renderer
                .gl
                .uniform_1_f32(Some(&location), pass.blur_radius.unwrap_or(1.0));
        }
        if let Some(fusion) = capture_fusion {
            if let Some(location) = uniform_location(
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
                shader_key,
                program,
                "u_effect_output_size",
            ) {
                renderer.gl.uniform_2_f32(
                    Some(&location),
                    renderer.scene.current_size.0 as f32,
                    renderer.scene.current_size.1 as f32,
                );
            }
            if let Some(location) = uniform_location(
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
                shader_key,
                program,
                "u_effect_capture_domain",
            ) {
                renderer.gl.uniform_4_f32(
                    Some(&location),
                    fusion.capture_domain.x as f32,
                    fusion.capture_domain.y as f32,
                    fusion.capture_domain.width as f32,
                    fusion.capture_domain.height as f32,
                );
            }
            if let Some(location) = uniform_location(
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
                shader_key,
                program,
                "u_effect_capture_size",
            ) {
                renderer.gl.uniform_2_f32(
                    Some(&location),
                    input_plan.width as f32,
                    input_plan.height as f32,
                );
            }
            if let Some(location) = uniform_location(
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
                shader_key,
                program,
                "u_effect_capture_origin_bottom_left",
            ) {
                renderer.gl.uniform_1_i32(
                    Some(&location),
                    i32::from(matches!(
                        framebuffer_origin,
                        OutputFramebufferOrigin::BottomLeft
                    )),
                );
            }
        }
        if !blur_shader
            && let Some(location) = uniform_location(
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
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
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
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
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
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
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
                shader_key,
                program,
                "u_effect_encode_srgb",
            ) {
                renderer
                    .gl
                    .uniform_1_i32(Some(&location), i32::from(pass.encode_output));
            }
            if let Some(location) = uniform_location(
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
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
                    &mut renderer.runtime.effect_shaders,
                    renderer.gl,
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
                &mut renderer.runtime.effect_shaders,
                renderer.gl,
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
            renderer.gl,
            execution_damage,
            output_plan,
            framebuffer_origin,
            presentation_clip,
        );
        renderer.gl.bind_vertex_array(None);
        renderer.gl.bind_texture(glow::TEXTURE_2D, None);
        renderer.gl.disable(glow::SCISSOR_TEST);
        renderer.gl.disable(glow::BLEND);
    }
    if output_texture.is_some() {
        renderer
            .runtime
            .effect_resources
            .unbind_render_target(renderer.gl);
        renderer.establish_effect_composition_state(targets.composition_draw);
        restore_output_viewport(renderer.gl, renderer.scene.current_size);
    }
    Ok(())
}

pub(super) fn restore_output_viewport(gl: &glow::Context, output_size: (u32, u32)) {
    unsafe {
        gl.viewport(0, 0, output_size.0 as i32, output_size.1 as i32);
    }
}

pub(super) fn trusted_effect_output_size(
    renderer_size: (u32, u32),
    _stage_texture_size: (u32, u32),
) -> (f32, f32) {
    (renderer_size.0 as f32, renderer_size.1 as f32)
}
