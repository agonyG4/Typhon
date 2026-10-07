use super::*;

#[test]
fn fused_downsample_matches_materialized_rgba8_capture_for_edges_origins_and_damage() {
    // The Mesa GL_LINEAR path differs by 11 RGBA16F ULPs at the worst
    // observed sample; allow one ULP of comparison headroom.
    const MAX_RGBA16F_SAMPLER_DIFFERENCE: f32 = 12.0 / 8192.0;
    let mut harness = GlesEffectTestHarness::new(19, 17);
    harness.install_texture_backed_output();
    let gl = &harness.gl;
    let output_size = (19_u32, 17_u32);
    let active_output = harness.renderer.scene_state.active_output_texture.unwrap();
    let (quad, _) = harness
        .renderer
        .ensure_effect_quad()
        .expect("fullscreen effect quad is available");
    let capture_copy = harness.renderer.effect_runtime.capture_copy_program;
    let downsample_module = oblivion_one::effects::ShaderModuleId::new(
        oblivion_one::effects::INTERNAL_EFFECT_SHADER_MODULE_DOWNSAMPLE,
    )
    .unwrap();
    let shader_space = oblivion_one::effects::EffectWorkingSpace::LinearSrgb;
    let reference_program = harness
        .renderer
        .effect_runtime
        .effect_shaders
        .lookup(effects::ShaderProgramKey::new(
            downsample_module,
            0,
            shader_space,
        ))
        .expect("existing first-downsample shader is prewarmed");
    let fused_program = harness
        .renderer
        .effect_runtime
        .effect_shaders
        .lookup(effects::ShaderProgramKey::new(
            downsample_module,
            2,
            shader_space,
        ))
        .expect("fused first-downsample shader is prewarmed");
    let color_order_domain = oblivion_one::effects::EffectRect::new(5, 6, 4, 2).unwrap();
    let domains = [
        oblivion_one::effects::EffectRect::new(5, 4, 6, 4).unwrap(),
        oblivion_one::effects::EffectRect::new(6, 5, 7, 5).unwrap(),
        oblivion_one::effects::EffectRect::new(9, 8, 1, 1).unwrap(),
        oblivion_one::effects::EffectRect::new(2, 7, 2, 3).unwrap(),
        oblivion_one::effects::EffectRect::new(0, 4, 5, 5).unwrap(),
        oblivion_one::effects::EffectRect::new(14, 3, 5, 7).unwrap(),
        oblivion_one::effects::EffectRect::new(6, 0, 7, 4).unwrap(),
        oblivion_one::effects::EffectRect::new(5, 12, 8, 5).unwrap(),
        oblivion_one::effects::EffectRect::new(0, 0, 19, 17).unwrap(),
        color_order_domain,
    ];
    let mut worst_differential = (0.0_f32, String::new());

    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        for domain in domains {
            let capture_size = (domain.width, domain.height);
            let sample_pixels = (0..output_size.1)
                .flat_map(|gl_y| {
                    (0..output_size.0).flat_map(move |x| {
                        let local_y = match origin {
                            OutputFramebufferOrigin::BottomLeft => {
                                let first_y = output_size.1 - domain.y as u32 - domain.height;
                                (gl_y >= first_y && gl_y < first_y + domain.height)
                                    .then(|| gl_y - first_y)
                            }
                            OutputFramebufferOrigin::TopLeftScanout => (gl_y >= domain.y as u32
                                && gl_y < domain.y as u32 + domain.height)
                                .then(|| domain.y as u32 + domain.height - 1 - gl_y),
                        };
                        let inside = x >= domain.x as u32
                            && x < domain.x as u32 + domain.width
                            && local_y.is_some();
                        let pixel = if !inside {
                            [255, 0, 255, 255]
                        } else {
                            let local_x = x - domain.x as u32;
                            let local_y = local_y.unwrap();
                            if domain == color_order_domain {
                                let encoded = if local_x < 2 { 0 } else { 255 };
                                [encoded, encoded, encoded, 255]
                            } else {
                                let alpha = ((local_x * 37 + local_y * 71 + 80) % 256) as u8;
                                [
                                    (((local_x * 73 + local_y * 29 + 11) % 256) as u8).min(alpha),
                                    (((local_x * 17 + local_y * 61 + 53) % 256) as u8).min(alpha),
                                    (((local_x * 31 + local_y * 47 + 97) % 256) as u8).min(alpha),
                                    alpha,
                                ]
                            }
                        };
                        pixel.into_iter()
                    })
                })
                .collect::<Vec<_>>();
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(active_output));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.tex_sub_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    0,
                    0,
                    output_size.0 as i32,
                    output_size.1 as i32,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&sample_pixels)),
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
            }

            let capture_texture =
                unsafe { gl.create_texture().expect("RGBA8 capture texture creates") };
            let capture_framebuffer = unsafe {
                gl.create_framebuffer()
                    .expect("capture framebuffer creates")
            };
            unsafe {
                gl.bind_texture(glow::TEXTURE_2D, Some(capture_texture));
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::LINEAR as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_S,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_WRAP_T,
                    glow::CLAMP_TO_EDGE as i32,
                );
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    capture_size.0 as i32,
                    capture_size.1 as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(capture_framebuffer));
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(capture_texture),
                    0,
                );
                assert_eq!(
                    gl.check_framebuffer_status(glow::FRAMEBUFFER),
                    glow::FRAMEBUFFER_COMPLETE,
                    "RGBA8 capture framebuffer is complete"
                );
                gl.viewport(0, 0, capture_size.0 as i32, capture_size.1 as i32);
                gl.disable(glow::SCISSOR_TEST);
                gl.disable(glow::BLEND);
                gl.use_program(Some(capture_copy));
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(active_output));
                set_effect_test_uniform_i32(gl, capture_copy, "u_output_texture", 0);
                set_effect_test_uniform_2_f32(
                    gl,
                    capture_copy,
                    "u_capture_output_size",
                    output_size.0 as f32,
                    output_size.1 as f32,
                );
                set_effect_test_uniform_4_f32(
                    gl,
                    capture_copy,
                    "u_capture_domain",
                    [
                        domain.x as f32,
                        domain.y as f32,
                        domain.width as f32,
                        domain.height as f32,
                    ],
                );
                set_effect_test_uniform_2_f32(
                    gl,
                    capture_copy,
                    "u_capture_target_size",
                    capture_size.0 as f32,
                    capture_size.1 as f32,
                );
                set_effect_test_uniform_i32(
                    gl,
                    capture_copy,
                    "u_capture_origin_bottom_left",
                    i32::from(origin == OutputFramebufferOrigin::BottomLeft),
                );
                gl.bind_vertex_array(Some(quad));
                gl.draw_arrays(glow::TRIANGLES, 0, 6);
                gl.bind_vertex_array(None);
                gl.bind_texture(glow::TEXTURE_2D, None);
                gl.use_program(None);
            }

            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, Some(capture_framebuffer));
            }
            let captured_pixels = read_effect_test_pixels(gl, capture_size.0, capture_size.1);
            for local_y in 0..capture_size.1 {
                let logical_y = capture_size.1 - 1 - local_y;
                let source_y = match origin {
                    OutputFramebufferOrigin::BottomLeft => {
                        output_size.1 - 1 - domain.y as u32 - logical_y
                    }
                    OutputFramebufferOrigin::TopLeftScanout => domain.y as u32 + logical_y,
                };
                for local_x in 0..capture_size.0 {
                    let source_x = domain.x as u32 + local_x;
                    let source_offset = ((source_y * output_size.0 + source_x) * 4) as usize;
                    let capture_offset = ((local_y * capture_size.0 + local_x) * 4) as usize;
                    assert_eq!(
                        &captured_pixels[capture_offset..capture_offset + 4],
                        &sample_pixels[source_offset..source_offset + 4],
                        "capture-copy bytes at {origin:?}, domain {domain:?}, local ({local_x}, {local_y})"
                    );
                }
            }

            let downsample_size = (capture_size.0.div_ceil(2), capture_size.1.div_ceil(2));
            let make_downsample_target = || {
                let texture =
                    unsafe { gl.create_texture().expect("linear output texture creates") };
                let framebuffer = unsafe {
                    gl.create_framebuffer()
                        .expect("linear output framebuffer creates")
                };
                unsafe {
                    gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_MIN_FILTER,
                        glow::NEAREST as i32,
                    );
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_MAG_FILTER,
                        glow::NEAREST as i32,
                    );
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_WRAP_S,
                        glow::CLAMP_TO_EDGE as i32,
                    );
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_WRAP_T,
                        glow::CLAMP_TO_EDGE as i32,
                    );
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA16F as i32,
                        downsample_size.0 as i32,
                        downsample_size.1 as i32,
                        0,
                        glow::RGBA,
                        glow::HALF_FLOAT,
                        glow::PixelUnpackData::Slice(None),
                    );
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                    gl.framebuffer_texture_2d(
                        glow::FRAMEBUFFER,
                        glow::COLOR_ATTACHMENT0,
                        glow::TEXTURE_2D,
                        Some(texture),
                        0,
                    );
                    assert_eq!(
                        gl.check_framebuffer_status(glow::FRAMEBUFFER),
                        glow::FRAMEBUFFER_COMPLETE,
                        "RGBA16F downsample framebuffer is complete"
                    );
                }
                (texture, framebuffer)
            };
            let (reference_texture, reference_framebuffer) = make_downsample_target();
            let (fused_texture, fused_framebuffer) = make_downsample_target();

            let render_downsample = |framebuffer: glow::Framebuffer,
                                     input_texture: glow::Texture,
                                     program: glow::Program,
                                     fused: bool,
                                     blur_radius: f32,
                                     partial: bool| {
                unsafe {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
                    gl.viewport(0, 0, downsample_size.0 as i32, downsample_size.1 as i32);
                    gl.disable(glow::SCISSOR_TEST);
                    gl.disable(glow::BLEND);
                    gl.clear_color(0.13, 0.27, 0.41, 1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                    if partial {
                        gl.enable(glow::SCISSOR_TEST);
                        gl.scissor(
                            0,
                            0,
                            (downsample_size.0 / 2).max(1) as i32,
                            (downsample_size.1 / 2).max(1) as i32,
                        );
                    }
                    gl.use_program(Some(program));
                }
                set_effect_test_uniform_i32(gl, program, "u_effect_target_flip_y", 0);
                set_effect_test_uniform_i32(gl, program, "u_effect_input_flip_y", 1);
                set_effect_test_uniform_i32(gl, program, "u_effect_input", 0);
                set_effect_test_uniform_2_f32(
                    gl,
                    program,
                    "u_effect_texel_size",
                    1.0 / capture_size.0 as f32,
                    1.0 / capture_size.1 as f32,
                );
                let radius_location = unsafe {
                    gl.get_uniform_location(program, "u_effect_blur_radius")
                        .expect("downsample blur radius uniform is active")
                };
                unsafe { gl.uniform_1_f32(Some(&radius_location), blur_radius) };
                if fused {
                    set_effect_test_uniform_2_f32(
                        gl,
                        program,
                        "u_effect_output_size",
                        output_size.0 as f32,
                        output_size.1 as f32,
                    );
                    set_effect_test_uniform_4_f32(
                        gl,
                        program,
                        "u_effect_capture_domain",
                        [
                            domain.x as f32,
                            domain.y as f32,
                            domain.width as f32,
                            domain.height as f32,
                        ],
                    );
                    set_effect_test_uniform_2_f32(
                        gl,
                        program,
                        "u_effect_capture_size",
                        capture_size.0 as f32,
                        capture_size.1 as f32,
                    );
                    set_effect_test_uniform_i32(
                        gl,
                        program,
                        "u_effect_capture_origin_bottom_left",
                        i32::from(origin == OutputFramebufferOrigin::BottomLeft),
                    );
                }
                unsafe {
                    gl.active_texture(glow::TEXTURE0);
                    gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
                    gl.bind_vertex_array(Some(quad));
                    gl.draw_arrays(glow::TRIANGLES, 0, 6);
                    gl.bind_vertex_array(None);
                    gl.bind_texture(glow::TEXTURE_2D, None);
                    gl.disable(glow::SCISSOR_TEST);
                    gl.use_program(None);
                }
            };

            for blur_radius in [0.5_f32, 1.0, 2.75] {
                for partial in [false, true] {
                    render_downsample(
                        reference_framebuffer,
                        capture_texture,
                        reference_program,
                        false,
                        blur_radius,
                        partial,
                    );
                    render_downsample(
                        fused_framebuffer,
                        active_output,
                        fused_program,
                        true,
                        blur_radius,
                        partial,
                    );
                    unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, Some(reference_framebuffer)) };
                    let reference =
                        read_effect_test_pixels_f32(gl, downsample_size.0, downsample_size.1);
                    unsafe { gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fused_framebuffer)) };
                    let fused =
                        read_effect_test_pixels_f32(gl, downsample_size.0, downsample_size.1);
                    let (maximum_error_index, maximum_error) = reference
                        .iter()
                        .zip(&fused)
                        .enumerate()
                        .map(|(index, (expected, actual))| (index, (expected - actual).abs()))
                        .max_by(|left, right| left.1.total_cmp(&right.1))
                        .unwrap_or((0, 0.0));
                    if maximum_error > worst_differential.0 {
                        worst_differential = (
                            maximum_error,
                            format!(
                                "component {maximum_error_index}: reference={}, fused={} for {origin:?}, domain {domain:?}, radius {blur_radius}, partial={partial}",
                                reference[maximum_error_index], fused[maximum_error_index],
                            ),
                        );
                    }
                    if domain == color_order_domain
                        && origin == OutputFramebufferOrigin::BottomLeft
                        && blur_radius == 1.0
                        && !partial
                    {
                        assert!(
                            (0.10..0.115).contains(&fused[0]),
                            "fused output must bilinear-filter encoded 0/1 before sRGB decode: {}",
                            fused[0]
                        );
                    }
                }
            }

            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.delete_framebuffer(capture_framebuffer);
                gl.delete_texture(capture_texture);
                gl.delete_framebuffer(reference_framebuffer);
                gl.delete_texture(reference_texture);
                gl.delete_framebuffer(fused_framebuffer);
                gl.delete_texture(fused_texture);
            }
        }
    }

    assert!(
        worst_differential.0 <= MAX_RGBA16F_SAMPLER_DIFFERENCE,
        "fused/reference max error {} at {}",
        worst_differential.0,
        worst_differential.1
    );
}
#[test]
fn real_gles_scene_work_preservation_restores_only_planned_regions() {
    let regions = [OutputRect::new(1, 0, 3, 2)];

    for framebuffer_origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        let mut harness = GlesEffectTestHarness::new(8, 6);
        fill_non_uniform_diagnostic_output(&harness);
        let original = read_diagnostic_pixels(&harness);
        let preservation = effects::capture_scene_work_preservation(
            &mut harness.renderer,
            (8, 6),
            framebuffer_origin,
            &regions,
        )
        .expect("scene-work preservation capture succeeds");

        assert_eq!(preservation.transfer_count(), 1);
        assert_eq!(preservation.preserved_pixels(), 6);

        poison_diagnostic_output(&harness, [0.07, 0.19, 0.31, 1.0]);
        let overwritten = read_diagnostic_pixels(&harness);
        let restore_result =
            effects::restore_scene_work_preservation(&mut harness.renderer, &preservation);
        let release_result = preservation.release(&mut harness.renderer);
        restore_result.expect("scene-work preservation restore succeeds");
        release_result.expect("scene-work preservation texture releases");

        let actual = read_diagnostic_pixels(&harness);
        for y in 0..6 {
            for x in 0..8 {
                let source = if diagnostic_pixel_is_inside_repairs(&regions, x, y) {
                    &original
                } else {
                    &overwritten
                };
                let expected = diagnostic_pixel_for_origin(source, 8, 6, x, y, framebuffer_origin);
                let actual_pixel =
                    diagnostic_pixel_for_origin(&actual, 8, 6, x, y, framebuffer_origin);
                assert_eq!(
                    actual_pixel, expected,
                    "pixel ({x}, {y}) for {framebuffer_origin:?}"
                );
            }
        }
    }
}

