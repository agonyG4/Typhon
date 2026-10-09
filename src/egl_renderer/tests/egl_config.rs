use super::*;

fn native_candidate(config_id: egl::Int, native_visual_id: u32) -> NativeEglConfigCandidate {
    NativeEglConfigCandidate {
        config_id,
        native_visual_id,
        surface_type: egl::WINDOW_BIT,
        renderable_type: egl::OPENGL_ES3_BIT,
        red_size: 8,
        green_size: 8,
        blue_size: 8,
        alpha_size: 0,
    }
}

fn config_attribute_value(attributes: &[egl::Int], key: egl::Int) -> Option<egl::Int> {
    attributes
        .chunks_exact(2)
        .take_while(|pair| pair[0] != egl::NONE)
        .find(|pair| pair[0] == key)
        .map(|pair| pair[1])
}

#[test]
fn gles_context_attributes_request_client_version_3_only() {
    assert_eq!(
        gles_context_attributes(),
        &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE]
    );
}

#[test]
fn egl_context_priority_policy_parses_values_conservatively() {
    for (input, expected, unknown) in [
        (None, EglContextPriorityPolicy::Auto, false),
        (Some("auto"), EglContextPriorityPolicy::Auto, false),
        (Some("AUTO"), EglContextPriorityPolicy::Auto, false),
        (Some(" high "), EglContextPriorityPolicy::High, false),
        (Some("medium"), EglContextPriorityPolicy::Medium, false),
        (Some("unknown"), EglContextPriorityPolicy::Auto, true),
        (Some("low"), EglContextPriorityPolicy::Auto, true),
        (Some("realtime"), EglContextPriorityPolicy::Auto, true),
        (Some("3101"), EglContextPriorityPolicy::Auto, true),
    ] {
        let parsed = EglContextPriorityPolicy::parse(input);
        assert_eq!(parsed.policy, expected, "input={input:?}");
        assert_eq!(parsed.unknown_value, unknown, "input={input:?}");
    }
}

#[test]
fn egl_context_priority_environment_wrapper_reads_the_policy() {
    use std::ffi::OsString;

    let _environment_lock = crate::native_output::ASTREA_ENV_LOCK.lock().unwrap();
    const VARIABLE: &str = "TYPHON_EGL_CONTEXT_PRIORITY";
    let previous = std::env::var_os(VARIABLE);
    struct RestoreEnvironment(Option<OsString>);
    impl Drop for RestoreEnvironment {
        fn drop(&mut self) {
            // SAFETY: the test owns ASTREA_ENV_LOCK until this restoration guard drops.
            unsafe {
                if let Some(value) = self.0.take() {
                    std::env::set_var("TYPHON_EGL_CONTEXT_PRIORITY", value);
                } else {
                    std::env::remove_var("TYPHON_EGL_CONTEXT_PRIORITY");
                }
            }
        }
    }
    let _restore = RestoreEnvironment(previous);
    // SAFETY: ASTREA_ENV_LOCK serializes this test's environment mutation.
    unsafe { std::env::set_var(VARIABLE, " medium ") };

    assert_eq!(
        EglContextPriorityPolicy::from_environment(),
        EglContextPriorityPolicy::Medium
    );
}

#[test]
fn egl_context_priority_extension_matching_requires_an_exact_whitespace_token() {
    for (extensions, expected) in [
        ("EGL_IMG_context_priority", true),
        ("EGL_KHR_x EGL_IMG_context_priority EGL_EXT_y", true),
        ("EGL_IMG_context_priority_other", false),
        ("X_EGL_IMG_context_priority", false),
        ("", false),
    ] {
        assert_eq!(
            has_egl_img_context_priority(extensions),
            expected,
            "extensions={extensions:?}"
        );
    }
}

#[test]
fn egl_context_priority_candidate_plans_follow_policy_and_extension_state() {
    use EglContextCreationCandidate::{Default, High};
    use EglPriorityExtensionSupport::{Available, QueryFailed, Unsupported};

    for (policy, extension, expected) in [
        (
            EglContextPriorityPolicy::Auto,
            Available,
            vec![High, Default],
        ),
        (
            EglContextPriorityPolicy::High,
            Available,
            vec![High, Default],
        ),
        (EglContextPriorityPolicy::Medium, Available, vec![Default]),
        (EglContextPriorityPolicy::Auto, Unsupported, vec![Default]),
        (EglContextPriorityPolicy::High, Unsupported, vec![Default]),
        (EglContextPriorityPolicy::Auto, QueryFailed, vec![Default]),
        (EglContextPriorityPolicy::High, QueryFailed, vec![Default]),
    ] {
        assert_eq!(
            plan_gles_context_candidates(policy, extension),
            expected,
            "policy={policy:?}, extension={extension:?}"
        );
    }
}

#[test]
fn egl_context_priority_candidates_always_require_gles3() {
    use EglContextCreationCandidate::{Default, High};

    for candidate in [High, Default] {
        let attributes = candidate.attributes();
        assert_eq!(
            config_attribute_value(attributes, egl::CONTEXT_CLIENT_VERSION),
            Some(3)
        );
        assert!(!attributes.contains(&2));
    }
}

#[test]
fn egl_context_priority_high_candidate_has_one_pair_and_default_has_none() {
    use EglContextCreationCandidate::{Default, High};

    let high_attributes = High.attributes();
    let high_pairs = high_attributes
        .chunks_exact(2)
        .take_while(|pair| pair[0] != egl::NONE)
        .filter(|pair| pair[0] == EGL_CONTEXT_PRIORITY_LEVEL_IMG)
        .collect::<Vec<_>>();
    assert_eq!(
        high_pairs,
        [&[
            EGL_CONTEXT_PRIORITY_LEVEL_IMG,
            EGL_CONTEXT_PRIORITY_HIGH_IMG
        ][..]]
    );

    let default_attributes = Default.attributes();
    assert_eq!(
        default_attributes,
        &[egl::CONTEXT_CLIENT_VERSION, 3, egl::NONE]
    );
    assert!(!default_attributes.contains(&EGL_CONTEXT_PRIORITY_LEVEL_IMG));
}

#[test]
fn egl_context_priority_fallback_uses_default_context_after_high_failure() {
    use EglContextCreationCandidate::{Default, High};

    let candidates = plan_gles_context_candidates(
        EglContextPriorityPolicy::High,
        EglPriorityExtensionSupport::Available,
    );
    let mut attempted = Vec::new();
    let (context, report) = create_context_from_candidates(&candidates, |candidate| {
        attempted.push(candidate);
        match candidate {
            High => Err("high rejected"),
            Default => Ok("default-context"),
        }
    })
    .expect("the default GLES3 candidate succeeds");

    assert_eq!(context, "default-context");
    assert_eq!(attempted, [High, Default]);
    assert!(report.high_attempted);
    assert!(report.high_creation_failed);
    assert!(report.fallback_used);
    assert_eq!(report.requested_priority, EglContextPriorityLevel::High);
}

#[test]
fn egl_context_priority_does_not_try_default_after_high_succeeds() {
    use EglContextCreationCandidate::High;

    let candidates = plan_gles_context_candidates(
        EglContextPriorityPolicy::Auto,
        EglPriorityExtensionSupport::Available,
    );
    let mut attempted = Vec::new();
    let (context, report) = create_context_from_candidates(&candidates, |candidate| {
        attempted.push(candidate);
        Ok::<_, &'static str>("high-context")
    })
    .expect("HIGH context succeeds");

    assert_eq!(context, "high-context");
    assert_eq!(attempted, [High]);
    assert!(report.high_attempted);
    assert!(!report.high_creation_failed);
    assert!(!report.fallback_used);
}

#[test]
fn egl_context_priority_maps_driver_values_exactly() {
    assert_eq!(
        EglContextPriorityLevel::from_img_value(EGL_CONTEXT_PRIORITY_HIGH_IMG),
        EglContextPriorityLevel::High
    );
    assert_eq!(
        EglContextPriorityLevel::from_img_value(EGL_CONTEXT_PRIORITY_MEDIUM_IMG),
        EglContextPriorityLevel::Medium
    );
    assert_eq!(
        EglContextPriorityLevel::from_img_value(EGL_CONTEXT_PRIORITY_LOW_IMG),
        EglContextPriorityLevel::Low
    );
    assert_eq!(
        EglContextPriorityLevel::from_img_value(0),
        EglContextPriorityLevel::Unknown
    );
}