#[test]
fn real_gles_copy_preserves_logical_and_physical_orientation_contracts() {
    let mut harness = GlesEffectTestHarness::new(2, 2);
    let program = program::create_program_from_sources(
        &harness.gl,
        effects::DUAL_KAWASE_VERTEX_SHADER,
        effects::COPY_FRAGMENT_SHADER,
    )
    .expect("copy effect program compiles with the shared vertex shader");
    let input_texture =
        create_effect_test_texture(&harness.gl, 2, 2, &canonical_two_by_two_pixels());
    let quad = harness
        .renderer
        .ensure_effect_quad()
        .expect("effect quad creates")
        .0;

    draw_effect_test(
        &harness.gl,
        program,
        quad,
        input_texture,
        2,
        2,
        false,
        Some(("u_effect_input_flip_y", true)),
        |_, _| {},
    );
    let bottom_left_pixels = read_effect_test_pixels(&harness.gl, 2, 2);
    assert_effect_test_pixel(&bottom_left_pixels, 2, 0, 0, [0, 0, 255, 255]);
    assert_effect_test_pixel(&bottom_left_pixels, 2, 0, 1, [255, 0, 0, 255]);

    draw_effect_test(
        &harness.gl,
        program,
        quad,
        input_texture,
        2,
        2,
        true,
        Some(("u_effect_input_flip_y", true)),
        |_, _| {},
    );
    let top_left_scanout_pixels = read_effect_test_pixels(&harness.gl, 2, 2);
    assert_effect_test_pixel(&top_left_scanout_pixels, 2, 0, 0, [255, 0, 0, 255]);
    assert_effect_test_pixel(&top_left_scanout_pixels, 2, 0, 1, [0, 0, 255, 255]);

    unsafe {
        harness.gl.delete_texture(input_texture);
        harness.gl.delete_program(program);
    }
}

#[test]
fn real_gles_partial_kawase_is_independent_of_unproduced_sentinels() {
    let mut harness = GlesEffectTestHarness::new(556, 437);
    let program = program::create_program_from_sources(
        &harness.gl,
        effects::DUAL_KAWASE_VERTEX_SHADER,
        effects::DUAL_KAWASE_DOWNSAMPLE_LINEAR_SHADER,
    )
    .expect("sentinel regression downsample program compiles");
    let quad = harness
        .renderer
        .ensure_effect_quad()
        .expect("sentinel regression effect quad creates")
        .0;
    let output_coverage = GraphTexturePhysicalRect {
        left: 251,
        top: 376,
        right: 556,
        bottom: 437,
    };
    let produced_input = GraphTexturePhysicalRect {
        left: 497,
        top: 747,
        right: 1112,
        bottom: 873,
    };
    let scissor = [
        output_coverage.left as i32,
        {
            let output_height = 437_u32;
            let output_bottom = output_coverage.bottom;
            output_height.saturating_sub(output_bottom) as i32
        },
        output_coverage.width() as i32,
        output_coverage.height() as i32,
    ];
    let mut gradient = vec![0_u8; 1112 * 873 * 4];
    for logical_y in 0..873_u32 {
        let texture_y = 872 - logical_y;
        for x in 0..1112_u32 {
            let offset = ((texture_y * 1112 + x) * 4) as usize;
            gradient[offset..offset + 4].copy_from_slice(&[
                (x & 0xff) as u8,
                (logical_y & 0xff) as u8,
                ((x.wrapping_add(logical_y)) & 0xff) as u8,
                255,
            ]);
        }
    }
    let sentinel_texture_pixels = |sentinel: [u8; 4]| {
        let mut pixels = gradient.clone();
        for logical_y in 0..873_u32 {
            let texture_y = 872 - logical_y;
            for x in 0..1112_u32 {
                if !produced_input.contains(x, logical_y) {
                    let offset = ((texture_y * 1112 + x) * 4) as usize;
                    pixels[offset..offset + 4].copy_from_slice(&sentinel);
                }
            }
        }
        pixels
    };
    let configure = |gl: &glow::Context, program: glow::Program| {
        set_effect_test_uniform_i32(gl, program, "u_effect_input", 0);
        set_effect_test_uniform_2_f32(
            gl,
            program,
            "u_effect_texel_size",
            1.0 / 1112.0,
            1.0 / 873.0,
        );
        let radius = unsafe {
            gl.get_uniform_location(program, "u_effect_blur_radius")
                .expect("sentinel blur radius is active")
        };
        unsafe { gl.uniform_1_f32(Some(&radius), 4.0) };
    };
    let render = |input_texture: glow::Texture, scissor: Option<[i32; 4]>| {
        unsafe {
            harness.gl.viewport(0, 0, 556, 437);
            harness.gl.disable(glow::SCISSOR_TEST);
            harness.gl.clear_color(0.0, 0.0, 0.0, 0.0);
            harness.gl.clear(glow::COLOR_BUFFER_BIT);
            harness.gl.use_program(Some(program));
            set_effect_test_uniform_i32(&harness.gl, program, "u_effect_target_flip_y", 0);
            set_effect_test_uniform_i32(&harness.gl, program, "u_effect_input_flip_y", 1);
            configure(&harness.gl, program);
            harness
                .gl
                .bind_texture(glow::TEXTURE_2D, Some(input_texture));
            harness.gl.bind_vertex_array(Some(quad));
            if let Some([x, y, width, height]) = scissor {
                harness.gl.enable(glow::SCISSOR_TEST);
                harness.gl.scissor(x, y, width, height);
            }
            harness.gl.draw_arrays(glow::TRIANGLES, 0, 6);
            harness.gl.bind_vertex_array(None);
            harness.gl.disable(glow::SCISSOR_TEST);
            harness.gl.bind_texture(glow::TEXTURE_2D, None);
            harness.gl.use_program(None);
        }
        read_effect_test_pixels(&harness.gl, 556, 437)
    };

    let full_input = create_effect_test_texture(&harness.gl, 1112, 873, &gradient);
    set_effect_test_texture_filter(&harness.gl, full_input, glow::LINEAR);
    let full_reference = render(full_input, None);
    let sentinel_a = sentinel_texture_pixels([255, 0, 255, 255]);
    let input_a = create_effect_test_texture(&harness.gl, 1112, 873, &sentinel_a);
    set_effect_test_texture_filter(&harness.gl, input_a, glow::LINEAR);
    let partial_a = render(input_a, Some(scissor));
    let sentinel_b = sentinel_texture_pixels([0, 0, 0, 0]);
    let input_b = create_effect_test_texture(&harness.gl, 1112, 873, &sentinel_b);
    set_effect_test_texture_filter(&harness.gl, input_b, glow::LINEAR);
    let partial_b = render(input_b, Some(scissor));

    for logical_y in output_coverage.top..output_coverage.bottom {
        let framebuffer_y = output_coverage.bottom - logical_y - 1;
        for x in output_coverage.left..output_coverage.right {
            let output_pixel = ((framebuffer_y * 556 + x) * 4) as usize;
            assert_eq!(
                &partial_a[output_pixel..output_pixel + 4],
                &full_reference[output_pixel..output_pixel + 4],
                "partial Kawase output samples only producer texels for sentinel A at logical ({x},{logical_y})"
            );
            assert_eq!(
                &partial_b[output_pixel..output_pixel + 4],
                &full_reference[output_pixel..output_pixel + 4],
                "partial Kawase output samples only producer texels for sentinel B at logical ({x},{logical_y})"
            );
            assert_eq!(
                &partial_a[output_pixel..output_pixel + 4],
                &partial_b[output_pixel..output_pixel + 4],
                "partial Kawase output is sentinel-independent at logical ({x},{logical_y})"
            );
        }
    }

    unsafe {
        harness.gl.delete_texture(full_input);
        harness.gl.delete_texture(input_a);
        harness.gl.delete_texture(input_b);
        harness.gl.delete_program(program);
    }
}