#[test]
fn egl_context_priority_query_failure_keeps_created_context_and_reports_unknown() {
    let (context, assignment) = retain_context_when_priority_query_fails("created-context", || {
        Err::<egl::Int, _>("query rejected")
    });

    assert_eq!(context, "created-context");
    assert_eq!(assignment.priority, EglContextPriorityLevel::Unknown);
    assert!(!assignment.assigned_known);
    assert!(assignment.query_failed);
}

#[test]
fn egl_context_priority_unknown_driver_value_is_not_claimed_as_known() {
    let (context, assignment) =
        retain_context_when_priority_query_fails("created-context", || Ok::<_, ()>(0));

    assert_eq!(context, "created-context");
    assert_eq!(assignment.priority, EglContextPriorityLevel::Unknown);
    assert!(!assignment.assigned_known);
    assert!(!assignment.query_failed);
}

#[test]
fn gles_context_creation_error_mentions_required_gles3() {
    let error = io::Error::other("driver rejected context");

    let message = format_gles3_context_error(&error);

    assert!(message.contains("required GLES3 context"));
    assert!(message.contains("driver rejected context"));
}

#[test]
fn trusted_custom_wrapper_compiles_and_links_in_real_gles_context() {
    let _egl_test_lock = egl_test_lock();
    const EGL_PLATFORM_SURFACELESS_MESA: egl::Enum = 0x31dd;
    let egl = unsafe { EglInstance::load_required() }
        .expect("EGL loader is required for the GLES wrapper compile test");
    let display = unsafe {
        egl.get_platform_display(
            EGL_PLATFORM_SURFACELESS_MESA,
            std::ptr::null_mut(),
            &[egl::ATTRIB_NONE],
        )
        .or_else(|_| {
            egl.get_display(egl::DEFAULT_DISPLAY)
                .ok_or(egl::Error::BadDisplay)
        })
    }
    .expect("EGL display is available");
    egl.initialize(display).expect("EGL initializes");
    egl.bind_api(egl::OPENGL_ES_API)
        .expect("EGL binds the GLES API");
    let config_attributes = [
        egl::SURFACE_TYPE,
        egl::PBUFFER_BIT,
        egl::RENDERABLE_TYPE,
        egl::OPENGL_ES3_BIT,
        egl::RED_SIZE,
        8,
        egl::GREEN_SIZE,
        8,
        egl::BLUE_SIZE,
        8,
        egl::ALPHA_SIZE,
        8,
        egl::NONE,
    ];
    let count = egl
        .matching_config_count(display, &config_attributes)
        .expect("EGL returns GLES3 pbuffer configs");
    assert!(count > 0, "EGL exposes a GLES3 pbuffer config");
    let mut configs = Vec::with_capacity(count);
    egl.choose_config(display, &config_attributes, &mut configs)
        .expect("EGL chooses a GLES3 pbuffer config");
    let config = configs[0];
    let context = create_gles_context(&egl, display, config).expect("GLES3 context creates");
    let surface = egl
        .create_pbuffer_surface(display, config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE])
        .expect("EGL pbuffer surface creates");
    egl.make_current(display, Some(surface), Some(surface), Some(context))
        .expect("EGL makes the GLES3 context current");

    let gl = unsafe {
        glow::Context::from_loader_function(|name| {
            egl.get_proc_address(name)
                .map(|symbol| symbol as *const c_void)
                .unwrap_or(ptr::null())
        })
    };
    let asset = oblivion_one::effects::TrustedShaderAsset {
        module: oblivion_one::effects::ShaderModuleId::new(9001).unwrap(),
        relative_path: std::path::PathBuf::from("test.glsl"),
        source: r#"
            uniform float u_float;
            uniform vec2 u_vec2;
            uniform vec4 u_vec4;
            vec4 typhon_effect_main(TyphonEffectContext ctx) {
                vec4 aux = typhon_sample_aux(0, ctx.uv) + typhon_sample_aux(7, ctx.uv);
                return typhon_sample_primary(ctx.uv) + aux * 0.0
                    + vec4(u_float + ctx.time + ctx.delta + u_vec2.x + u_vec4.x);
            }
        "#
        .to_owned(),
        uniforms: vec![
            oblivion_one::effects::EffectUniformBinding {
                parameter: oblivion_one::effects::EffectParameterId::new(1).unwrap(),
                shader_name: "u_float".to_owned(),
            },
            oblivion_one::effects::EffectUniformBinding {
                parameter: oblivion_one::effects::EffectParameterId::new(2).unwrap(),
                shader_name: "u_vec2".to_owned(),
            },
            oblivion_one::effects::EffectUniformBinding {
                parameter: oblivion_one::effects::EffectParameterId::new(3).unwrap(),
                shader_name: "u_vec4".to_owned(),
            },
        ],
    };
    let mut cache = ShaderProgramCache::new(4).unwrap();
    cache
        .prewarm_trusted_custom(&gl, &asset)
        .expect("trusted custom wrapper compiles and links in GLES3");
    let cursor_image = Arc::new(
        CompositorCursorImage::from_argb8888(vec![0xffff_ffff], 1, 1, 0, 0)
            .expect("test cursor image is valid"),
    );
    let mut renderer = GlesSceneRenderer::new_current(
        &egl,
        1,
        1,
        None,
        EglPartialRepaintCapabilities {
            buffer_age: false,
            partial_render_repair: false,
            swap_buffers_with_damage: false,
        },
        cursor_image,
    )
    .expect("test GLES renderer creates");
    renderer.establish_ordinary_scene_state();
    unsafe {
        assert!(gl.is_enabled(glow::BLEND));
        assert!(!gl.is_enabled(glow::SCISSOR_TEST));
        assert_eq!(
            gl.get_parameter_i32(glow::ACTIVE_TEXTURE),
            glow::TEXTURE0 as i32
        );
        assert_eq!(gl.get_parameter_i32(glow::BLEND_SRC_RGB), glow::ONE as i32);
        assert_eq!(
            gl.get_parameter_i32(glow::BLEND_DST_RGB),
            glow::ONE_MINUS_SRC_ALPHA as i32
        );
        assert_eq!(
            gl.get_parameter_i32(glow::BLEND_SRC_ALPHA),
            glow::ONE as i32
        );
        assert_eq!(
            gl.get_parameter_i32(glow::BLEND_DST_ALPHA),
            glow::ONE_MINUS_SRC_ALPHA as i32
        );
        assert_ne!(gl.get_parameter_i32(glow::CURRENT_PROGRAM), 0);
        let mut viewport = [0; 4];
        gl.get_parameter_i32_slice(glow::VIEWPORT, &mut viewport);
        assert_eq!(viewport, [0, 0, 1, 1]);
    }

    let source_over_program = program::create_program_from_sources(
        &gl,
        r#"#version 300 es
            layout(location = 0) in vec2 a_position;
            void main() { gl_Position = vec4(a_position, 0.0, 1.0); }
        "#,
        r#"#version 300 es
            precision highp float;
            uniform vec4 u_color;
            out vec4 out_color;
            void main() { out_color = u_color; }
        "#,
    )
    .expect("source-over regression shader compiles");
    let (quad, _) = renderer
        .ensure_effect_quad()
        .expect("source-over regression quad creates");
    let destination = oblivion_one::effects::PremultipliedRgba::new(0.2, 0.1, 0.05, 0.5);
    let source = oblivion_one::effects::PremultipliedRgba::new(0.4, 0.2, 0.1, 0.5);
    let expected = destination.blend(source, oblivion_one::effects::BlendMode::SourceOver, 1.0);
    unsafe {
        gl.viewport(0, 0, 1, 1);
        gl.disable(glow::SCISSOR_TEST);
        gl.enable(glow::BLEND);
        gl.blend_func_separate(
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
            glow::ONE,
            glow::ONE_MINUS_SRC_ALPHA,
        );
        gl.clear_color(0.0, 0.0, 0.0, 0.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.use_program(Some(source_over_program));
        gl.bind_vertex_array(Some(quad));
        let color = gl
            .get_uniform_location(source_over_program, "u_color")
            .expect("source-over color uniform is active");
        gl.uniform_4_f32(
            Some(&color),
            destination.r,
            destination.g,
            destination.b,
            destination.a,
        );
        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        gl.uniform_4_f32(Some(&color), source.r, source.g, source.b, source.a);
        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        gl.bind_vertex_array(None);
        gl.flush();
        let mut pixel = [0_u8; 4];
        gl.read_pixels(
            0,
            0,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixel)),
        );
        for (actual, expected) in pixel.iter().zip([
            (expected.r * 255.0).round() as u8,
            (expected.g * 255.0).round() as u8,
            (expected.b * 255.0).round() as u8,
            (expected.a * 255.0).round() as u8,
        ]) {
            assert!(
                (*actual as i16 - expected as i16).abs() <= 2,
                "source-over channel mismatch: actual={actual}, expected={expected}, pixel={pixel:?}"
            );
        }
        gl.delete_program(source_over_program);
    }

    let blend_program = program::create_program_from_sources(
        &gl,
        r#"#version 300 es
            layout(location = 0) in vec2 a_position;
            layout(location = 1) in vec2 a_uv;
            out vec2 v_uv;
            void main() {
                gl_Position = vec4(a_position, 0.0, 1.0);
                v_uv = a_uv;
            }
        "#,
        effects::BLEND_STAGE_FRAGMENT_SHADER,
    )
    .expect("blend regression shader compiles");
    let destination_texture = unsafe { gl.create_texture().expect("blend destination texture") };
    let source_texture = unsafe { gl.create_texture().expect("blend source texture") };
    let (quad, _) = renderer
        .ensure_effect_quad()
        .expect("blend regression quad creates");
    let blend_modes = [
        (0, oblivion_one::effects::BlendMode::SourceOver),
        (1, oblivion_one::effects::BlendMode::Add),
        (2, oblivion_one::effects::BlendMode::Multiply),
        (3, oblivion_one::effects::BlendMode::Screen),
    ];
    let opacities = [0.0_f32, 0.25, 0.5, 0.75, 1.0];
    let destination_straight = [0.3_f32, 0.6, 0.2];
    let source_straight = [0.8_f32, 0.25, 0.7];
    unsafe {
        gl.viewport(0, 0, 1, 1);
        gl.disable(glow::BLEND);
        gl.use_program(Some(blend_program));
        gl.bind_vertex_array(Some(quad));
        gl.uniform_1_i32(
            Some(
                &gl.get_uniform_location(blend_program, "u_effect_input")
                    .expect("blend input uniform"),
            ),
            0,
        );
        gl.uniform_1_i32(
            Some(
                &gl.get_uniform_location(blend_program, "u_effect_input_secondary")
                    .expect("blend secondary input uniform"),
            ),
            1,
        );
        gl.uniform_1_i32(
            Some(
                &gl.get_uniform_location(blend_program, "u_effect_decode_srgb")
                    .expect("blend decode uniform"),
            ),
            0,
        );
        gl.uniform_1_i32(
            Some(
                &gl.get_uniform_location(blend_program, "u_effect_encode_srgb")
                    .expect("blend encode uniform"),
            ),
            0,
        );
        let mode_location = gl
            .get_uniform_location(blend_program, "u_effect_blend_mode")
            .expect("blend mode uniform");
        let opacity_location = gl
            .get_uniform_location(blend_program, "u_effect_blend_opacity")
            .expect("blend opacity uniform");
        for destination_alpha in [0.25_f32, 0.5, 1.0] {
            for source_alpha in [0.25_f32, 0.5, 1.0] {
                let destination = oblivion_one::effects::PremultipliedRgba::new(
                    destination_straight[0] * destination_alpha,
                    destination_straight[1] * destination_alpha,
                    destination_straight[2] * destination_alpha,
                    destination_alpha,
                );
                let source = oblivion_one::effects::PremultipliedRgba::new(
                    source_straight[0] * source_alpha,
                    source_straight[1] * source_alpha,
                    source_straight[2] * source_alpha,
                    source_alpha,
                );
                let destination_pixels = [
                    (destination.r * 255.0).round() as u8,
                    (destination.g * 255.0).round() as u8,
                    (destination.b * 255.0).round() as u8,
                    (destination.a * 255.0).round() as u8,
                ];
                let source_pixels = [
                    (source.r * 255.0).round() as u8,
                    (source.g * 255.0).round() as u8,
                    (source.b * 255.0).round() as u8,
                    (source.a * 255.0).round() as u8,
                ];
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(destination_texture));
                RendererResourceState::test_configure_texture(&gl);
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA as i32,
                    1,
                    1,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&destination_pixels)),
                );
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(source_texture));
                RendererResourceState::test_configure_texture(&gl);
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA as i32,
                    1,
                    1,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&source_pixels)),
                );
                for (shader_mode, mode) in blend_modes {
                    gl.uniform_1_i32(Some(&mode_location), shader_mode);
                    for opacity in opacities {
                        let expected = destination.blend(source, mode, opacity);
                        gl.uniform_1_f32(Some(&opacity_location), opacity);
                        gl.clear_color(0.0, 0.0, 0.0, 0.0);
                        gl.clear(glow::COLOR_BUFFER_BIT);
                        gl.draw_arrays(glow::TRIANGLES, 0, 6);
                        gl.flush();
                        let mut actual = [0_u8; 4];
                        gl.read_pixels(
                            0,
                            0,
                            1,
                            1,
                            glow::RGBA,
                            glow::UNSIGNED_BYTE,
                            glow::PixelPackData::Slice(Some(&mut actual)),
                        );
                        let expected = [
                            (expected.r * 255.0).round() as u8,
                            (expected.g * 255.0).round() as u8,
                            (expected.b * 255.0).round() as u8,
                            (expected.a * 255.0).round() as u8,
                        ];
                        if opacity == 0.0
                            && matches!(
                                mode,
                                oblivion_one::effects::BlendMode::Multiply
                                    | oblivion_one::effects::BlendMode::Screen
                            )
                        {
                            assert_eq!(
                                actual, destination_pixels,
                                "zero-opacity {:?} must preserve the destination exactly",
                                mode
                            );
                        }
                        for (actual, expected) in actual.iter().zip(expected) {
                            assert!(
                                (*actual as i16 - expected as i16).abs() <= 3,
                                "blend mismatch mode={mode:?} opacity={opacity} destination_alpha={destination_alpha} source_alpha={source_alpha}: actual={actual:?} expected={expected:?}",
                            );
                        }
                    }
                }
            }
        }
        gl.bind_vertex_array(None);
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.active_texture(glow::TEXTURE1);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.delete_texture(destination_texture);
        gl.delete_texture(source_texture);
        gl.delete_program(blend_program);
    }

    let normalization_surface = egl
        .create_pbuffer_surface(display, config, &[egl::WIDTH, 2, egl::HEIGHT, 1, egl::NONE])
        .expect("normalization pbuffer surface creates");
    egl.make_current(
        display,
        Some(normalization_surface),
        Some(normalization_surface),
        Some(context),
    )
    .expect("normalization surface becomes current");
    let normalization_program = program::create_program_from_sources(
        &gl,
        r#"#version 300 es
            layout(location = 0) in vec2 a_position;
            layout(location = 1) in vec2 a_uv;
            out vec2 v_uv;
            void main() {
                gl_Position = vec4(a_position, 0.0, 1.0);
                v_uv = a_uv;
            }
        "#,
        effects::NORMALIZE_FRAGMENT_SHADER,
    )
    .expect("normalize regression shader compiles");
    let input_texture = unsafe { gl.create_texture().expect("normalize input texture") };
    let (quad, _) = renderer
        .ensure_effect_quad()
        .expect("normalize regression quad creates");
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
        RendererResourceState::test_configure_texture(&gl);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            1,
            1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&[255, 0, 0, 255])),
        );
        gl.viewport(0, 0, 2, 1);
        gl.disable(glow::BLEND);
        gl.clear_color(0.2, 0.3, 0.4, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.use_program(Some(normalization_program));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
        gl.uniform_1_i32(
            Some(
                &gl.get_uniform_location(normalization_program, "u_effect_input")
                    .expect("normalize input uniform"),
            ),
            0,
        );
        gl.uniform_4_f32(
            Some(
                &gl.get_uniform_location(normalization_program, "u_effect_input_domain")
                    .expect("normalize input domain uniform"),
            ),
            500.0,
            200.0,
            10.0,
            10.0,
        );
        gl.uniform_4_f32(
            Some(
                &gl.get_uniform_location(normalization_program, "u_effect_output_domain")
                    .expect("normalize output domain uniform"),
            ),
            500.0,
            200.0,
            20.0,
            10.0,
        );
        gl.uniform_1_i32(
            Some(
                &gl.get_uniform_location(normalization_program, "u_effect_decode_srgb")
                    .expect("normalize decode uniform"),
            ),
            0,
        );
        gl.uniform_1_i32(
            Some(
                &gl.get_uniform_location(normalization_program, "u_effect_encode_srgb")
                    .expect("normalize encode uniform"),
            ),
            0,
        );
        gl.bind_vertex_array(Some(quad));
        gl.draw_arrays(glow::TRIANGLES, 0, 6);
        gl.bind_vertex_array(None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.flush();
        let mut pixels = [0_u8; 8];
        gl.read_pixels(
            0,
            0,
            2,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut pixels)),
        );
        assert!(pixels[0] > 240 && pixels[1] < 10 && pixels[2] < 10 && pixels[3] > 240);
        assert_eq!(&pixels[4..8], &[0, 0, 0, 0]);
        gl.delete_texture(input_texture);
        gl.delete_program(normalization_program);
    }
    egl.make_current(display, None, None, None)
        .expect("EGL releases the normalization context");
    egl.destroy_surface(display, normalization_surface)
        .expect("EGL destroys the normalization surface");

    let mask_surface = egl
        .create_pbuffer_surface(display, config, &[egl::WIDTH, 4, egl::HEIGHT, 1, egl::NONE])
        .expect("mask pbuffer surface creates");
    egl.make_current(
        display,
        Some(mask_surface),
        Some(mask_surface),
        Some(context),
    )
    .expect("mask surface becomes current");
    let mask_program = program::create_program_from_sources(
        &gl,
        r#"#version 300 es
            layout(location = 0) in vec2 a_position;
            layout(location = 1) in vec2 a_uv;
            out vec2 v_uv;
            void main() {
                gl_Position = vec4(a_position, 0.0, 1.0);
                v_uv = a_uv;
            }
        "#,
        effects::MASK_STAGE_FRAGMENT_SHADER,
    )
    .expect("mask regression shader compiles");
    let mask_texture = unsafe { gl.create_texture().expect("mask input texture") };
    let (quad, _) = renderer
        .ensure_effect_quad()
        .expect("mask regression quad creates");
    let alphas = [0.0_f32, 0.25, 0.5, 1.0];
    let mut mask_pixels = Vec::with_capacity(alphas.len() * 4);
    for alpha in alphas {
        let channel = (0.8 * alpha * 255.0).round() as u8;
        mask_pixels.extend_from_slice(&[channel, channel, channel, (alpha * 255.0).round() as u8]);
    }
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(mask_texture));
        RendererResourceState::test_configure_texture(&gl);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA as i32,
            4,
            1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&mask_pixels)),
        );
        gl.viewport(0, 0, 4, 1);
        gl.disable(glow::BLEND);
        gl.use_program(Some(mask_program));
        gl.active_texture(glow::TEXTURE0);
        gl.bind_texture(glow::TEXTURE_2D, Some(mask_texture));
        gl.uniform_1_i32(
            Some(
                &gl.get_uniform_location(mask_program, "u_effect_input")
                    .expect("mask input uniform"),
            ),
            0,
        );
        gl.uniform_1_i32(
            Some(
                &gl.get_uniform_location(mask_program, "u_effect_decode_srgb")
                    .expect("mask decode uniform"),
            ),
            0,
        );
        gl.uniform_1_i32(
            Some(
                &gl.get_uniform_location(mask_program, "u_effect_encode_srgb")
                    .expect("mask encode uniform"),
            ),
            0,
        );
        gl.bind_vertex_array(Some(quad));
        for inverted in [false, true] {
            gl.uniform_1_i32(
                Some(
                    &gl.get_uniform_location(mask_program, "u_effect_inverted")
                        .expect("mask mode uniform"),
                ),
                i32::from(inverted),
            );
            gl.clear_color(0.0, 0.0, 0.0, 0.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.draw_arrays(glow::TRIANGLES, 0, 6);
            gl.flush();
            let mut pixels = [0_u8; 16];
            gl.read_pixels(
                0,
                0,
                4,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixels)),
            );
            for (index, alpha) in alphas.into_iter().enumerate() {
                let coverage = if inverted { 1.0 - alpha } else { alpha };
                let expected = [
                    (0.8 * alpha * coverage * 255.0).round() as u8,
                    (0.8 * alpha * coverage * 255.0).round() as u8,
                    (0.8 * alpha * coverage * 255.0).round() as u8,
                    (alpha * coverage * 255.0).round() as u8,
                ];
                let actual = &pixels[index * 4..index * 4 + 4];
                for (actual, expected) in actual.iter().zip(expected) {
                    assert!(
                        (*actual as i16 - expected as i16).abs() <= 3,
                        "mask mismatch inverted={inverted} alpha={alpha} coverage={coverage}: actual={actual:?} expected={expected:?}"
                    );
                }
            }
        }
        gl.bind_vertex_array(None);
        gl.bind_texture(glow::TEXTURE_2D, None);
        gl.delete_texture(mask_texture);
        gl.delete_program(mask_program);
    }
    egl.make_current(display, None, None, None)
        .expect("EGL releases the mask context");
    egl.destroy_surface(display, mask_surface)
        .expect("EGL destroys the mask surface");

    egl.make_current(display, None, None, None)
        .expect("EGL releases the GLES3 context");
    egl.destroy_surface(display, surface)
        .expect("EGL destroys the pbuffer surface");
    egl.destroy_context(display, context)
        .expect("EGL destroys the GLES3 context");
    egl.terminate(display).expect("EGL terminates");
}