#[test]
fn internal_fullscreen_pass_is_independent_of_stale_target_contents() {
    let mut harness = GlesEffectTestHarness::new(1, 1);
    let program = program::create_program_from_sources(
        &harness.gl,
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
    .expect("internal pass stale-target regression shader compiles");
    let quad = harness
        .renderer
        .ensure_effect_quad()
        .expect("internal pass stale-target regression quad creates")
        .0;
    let color = unsafe {
        harness
            .gl
            .get_uniform_location(program, "u_color")
            .expect("internal pass color uniform is active")
    };

    let render_with_stale_destination = |stale: [f32; 4]| {
        unsafe {
            harness.gl.viewport(0, 0, 1, 1);
            harness.gl.disable(glow::SCISSOR_TEST);
            harness
                .gl
                .clear_color(stale[0], stale[1], stale[2], stale[3]);
            harness.gl.clear(glow::COLOR_BUFFER_BIT);
            effects::establish_effect_pass_blend_state(
                &harness.gl,
                effects::EffectPassBlendMode::Replace,
            );
            assert!(!harness.gl.is_enabled(glow::BLEND));
            harness.gl.use_program(Some(program));
            harness.gl.uniform_4_f32(Some(&color), 0.2, 0.1, 0.05, 0.5);
            harness.gl.bind_vertex_array(Some(quad));
            harness.gl.draw_arrays(glow::TRIANGLES, 0, 6);
            harness.gl.bind_vertex_array(None);
            harness.gl.flush();
        }
        read_effect_test_pixels(&harness.gl, 1, 1)
    };

    let first = render_with_stale_destination([0.05, 0.1, 0.15, 0.25]);
    let second = render_with_stale_destination([0.75, 0.6, 0.45, 0.9]);

    assert_eq!(first, second);

    unsafe {
        harness.gl.use_program(None);
        harness.gl.delete_program(program);
    }
}

#[test]
fn real_gles_effect_pass_modes_establish_their_fixed_function_state() {
    let harness = GlesEffectTestHarness::new(1, 1);

    effects::establish_effect_pass_blend_state(
        &harness.gl,
        effects::EffectPassBlendMode::PremultipliedSourceOver,
    );
    unsafe {
        assert!(harness.gl.is_enabled(glow::BLEND));
        assert_eq!(
            harness.gl.get_parameter_i32(glow::BLEND_SRC_RGB),
            glow::ONE as i32
        );
        assert_eq!(
            harness.gl.get_parameter_i32(glow::BLEND_DST_RGB),
            glow::ONE_MINUS_SRC_ALPHA as i32
        );
    }

    effects::establish_effect_pass_blend_state(&harness.gl, effects::EffectPassBlendMode::Replace);
    unsafe { assert!(!harness.gl.is_enabled(glow::BLEND)) };
}

#[test]
fn real_gles_capture_regions_source_over_each_pixel_once() {
    let mut harness = GlesEffectTestHarness::new(3, 1);
    let program = program::create_program_from_sources(
        &harness.gl,
        r#"#version 300 es
            layout(location = 0) in vec2 a_position;
            void main() { gl_Position = vec4(a_position, 0.0, 1.0); }
        "#,
        r#"#version 300 es
            precision highp float;
            out vec4 out_color;
            void main() { out_color = vec4(0.2, 0.1, 0.05, 0.5); }
        "#,
    )
    .expect("capture overlap regression shader compiles");
    let quad = harness
        .renderer
        .ensure_effect_quad()
        .expect("capture overlap regression quad creates")
        .0;
    let mut requested =
        EffectRegion::from_rect(oblivion_one::effects::EffectRect::new(0, 0, 2, 1).unwrap());
    requested.push(oblivion_one::effects::EffectRect::new(1, 0, 2, 1).unwrap());
    let execution = requested.disjoint_bounded();

    unsafe {
        harness.gl.viewport(0, 0, 3, 1);
        harness.gl.disable(glow::SCISSOR_TEST);
        harness.gl.clear_color(0.0, 0.0, 0.0, 0.0);
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
        effects::establish_effect_pass_blend_state(
            &harness.gl,
            effects::EffectPassBlendMode::PremultipliedSourceOver,
        );
        harness.gl.use_program(Some(program));
        harness.gl.bind_vertex_array(Some(quad));
        for rect in execution.region.rects() {
            harness.gl.enable(glow::SCISSOR_TEST);
            harness
                .gl
                .scissor(rect.x, rect.y, rect.width as i32, rect.height as i32);
            harness.gl.draw_arrays(glow::TRIANGLES, 0, 6);
        }
        harness.gl.bind_vertex_array(None);
        harness.gl.disable(glow::SCISSOR_TEST);
        harness.gl.flush();
    }
    let pixels = read_effect_test_pixels(&harness.gl, 3, 1);
    for x in 0..3 {
        assert_effect_test_pixel(&pixels, 3, x, 0, [51, 25, 13, 127]);
    }

    unsafe {
        harness.gl.use_program(None);
        harness.gl.delete_program(program);
    }
}

#[test]
fn real_gles_composite_keeps_logical_domain_and_orientation() {
    let mut harness = GlesEffectTestHarness::new(4, 6);
    let program = program::create_program_from_sources(
        &harness.gl,
        effects::DUAL_KAWASE_VERTEX_SHADER,
        effects::COMPOSITE_FRAGMENT_SHADER,
    )
    .expect("composite effect program compiles with the shared vertex shader");
    let input_texture =
        create_effect_test_texture(&harness.gl, 2, 2, &canonical_two_by_two_pixels());
    let quad = harness
        .renderer
        .ensure_effect_quad()
        .expect("effect quad creates")
        .0;
    let configure = |gl: &glow::Context, program: glow::Program| {
        set_effect_test_uniform_4_f32(gl, program, "u_effect_input_domain", [1.0, 1.0, 2.0, 2.0]);
        set_effect_test_uniform_2_f32(gl, program, "u_effect_output_size", 4.0, 6.0);
        set_effect_test_uniform_i32(gl, program, "u_effect_encode_srgb", 0);
        set_effect_test_uniform_i32(gl, program, "u_effect_force_opaque", 0);
        let opacity = unsafe {
            gl.get_uniform_location(program, "u_presentation_opacity")
                .expect("composite presentation opacity is active")
        };
        unsafe { gl.uniform_1_f32(Some(&opacity), 1.0) };
    };

    draw_effect_test(
        &harness.gl,
        program,
        quad,
        input_texture,
        4,
        6,
        false,
        Some(("u_effect_input_flip_y", true)),
        configure,
    );
    let bottom_left_pixels = read_effect_test_pixels(&harness.gl, 4, 6);
    assert_effect_test_pixel(&bottom_left_pixels, 4, 1, 4, [255, 0, 0, 255]);
    assert_effect_test_pixel(&bottom_left_pixels, 4, 2, 4, [255, 0, 0, 255]);
    assert_effect_test_pixel(&bottom_left_pixels, 4, 1, 3, [0, 0, 255, 255]);
    assert_effect_test_pixel(&bottom_left_pixels, 4, 2, 3, [0, 0, 255, 255]);
    assert_effect_test_pixel(&bottom_left_pixels, 4, 1, 1, [0, 0, 0, 0]);

    draw_effect_test(
        &harness.gl,
        program,
        quad,
        input_texture,
        4,
        6,
        true,
        Some(("u_effect_input_flip_y", true)),
        configure,
    );
    let top_left_scanout_pixels = read_effect_test_pixels(&harness.gl, 4, 6);
    assert_effect_test_pixel(&top_left_scanout_pixels, 4, 1, 1, [255, 0, 0, 255]);
    assert_effect_test_pixel(&top_left_scanout_pixels, 4, 2, 1, [255, 0, 0, 255]);
    assert_effect_test_pixel(&top_left_scanout_pixels, 4, 1, 2, [0, 0, 255, 255]);
    assert_effect_test_pixel(&top_left_scanout_pixels, 4, 2, 2, [0, 0, 255, 255]);
    assert_effect_test_pixel(&top_left_scanout_pixels, 4, 1, 4, [0, 0, 0, 0]);

    unsafe {
        harness.gl.delete_texture(input_texture);
        harness.gl.delete_program(program);
    }
}

#[test]
fn real_gles_opaque_effect_with_translucent_presentation_uses_source_over() {
    let mut harness = GlesEffectTestHarness::new(1, 1);
    let program = program::create_program_from_sources(
        &harness.gl,
        effects::DUAL_KAWASE_VERTEX_SHADER,
        effects::COMPOSITE_FRAGMENT_SHADER,
    )
    .expect("opaque-effect presentation regression program compiles");
    let input_texture = create_effect_test_texture(&harness.gl, 1, 1, &[102, 51, 26, 255]);
    let quad = harness
        .renderer
        .ensure_effect_quad()
        .expect("opaque-effect presentation regression quad creates")
        .0;
    let source = oblivion_one::effects::PremultipliedRgba::new(0.4, 0.2, 0.1, 1.0);
    let destination = oblivion_one::effects::PremultipliedRgba::new(0.1, 0.05, 0.02, 0.25);
    let expected = destination.blend(source, oblivion_one::effects::BlendMode::SourceOver, 0.5);

    unsafe {
        harness.gl.viewport(0, 0, 1, 1);
        harness.gl.disable(glow::SCISSOR_TEST);
        harness
            .gl
            .clear_color(destination.r, destination.g, destination.b, destination.a);
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
        effects::establish_effect_pass_blend_state(
            &harness.gl,
            effects::EffectPassBlendMode::PremultipliedSourceOver,
        );
        harness.gl.use_program(Some(program));
        set_effect_test_uniform_i32(&harness.gl, program, "u_effect_input", 0);
        set_effect_test_uniform_i32(&harness.gl, program, "u_effect_target_flip_y", 0);
        set_effect_test_uniform_i32(&harness.gl, program, "u_effect_input_flip_y", 0);
        set_effect_test_uniform_i32(&harness.gl, program, "u_effect_encode_srgb", 0);
        set_effect_test_uniform_i32(&harness.gl, program, "u_effect_force_opaque", 1);
        set_effect_test_uniform_4_f32(
            &harness.gl,
            program,
            "u_effect_input_domain",
            [0.0, 0.0, 1.0, 1.0],
        );
        set_effect_test_uniform_2_f32(&harness.gl, program, "u_effect_output_size", 1.0, 1.0);
        let opacity = harness
            .gl
            .get_uniform_location(program, "u_presentation_opacity")
            .expect("presentation opacity uniform is active");
        harness.gl.uniform_1_f32(Some(&opacity), 0.5);
        harness.gl.active_texture(glow::TEXTURE0);
        harness
            .gl
            .bind_texture(glow::TEXTURE_2D, Some(input_texture));
        harness.gl.bind_vertex_array(Some(quad));
        harness.gl.draw_arrays(glow::TRIANGLES, 0, 6);
        harness.gl.bind_vertex_array(None);
        harness.gl.bind_texture(glow::TEXTURE_2D, None);
        harness.gl.use_program(None);
    }

    let pixels = read_effect_test_pixels(&harness.gl, 1, 1);
    let expected = [
        (expected.r * 255.0).round() as u8,
        (expected.g * 255.0).round() as u8,
        (expected.b * 255.0).round() as u8,
        (expected.a * 255.0).round() as u8,
    ];
    let actual = &pixels[..4];
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (*actual as i16 - expected as i16).abs() <= 3,
            "opaque effect source-over mismatch: actual={actual}, expected={expected}, pixel={pixels:?}"
        );
    }

    unsafe {
        harness.gl.delete_texture(input_texture);
        harness.gl.delete_program(program);
    }
}

#[test]
fn real_gles_analytic_coverage_has_fractional_1x_edges_and_preserves_opaque_parity() {
    const WIDTH: u32 = 90;
    const HEIGHT: u32 = 50;
    let mut harness = GlesEffectTestHarness::new(WIDTH, HEIGHT);
    let program = program::create_program_from_sources(
        &harness.gl,
        effects::DUAL_KAWASE_VERTEX_SHADER,
        effects::COMPOSITE_FRAGMENT_SHADER,
    )
    .expect("analytic coverage composite shader compiles");
    let input_texture = create_effect_test_texture(&harness.gl, 1, 1, &[200, 100, 50, 255]);
    let quad = harness
        .renderer
        .ensure_effect_quad()
        .expect("analytic coverage quad creates")
        .0;
    let background = [20.0_f32 / 255.0, 40.0 / 255.0, 80.0 / 255.0, 1.0];
    let configure = |coverage_enabled: i32| unsafe {
        set_effect_test_uniform_i32(&harness.gl, program, "u_effect_input", 0);
        set_effect_test_uniform_i32(&harness.gl, program, "u_effect_target_flip_y", 0);
        set_effect_test_uniform_i32(&harness.gl, program, "u_effect_input_flip_y", 0);
        set_effect_test_uniform_i32(&harness.gl, program, "u_effect_encode_srgb", 0);
        set_effect_test_uniform_i32(&harness.gl, program, "u_effect_force_opaque", 1);
        let opacity = harness
            .gl
            .get_uniform_location(program, "u_presentation_opacity")
            .expect("presentation opacity uniform is active");
        harness.gl.uniform_1_f32(Some(&opacity), 1.0);
        set_effect_test_uniform_4_f32(
            &harness.gl,
            program,
            "u_effect_input_domain",
            [0.0, 0.0, WIDTH as f32, HEIGHT as f32],
        );
        set_effect_test_uniform_2_f32(
            &harness.gl,
            program,
            "u_effect_output_size",
            WIDTH as f32,
            HEIGHT as f32,
        );
        set_effect_test_uniform_i32(
            &harness.gl,
            program,
            "u_effect_coverage_enabled",
            coverage_enabled,
        );
        set_effect_test_uniform_i32(
            &harness.gl,
            program,
            "u_effect_coverage_has_rounded_rect",
            1,
        );
        set_effect_test_uniform_4_f32(
            &harness.gl,
            program,
            "u_effect_coverage_rounded_rect",
            [9.25, 7.5, 72.0, 34.0],
        );
        let radius = harness
            .gl
            .get_uniform_location(program, "u_effect_coverage_radius")
            .expect("rounded rectangle radius uniform is active");
        harness.gl.uniform_1_f32(Some(&radius), 17.0);
        set_effect_test_uniform_i32(&harness.gl, program, "u_effect_coverage_has_triangle", 1);
        set_effect_test_uniform_2_f32(
            &harness.gl,
            program,
            "u_effect_coverage_triangle_a",
            35.0,
            40.5,
        );
        set_effect_test_uniform_2_f32(
            &harness.gl,
            program,
            "u_effect_coverage_triangle_b",
            47.0,
            40.5,
        );
        set_effect_test_uniform_2_f32(
            &harness.gl,
            program,
            "u_effect_coverage_triangle_c",
            41.0,
            48.0,
        );
    };

    unsafe {
        harness.gl.viewport(0, 0, WIDTH as i32, HEIGHT as i32);
        harness.gl.disable(glow::SCISSOR_TEST);
        harness
            .gl
            .clear_color(background[0], background[1], background[2], background[3]);
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
        effects::establish_effect_pass_blend_state(
            &harness.gl,
            effects::EffectPassBlendMode::PremultipliedSourceOver,
        );
        harness.gl.use_program(Some(program));
        configure(1);
        harness.gl.active_texture(glow::TEXTURE0);
        harness
            .gl
            .bind_texture(glow::TEXTURE_2D, Some(input_texture));
        harness.gl.enable(glow::SCISSOR_TEST);
        // Coarse public work clip contains the body, tail, and a conservative AA guard.
        harness.gl.scissor(7, 5, 78, 44);
        harness.gl.bind_vertex_array(Some(quad));
        harness.gl.draw_arrays(glow::TRIANGLES, 0, 6);
        harness.gl.bind_vertex_array(None);
        harness.gl.disable(glow::SCISSOR_TEST);
        harness.gl.bind_texture(glow::TEXTURE_2D, None);
        harness.gl.use_program(None);
    }
    let pixels = read_effect_test_pixels(&harness.gl, WIDTH, HEIGHT);
    assert_effect_test_pixel(&pixels, WIDTH, 45, 24, [200, 100, 50, 255]);
    assert_effect_test_pixel(&pixels, WIDTH, 2, 24, [20, 40, 80, 255]);
    let partial = (5..49)
        .flat_map(|y| (7..85).map(move |x| (x, y)))
        .find_map(|(x, y)| {
            let pixel = effect_test_pixel(&pixels, WIDTH, x, y);
            (pixel[0] > 22 && pixel[0] < 198).then_some((x, y, pixel))
        })
        .expect("1x GLES edge contains a genuinely fractional coverage pixel");
    let coverage = f32::from(partial.2[0] - 20) / 180.0;
    eprintln!(
        "analytic coverage GLES edge at ({}, {}): rgba={:?}, inferred_coverage={coverage:.3}",
        partial.0, partial.1, partial.2
    );
    let expected_g = (100.0 * coverage + 40.0 * (1.0 - coverage)).round() as i32;
    let expected_b = (50.0 * coverage + 80.0 * (1.0 - coverage)).round() as i32;
    assert!((i32::from(partial.2[1]) - expected_g).abs() <= 2);
    assert!((i32::from(partial.2[2]) - expected_b).abs() <= 2);
    assert_eq!(partial.2[3], 255);

    unsafe {
        harness.gl.disable(glow::BLEND);
        harness
            .gl
            .clear_color(background[0], background[1], background[2], background[3]);
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
        harness.gl.use_program(Some(program));
        configure(0);
        harness.gl.active_texture(glow::TEXTURE0);
        harness
            .gl
            .bind_texture(glow::TEXTURE_2D, Some(input_texture));
        harness.gl.enable(glow::SCISSOR_TEST);
        harness.gl.scissor(7, 5, 78, 44);
        harness.gl.bind_vertex_array(Some(quad));
        harness.gl.draw_arrays(glow::TRIANGLES, 0, 6);
        harness.gl.bind_vertex_array(None);
        harness.gl.disable(glow::SCISSOR_TEST);
        harness.gl.bind_texture(glow::TEXTURE_2D, None);
        harness.gl.use_program(None);
    }
    let legacy_pixels = read_effect_test_pixels(&harness.gl, WIDTH, HEIGHT);
    assert_effect_test_pixel(&legacy_pixels, WIDTH, 45, 24, [200, 100, 50, 255]);
    assert_effect_test_pixel(&legacy_pixels, WIDTH, 2, 24, [20, 40, 80, 255]);
    assert_effect_test_pixel(&legacy_pixels, WIDTH, 7, 24, [200, 100, 50, 255]);

    unsafe {
        harness.gl.delete_texture(input_texture);
        harness.gl.delete_program(program);
    }
}