#[test]
fn native_egl_config_selection_rejects_gles2_only_candidates() {
    let mut candidate = native_candidate(1, XR24);
    candidate.renderable_type = egl::OPENGL_ES2_BIT;
    let candidates = [candidate];

    let error = select_native_egl_config_candidate(&candidates, XR24).unwrap_err();

    assert!(error.to_string().contains("GLES3"));
}

#[test]
fn surfaceless_config_does_not_require_window_bit() {
    let mut candidate = native_candidate(1, XR24);
    candidate.surface_type = 0;

    assert!(native_egl_config_candidate_matches_common(&candidate, XR24));
    assert!(!native_egl_config_candidate_matches(&candidate, XR24));
}

#[test]
fn native_egl_config_selection_prefers_requested_xrgb8888() {
    let candidates = [
        native_candidate(1, AR24),
        native_candidate(2, XR24),
        native_candidate(3, XR24),
    ];

    let selected = select_native_egl_config_candidate(&candidates, XR24).unwrap();

    assert_eq!(selected, 1);
}

#[test]
fn native_egl_config_selection_ignores_wrong_visual() {
    let candidates = [native_candidate(1, AR24)];

    assert!(select_native_egl_config_candidate(&candidates, XR24).is_err());
}

#[test]
fn native_egl_config_selection_accepts_zero_alpha_for_xrgb8888() {
    let candidates = [native_candidate(7, XR24)];

    let selected = select_native_egl_config_candidate(&candidates, XR24).unwrap();

    assert_eq!(selected, 0);
}

#[test]
fn native_egl_format_selection_falls_back_to_argb8888_when_xrgb8888_absent() {
    let available_formats = [XR24, AR24];
    let candidates = [native_candidate(9, AR24)];

    let selected = select_native_egl_visual_format(&available_formats, &candidates).unwrap();

    assert_eq!(selected, AR24);
}

#[test]
fn native_egl_config_selection_rejects_missing_window_bit() {
    let mut candidate = native_candidate(1, XR24);
    candidate.surface_type = 0;

    assert!(select_native_egl_config_candidate(&[candidate], XR24).is_err());
}

#[test]
fn native_egl_config_selection_rejects_missing_gles_renderable_bit() {
    let mut candidate = native_candidate(1, XR24);
    candidate.renderable_type = 0;

    assert!(select_native_egl_config_candidate(&[candidate], XR24).is_err());
}

#[test]
fn native_egl_config_selection_diagnostic_names_requested_fourcc_and_hex() {
    let error = select_native_egl_config_candidate(&[], XR24).unwrap_err();
    let diagnostic = error.to_string();

    assert!(diagnostic.contains("XR24"));
    assert!(diagnostic.contains("0x34325258"));
}