#[test]
fn real_gles_normalize_uses_shared_logical_uv_and_sample_conversion() {
    let mut harness = GlesEffectTestHarness::new(4, 4);
    let program = program::create_program_from_sources(
        &harness.gl,
        effects::DUAL_KAWASE_VERTEX_SHADER,
        effects::NORMALIZE_FRAGMENT_SHADER,
    )
    .expect("normalize effect program compiles with the shared vertex shader");
    let mut input_pixels = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4 {
        let color = if y < 2 {
            [0, 0, 255, 255]
        } else {
            [255, 0, 0, 255]
        };
        for _ in 0..4 {
            input_pixels.extend(color);
        }
    }
    let input_texture = create_effect_test_texture(&harness.gl, 4, 4, &input_pixels);
    let quad = harness
        .renderer
        .ensure_effect_quad()
        .expect("effect quad creates")
        .0;

    draw_effect_test(
        &harness.gl,
        program,
        quad,
        input_texture,
        4,
        4,
        false,
        Some(("u_effect_input_flip_y", true)),
        |gl, program| {
            set_effect_test_uniform_4_f32(
                gl,
                program,
                "u_effect_input_domain",
                [1.0, 1.0, 2.0, 2.0],
            );
            set_effect_test_uniform_4_f32(
                gl,
                program,
                "u_effect_output_domain",
                [0.0, 0.0, 4.0, 4.0],
            );
            set_effect_test_uniform_i32(gl, program, "u_effect_decode_srgb", 0);
            set_effect_test_uniform_i32(gl, program, "u_effect_encode_srgb", 0);
        },
    );
    let pixels = read_effect_test_pixels(&harness.gl, 4, 4);
    assert_effect_test_pixel(&pixels, 4, 1, 2, [255, 0, 0, 255]);
    assert_effect_test_pixel(&pixels, 4, 2, 2, [255, 0, 0, 255]);
    assert_effect_test_pixel(&pixels, 4, 1, 1, [0, 0, 255, 255]);
    assert_effect_test_pixel(&pixels, 4, 2, 1, [0, 0, 255, 255]);
    assert_effect_test_pixel(&pixels, 4, 0, 0, [0, 0, 0, 0]);

    unsafe {
        harness.gl.delete_texture(input_texture);
        harness.gl.delete_program(program);
    }
}

#[test]
fn real_gles_trusted_wrapper_preserves_logical_context_uv() {
    let mut harness = GlesEffectTestHarness::new(2, 2);
    let wrapper = effects::generate_fragment_wrapper(
        "vec4 typhon_effect_main(TyphonEffectContext ctx) { return typhon_sample_primary(ctx.uv); }",
        &[],
    )
    .expect("trusted wrapper generates");
    let program = program::create_program_from_sources(
        &harness.gl,
        effects::DUAL_KAWASE_VERTEX_SHADER,
        &wrapper,
    )
    .expect("trusted wrapper compiles with the shared vertex shader");
    let input_texture =
        create_effect_test_texture(&harness.gl, 2, 2, &canonical_two_by_two_pixels());
    let quad = harness
        .renderer
        .ensure_effect_quad()
        .expect("effect quad creates")
        .0;

    draw_effect_test(
        &harness.gl,
        program,
        quad,
        input_texture,
        2,
        2,
        false,
        None,
        |gl, program| {
            set_effect_test_uniform_i32(gl, program, "u_typhon_primary", 0);
            set_effect_test_uniform_i32(gl, program, "u_typhon_input_flip_y", 1);
            set_effect_test_uniform_i32(gl, program, "u_typhon_decode_srgb", 0);
            set_effect_test_uniform_i32(gl, program, "u_typhon_encode_srgb", 0);
        },
    );
    let pixels = read_effect_test_pixels(&harness.gl, 2, 2);
    assert_effect_test_pixel(&pixels, 2, 0, 0, [0, 0, 255, 255]);
    assert_effect_test_pixel(&pixels, 2, 0, 1, [255, 0, 0, 255]);

    unsafe {
        harness.gl.delete_texture(input_texture);
        harness.gl.delete_program(program);
    }
}
