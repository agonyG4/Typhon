use super::*;
use glow::HasContext;

#[test]
fn canonical_offscreen_dither_guard_restores_the_exact_previous_state() {
    let harness = GlesEffectTestHarness::new(4, 4);
    for initially_enabled in [true, false] {
        unsafe {
            if initially_enabled {
                harness.gl.enable(glow::DITHER);
            } else {
                harness.gl.disable(glow::DITHER);
            }
        }
        let draw_result = (|| -> Result<(), ()> {
            let _restore = effects::canonical_offscreen_dither_restore(
                &harness.gl,
                effects::CanonicalBlurSemanticsPolicy::Canonical,
                true,
            )
            .expect("canonical offscreen writes must own a DITHER guard");
            assert!(!unsafe { harness.gl.is_enabled(glow::DITHER) });
            Err(())
        })();
        assert_eq!(draw_result, Err(()));
        assert_eq!(
            unsafe { harness.gl.is_enabled(glow::DITHER) },
            initially_enabled,
            "the offscreen DITHER state must restore after an early return"
        );
    }

    unsafe { harness.gl.enable(glow::DITHER) };
    assert!(
        effects::canonical_offscreen_dither_restore(
            &harness.gl,
            effects::CanonicalBlurSemanticsPolicy::Legacy,
            true,
        )
        .is_none()
    );
    assert!(
        effects::canonical_offscreen_dither_restore(
            &harness.gl,
            effects::CanonicalBlurSemanticsPolicy::Canonical,
            false,
        )
        .is_none()
    );
    assert!(unsafe { harness.gl.is_enabled(glow::DITHER) });
}

fn compare_mrt_store_with_capture(
    label: &str,
    capture: &effects::DiagnosticTextureCapture,
    probe: &super::rgba16f_store::MrtProbeReadback,
) {
    assert_eq!((capture.width, capture.height), (probe.width, probe.height));
    let mut differing_texels = 0;
    let mut differing_channels = 0;
    let mut max_delta = 0.0_f32;
    for (expected, actual) in capture.pixels.iter().zip(&probe.stored) {
        let mut texel_differs = false;
        for channel in 0..4 {
            let delta = (expected[channel] - actual[channel]).abs();
            max_delta = max_delta.max(delta);
            if expected[channel].to_bits() != actual[channel].to_bits() {
                differing_channels += 1;
                texel_differs = true;
            }
        }
        differing_texels += usize::from(texel_differs);
    }
    println!(
        "{label} vs MRT store texels={} channels={} max_delta={max_delta}",
        differing_texels, differing_channels
    );
    let classification = super::rgba16f_store::classify_mrt(probe);
    println!("{label} MRT classification={classification:?}");
    super::rgba16f_store::report_mrt_samples(label, probe, 4);
}

fn diagnostic_saturation_matrix(saturation: f32) -> [f32; 16] {
    let desaturation = 1.0 - saturation;
    let red = 0.2126 * desaturation;
    let green = 0.7152 * desaturation;
    let blue = 0.0722 * desaturation;
    [
        red + saturation,
        red,
        red,
        0.0,
        green,
        green + saturation,
        green,
        0.0,
        blue,
        blue,
        blue + saturation,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

fn set_mrt_i32(gl: &glow::Context, program: glow::Program, name: &str, value: i32) {
    unsafe {
        if let Some(location) = gl.get_uniform_location(program, name) {
            gl.uniform_1_i32(Some(&location), value);
        }
    }
}

fn set_mrt_f32(gl: &glow::Context, program: glow::Program, name: &str, value: f32) {
    unsafe {
        if let Some(location) = gl.get_uniform_location(program, name) {
            gl.uniform_1_f32(Some(&location), value);
        }
    }
}

fn set_mrt_vec2(gl: &glow::Context, program: glow::Program, name: &str, value: [f32; 2]) {
    unsafe {
        if let Some(location) = gl.get_uniform_location(program, name) {
            gl.uniform_2_f32(Some(&location), value[0], value[1]);
        }
    }
}

fn run_same_invocation_boundary_probes(
    harness: &GlesEffectTestHarness,
    replay: &effects::TerminalBoundaryReplayExecution,
    up2_input_pass: oblivion_one::effects::GraphPassId,
    up2_pass: &CompiledRenderPass,
    up2_id: oblivion_one::effects::GraphPassId,
    fragment_id: oblivion_one::effects::GraphPassId,
    saturation: f32,
    dither: bool,
    case_name: &str,
    origin: OutputFramebufferOrigin,
) {
    let up2_input = replay
        .captures
        .get(&up2_input_pass)
        .expect("legacy replay captures the stored Up2 input");
    let a0 = replay
        .captures
        .get(&up2_id)
        .expect("legacy replay captures A0");
    let b0 = replay
        .captures
        .get(&fragment_id)
        .expect("legacy replay captures B0");
    let gl = &harness.gl;
    let up2_input_texture = super::rgba16f_store::upload_rgba16f_texture(
        gl,
        up2_input.width,
        up2_input.height,
        &up2_input.pixels,
        true,
    );
    let up2_probe_shader = super::rgba16f_store::instrument_mrt_fragment_shader(
        effects::DUAL_KAWASE_UPSAMPLE_SHADER,
        "    out_color = result;",
    );
    let up2_probe = super::rgba16f_store::run_mrt_probe(
        gl,
        a0.width,
        a0.height,
        effects::DUAL_KAWASE_VERTEX_SHADER,
        &up2_probe_shader,
        Some(up2_input_texture),
        dither,
        |gl, program| {
            set_mrt_i32(
                gl,
                program,
                "u_effect_input_flip_y",
                i32::from(
                    up2_input.origin == oblivion_one::effects::GraphTextureOrigin::BottomLeft,
                ),
            );
            set_mrt_vec2(
                gl,
                program,
                "u_effect_texel_size",
                [
                    1.0 / up2_input.width.max(1) as f32,
                    1.0 / up2_input.height.max(1) as f32,
                ],
            );
            set_mrt_f32(
                gl,
                program,
                "u_effect_blur_radius",
                up2_pass.blur_radius.unwrap_or(1.0),
            );
        },
    );
    unsafe { gl.delete_texture(up2_input_texture) };
    assert_eq!((up2_probe.width, up2_probe.height), (a0.width, a0.height));
    let up2_label = format!("A0_vs_M0 case={case_name} origin={origin:?} dither={dither}");
    compare_mrt_store_with_capture(&up2_label, a0, &up2_probe);

    let a0_texture =
        super::rgba16f_store::upload_rgba16f_texture(gl, a0.width, a0.height, &a0.pixels, true);
    let fragment_probe_shader = super::rgba16f_store::instrument_mrt_fragment_shader(
        effects::FRAGMENT_STAGE_FRAGMENT_SHADER,
        "    out_color = typhon_sanitize_premultiplied(result);",
    );
    let matrix = diagnostic_saturation_matrix(saturation);
    let fragment_probe = super::rgba16f_store::run_mrt_probe(
        gl,
        b0.width,
        b0.height,
        effects::DUAL_KAWASE_VERTEX_SHADER,
        &fragment_probe_shader,
        Some(a0_texture),
        dither,
        |gl, program| unsafe {
            set_mrt_i32(
                gl,
                program,
                "u_effect_input_flip_y",
                i32::from(a0.origin == oblivion_one::effects::GraphTextureOrigin::BottomLeft),
            );
            set_mrt_i32(gl, program, "u_effect_decode_srgb", 0);
            set_mrt_i32(gl, program, "u_effect_encode_srgb", 0);
            if let Some(location) = gl.get_uniform_location(program, "u_effect_color_matrix") {
                gl.uniform_matrix_4_f32_slice(Some(&location), false, &matrix);
            }
            if let Some(location) = gl.get_uniform_location(program, "u_effect_color_bias") {
                gl.uniform_4_f32(Some(&location), 0.0, 0.0, 0.0, 0.0);
            }
            if let Some(location) = gl.get_uniform_location(program, "u_effect_tint_color") {
                gl.uniform_4_f32(Some(&location), 1.0, 1.0, 1.0, 1.0);
            }
            set_mrt_f32(gl, program, "u_effect_tint_amount", 0.0);
            set_mrt_f32(gl, program, "u_effect_noise_amount", 0.0);
        },
    );
    unsafe { gl.delete_texture(a0_texture) };
    assert_eq!(
        (fragment_probe.width, fragment_probe.height),
        (b0.width, b0.height)
    );
    let fragment_label = format!(
        "B0_vs_MB0 case={case_name} saturation={saturation} origin={origin:?} dither={dither}"
    );
    compare_mrt_store_with_capture(&fragment_label, b0, &fragment_probe);

    for probe in [&up2_probe, &fragment_probe] {
        for (index, raw_bits) in probe.raw_bits.iter().enumerate() {
            for channel in 0..4 {
                let raw = f32::from_bits(raw_bits[channel]);
                let expected = super::rgba16f_store::binary16_toward_zero_for_test(raw);
                assert_eq!(
                    probe.manual_half_bits[index][channel] as u16, expected,
                    "manual converter differs from CPU RTZ reference at channel={channel}"
                );
            }
        }
    }
}

fn terminal_builtin_blur_scene(
    rect: EffectRect,
    visual_group: VisualGroupId,
    material: oblivion_one::material::EffectiveMaterial,
    coverage: Option<oblivion_one::effects::EffectCoverage>,
) -> (ResolvedEffectScene, oblivion_one::effects::EffectRegistry) {
    terminal_builtin_blur_scene_with_passes(
        rect,
        visual_group,
        material,
        coverage,
        oblivion_one::effects::BUILTIN_MATERIAL_BLUR_PASSES,
    )
}

fn terminal_builtin_blur_scene_with_passes(
    rect: EffectRect,
    visual_group: VisualGroupId,
    material: oblivion_one::material::EffectiveMaterial,
    coverage: Option<oblivion_one::effects::EffectCoverage>,
    passes: u8,
) -> (ResolvedEffectScene, oblivion_one::effects::EffectRegistry) {
    let program =
        oblivion_one::effects::builtin_background_material_program_with_passes(material, passes);
    let program_id = program.program.id;
    let mut registry = oblivion_one::effects::EffectRegistry::empty();
    registry
        .insert(program)
        .expect("canonical material inserts");
    let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);
    let instance = oblivion_one::compositor::ResolvedEffectInstance {
        composite_opacity: None,
        id: oblivion_one::effects::EffectInstanceId::new(1).expect("test effect id"),
        program: program_id,
        anchor,
        target_bounds: rect,
        region: EffectRegion::from_rect(rect),
        parameter_block: EffectParameterBlock::default(),
        coverage,
        signature: 1,
        frame_demand: EffectFrameDemand::OnDamage,
        visual_group: Some(visual_group),
        anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
        backdrop_reuse: oblivion_one::compositor::EffectBackdropReuse::Exact,
        scene_order: oblivion_one::compositor::EffectSceneOrder::for_anchor(anchor),
    };
    (ResolvedEffectScene::new(1, vec![instance]), registry)
}

fn terminal_builtin_blur_scene_with_tint(
    rect: EffectRect,
    visual_group: VisualGroupId,
    material: oblivion_one::material::EffectiveMaterial,
    tint: oblivion_one::effects::TintSpec,
) -> (ResolvedEffectScene, oblivion_one::effects::EffectRegistry) {
    let (scene, _) = terminal_builtin_blur_scene(rect, visual_group, material, None);
    let mut program = oblivion_one::effects::builtin_background_material_program(material).program;
    let matrix_id = program
        .nodes
        .iter()
        .find(|node| {
            matches!(
                node.kind,
                oblivion_one::effects::EffectNodeKind::ColorMatrix(_)
            )
        })
        .map(|node| node.id)
        .expect("builtin material has a ColorMatrix");
    let noise_index = program
        .nodes
        .iter()
        .position(|node| matches!(node.kind, oblivion_one::effects::EffectNodeKind::Noise(_)))
        .expect("builtin material has a Noise stage");
    let tint_id = oblivion_one::effects::EffectNodeId::new(5).expect("Tint node id");
    program.nodes[noise_index].inputs = vec![tint_id];
    program.nodes.insert(
        noise_index,
        oblivion_one::effects::EffectNode::tint(tint_id, matrix_id, tint),
    );
    let program = oblivion_one::effects::validate_effect_program(program)
        .expect("synthetic Tint material validates");
    let mut registry = oblivion_one::effects::EffectRegistry::empty();
    registry.insert(program).expect("Tint material inserts");
    (scene, registry)
}

fn terminal_graph_variant(
    scene: &ResolvedEffectScene,
    registry: &oblivion_one::effects::EffectRegistry,
    bounds: EffectRect,
    enabled: bool,
) -> CompiledFrameGraph {
    terminal_graph_variant_with_source_damage(
        scene,
        registry,
        bounds,
        &EffectRegion::empty(),
        enabled,
    )
}

fn terminal_graph_variant_with_source_damage(
    scene: &ResolvedEffectScene,
    registry: &oblivion_one::effects::EffectRegistry,
    bounds: EffectRect,
    source_damage: &EffectRegion,
    terminal_fusion: bool,
) -> CompiledFrameGraph {
    let variants = oblivion_one::effects::compile_frame_execution_variants_with_options(
        scene,
        source_damage,
        bounds,
        registry,
        oblivion_one::effects::FrameExecutionCompileOptions {
            precomputed_window_blur: false,
            terminal_kawase_fusion: terminal_fusion,
            checkpoint_dependency_pruning: false,
        },
    )
    .expect("terminal blur graph compiles");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = variants.exact else {
        panic!("canonical blur compiles to an effect graph");
    };
    graph
}

fn render_terminal_graph(
    graph: &CompiledFrameGraph,
    size: (u32, u32),
    origin: OutputFramebufferOrigin,
    install_scene: impl FnOnce(&mut GlesEffectTestHarness),
) -> Vec<u8> {
    let mut harness = GlesEffectTestHarness::new(size.0, size.1);
    install_scene(&mut harness);
    let bounds = EffectRect::new(0, 0, size.0, size.1).expect("output bounds");
    execute_diagnostic_frame_with_origin(
        &mut harness,
        graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[], true, size),
        EffectRegion::from_rect(bounds),
        true,
        effects::EffectDebugConfig::new(
            effects::EffectDebugCaptureMode::Replay,
            effects::EffectDebugKawaseMode::Partial,
        ),
        origin,
    );
    read_diagnostic_pixels(&harness)
}

fn render_terminal_graph_after_partial_edge_damage(
    graph: &CompiledFrameGraph,
    size: (u32, u32),
    origin: OutputFramebufferOrigin,
    install_scene: impl FnOnce(&mut GlesEffectTestHarness),
    repair: OutputRect,
) -> Vec<u8> {
    let mut harness = GlesEffectTestHarness::new(size.0, size.1);
    install_scene(&mut harness);
    let bounds = EffectRect::new(0, 0, size.0, size.1).expect("output bounds");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    execute_diagnostic_frame_with_origin(
        &mut harness,
        graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[], true, size),
        EffectRegion::from_rect(bounds),
        true,
        config,
        origin,
    );
    update_diagnostic_background_for_surface(&harness, 7, repair, [207, 43, 89, 255]);
    execute_diagnostic_frame_with_origin(
        &mut harness,
        graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[repair], false, size),
        diagnostic_region(repair),
        false,
        config,
        origin,
    );
    let partial_pixels = read_diagnostic_pixels(&harness);
    execute_diagnostic_frame_with_origin(
        &mut harness,
        graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[], true, size),
        EffectRegion::from_rect(bounds),
        true,
        config,
        origin,
    );
    let full_pixels = read_diagnostic_pixels(&harness);
    println!(
        "partial-vs-full fusions={} {origin:?}: {}",
        graph.stats.terminal_blur_fusions,
        terminal_pixel_difference_summary(&full_pixels, &partial_pixels, size.0, size.1, origin,)
    );
    partial_pixels
}

fn terminal_pixel_difference_summary(
    expected: &[u8],
    actual: &[u8],
    width: u32,
    height: u32,
    origin: OutputFramebufferOrigin,
) -> String {
    assert_eq!(expected.len(), actual.len(), "output lengths differ");
    let mut changed_pixels = 0usize;
    let mut max_channel_delta = 0u8;
    let mut bounds = [u32::MAX, u32::MAX, 0, 0];
    let mut samples = Vec::new();
    for (pixel_index, (expected, actual)) in expected
        .chunks_exact(4)
        .zip(actual.chunks_exact(4))
        .enumerate()
    {
        if expected == actual {
            continue;
        }
        changed_pixels += 1;
        let x = u32::try_from(pixel_index).unwrap() % width;
        let storage_y = u32::try_from(pixel_index).unwrap() / width;
        let logical_y = match origin {
            OutputFramebufferOrigin::BottomLeft => height - 1 - storage_y,
            OutputFramebufferOrigin::TopLeftScanout => storage_y,
        };
        bounds[0] = bounds[0].min(x);
        bounds[1] = bounds[1].min(storage_y);
        bounds[2] = bounds[2].max(x);
        bounds[3] = bounds[3].max(storage_y);
        let absolute_difference = expected
            .iter()
            .zip(actual)
            .map(|(expected, actual)| expected.abs_diff(*actual))
            .collect::<Vec<_>>();
        for (expected, actual) in expected.iter().zip(actual) {
            max_channel_delta = max_channel_delta.max(expected.abs_diff(*actual));
        }
        if samples.len() < 8 {
            samples.push((
                [x, storage_y],
                [x, logical_y],
                expected.to_vec(),
                actual.to_vec(),
                absolute_difference,
            ));
        }
    }
    if changed_pixels == 0 {
        String::new()
    } else {
        format!(
            "changed pixels={changed_pixels}, bounds={bounds:?}, \
             max channel delta={max_channel_delta}, samples={samples:?}"
        )
    }
}

#[derive(Clone, Debug)]
struct Rgba16fDifferenceSample {
    storage_coordinate: [u32; 2],
    logical_effect_local_coordinate: [i32; 2],
    legacy_rgba: [f32; 4],
    diagnostic_rgba: [f32; 4],
    #[allow(dead_code, reason = "Reserved for diagnostic tests")]
    absolute_difference: [f32; 4],
    ulp_difference: [u32; 4],
}

#[derive(Clone, Debug)]
struct Rgba16fDifferenceSummary {
    total_texels: usize,
    differing_texels: usize,
    differing_channels: usize,
    differing_alpha_texels: usize,
    first: Option<Rgba16fDifferenceSample>,
    maximum_absolute_difference: f32,
    maximum_ulp_difference: u32,
    logical_bounds: Option<[i32; 4]>,
    exactly_equal: bool,
}

fn ordered_f32_bits(value: f32) -> u32 {
    let bits = value.to_bits();
    if bits & 0x8000_0000 == 0 {
        bits | 0x8000_0000
    } else {
        !bits
    }
}

fn summarize_rgba16f_difference(
    legacy: &effects::DiagnosticTextureCapture,
    diagnostic: &effects::DiagnosticTextureCapture,
) -> Rgba16fDifferenceSummary {
    assert_eq!(
        (legacy.width, legacy.height),
        (diagnostic.width, diagnostic.height)
    );
    assert_eq!(legacy.domain, diagnostic.domain);
    assert_eq!(legacy.origin, diagnostic.origin);
    let total_texels = legacy.width as usize * legacy.height as usize;
    assert_eq!(legacy.pixels.len(), total_texels);
    assert_eq!(diagnostic.pixels.len(), total_texels);

    let mut summary = Rgba16fDifferenceSummary {
        total_texels,
        differing_texels: 0,
        differing_channels: 0,
        differing_alpha_texels: 0,
        first: None,
        maximum_absolute_difference: 0.0,
        maximum_ulp_difference: 0,
        logical_bounds: None,
        exactly_equal: true,
    };
    for logical_y in 0..legacy.height {
        let storage_y = match legacy.origin {
            oblivion_one::effects::GraphTextureOrigin::BottomLeft => legacy.height - 1 - logical_y,
        };
        for x in 0..legacy.width {
            let offset = (storage_y * legacy.width + x) as usize;
            let left = legacy.pixels[offset];
            let right = diagnostic.pixels[offset];
            if left == right {
                continue;
            }
            summary.exactly_equal = false;
            summary.differing_texels += 1;
            if left[3] != right[3] {
                summary.differing_alpha_texels += 1;
            }
            let mut absolute_difference = [0.0; 4];
            let mut ulp_difference = [0; 4];
            for channel in 0..4 {
                absolute_difference[channel] = (left[channel] - right[channel]).abs();
                ulp_difference[channel] =
                    ordered_f32_bits(left[channel]).abs_diff(ordered_f32_bits(right[channel]));
                if ulp_difference[channel] != 0 {
                    summary.differing_channels += 1;
                }
                summary.maximum_absolute_difference = summary
                    .maximum_absolute_difference
                    .max(absolute_difference[channel]);
                summary.maximum_ulp_difference =
                    summary.maximum_ulp_difference.max(ulp_difference[channel]);
            }
            let logical_x = legacy.domain.x + x as i32;
            let logical_y = legacy.domain.y + logical_y as i32;
            summary.logical_bounds = Some(summary.logical_bounds.map_or(
                [logical_x, logical_y, logical_x, logical_y],
                |bounds| {
                    [
                        bounds[0].min(logical_x),
                        bounds[1].min(logical_y),
                        bounds[2].max(logical_x),
                        bounds[3].max(logical_y),
                    ]
                },
            ));
            summary.first.get_or_insert(Rgba16fDifferenceSample {
                storage_coordinate: [x, storage_y],
                logical_effect_local_coordinate: [logical_x, logical_y],
                legacy_rgba: left,
                diagnostic_rgba: right,
                absolute_difference,
                ulp_difference,
            });
        }
    }
    summary
}

#[test]
fn rgba16f_texture_diff_reports_storage_and_logical_coordinates() {
    let domain = EffectRect::new(100, 200, 2, 3).expect("capture domain");
    let legacy = effects::DiagnosticTextureCapture {
        width: 2,
        height: 3,
        domain,
        origin: oblivion_one::effects::GraphTextureOrigin::BottomLeft,
        pixels: vec![[0.5, 0.25, 0.125, 1.0]; 6],
    };
    let mut diagnostic = legacy.clone();
    // Logical local row 2 is stored in framebuffer row 0 for BottomLeft origin.
    diagnostic.pixels[1][0] = f32::from_bits(legacy.pixels[1][0].to_bits() + 1);

    let difference = summarize_rgba16f_difference(&legacy, &diagnostic);

    assert_eq!(difference.total_texels, 6);
    assert_eq!(difference.differing_texels, 1);
    assert_eq!(difference.differing_channels, 1);
    let first = difference.first.expect("one differing sample");
    assert_eq!(first.storage_coordinate, [1, 0]);
    assert_eq!(first.logical_effect_local_coordinate, [101, 202]);
    assert_eq!(first.legacy_rgba, legacy.pixels[1]);
    assert_eq!(first.diagnostic_rgba, diagnostic.pixels[1]);
    assert_eq!(first.ulp_difference, [1, 0, 0, 0]);
    assert!(!difference.exactly_equal);
}

#[derive(Clone, Debug)]
struct Rgba8DifferenceSample {
    storage_coordinate: [u32; 2],
    logical_output_coordinate: [u32; 2],
    legacy_rgba: [u8; 4],
    candidate_rgba: [u8; 4],
    #[allow(dead_code, reason = "Reserved for diagnostic tests")]
    absolute_difference: [u8; 4],
}

#[derive(Clone, Debug)]
struct Rgba8DifferenceSummary {
    changed_pixels: usize,
    changed_ratio: f64,
    max_rgb_delta: u8,
    mean_absolute_rgb_error: f64,
    rms_rgb_error: f64,
    bounds: Option<[u32; 4]>,
    alpha_changed_pixels: usize,
    first: Option<Rgba8DifferenceSample>,
    maximum_absolute_difference: u8,
}

fn summarize_rgba8_difference(
    expected: &[u8],
    actual: &[u8],
    width: u32,
    height: u32,
    origin: OutputFramebufferOrigin,
) -> Rgba8DifferenceSummary {
    assert_eq!(expected.len(), actual.len());
    assert_eq!(expected.len() % 4, 0);
    let total_pixels = expected.len() / 4;
    let mut summary = Rgba8DifferenceSummary {
        changed_pixels: 0,
        changed_ratio: 0.0,
        max_rgb_delta: 0,
        mean_absolute_rgb_error: 0.0,
        rms_rgb_error: 0.0,
        bounds: None,
        alpha_changed_pixels: 0,
        first: None,
        maximum_absolute_difference: 0,
    };
    let mut absolute_sum = 0.0;
    let mut squared_sum = 0.0;
    for (index, (left, right)) in expected
        .chunks_exact(4)
        .zip(actual.chunks_exact(4))
        .enumerate()
    {
        let rgb_deltas = [
            left[0].abs_diff(right[0]),
            left[1].abs_diff(right[1]),
            left[2].abs_diff(right[2]),
        ];
        let changed = rgb_deltas.iter().any(|delta| *delta != 0) || left[3] != right[3];
        if changed {
            summary.changed_pixels += 1;
            let x = (index as u32) % width;
            let storage_y = (index as u32) / width;
            let logical_y = match origin {
                OutputFramebufferOrigin::BottomLeft => height - 1 - storage_y,
                OutputFramebufferOrigin::TopLeftScanout => storage_y,
            };
            let y = storage_y;
            summary.bounds = Some(summary.bounds.map_or([x, y, x, y], |bounds| {
                [
                    bounds[0].min(x),
                    bounds[1].min(y),
                    bounds[2].max(x),
                    bounds[3].max(y),
                ]
            }));
            let absolute_difference = [
                left[0].abs_diff(right[0]),
                left[1].abs_diff(right[1]),
                left[2].abs_diff(right[2]),
                left[3].abs_diff(right[3]),
            ];
            summary.maximum_absolute_difference = summary
                .maximum_absolute_difference
                .max(*absolute_difference.iter().max().unwrap_or(&0));
            summary.first.get_or_insert(Rgba8DifferenceSample {
                storage_coordinate: [x, storage_y],
                logical_output_coordinate: [x, logical_y],
                legacy_rgba: left.try_into().expect("four RGBA channels"),
                candidate_rgba: right.try_into().expect("four RGBA channels"),
                absolute_difference,
            });
        }
        if left[3] != right[3] {
            summary.alpha_changed_pixels += 1;
        }
        for delta in rgb_deltas {
            summary.max_rgb_delta = summary.max_rgb_delta.max(delta);
            absolute_sum += f64::from(delta);
            squared_sum += f64::from(delta) * f64::from(delta);
        }
    }
    summary.changed_ratio = summary.changed_pixels as f64 / total_pixels.max(1) as f64;
    summary.mean_absolute_rgb_error = absolute_sum / (total_pixels.max(1) * 3) as f64;
    summary.rms_rgb_error = (squared_sum / (total_pixels.max(1) * 3) as f64).sqrt();
    summary
}

#[derive(Clone, Copy, Debug)]
struct TerminalTailCaptureIds {
    instance: oblivion_one::effects::EffectInstanceId,
    upsample: oblivion_one::effects::GraphPassId,
    fragment: oblivion_one::effects::GraphPassId,
    composite: oblivion_one::effects::GraphPassId,
}

fn terminal_tail_capture_ids(graph: &CompiledFrameGraph) -> Vec<TerminalTailCaptureIds> {
    graph
        .passes
        .windows(3)
        .filter_map(|passes| {
            let [upsample, fragment, composite] = passes else {
                return None;
            };
            (upsample.kind == RenderPassKind::DualKawaseUpsample
                && fragment.kind == RenderPassKind::Fragment
                && composite.kind == RenderPassKind::Composite)
                .then_some(TerminalTailCaptureIds {
                    instance: upsample.instance,
                    upsample: upsample.id,
                    fragment: fragment.id,
                    composite: composite.id,
                })
        })
        .collect()
}

fn terminal_stage_output_domain(
    graph: &CompiledFrameGraph,
    pass_id: oblivion_one::effects::GraphPassId,
) -> EffectRect {
    let pass = graph
        .passes
        .iter()
        .find(|pass| pass.id == pass_id)
        .expect("terminal stage pass exists");
    let output = pass.output.expect("terminal stage has an output texture");
    graph
        .textures
        .iter()
        .find(|texture| texture.id == output)
        .expect("terminal stage output texture exists")
        .domain
}

fn render_terminal_replay_mode(
    graph: &CompiledFrameGraph,
    output_size: (u32, u32),
    origin: OutputFramebufferOrigin,
    install_scene: impl FnOnce(&mut GlesEffectTestHarness),
    mode: effects::TerminalBoundaryReplay,
    capture_passes: impl IntoIterator<Item = oblivion_one::effects::GraphPassId>,
    probe_passes: impl IntoIterator<Item = oblivion_one::effects::GraphPassId>,
) -> (effects::TerminalBoundaryReplayExecution, Vec<u8>) {
    let mut harness = GlesEffectTestHarness::new(output_size.0, output_size.1);
    install_scene(&mut harness);
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let plan = diagnostic_repaint_plan_for_repairs_in_size(&[], true, output_size);
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    let replay = execute_diagnostic_frame_with_terminal_replay(
        &mut harness,
        graph,
        &plan,
        EffectRegion::from_rect(bounds),
        true,
        config,
        origin,
        mode,
        capture_passes,
        probe_passes,
    );
    (replay, read_diagnostic_pixels(&harness))
}

fn render_production_canonical_staged(
    graph: &CompiledFrameGraph,
    output_size: (u32, u32),
    origin: OutputFramebufferOrigin,
    install_scene: impl FnOnce(&mut GlesEffectTestHarness),
    capture_passes: impl IntoIterator<Item = oblivion_one::effects::GraphPassId>,
) -> (effects::TerminalBoundaryReplayExecution, Vec<u8>, bool) {
    let mut harness = GlesEffectTestHarness::new(output_size.0, output_size.1);
    install_scene(&mut harness);
    harness
        .renderer
        .effect_runtime
        .canonical_blur_semantics_policy = effects::CanonicalBlurSemanticsPolicy::Canonical;
    harness.renderer.effect_runtime.terminal_blur_fusion_policy =
        effects::TerminalBlurFusionPolicy::Off;
    unsafe { harness.gl.enable(glow::DITHER) };
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    let replay = execute_diagnostic_frame_with_terminal_replay(
        &mut harness,
        graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[], true, output_size),
        EffectRegion::from_rect(bounds),
        true,
        config,
        origin,
        effects::TerminalBoundaryReplay::ProductionCanonicalStaged,
        capture_passes,
        [],
    );
    let dither_restored = unsafe { harness.gl.is_enabled(glow::DITHER) };
    (replay, read_diagnostic_pixels(&harness), dither_restored)
}

fn render_production_canonical_fused(
    graph: &CompiledFrameGraph,
    output_size: (u32, u32),
    origin: OutputFramebufferOrigin,
    install_scene: impl FnOnce(&mut GlesEffectTestHarness),
) -> (Vec<u8>, bool) {
    let mut harness = GlesEffectTestHarness::new(output_size.0, output_size.1);
    install_scene(&mut harness);
    harness
        .renderer
        .effect_runtime
        .canonical_blur_semantics_policy = effects::CanonicalBlurSemanticsPolicy::Canonical;
    harness.renderer.effect_runtime.terminal_blur_fusion_policy =
        effects::TerminalBlurFusionPolicy::On;
    unsafe { harness.gl.enable(glow::DITHER) };
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    execute_diagnostic_frame_with_terminal_replay(
        &mut harness,
        graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[], true, output_size),
        EffectRegion::from_rect(bounds),
        true,
        config,
        origin,
        effects::TerminalBoundaryReplay::ProductionCanonicalFused,
        [],
        [],
    );
    let dither_untouched = unsafe { harness.gl.is_enabled(glow::DITHER) };
    (read_diagnostic_pixels(&harness), dither_untouched)
}

fn render_canonical_single_tail_pair(
    staged_graph: &CompiledFrameGraph,
    fused_graph: &CompiledFrameGraph,
    output_size: (u32, u32),
    origin: OutputFramebufferOrigin,
    install_scene: impl Fn(&mut GlesEffectTestHarness),
    case_name: &str,
) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let staged_tails = terminal_tail_capture_ids(staged_graph);
    assert_eq!(staged_tails.len(), 1, "{case_name} staged graph tail count");
    let staged_tail = staged_tails[0];
    let fused_terminal = fused_graph
        .passes
        .iter()
        .find(|pass| pass.terminal_kawase.is_some())
        .expect("fused graph retains one terminal Composite");
    assert_eq!(fused_terminal.instance, staged_tail.instance);

    let (fused_probe_replay, _) = effects::with_test_offscreen_dither_policy(Some(false), || {
        render_terminal_replay_mode(
            fused_graph,
            output_size,
            origin,
            |harness| install_scene(harness),
            effects::TerminalBoundaryReplay::CanonicalTerminalQ1Q2,
            [],
            [fused_terminal.id],
        )
    });

    let fused_a = &fused_probe_replay.stage_probes
        [&(fused_terminal.id, effects::TerminalDiagnosticStage::Up2)]
        .pixels;
    let fused_b = &fused_probe_replay.stage_probes[&(
        fused_terminal.id,
        effects::TerminalDiagnosticStage::Fragment,
    )]
        .pixels;
    let staged_noise_replay = effects::with_test_offscreen_dither_policy(Some(false), || {
        render_terminal_replay_mode(
            staged_graph,
            output_size,
            origin,
            |harness| install_scene(harness),
            effects::TerminalBoundaryReplay::CanonicalStaged,
            [],
            [staged_tail.fragment],
        )
        .0
    });
    assert_eq!(
        staged_noise_replay.stage_probes[&(
            staged_tail.fragment,
            effects::TerminalDiagnosticStage::CanonicalNoiseBits
        )]
            .pixels,
        fused_probe_replay.stage_probes[&(
            fused_terminal.id,
            effects::TerminalDiagnosticStage::CanonicalNoiseBits
        )]
            .pixels,
        "{case_name} canonical Noise bits differ for {origin:?}"
    );
    let (production_replay, production_pixels, dither_restored) =
        render_production_canonical_staged(
            staged_graph,
            output_size,
            origin,
            |harness| install_scene(harness),
            [staged_tail.upsample, staged_tail.fragment],
        );
    assert!(
        dither_restored,
        "canonical offscreen draws must restore enabled DITHER for {origin:?}"
    );
    assert_eq!(
        production_replay.captures[&staged_tail.upsample]
            .pixels
            .as_slice(),
        fused_a.as_slice(),
        "production canonical A differs for {case_name} at {origin:?}"
    );
    assert_eq!(
        production_replay.captures[&staged_tail.fragment]
            .pixels
            .as_slice(),
        fused_b.as_slice(),
        "production canonical B differs for {case_name} at {origin:?}"
    );
    let (fused_pixels, fused_dither_untouched) =
        render_production_canonical_fused(fused_graph, output_size, origin, |harness| {
            install_scene(harness)
        });
    assert!(
        fused_dither_untouched,
        "terminal output must leave DITHER untouched"
    );
    assert_eq!(
        production_pixels, fused_pixels,
        "production canonical staged and fused C differ for {case_name} at {origin:?}"
    );
    let summary = summarize_rgba8_difference(
        &production_pixels,
        &fused_pixels,
        output_size.0,
        output_size.1,
        origin,
    );
    assert_eq!(summary.changed_pixels, 0, "{case_name} C changed pixels");
    assert_eq!(summary.alpha_changed_pixels, 0, "{case_name} alpha changed");
    (production_pixels.clone(), fused_pixels, production_pixels)
}

fn render_canonical_multi_tail_pair(
    staged_graph: &CompiledFrameGraph,
    fused_graph: &CompiledFrameGraph,
    output_size: (u32, u32),
    origin: OutputFramebufferOrigin,
    install_scene: impl Fn(&mut GlesEffectTestHarness),
    case_name: &str,
) -> (Vec<u8>, Vec<u8>) {
    let staged_tails = terminal_tail_capture_ids(staged_graph);
    let fused_terminals = fused_graph
        .passes
        .iter()
        .filter(|pass| pass.terminal_kawase.is_some())
        .collect::<Vec<_>>();
    assert_eq!(
        staged_tails.len(),
        fused_terminals.len(),
        "{case_name} staged/fused terminal tail count"
    );
    let capture_passes = staged_tails
        .iter()
        .flat_map(|tail| [tail.upsample, tail.fragment])
        .collect::<Vec<_>>();
    let probe_passes = fused_terminals
        .iter()
        .map(|pass| pass.id)
        .collect::<Vec<_>>();
    let (staged_replay, _) = effects::with_test_offscreen_dither_policy(Some(false), || {
        render_terminal_replay_mode(
            staged_graph,
            output_size,
            origin,
            |harness| install_scene(harness),
            effects::TerminalBoundaryReplay::CanonicalStaged,
            capture_passes,
            staged_tails.iter().map(|tail| tail.fragment),
        )
    });
    let (fused_replay, _) = effects::with_test_offscreen_dither_policy(Some(false), || {
        render_terminal_replay_mode(
            fused_graph,
            output_size,
            origin,
            |harness| install_scene(harness),
            effects::TerminalBoundaryReplay::CanonicalTerminalQ1Q2,
            [],
            probe_passes,
        )
    });

    for tail in &staged_tails {
        let fused_terminal = fused_terminals
            .iter()
            .find(|pass| pass.instance == tail.instance)
            .expect("matching fused tail exists");
        assert_eq!(
            staged_replay.captures[&tail.upsample].pixels,
            fused_replay.stage_probes[&(fused_terminal.id, effects::TerminalDiagnosticStage::Up2)]
                .pixels,
            "{case_name} A differs at instance {} for {origin:?}",
            tail.instance.get()
        );
        assert_eq!(
            staged_replay.captures[&tail.fragment].pixels,
            fused_replay.stage_probes[&(
                fused_terminal.id,
                effects::TerminalDiagnosticStage::Fragment
            )]
                .pixels,
            "{case_name} B differs at instance {} for {origin:?}",
            tail.instance.get()
        );
        assert_eq!(
            staged_replay.stage_probes[&(
                tail.fragment,
                effects::TerminalDiagnosticStage::CanonicalNoiseBits
            )]
                .pixels,
            fused_replay.stage_probes[&(
                fused_terminal.id,
                effects::TerminalDiagnosticStage::CanonicalNoiseBits
            )]
                .pixels,
            "{case_name} Noise bits differ at instance {} for {origin:?}",
            tail.instance.get()
        );
    }
    let production_capture_passes = staged_tails
        .iter()
        .flat_map(|tail| [tail.upsample, tail.fragment])
        .collect::<Vec<_>>();
    let (production_replay, production_pixels, dither_restored) =
        render_production_canonical_staged(
            staged_graph,
            output_size,
            origin,
            |harness| install_scene(harness),
            production_capture_passes,
        );
    assert!(dither_restored, "canonical causal draw restores DITHER");
    for tail in &staged_tails {
        let fused_terminal = fused_terminals
            .iter()
            .find(|pass| pass.instance == tail.instance)
            .expect("matching fused tail exists");
        assert_eq!(
            production_replay.captures[&tail.upsample].pixels,
            fused_replay.stage_probes[&(fused_terminal.id, effects::TerminalDiagnosticStage::Up2)]
                .pixels,
            "production causal canonical Up2 differs for instance {} at {origin:?}",
            tail.instance.get()
        );
        assert_eq!(
            production_replay.captures[&tail.fragment].pixels,
            fused_replay.stage_probes[&(
                fused_terminal.id,
                effects::TerminalDiagnosticStage::Fragment
            )]
                .pixels,
            "production causal canonical Fragment differs for instance {} at {origin:?}",
            tail.instance.get()
        );
    }
    let (fused_pixels, fused_dither_untouched) =
        render_production_canonical_fused(fused_graph, output_size, origin, |harness| {
            install_scene(harness)
        });
    assert!(
        fused_dither_untouched,
        "canonical terminal output must leave DITHER untouched"
    );
    assert_eq!(
        production_pixels, fused_pixels,
        "production causal canonical staged/fused C differs for {case_name} at {origin:?}"
    );
    let summary = summarize_rgba8_difference(
        &production_pixels,
        &fused_pixels,
        output_size.0,
        output_size.1,
        origin,
    );
    assert_eq!(summary.changed_pixels, 0, "{case_name} changed pixels");
    assert_eq!(summary.alpha_changed_pixels, 0, "{case_name} alpha changed");
    (production_pixels, fused_pixels)
}

struct TerminalPartialRepairReplay {
    initial_pixels: Vec<u8>,
    partial_pixels: Vec<u8>,
    full_reference_pixels: Vec<u8>,
    source_damage: EffectRegion,
    repair_graph: CompiledFrameGraph,
}

fn render_terminal_replay_after_partial_repair(
    scene: &ResolvedEffectScene,
    registry: &oblivion_one::effects::EffectRegistry,
    initial_graph: &CompiledFrameGraph,
    output_size: (u32, u32),
    origin: OutputFramebufferOrigin,
    install_scene: impl Fn(&mut GlesEffectTestHarness),
    repair: OutputRect,
    terminal_fusion: bool,
    mode: effects::TerminalBoundaryReplay,
    production_canonical: bool,
) -> TerminalPartialRepairReplay {
    let mut harness = GlesEffectTestHarness::new(output_size.0, output_size.1);
    install_scene(&mut harness);
    if production_canonical {
        harness
            .renderer
            .effect_runtime
            .canonical_blur_semantics_policy = effects::CanonicalBlurSemanticsPolicy::Canonical;
        harness.renderer.effect_runtime.terminal_blur_fusion_policy = if terminal_fusion {
            effects::TerminalBlurFusionPolicy::On
        } else {
            effects::TerminalBlurFusionPolicy::Off
        };
        unsafe { harness.gl.enable(glow::DITHER) };
    }
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    println!("partial repair {origin:?} {terminal_fusion}: rendering frame 0");
    execute_diagnostic_frame_with_terminal_replay(
        &mut harness,
        initial_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[], true, output_size),
        EffectRegion::from_rect(bounds),
        true,
        config,
        origin,
        mode,
        [],
        [],
    );
    if production_canonical {
        assert!(unsafe { harness.gl.is_enabled(glow::DITHER) });
    }
    println!("partial repair {origin:?} {terminal_fusion}: frame 0 complete");
    let initial_pixels = read_diagnostic_pixels(&harness);
    update_diagnostic_background_for_surface(&harness, 7, repair, [207, 43, 89, 255]);
    let source_repair_rect = EffectRect::new(repair.x, repair.y, repair.width, repair.height)
        .expect("source repair region");
    let source_damage = EffectRegion::from_rect(source_repair_rect);
    println!("partial repair {origin:?} {terminal_fusion}: compiling frame 1");
    let repair_staged_graph =
        terminal_graph_variant_with_source_damage(scene, registry, bounds, &source_damage, false);
    let repair_fused_graph =
        terminal_graph_variant_with_source_damage(scene, registry, bounds, &source_damage, true);
    assert_eq!(
        repair_staged_graph.final_damage, repair_fused_graph.final_damage,
        "repair-specific staged/fused graphs must report the same final damage"
    );
    let program = registry
        .get(scene.instances[0].program)
        .expect("repair fixture program exists");
    let planned_effect_damage = oblivion_one::effects::plan_effect_damage(
        program.aggregate_footprint,
        &scene.instances[0].region,
        &source_damage,
        bounds,
    );
    println!(
        "canonical repair aggregate footprint={:?}",
        program.aggregate_footprint
    );
    assert!(
        effect_damage_extends_beyond_source_repair(
            &planned_effect_damage.output_damage,
            source_repair_rect,
        ),
        "blur kernel footprint must expand the raw 3x2 source repair; output_damage={}",
        format_effect_region(&planned_effect_damage.output_damage)
    );
    let expected_final_damage = source_damage.union(&planned_effect_damage.output_damage);
    assert_eq!(
        repair_staged_graph.final_damage, expected_final_damage,
        "compiled final damage must combine source repair and planned effect output damage"
    );
    println!(
        "canonical source repair={}; plan_effect_damage output_damage={}; compiled final_damage={}",
        format_effect_region(&source_damage),
        format_effect_region(&planned_effect_damage.output_damage),
        format_effect_region(&repair_staged_graph.final_damage),
    );
    let repair_graph = if terminal_fusion {
        repair_fused_graph
    } else {
        repair_staged_graph
    };
    println!("partial repair {origin:?} {terminal_fusion}: executing frame 1");
    let repair_rects = repair_graph
        .final_damage
        .rects()
        .iter()
        .map(|rect| OutputRect::new(rect.x, rect.y, rect.width, rect.height))
        .collect::<Vec<_>>();
    let repair_is_conservative_full = !repair_graph.final_damage.is_empty()
        && repair_graph.final_damage.bounding_rect().is_none();
    execute_diagnostic_frame_with_terminal_replay(
        &mut harness,
        &repair_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(
            &repair_rects,
            repair_is_conservative_full,
            output_size,
        ),
        repair_graph.final_damage.clone(),
        repair_is_conservative_full,
        config,
        origin,
        mode,
        [],
        [],
    );
    if production_canonical {
        assert!(unsafe { harness.gl.is_enabled(glow::DITHER) });
    }
    println!("partial repair {origin:?} {terminal_fusion}: frame 1 complete");
    println!("partial repair {origin:?} {terminal_fusion}: reading partial pixels");
    let partial_pixels = read_diagnostic_pixels(&harness);
    println!("partial repair {origin:?} {terminal_fusion}: partial pixels read");

    drop(harness);
    println!("partial repair {origin:?} {terminal_fusion}: creating reference harness");
    let mut reference_harness = GlesEffectTestHarness::new(output_size.0, output_size.1);
    println!("partial repair {origin:?} {terminal_fusion}: reference harness ready");
    install_scene(&mut reference_harness);
    if production_canonical {
        reference_harness
            .renderer
            .effect_runtime
            .canonical_blur_semantics_policy = effects::CanonicalBlurSemanticsPolicy::Canonical;
        reference_harness
            .renderer
            .effect_runtime
            .terminal_blur_fusion_policy = if terminal_fusion {
            effects::TerminalBlurFusionPolicy::On
        } else {
            effects::TerminalBlurFusionPolicy::Off
        };
        unsafe { reference_harness.gl.enable(glow::DITHER) };
    }
    update_diagnostic_background_for_surface(&reference_harness, 7, repair, [207, 43, 89, 255]);
    println!("partial repair {origin:?} {terminal_fusion}: compiling full reference");
    let full_source_damage = EffectRegion::from_rect(bounds);
    let full_reference_graph = terminal_graph_variant_with_source_damage(
        scene,
        registry,
        bounds,
        &full_source_damage,
        terminal_fusion,
    );
    execute_diagnostic_frame_with_terminal_replay(
        &mut reference_harness,
        &full_reference_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[], true, output_size),
        EffectRegion::from_rect(bounds),
        true,
        config,
        origin,
        mode,
        [],
        [],
    );
    if production_canonical {
        assert!(unsafe { reference_harness.gl.is_enabled(glow::DITHER) });
    }
    println!("partial repair {origin:?} {terminal_fusion}: full reference complete");
    println!("partial repair {origin:?} {terminal_fusion}: reading full-reference pixels");
    let full_reference_pixels = read_diagnostic_pixels(&reference_harness);
    println!("partial repair {origin:?} {terminal_fusion}: full-reference pixels read");
    TerminalPartialRepairReplay {
        initial_pixels,
        partial_pixels,
        full_reference_pixels,
        source_damage,
        repair_graph,
    }
}

fn format_effect_region(region: &EffectRegion) -> String {
    let conservative_full = !region.is_empty() && region.bounding_rect().is_none();
    format!(
        "rects={:?}, conservative_full={conservative_full}",
        region.rects()
    )
}

fn repair_pass_demand_summary(graph: &CompiledFrameGraph) -> Vec<String> {
    let demand = oblivion_one::effects::plan_effect_execution_demand_with_kawase_mode(
        graph,
        &graph.final_damage,
        !graph.final_damage.is_empty() && graph.final_damage.bounding_rect().is_none(),
        false,
    );
    graph
        .passes
        .iter()
        .filter_map(|pass| {
            demand.pass_output_region(pass.id).map(|region| {
                format!(
                    "pass={:?} kind={:?} output={}",
                    pass.id,
                    pass.kind,
                    format_effect_region(region)
                )
            })
        })
        .collect()
}

fn repair_pixel_mismatch(
    label: &str,
    expected_label: &str,
    actual_label: &str,
    expected: &[u8],
    actual: &[u8],
    initial: Option<&[u8]>,
    output_size: (u32, u32),
    origin: OutputFramebufferOrigin,
    source_repair: &EffectRegion,
    graph: &CompiledFrameGraph,
) -> Option<String> {
    let summary =
        summarize_rgba8_difference(expected, actual, output_size.0, output_size.1, origin);
    println!("{label} {origin:?}: {summary:?}");
    if summary.changed_pixels == 0 && summary.alpha_changed_pixels == 0 {
        return None;
    }

    let first = summary.first.expect("a pixel mismatch has a first sample");
    let initial_pixel = initial.map(|pixels| {
        let offset = ((first.storage_coordinate[1] * output_size.0 + first.storage_coordinate[0])
            * 4) as usize;
        <[u8; 4]>::try_from(&pixels[offset..offset + 4]).expect("four initial RGBA channels")
    });
    let partial_matches_initial = initial_pixel.is_some_and(|pixel| pixel == first.candidate_rgba);
    let inside_final_damage = graph.final_damage.contains_point(
        first.logical_output_coordinate[0] as i32,
        first.logical_output_coordinate[1] as i32,
    );
    Some(format!(
        "{label}: source repair={}; compiled final_damage={}; per-pass demanded regions={:?}; \
         first differing output coordinate={:?}; {expected_label} pixel={:?}; \
         {actual_label} pixel={:?}; initial pixel={initial_pixel:?}; partial matches initial={partial_matches_initial}; \
         difference is {} final_damage",
        format_effect_region(source_repair),
        format_effect_region(&graph.final_damage),
        repair_pass_demand_summary(graph),
        first.logical_output_coordinate,
        first.legacy_rgba,
        first.candidate_rgba,
        if inside_final_damage {
            "inside"
        } else {
            "outside"
        },
    ))
}

fn effect_damage_extends_beyond_source_repair(
    output_damage: &EffectRegion,
    source_repair: EffectRect,
) -> bool {
    output_damage.rects().iter().any(|rect| {
        rect.x < source_repair.x
            || rect.y < source_repair.y
            || i64::from(rect.x) + i64::from(rect.width)
                > i64::from(source_repair.x) + i64::from(source_repair.width)
            || i64::from(rect.y) + i64::from(rect.height)
                > i64::from(source_repair.y) + i64::from(source_repair.height)
    })
}

fn capture_canonical_staged_noise_field(
    graph: &CompiledFrameGraph,
    output_size: (u32, u32),
    origin: OutputFramebufferOrigin,
    fragment_pass: oblivion_one::effects::GraphPassId,
    requested_texture_domain: EffectRect,
    effect_domain: EffectRect,
    install_scene: impl FnOnce(&mut GlesEffectTestHarness),
) -> effects::DiagnosticTextureCapture {
    let mut harness = GlesEffectTestHarness::new(output_size.0, output_size.1);
    install_scene(&mut harness);
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let replay = effects::with_test_offscreen_dither_policy(Some(false), || {
        execute_diagnostic_frame_with_terminal_replay(
            &mut harness,
            graph,
            &diagnostic_repaint_plan_for_repairs_in_size(&[], true, output_size),
            EffectRegion::from_rect(bounds),
            true,
            effects::EffectDebugConfig::new(
                effects::EffectDebugCaptureMode::Replay,
                effects::EffectDebugKawaseMode::Partial,
            ),
            origin,
            effects::TerminalBoundaryReplay::CanonicalStaged,
            [],
            [fragment_pass],
        )
    });
    let capture = replay.stage_probes[&(
        fragment_pass,
        effects::TerminalDiagnosticStage::CanonicalNoiseBits,
    )]
        .clone();
    assert_eq!(
        capture.domain, requested_texture_domain,
        "stage probe is cropped to the graph texture output domain"
    );
    assert_eq!(
        (capture.width, capture.height),
        (
            requested_texture_domain.width,
            requested_texture_domain.height
        ),
        "stage probe storage is local to its graph texture domain"
    );
    crop_diagnostic_capture_to_effect_domain(&capture, effect_domain)
}

fn capture_canonical_terminal_noise_field(
    graph: &CompiledFrameGraph,
    output_size: (u32, u32),
    origin: OutputFramebufferOrigin,
    terminal_pass: oblivion_one::effects::GraphPassId,
    effect_domain: EffectRect,
    install_scene: impl FnOnce(&mut GlesEffectTestHarness),
) -> effects::DiagnosticTextureCapture {
    let mut harness = GlesEffectTestHarness::new(output_size.0, output_size.1);
    install_scene(&mut harness);
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let replay = effects::with_test_offscreen_dither_policy(Some(false), || {
        execute_diagnostic_frame_with_terminal_replay(
            &mut harness,
            graph,
            &diagnostic_repaint_plan_for_repairs_in_size(&[], true, output_size),
            EffectRegion::from_rect(bounds),
            true,
            effects::EffectDebugConfig::new(
                effects::EffectDebugCaptureMode::Replay,
                effects::EffectDebugKawaseMode::Partial,
            ),
            origin,
            effects::TerminalBoundaryReplay::CanonicalTerminalQ1Q2,
            [],
            [terminal_pass],
        )
    });
    let capture = replay.stage_probes[&(
        terminal_pass,
        effects::TerminalDiagnosticStage::CanonicalNoiseBits,
    )]
        .clone();
    crop_diagnostic_capture_to_effect_domain(&capture, effect_domain)
}

fn crop_diagnostic_capture_to_effect_domain(
    capture: &effects::DiagnosticTextureCapture,
    effect_domain: EffectRect,
) -> effects::DiagnosticTextureCapture {
    assert_eq!(
        capture.origin,
        oblivion_one::effects::GraphTextureOrigin::BottomLeft
    );
    assert_eq!(
        (capture.width, capture.height),
        (capture.domain.width, capture.domain.height),
        "graph stage capture storage matches its domain"
    );
    assert!(effect_domain.x >= capture.domain.x && effect_domain.y >= capture.domain.y);
    assert!(
        i64::from(effect_domain.x) + i64::from(effect_domain.width)
            <= i64::from(capture.domain.x) + i64::from(capture.domain.width)
    );
    assert!(
        i64::from(effect_domain.y) + i64::from(effect_domain.height)
            <= i64::from(capture.domain.y) + i64::from(capture.domain.height)
    );
    let offset_x = (effect_domain.x - capture.domain.x) as u32;
    let offset_y = (effect_domain.y - capture.domain.y) as u32;
    let mut pixels = vec![[0.0; 4]; effect_domain.width as usize * effect_domain.height as usize];
    for storage_y in 0..effect_domain.height {
        for storage_x in 0..effect_domain.width {
            let source_index =
                ((offset_y + storage_y) * capture.width + offset_x + storage_x) as usize;
            let target_index = (storage_y * effect_domain.width + storage_x) as usize;
            pixels[target_index] = capture.pixels[source_index];
        }
    }
    effects::DiagnosticTextureCapture {
        width: effect_domain.width,
        height: effect_domain.height,
        domain: effect_domain,
        origin: capture.origin,
        pixels,
    }
}

fn canonical_noise_bits_at_local_pixel(
    capture: &effects::DiagnosticTextureCapture,
    domain: EffectRect,
    local_x: u32,
    local_y: u32,
) -> u32 {
    assert_eq!(
        capture.domain, domain,
        "capture belongs to the requested domain"
    );
    assert_eq!(
        (capture.width, capture.height),
        (domain.width, domain.height),
        "capture storage is cropped to the requested effect domain"
    );
    assert!(local_x < capture.width && local_y < capture.height);
    let storage_x = local_x;
    let storage_y = match capture.origin {
        oblivion_one::effects::GraphTextureOrigin::BottomLeft => capture.height - 1 - local_y,
    };
    let index = storage_y as usize * capture.width as usize + storage_x as usize;
    let encoded = capture.pixels[index].map(|byte| byte.round() as u32);
    encoded[0] | (encoded[1] << 8) | (encoded[2] << 16) | (encoded[3] << 24)
}

fn report_rgba16f_comparison(
    label: &str,
    legacy: &effects::DiagnosticTextureCapture,
    diagnostic: &effects::DiagnosticTextureCapture,
) -> Rgba16fDifferenceSummary {
    let summary = summarize_rgba16f_difference(legacy, diagnostic);
    println!("    {label}: {summary:?}");
    summary
}

fn report_rgba8_comparison(
    label: &str,
    legacy: &[u8],
    diagnostic: &[u8],
    width: u32,
    height: u32,
    origin: OutputFramebufferOrigin,
) -> Rgba8DifferenceSummary {
    let summary = summarize_rgba8_difference(legacy, diagnostic, width, height, origin);
    println!("    {label}: {summary:?}");
    summary
}

fn decode_float_bits_capture(capture: &effects::DiagnosticTextureCapture, index: usize) -> f32 {
    let encoded = capture.pixels[index];
    let bytes = encoded.map(|value| (value * 255.0).round().clamp(0.0, 255.0) as u8);
    f32::from_bits(u32::from_le_bytes(bytes))
}

fn check_virtual_boundary_rgb_matches_legacy(
    effect_rect: EffectRect,
    boundary: &str,
    name: &str,
    legacy_stage: &effects::DiagnosticTextureCapture,
    raw_bits: [&effects::DiagnosticTextureCapture; 3],
    virtual_bits: [&effects::DiagnosticTextureCapture; 3],
) -> Result<(), String> {
    for capture in raw_bits.into_iter().chain(virtual_bits) {
        assert_eq!(
            (capture.width, capture.height),
            (legacy_stage.width, legacy_stage.height)
        );
        assert_eq!(capture.domain, legacy_stage.domain);
        assert_eq!(capture.origin, legacy_stage.origin);
    }
    let mut representative = None;
    let mut first_pixel = None;
    let mut first_mismatch = None;
    let mut difference_count = 0usize;
    let mut raw_virtual_difference_count = 0usize;
    let mut maximum_absolute_difference = 0.0f32;
    let channels = ["r", "g", "b"];
    for index in 0..legacy_stage.pixels.len() {
        let raw = raw_bits.map(|capture| decode_float_bits_capture(capture, index));
        let virtual_value = virtual_bits.map(|capture| decode_float_bits_capture(capture, index));
        let stored = [
            legacy_stage.pixels[index][0],
            legacy_stage.pixels[index][1],
            legacy_stage.pixels[index][2],
        ];
        let storage = [
            (index as u32) % legacy_stage.width,
            (index as u32) / legacy_stage.width,
        ];
        let logical = [
            legacy_stage.domain.x + storage[0] as i32,
            legacy_stage.domain.y + (legacy_stage.height - 1 - storage[1]) as i32,
        ];
        if logical[0] < effect_rect.x
            || logical[1] < effect_rect.y
            || logical[0] >= effect_rect.x + effect_rect.width as i32
            || logical[1] >= effect_rect.y + effect_rect.height as i32
        {
            continue;
        }
        first_pixel.get_or_insert((storage, logical, raw, virtual_value, stored));
        if representative.is_none()
            && (0..3).any(|channel| raw[channel].to_bits() != virtual_value[channel].to_bits())
        {
            representative = Some((storage, logical, raw, virtual_value, stored));
        }
        for channel in 0..3 {
            if raw[channel].to_bits() != virtual_value[channel].to_bits() {
                raw_virtual_difference_count += 1;
            }
            let difference = (virtual_value[channel] - stored[channel]).abs();
            maximum_absolute_difference = maximum_absolute_difference.max(difference);
            if virtual_value[channel].to_bits() != stored[channel].to_bits() {
                difference_count += 1;
                first_mismatch.get_or_insert((
                    storage,
                    channels[channel],
                    raw[channel],
                    virtual_value[channel],
                    stored[channel],
                ));
            }
        }
    }
    let sample = representative
        .or(first_pixel)
        .expect("non-empty RGBA16F capture");
    let (storage, logical, raw, virtual_value, stored) = sample;
    println!(
        "    {boundary} raw boundary case={name} storage={storage:?} logical={logical:?} raw_rgb={raw:?} virtual_rgb={virtual_value:?} legacy_stored_rgb={stored:?} raw_virtual_differing_values={raw_virtual_difference_count} virtual_legacy_differing_values={difference_count} max_abs_delta={maximum_absolute_difference:?}"
    );
    if raw_virtual_difference_count == 0 {
        println!(
            "    {boundary} did not alter raw RGB values for {name}; the direct stored-stage comparison above determines whether this is exact"
        );
    }
    if let Some((storage, channel, raw, virtual_value, stored)) = first_mismatch {
        Err(format!(
            "{boundary} mismatch {name} storage={storage:?} channel={channel}: raw={raw:?}, virtual={virtual_value:?}, legacy={stored:?}; differing_values={difference_count}, max_abs_delta={maximum_absolute_difference:?}"
        ))
    } else {
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn run_terminal_builtin_diagnostic_case(
    name: &str,
    output_size: (u32, u32),
    rect: EffectRect,
    visual_group: VisualGroupId,
    blur: f32,
    saturation: f32,
    noise: f32,
    coverage: Option<oblivion_one::effects::EffectCoverage>,
    origin: OutputFramebufferOrigin,
) -> Vec<String> {
    let mut exactness_failures = Vec::new();
    let material = oblivion_one::material::EffectiveMaterial {
        blur,
        saturation,
        noise,
        blur_passes: 3,
    };
    let (scene, registry) = terminal_builtin_blur_scene(rect, visual_group, material, coverage);
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let legacy_graph = terminal_graph_variant(&scene, &registry, bounds, false);
    let terminal_graph = terminal_graph_variant(&scene, &registry, bounds, true);
    let tails = terminal_tail_capture_ids(&legacy_graph);
    assert_eq!(tails.len(), 1, "single blur has one canonical tail");
    let tail = &tails[0];
    let fused_composite = terminal_graph
        .passes
        .iter()
        .find(|pass| pass.instance == tail.instance && pass.terminal_kawase.is_some())
        .expect("L3 retains the terminal Composite");
    assert_eq!(fused_composite.id, tail.composite);
    let install = |harness: &mut GlesEffectTestHarness| {
        install_diagnostic_scene(harness, rect, visual_group);
    };

    let (l0, c0) = render_terminal_replay_mode(
        &legacy_graph,
        output_size,
        origin,
        install,
        effects::TerminalBoundaryReplay::Legacy,
        [tail.upsample, tail.fragment],
        [],
    );
    let (l_canonical, c_canonical) = render_terminal_replay_mode(
        &legacy_graph,
        output_size,
        origin,
        |harness| install_diagnostic_scene(harness, rect, visual_group),
        effects::TerminalBoundaryReplay::CanonicalLegacyUp2,
        [tail.upsample, tail.fragment],
        [tail.fragment],
    );
    let (l1, c1) = render_terminal_replay_mode(
        &legacy_graph,
        output_size,
        origin,
        |harness| install_diagnostic_scene(harness, rect, visual_group),
        effects::TerminalBoundaryReplay::FuseUp2AndFragment,
        [tail.fragment],
        [],
    );
    let (l2, c2) = render_terminal_replay_mode(
        &legacy_graph,
        output_size,
        origin,
        |harness| install_diagnostic_scene(harness, rect, visual_group),
        effects::TerminalBoundaryReplay::FuseFragmentAndComposite,
        [tail.upsample],
        [],
    );
    let (material_probe, c_material_probe) = render_terminal_replay_mode(
        &legacy_graph,
        output_size,
        origin,
        |harness| install_diagnostic_scene(harness, rect, visual_group),
        effects::TerminalBoundaryReplay::MaterialOnlyProbe,
        [tail.fragment],
        [],
    );
    let (kawase_probe, _) = render_terminal_replay_mode(
        &legacy_graph,
        output_size,
        origin,
        |harness| install_diagnostic_scene(harness, rect, visual_group),
        effects::TerminalBoundaryReplay::KawaseOnlyProbe,
        [],
        [tail.upsample],
    );
    let (l3, c3) = render_terminal_replay_mode(
        &terminal_graph,
        output_size,
        origin,
        |harness| install_diagnostic_scene(harness, rect, visual_group),
        effects::TerminalBoundaryReplay::CaptureTerminalStages,
        [],
        [tail.composite],
    );
    let c_q1_only = (noise == 0.0).then(|| {
        render_terminal_replay_mode(
            &terminal_graph,
            output_size,
            origin,
            |harness| install_diagnostic_scene(harness, rect, visual_group),
            effects::TerminalBoundaryReplay::TerminalQ1Only,
            [],
            [],
        )
        .1
    });
    let c_q2_only = (noise == 0.0).then(|| {
        render_terminal_replay_mode(
            &terminal_graph,
            output_size,
            origin,
            |harness| install_diagnostic_scene(harness, rect, visual_group),
            effects::TerminalBoundaryReplay::TerminalQ2Only,
            [],
            [],
        )
        .1
    });
    let c_q1_q2 = render_terminal_replay_mode(
        &terminal_graph,
        output_size,
        origin,
        |harness| install_diagnostic_scene(harness, rect, visual_group),
        effects::TerminalBoundaryReplay::TerminalQ1Q2,
        [],
        [],
    )
    .1;

    let a0 = l0.captures.get(&tail.upsample).expect("L0 captures A0");
    let b0 = l0.captures.get(&tail.fragment).expect("L0 captures B0");
    let a_canonical = l_canonical
        .captures
        .get(&tail.upsample)
        .expect("canonical legacy replay captures Acanonical");
    let b_canonical = l_canonical
        .captures
        .get(&tail.fragment)
        .expect("canonical legacy replay captures Bcanonical");
    let a1 = kawase_probe
        .stage_probes
        .get(&(tail.upsample, effects::TerminalDiagnosticStage::Up2))
        .expect("A1 terminal-style Up2 probe");
    let a_q1 = kawase_probe
        .stage_probes
        .get(&(tail.upsample, effects::TerminalDiagnosticStage::Up2Q1))
        .expect("Q1-only Up2 probe");
    let a_raw_bits = [
        kawase_probe
            .stage_probes
            .get(&(
                tail.upsample,
                effects::TerminalDiagnosticStage::Up2RawRedBits,
            ))
            .expect("raw Up2 red-bit probe"),
        kawase_probe
            .stage_probes
            .get(&(
                tail.upsample,
                effects::TerminalDiagnosticStage::Up2RawGreenBits,
            ))
            .expect("raw Up2 green-bit probe"),
        kawase_probe
            .stage_probes
            .get(&(
                tail.upsample,
                effects::TerminalDiagnosticStage::Up2RawBlueBits,
            ))
            .expect("raw Up2 blue-bit probe"),
    ];
    let a_q1_bits = [
        kawase_probe
            .stage_probes
            .get(&(
                tail.upsample,
                effects::TerminalDiagnosticStage::Up2Q1RedBits,
            ))
            .expect("Q1 Up2 red-bit probe"),
        kawase_probe
            .stage_probes
            .get(&(
                tail.upsample,
                effects::TerminalDiagnosticStage::Up2Q1GreenBits,
            ))
            .expect("Q1 Up2 green-bit probe"),
        kawase_probe
            .stage_probes
            .get(&(
                tail.upsample,
                effects::TerminalDiagnosticStage::Up2Q1BlueBits,
            ))
            .expect("Q1 Up2 blue-bit probe"),
    ];
    let b_raw_bits = (name == "single_sat_031_noise_0" && noise == 0.0).then(|| {
        [
            l_canonical
                .stage_probes
                .get(&(
                    tail.fragment,
                    effects::TerminalDiagnosticStage::FragmentRawRedBits,
                ))
                .expect("raw Fragment red-bit probe"),
            l_canonical
                .stage_probes
                .get(&(
                    tail.fragment,
                    effects::TerminalDiagnosticStage::FragmentRawGreenBits,
                ))
                .expect("raw Fragment green-bit probe"),
            l_canonical
                .stage_probes
                .get(&(
                    tail.fragment,
                    effects::TerminalDiagnosticStage::FragmentRawBlueBits,
                ))
                .expect("raw Fragment blue-bit probe"),
        ]
    });
    let b_q2_bits = (name == "single_sat_031_noise_0" && noise == 0.0).then(|| {
        [
            l_canonical
                .stage_probes
                .get(&(
                    tail.fragment,
                    effects::TerminalDiagnosticStage::FragmentQ2RedBits,
                ))
                .expect("Q2 Fragment red-bit probe"),
            l_canonical
                .stage_probes
                .get(&(
                    tail.fragment,
                    effects::TerminalDiagnosticStage::FragmentQ2GreenBits,
                ))
                .expect("Q2 Fragment green-bit probe"),
            l_canonical
                .stage_probes
                .get(&(
                    tail.fragment,
                    effects::TerminalDiagnosticStage::FragmentQ2BlueBits,
                ))
                .expect("Q2 Fragment blue-bit probe"),
        ]
    });
    let b1 = l1.captures.get(&tail.fragment).expect("L1 captures B1");
    let a2 = l2.captures.get(&tail.upsample).expect("L2 captures A2");
    let b2 = material_probe
        .captures
        .get(&tail.fragment)
        .expect("material-only probe captures B2");
    let a3 = l3
        .stage_probes
        .get(&(tail.composite, effects::TerminalDiagnosticStage::Up2))
        .expect("A3 terminal Up2 probe");
    let b3 = l3
        .stage_probes
        .get(&(tail.composite, effects::TerminalDiagnosticStage::Fragment))
        .expect("B3 terminal Up2+Fragment probe");

    let pass_count_l1 = legacy_graph.passes.len() - 1;
    let pass_count_l2 = legacy_graph.passes.len() - 1;
    println!(
        "KAWASE_DIAG case={name} origin={origin:?} size={output_size:?} domain={rect:?} saturation={saturation} noise={noise} passes=L0:{} L1:{pass_count_l1} L2:{pass_count_l2} L3:{}",
        legacy_graph.passes.len(),
        terminal_graph.passes.len()
    );
    let a0_a1 = report_rgba16f_comparison("A0 vs A1", a0, a1);
    let a0_acanonical = report_rgba16f_comparison("A0 vs Acanonical", a0, a_canonical);
    let acanonical_a1 = report_rgba16f_comparison("Acanonical vs A1", a_canonical, a1);
    let acanonical_a3 = report_rgba16f_comparison("Acanonical vs A3", a_canonical, a3);
    report_rgba16f_comparison("A0 vs Q1(raw)", a0, a_q1);
    let acanonical_aq1 = report_rgba16f_comparison("Acanonical vs Q1(raw)", a_canonical, a_q1);
    report_rgba16f_comparison("A0 vs A2", a0, a2);
    let a0_a3 = report_rgba16f_comparison("A0 vs A3", a0, a3);
    let b0_b1 = report_rgba16f_comparison("B0 vs B1", b0, b1);
    let bcanonical_b1 = report_rgba16f_comparison("Bcanonical vs B1", b_canonical, b1);
    report_rgba16f_comparison("B0 vs Bcanonical", b0, b_canonical);
    let b0_b2 = report_rgba16f_comparison("B0 vs B2", b0, b2);
    let b0_b3 = report_rgba16f_comparison("B0 vs B3", b0, b3);
    let bcanonical_b3 = report_rgba16f_comparison("Bcanonical vs B3", b_canonical, b3);
    report_rgba8_comparison("C0 vs C1", &c0, &c1, output_size.0, output_size.1, origin);
    report_rgba8_comparison("C0 vs C2", &c0, &c2, output_size.0, output_size.1, origin);
    report_rgba8_comparison(
        "C0 vs C_B2",
        &c0,
        &c_material_probe,
        output_size.0,
        output_size.1,
        origin,
    );
    report_rgba8_comparison("C0 vs C3", &c0, &c3, output_size.0, output_size.1, origin);
    report_rgba8_comparison(
        "C0 vs Q1+Q2",
        &c0,
        &c_q1_q2,
        output_size.0,
        output_size.1,
        origin,
    );
    if let Some(candidate) = &c_q1_only {
        report_rgba8_comparison(
            "C0 vs Q1 only",
            &c0,
            candidate,
            output_size.0,
            output_size.1,
            origin,
        );
    }
    if let Some(candidate) = &c_q2_only {
        report_rgba8_comparison(
            "C0 vs Q2 only",
            &c0,
            candidate,
            output_size.0,
            output_size.1,
            origin,
        );
    }
    for (label, summary) in [
        ("A0 vs A1", &a0_a1),
        ("A0 vs Acanonical", &a0_acanonical),
        ("Acanonical vs A1", &acanonical_a1),
        ("Acanonical vs A3", &acanonical_a3),
        ("Acanonical vs Q1(raw)", &acanonical_aq1),
        ("A0 vs A3", &a0_a3),
    ] {
        if !summary.exactly_equal {
            exactness_failures.push(format!("{name} {origin:?}: {label} mismatch: {summary:?}"));
        }
    }
    if !bcanonical_b1.exactly_equal {
        exactness_failures.push(format!(
            "{name} {origin:?}: Q1 B-stage differs from canonical legacy B: {bcanonical_b1:?}"
        ));
    }
    if !bcanonical_b3.exactly_equal {
        exactness_failures.push(format!(
            "{name} {origin:?}: Q1/Q2 B-stage differs from canonical legacy B: {bcanonical_b3:?}"
        ));
    }
    let c0_canonical = report_rgba8_comparison(
        "C0 vs Ccanonical",
        &c0,
        &c_canonical,
        output_size.0,
        output_size.1,
        origin,
    );
    if c0 != c_canonical {
        exactness_failures.push(format!(
            "{name} {origin:?}: canonical legacy Up2 changed final bytes: {c0_canonical:?}"
        ));
    }
    if noise == 0.0 {
        if !b0_b1.exactly_equal {
            exactness_failures.push(format!("{name} {origin:?}: Q1 B-stage mismatch: {b0_b1:?}"));
        }
        if !b0_b2.exactly_equal {
            exactness_failures.push(format!(
                "{name} {origin:?}: Q2-only B-stage mismatch: {b0_b2:?}"
            ));
        }
        if !b0_b3.exactly_equal {
            exactness_failures.push(format!(
                "{name} {origin:?}: Q1/Q2 B-stage mismatch: {b0_b3:?}"
            ));
        }
        if c0 != c_q1_q2 {
            exactness_failures.push(format!(
                "{name} {origin:?}: Q1+Q2 candidate differs from legacy ({:?})",
                summarize_rgba8_difference(&c0, &c_q1_q2, output_size.0, output_size.1, origin)
            ));
        }
    }
    if name == "single_sat_031_noise_0" && noise == 0.0 {
        if let Err(error) = check_virtual_boundary_rgb_matches_legacy(
            rect,
            "Q1",
            name,
            a_canonical,
            a_raw_bits,
            a_q1_bits,
        ) {
            exactness_failures.push(error);
        }
        if let Err(error) = check_virtual_boundary_rgb_matches_legacy(
            rect,
            "Q2",
            name,
            b_canonical,
            b_raw_bits.expect("Q2 raw probe exists for representative case"),
            b_q2_bits.expect("Q2 virtual probe exists for representative case"),
        ) {
            exactness_failures.push(error);
        }
    }
    let first_divergence = if !a0_a1.exactly_equal || !a0_a3.exactly_equal {
        "final Up2 reconstruction/rasterization"
    } else if !b0_b1.exactly_equal || !b0_b3.exactly_equal {
        if b0_b2.exactly_equal {
            "removing the stored Up2 RGBA16F boundary before local Fragment"
        } else {
            "local Fragment operation after Up2"
        }
    } else {
        "Fragment materialization or Composite"
    };
    println!("    first_divergent_boundary={first_divergence}");
    exactness_failures
}

fn run_terminal_causal_diagnostic_case(noise: f32, origin: OutputFramebufferOrigin) -> Vec<String> {
    let mut exactness_failures = Vec::new();
    let fixture = native_three_checkpoint_fixture();
    let (mut scene, _) = native_three_checkpoint_backdrop_scene(fixture);
    let program = oblivion_one::effects::builtin_background_material_program(
        oblivion_one::material::EffectiveMaterial {
            blur: 0.4,
            saturation: 0.52,
            noise,
            blur_passes: 3,
        },
    );
    let program_id = program.program.id;
    let mut registry = oblivion_one::effects::EffectRegistry::empty();
    registry.insert(program).expect("causal material inserts");
    for instance in &mut scene.instances {
        instance.program = program_id;
    }
    let legacy_graph = terminal_graph_variant(&scene, &registry, fixture.output_bounds, false);
    let terminal_graph = terminal_graph_variant(&scene, &registry, fixture.output_bounds, true);
    let tails = terminal_tail_capture_ids(&legacy_graph);
    assert_eq!(tails.len(), 3, "causal A/B/C has three terminal tails");
    let composite_ids = tails.iter().map(|tail| tail.composite).collect::<Vec<_>>();
    let upsample_ids = tails.iter().map(|tail| tail.upsample).collect::<Vec<_>>();
    let fragment_ids = tails.iter().map(|tail| tail.fragment).collect::<Vec<_>>();
    for tail in &tails {
        assert!(
            terminal_graph
                .passes
                .iter()
                .any(|pass| { pass.id == tail.composite && pass.terminal_kawase.is_some() })
        );
    }
    let install = |harness: &mut GlesEffectTestHarness| {
        install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene);
    };
    let (l0, c0) = render_terminal_replay_mode(
        &legacy_graph,
        fixture.output_size,
        origin,
        install,
        effects::TerminalBoundaryReplay::Legacy,
        upsample_ids
            .iter()
            .copied()
            .chain(fragment_ids.iter().copied()),
        [],
    );
    let (l_canonical, c_canonical) = render_terminal_replay_mode(
        &legacy_graph,
        fixture.output_size,
        origin,
        |harness| install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene),
        effects::TerminalBoundaryReplay::CanonicalLegacyUp2,
        upsample_ids
            .iter()
            .copied()
            .chain(fragment_ids.iter().copied()),
        fragment_ids.iter().copied(),
    );
    let (l1, c1) = render_terminal_replay_mode(
        &legacy_graph,
        fixture.output_size,
        origin,
        |harness| install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene),
        effects::TerminalBoundaryReplay::FuseUp2AndFragment,
        fragment_ids.iter().copied(),
        [],
    );
    let (l2, c2) = render_terminal_replay_mode(
        &legacy_graph,
        fixture.output_size,
        origin,
        |harness| install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene),
        effects::TerminalBoundaryReplay::FuseFragmentAndComposite,
        upsample_ids.iter().copied(),
        [],
    );
    let (b2_probe, c_b2_probe) = render_terminal_replay_mode(
        &legacy_graph,
        fixture.output_size,
        origin,
        |harness| install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene),
        effects::TerminalBoundaryReplay::MaterialOnlyProbe,
        fragment_ids.iter().copied(),
        [],
    );
    let (a1_probe, _) = render_terminal_replay_mode(
        &legacy_graph,
        fixture.output_size,
        origin,
        |harness| install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene),
        effects::TerminalBoundaryReplay::KawaseOnlyProbe,
        [],
        upsample_ids.iter().copied(),
    );
    let (l3, c3) = render_terminal_replay_mode(
        &terminal_graph,
        fixture.output_size,
        origin,
        |harness| install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene),
        effects::TerminalBoundaryReplay::CaptureTerminalStages,
        [],
        composite_ids.iter().copied(),
    );
    let c_q1_only = (noise == 0.0).then(|| {
        render_terminal_replay_mode(
            &terminal_graph,
            fixture.output_size,
            origin,
            |harness| install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene),
            effects::TerminalBoundaryReplay::TerminalQ1Only,
            [],
            [],
        )
        .1
    });
    let c_q2_only = (noise == 0.0).then(|| {
        render_terminal_replay_mode(
            &terminal_graph,
            fixture.output_size,
            origin,
            |harness| install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene),
            effects::TerminalBoundaryReplay::TerminalQ2Only,
            [],
            [],
        )
        .1
    });
    let c_q1_q2 = render_terminal_replay_mode(
        &terminal_graph,
        fixture.output_size,
        origin,
        |harness| install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene),
        effects::TerminalBoundaryReplay::TerminalQ1Q2,
        [],
        [],
    )
    .1;
    println!(
        "KAWASE_DIAG case=causal_abc origin={origin:?} size={:?} tails={} noise={noise} passes=L0:{} L1:{} L2:{} L3:{}",
        fixture.output_size,
        tails.len(),
        legacy_graph.passes.len(),
        legacy_graph.passes.len() - tails.len(),
        legacy_graph.passes.len() - tails.len(),
        terminal_graph.passes.len()
    );
    for tail in &tails {
        let a0 = l0.captures.get(&tail.upsample).expect("causal A0");
        let a_canonical = l_canonical
            .captures
            .get(&tail.upsample)
            .expect("causal canonical legacy A");
        let b0 = l0.captures.get(&tail.fragment).expect("causal B0");
        let b_canonical = l_canonical
            .captures
            .get(&tail.fragment)
            .expect("causal Bcanonical");
        let a1 = a1_probe
            .stage_probes
            .get(&(tail.upsample, effects::TerminalDiagnosticStage::Up2))
            .expect("causal A1");
        let b1 = l1.captures.get(&tail.fragment).expect("causal B1");
        let a2 = l2.captures.get(&tail.upsample).expect("causal A2");
        let b2 = b2_probe.captures.get(&tail.fragment).expect("causal B2");
        let a3 = l3
            .stage_probes
            .get(&(tail.composite, effects::TerminalDiagnosticStage::Up2))
            .expect("causal A3");
        let b3 = l3
            .stage_probes
            .get(&(tail.composite, effects::TerminalDiagnosticStage::Fragment))
            .expect("causal B3");
        println!("    causal_instance={:?}", tail.instance);
        let a0_acanonical = report_rgba16f_comparison("A0 vs Acanonical", a0, a_canonical);
        let acanonical_a1 = report_rgba16f_comparison("Acanonical vs A1", a_canonical, a1);
        let acanonical_a3 = report_rgba16f_comparison("Acanonical vs A3", a_canonical, a3);
        report_rgba16f_comparison("A0 vs A1", a0, a1);
        report_rgba16f_comparison("A0 vs A2", a0, a2);
        report_rgba16f_comparison("B0 vs B1", b0, b1);
        let bcanonical_b1 = report_rgba16f_comparison("Bcanonical vs B1", b_canonical, b1);
        report_rgba16f_comparison("B0 vs B2", b0, b2);
        report_rgba16f_comparison("B0 vs B3", b0, b3);
        let bcanonical_b3 = report_rgba16f_comparison("Bcanonical vs B3", b_canonical, b3);
        if noise == 0.0 {
            if !(a0_acanonical.exactly_equal
                && acanonical_a1.exactly_equal
                && acanonical_a3.exactly_equal)
            {
                exactness_failures.push(format!(
                    "causal {:?} {origin:?}: A-stage mismatch A0/Acanonical={a0_acanonical:?}, Acanonical/A1={acanonical_a1:?}, Acanonical/A3={acanonical_a3:?}",
                    tail.instance,
                ));
            }
            if !bcanonical_b1.exactly_equal {
                exactness_failures.push(format!(
                    "causal {:?} {origin:?}: Q1 B-stage mismatch {bcanonical_b1:?}",
                    tail.instance
                ));
            }
            if !bcanonical_b3.exactly_equal {
                exactness_failures.push(format!(
                    "causal {:?} {origin:?}: Q1/Q2 B-stage mismatch {bcanonical_b3:?}",
                    tail.instance
                ));
            }
        }
    }
    report_rgba8_comparison(
        "C0 vs Ccanonical",
        &c0,
        &c_canonical,
        fixture.output_size.0,
        fixture.output_size.1,
        origin,
    );
    report_rgba8_comparison(
        "C0 vs C1",
        &c0,
        &c1,
        fixture.output_size.0,
        fixture.output_size.1,
        origin,
    );
    report_rgba8_comparison(
        "C0 vs C2",
        &c0,
        &c2,
        fixture.output_size.0,
        fixture.output_size.1,
        origin,
    );
    report_rgba8_comparison(
        "C0 vs C_B2",
        &c0,
        &c_b2_probe,
        fixture.output_size.0,
        fixture.output_size.1,
        origin,
    );
    report_rgba8_comparison(
        "C0 vs C3",
        &c0,
        &c3,
        fixture.output_size.0,
        fixture.output_size.1,
        origin,
    );
    report_rgba8_comparison(
        "C0 vs Q1+Q2",
        &c0,
        &c_q1_q2,
        fixture.output_size.0,
        fixture.output_size.1,
        origin,
    );
    if let Some(candidate) = &c_q1_only {
        report_rgba8_comparison(
            "C0 vs Q1 only",
            &c0,
            candidate,
            fixture.output_size.0,
            fixture.output_size.1,
            origin,
        );
    }
    if let Some(candidate) = &c_q2_only {
        report_rgba8_comparison(
            "C0 vs Q2 only",
            &c0,
            candidate,
            fixture.output_size.0,
            fixture.output_size.1,
            origin,
        );
    }
    if noise == 0.0 {
        assert!(
            c0 == c_canonical,
            "canonical Up2 changed causal legacy output bytes"
        );
        if c0 != c_q1_q2 {
            exactness_failures.push(format!(
                "causal_abc {origin:?}: Q1+Q2 candidate differs from legacy ({:?})",
                summarize_rgba8_difference(
                    &c0,
                    &c_q1_q2,
                    fixture.output_size.0,
                    fixture.output_size.1,
                    origin,
                )
            ));
        }
    }
    exactness_failures
}

#[test]
fn terminal_boundary_rgba16f_noise_free_matrix_and_legacy_noise_attribution() {
    let mut exactness_failures = Vec::new();
    let odd_bounds = (161, 121);
    let odd_rect = EffectRect::new(19, 13, 101, 75).expect("odd translated blur");
    let even_rect = EffectRect::new(18, 14, 96, 72).expect("even blur");
    let rounded_triangle = oblivion_one::effects::EffectCoverage {
        rounded_rect: Some(oblivion_one::effects::EffectCoverageRoundedRect {
            x: odd_rect.x as f64,
            y: odd_rect.y as f64,
            width: odd_rect.width as f64,
            height: odd_rect.height as f64,
            radius: 13.5,
        }),
        triangle: Some(oblivion_one::effects::EffectCoverageTriangle {
            a: [27.0, 23.0],
            b: [108.0, 20.0],
            c: [78.0, 79.0],
        }),
    };
    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        exactness_failures.extend(run_terminal_builtin_diagnostic_case(
            "single_sat_1_noise_0",
            (160, 120),
            even_rect,
            VisualGroupId::new(71).expect("visual group"),
            0.37,
            1.0,
            0.0,
            None,
            origin,
        ));
        exactness_failures.extend(run_terminal_builtin_diagnostic_case(
            "single_sat_031_noise_0",
            (160, 120),
            even_rect,
            VisualGroupId::new(72).expect("visual group"),
            0.37,
            0.31,
            0.0,
            None,
            origin,
        ));
        exactness_failures.extend(run_terminal_builtin_diagnostic_case(
            "odd_translated_sat_031_noise_0",
            odd_bounds,
            odd_rect,
            VisualGroupId::new(73).expect("visual group"),
            0.42,
            0.31,
            0.0,
            None,
            origin,
        ));
        exactness_failures.extend(run_terminal_builtin_diagnostic_case(
            "even_translated_sat_1_noise_0",
            (160, 120),
            even_rect,
            VisualGroupId::new(74).expect("visual group"),
            0.37,
            1.0,
            0.0,
            None,
            origin,
        ));
        exactness_failures.extend(run_terminal_builtin_diagnostic_case(
            "rounded_triangle_coverage_noise_0",
            odd_bounds,
            odd_rect,
            VisualGroupId::new(75).expect("visual group"),
            0.42,
            0.31,
            0.0,
            Some(rounded_triangle.clone()),
            origin,
        ));
        exactness_failures.extend(run_terminal_causal_diagnostic_case(0.0, origin));
    }

    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        let _ = run_terminal_builtin_diagnostic_case(
            "single_sat_031_noise_03_attribution",
            (160, 120),
            even_rect,
            VisualGroupId::new(76).expect("visual group"),
            0.37,
            0.31,
            0.3,
            None,
            origin,
        );
        let _ = run_terminal_causal_diagnostic_case(0.3, origin);
    }
    assert!(
        exactness_failures.is_empty(),
        "Noise-free terminal boundary exactness failures: {}",
        exactness_failures.join("; ")
    );
}

#[test]
fn terminal_kawase_gles_matches_causal_unfused_frames_exactly() {
    let translated_odd_bounds = EffectRect::new(0, 0, 161, 121).expect("odd output bounds");
    let translated_odd_rect = EffectRect::new(19, 13, 101, 75).expect("odd translated blur");
    let rounded = oblivion_one::effects::EffectCoverage {
        rounded_rect: Some(oblivion_one::effects::EffectCoverageRoundedRect {
            x: translated_odd_rect.x as f64,
            y: translated_odd_rect.y as f64,
            width: translated_odd_rect.width as f64,
            height: translated_odd_rect.height as f64,
            radius: 13.5,
        }),
        triangle: Some(oblivion_one::effects::EffectCoverageTriangle {
            a: [27.0, 23.0],
            b: [108.0, 20.0],
            c: [78.0, 79.0],
        }),
    };
    let (odd_scene, odd_registry) = terminal_builtin_blur_scene(
        translated_odd_rect,
        VisualGroupId::new(77).expect("odd test visual group"),
        oblivion_one::material::EffectiveMaterial {
            blur: 0.42,
            saturation: 0.31,
            noise: 0.0,
            blur_passes: 3,
        },
        Some(rounded),
    );
    let odd_off = terminal_graph_variant(&odd_scene, &odd_registry, translated_odd_bounds, false);
    let odd_on = terminal_graph_variant(&odd_scene, &odd_registry, translated_odd_bounds, true);
    assert_eq!(odd_off.stats.terminal_blur_fusions, 0);
    assert_eq!(
        odd_on.stats.terminal_blur_fusions, 1,
        "terminal candidate count={}, passes={:#?}, textures={:#?}",
        odd_on.stats.terminal_blur_fusion_candidates, odd_on.passes, odd_on.textures
    );
    let odd_composite = odd_on
        .passes
        .iter()
        .find(|pass| pass.kind == RenderPassKind::Composite)
        .expect("fused graph retains its final Composite");
    assert_eq!(odd_composite.coverage, odd_scene.instances[0].coverage);
    assert_eq!(
        odd_off
            .passes
            .iter()
            .filter(|pass| pass.kind == RenderPassKind::SceneCapture)
            .count(),
        odd_on
            .passes
            .iter()
            .filter(|pass| pass.kind == RenderPassKind::SceneCapture)
            .count(),
        "fusion must retain causal capture topology"
    );
    let odd_off_pixels = render_terminal_graph(
        &odd_off,
        (161, 121),
        OutputFramebufferOrigin::TopLeftScanout,
        |harness| {
            install_diagnostic_scene(
                harness,
                translated_odd_rect,
                VisualGroupId::new(77).unwrap(),
            );
        },
    );
    let odd_on_pixels = render_terminal_graph(
        &odd_on,
        (161, 121),
        OutputFramebufferOrigin::TopLeftScanout,
        |harness| {
            install_diagnostic_scene(
                harness,
                translated_odd_rect,
                VisualGroupId::new(77).unwrap(),
            );
        },
    );
    let odd_diff = terminal_pixel_difference_summary(
        &odd_off_pixels,
        &odd_on_pixels,
        161,
        121,
        OutputFramebufferOrigin::TopLeftScanout,
    );

    let repair = OutputRect::new(20, 14, 3, 2);
    let odd_off_partial = render_terminal_graph_after_partial_edge_damage(
        &odd_off,
        (161, 121),
        OutputFramebufferOrigin::TopLeftScanout,
        |harness| {
            install_diagnostic_scene(
                harness,
                translated_odd_rect,
                VisualGroupId::new(77).unwrap(),
            );
        },
        repair,
    );
    let odd_on_partial = render_terminal_graph_after_partial_edge_damage(
        &odd_on,
        (161, 121),
        OutputFramebufferOrigin::TopLeftScanout,
        |harness| {
            install_diagnostic_scene(
                harness,
                translated_odd_rect,
                VisualGroupId::new(77).unwrap(),
            );
        },
        repair,
    );
    let partial_diff = terminal_pixel_difference_summary(
        &odd_off_partial,
        &odd_on_partial,
        161,
        121,
        OutputFramebufferOrigin::TopLeftScanout,
    );

    let even_bounds = EffectRect::new(0, 0, 160, 120).expect("even output bounds");
    let even_rect = EffectRect::new(18, 14, 96, 72).expect("even blur domain");
    let (even_scene, even_registry) = terminal_builtin_blur_scene(
        even_rect,
        VisualGroupId::new(78).expect("even test visual group"),
        oblivion_one::material::EffectiveMaterial {
            blur: 0.37,
            saturation: 1.0,
            noise: 0.0,
            blur_passes: 3,
        },
        None,
    );
    let even_off = terminal_graph_variant(&even_scene, &even_registry, even_bounds, false);
    let even_on = terminal_graph_variant(&even_scene, &even_registry, even_bounds, true);
    let even_off_pixels = render_terminal_graph(
        &even_off,
        (160, 120),
        OutputFramebufferOrigin::BottomLeft,
        |harness| {
            install_diagnostic_scene(harness, even_rect, VisualGroupId::new(78).unwrap());
        },
    );
    let even_on_pixels = render_terminal_graph(
        &even_on,
        (160, 120),
        OutputFramebufferOrigin::BottomLeft,
        |harness| {
            install_diagnostic_scene(harness, even_rect, VisualGroupId::new(78).unwrap());
        },
    );
    let even_diff = terminal_pixel_difference_summary(
        &even_off_pixels,
        &even_on_pixels,
        160,
        120,
        OutputFramebufferOrigin::BottomLeft,
    );

    // Keep the real A -> C -> B causal checkpoint chain while comparing the
    // same canonical local material under the two physical graph variants.
    let fixture = native_three_checkpoint_fixture();
    let (mut causal_scene, _) = native_three_checkpoint_backdrop_scene(fixture);
    let program = oblivion_one::effects::builtin_background_material_program(
        oblivion_one::material::EffectiveMaterial {
            blur: 0.4,
            saturation: 0.52,
            noise: 0.3,
            blur_passes: 3,
        },
    );
    let program_id = program.program.id;
    let mut causal_registry = oblivion_one::effects::EffectRegistry::empty();
    causal_registry
        .insert(program)
        .expect("causal material inserts");
    for instance in &mut causal_scene.instances {
        instance.program = program_id;
    }
    let causal_off = terminal_graph_variant(
        &causal_scene,
        &causal_registry,
        fixture.output_bounds,
        false,
    );
    let causal_on =
        terminal_graph_variant(&causal_scene, &causal_registry, fixture.output_bounds, true);
    assert_eq!(causal_off.stats.terminal_blur_fusions, 0);
    assert_eq!(causal_on.stats.terminal_blur_fusions, 3);
    let capture_dependencies = |graph: &CompiledFrameGraph| {
        graph
            .passes
            .iter()
            .filter(|pass| pass.kind == RenderPassKind::SceneCapture)
            .map(|pass| (pass.instance, pass.checkpoint_dependencies.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        capture_dependencies(&causal_off),
        capture_dependencies(&causal_on)
    );
    let causal_off_pixels = render_terminal_graph(
        &causal_off,
        fixture.output_size,
        OutputFramebufferOrigin::BottomLeft,
        |harness| install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene),
    );
    let causal_on_pixels = render_terminal_graph(
        &causal_on,
        fixture.output_size,
        OutputFramebufferOrigin::BottomLeft,
        |harness| install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene),
    );
    let causal_diff = terminal_pixel_difference_summary(
        &causal_off_pixels,
        &causal_on_pixels,
        fixture.output_size.0,
        fixture.output_size.1,
        OutputFramebufferOrigin::BottomLeft,
    );
    assert!(
        odd_diff.is_empty()
            && partial_diff.is_empty()
            && even_diff.is_empty()
            && causal_diff.is_empty(),
        "pixel differences: odd={odd_diff}; partial={partial_diff}; even={even_diff}; causal={causal_diff}"
    );
}

fn precomputed_baseline_safety_scene(
    exact_region: EffectRect,
    candidate_regions: [EffectRect; 2],
) -> (ResolvedEffectScene, oblivion_one::effects::EffectRegistry) {
    // The safety geometry is tuned to the two-level material footprint.
    let program = oblivion_one::effects::builtin_background_material_program_with_passes(
        oblivion_one::material::MaterialConfiguration::default()
            .effective()
            .expect("default material validates"),
        2,
    );
    let program_id = program.program.id;
    let mut registry = oblivion_one::effects::EffectRegistry::empty();
    registry.insert(program).expect("built-in blur registry");
    let regions = [exact_region, candidate_regions[0], candidate_regions[1]];
    let instances = regions
        .into_iter()
        .enumerate()
        .map(|(index, rect)| {
            let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(
                10 + u32::try_from(index).expect("test surface index"),
            );
            oblivion_one::compositor::ResolvedEffectInstance {
                composite_opacity: None,
                id: oblivion_one::effects::EffectInstanceId::new(
                    u64::try_from(index + 1).expect("test effect instance id"),
                )
                .expect("test effect instance id is non-zero"),
                program: program_id,
                anchor,
                target_bounds: rect,
                region: EffectRegion::from_rect(rect),
                parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
                coverage: None,
                signature: u64::try_from(index + 1).expect("test signature"),
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: Some(
                    oblivion_one::compositor::VisualGroupId::new(1).expect("test visual group"),
                ),
                anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
                backdrop_reuse: if index == 0 {
                    oblivion_one::compositor::EffectBackdropReuse::Exact
                } else {
                    oblivion_one::compositor::EffectBackdropReuse::PrecomputedWindowCandidate
                },
                scene_order: oblivion_one::compositor::EffectSceneOrder {
                    group_order: 0,
                    surface_order: u32::try_from(index * 10 + 10).expect("test scene order"),
                    phase: 0,
                },
            }
        })
        .collect();
    (ResolvedEffectScene::new(1, instances), registry)
}

#[test]
fn precomputed_baseline_fails_closed_when_exact_capture_depends_on_earlier_effect() {
    let bounds = EffectRect::new(0, 0, 800, 240).expect("output bounds");
    let (scene, registry) = precomputed_baseline_safety_scene(
        EffectRect::new(145, 40, 80, 60).expect("exact effect region"),
        [
            EffectRect::new(160, 40, 80, 60).expect("first candidate region"),
            EffectRect::new(290, 40, 80, 60).expect("second candidate region"),
        ],
    );
    let variants = oblivion_one::effects::compile_frame_execution_variants(
        &scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        true,
    )
    .expect("execution variants compile");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(exact) = &variants.exact else {
        panic!("visible effects compile to an exact graph");
    };
    let earlier_composite = exact
        .passes
        .iter()
        .find(|pass| {
            pass.instance == scene.instances[0].id && pass.kind == RenderPassKind::Composite
        })
        .expect("earlier exact effect has a composite pass");
    let baseline_capture = exact
        .passes
        .iter()
        .find(|pass| {
            pass.instance == scene.instances[1].id && pass.kind == RenderPassKind::SceneCapture
        })
        .expect("earliest candidate has an exact SceneCapture");
    assert!(
        baseline_capture
            .checkpoint_dependencies
            .contains(&earlier_composite.id)
    );
    assert!(variants.precomputed_backdrop.is_none());
    assert!(variants.precomputed_graph.is_none());
}

#[test]
fn precomputed_baseline_allows_non_overlapping_earlier_exact_effect() {
    let bounds = EffectRect::new(0, 0, 2_400, 600).expect("output bounds");
    let (scene, registry) = precomputed_baseline_safety_scene(
        EffectRect::new(20, 40, 80, 60).expect("exact effect region"),
        [
            EffectRect::new(900, 40, 80, 60).expect("first candidate region"),
            EffectRect::new(1_500, 40, 80, 60).expect("second candidate region"),
        ],
    );
    let variants = oblivion_one::effects::compile_frame_execution_variants(
        &scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        true,
    )
    .expect("execution variants compile");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(exact) = &variants.exact else {
        panic!("visible effects compile to an exact graph");
    };
    let baseline_capture = exact
        .passes
        .iter()
        .find(|pass| {
            pass.instance == scene.instances[1].id && pass.kind == RenderPassKind::SceneCapture
        })
        .expect("earliest candidate has an exact SceneCapture");
    assert!(baseline_capture.checkpoint_dependencies.is_empty());
    let plan = variants
        .precomputed_backdrop
        .expect("unrelated exact work does not disable a safe baseline");
    assert_eq!(plan.baseline_instance, scene.instances[1].id);
}

#[test]
fn precomputed_policy_off_and_assignment_provenance_remain_exact() {
    let bounds = EffectRect::new(0, 0, 800, 240).expect("output bounds");
    let (scene, registry) = precomputed_baseline_safety_scene(
        EffectRect::new(8, 8, 32, 24).expect("earlier exact region"),
        [
            EffectRect::new(240, 40, 32, 24).expect("first candidate region"),
            EffectRect::new(640, 160, 32, 24).expect("second candidate region"),
        ],
    );
    let variants = oblivion_one::effects::compile_frame_execution_variants(
        &scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        false,
    )
    .expect("exact variants compile while policy is off");
    assert!(variants.precomputed_graph.is_none());
    assert!(variants.precomputed_backdrop.is_none());
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(exact) = variants.exact else {
        panic!("visible effects compile to an exact graph");
    };
    assert_eq!(
        exact
            .passes
            .iter()
            .filter(|pass| pass.kind == RenderPassKind::SceneCapture)
            .count(),
        3
    );

    use oblivion_one::compositor::EffectBackdropReuse;
    use oblivion_one::compositor::blur_assignment::BlurAssignmentSource;
    assert_eq!(
        EffectBackdropReuse::from(BlurAssignmentSource::Client),
        EffectBackdropReuse::Exact
    );
    assert_eq!(
        EffectBackdropReuse::from(BlurAssignmentSource::LayerRule),
        EffectBackdropReuse::Exact
    );
    assert_eq!(
        EffectBackdropReuse::from(BlurAssignmentSource::WaylandAuto),
        EffectBackdropReuse::PrecomputedWindowCandidate
    );
    assert_eq!(
        EffectBackdropReuse::from(BlurAssignmentSource::WindowRule),
        EffectBackdropReuse::PrecomputedWindowCandidate
    );

    let mut client_scene = scene;
    client_scene.instances[1].backdrop_reuse = EffectBackdropReuse::Exact;
    let client_variants = oblivion_one::effects::compile_frame_execution_variants(
        &client_scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        true,
    )
    .expect("client exact variants compile");
    assert_eq!(client_variants.eligible_instances, 1);
    let shared = client_variants
        .precomputed_backdrop
        .expect("one eligible window still shares the backdrop");
    assert!(
        shared
            .consumers
            .iter()
            .all(|consumer| consumer.instance != client_scene.instances[1].id),
        "exact client blur never consumes the shared backdrop"
    );
}

#[test]
fn shared_build_has_five_physical_stages_and_external_demand_stays_local() {
    let bounds = EffectRect::new(0, 0, 800, 240).expect("output bounds");
    let (scene, registry) = precomputed_baseline_safety_scene(
        EffectRect::new(8, 8, 32, 24).expect("earlier exact region"),
        [
            EffectRect::new(240, 40, 32, 24).expect("first candidate region"),
            EffectRect::new(640, 160, 32, 24).expect("second candidate region"),
        ],
    );
    let variants = oblivion_one::effects::compile_frame_execution_variants(
        &scene,
        &EffectRegion::from_rect(bounds),
        bounds,
        &registry,
        true,
    )
    .expect("precomputed variants compile");
    let plan = variants
        .precomputed_backdrop
        .as_ref()
        .expect("independent baseline selects shared backdrop");
    assert_eq!(plan.baseline_instance, scene.instances[1].id);
    assert_eq!(plan.build_passes, 5);
    let build = oblivion_one::effects::compile_precomputed_backdrop_build(plan, bounds);
    assert_eq!(
        build.stages,
        [
            oblivion_one::effects::PrecomputedBackdropBuildStage::SceneCaptureReplay,
            oblivion_one::effects::PrecomputedBackdropBuildStage::KawaseDownsample {
                level: 1,
                width: 400,
                height: 120,
            },
            oblivion_one::effects::PrecomputedBackdropBuildStage::KawaseDownsample {
                level: 2,
                width: 200,
                height: 60,
            },
            oblivion_one::effects::PrecomputedBackdropBuildStage::KawaseUpsample {
                level: 1,
                width: 400,
                height: 120,
            },
            oblivion_one::effects::PrecomputedBackdropBuildStage::KawaseUpsample {
                level: 2,
                width: 800,
                height: 240,
            },
        ]
    );
    let graph = variants
        .precomputed_graph
        .as_ref()
        .expect("optimized consumer graph exists");
    assert!(!graph.passes.iter().any(|pass| {
        plan.consumers
            .iter()
            .any(|consumer| consumer.instance == pass.instance)
            && matches!(
                pass.kind,
                RenderPassKind::SceneCapture
                    | RenderPassKind::DualKawaseDownsample
                    | RenderPassKind::DualKawaseUpsample
            )
    }));
    for consumer in &plan.consumers {
        let texture = graph
            .textures
            .iter()
            .find(|texture| texture.id == consumer.texture)
            .expect("consumer external texture exists");
        assert_eq!(texture.source, GraphTextureSource::PrecomputedBackdrop);
        assert_eq!(texture.domain, bounds);
        assert_eq!(texture.working_space, EffectWorkingSpace::LinearSrgb);
    }
    for pass in graph
        .passes
        .iter()
        .filter(|pass| pass.kind == RenderPassKind::Fragment)
    {
        let texture = graph
            .textures
            .iter()
            .find(|texture| texture.id == pass.output.expect("fragment output"))
            .expect("local material texture exists");
        assert_ne!(texture.domain, bounds);
    }
    let demand = plan_effect_execution_demand(graph, &EffectRegion::from_rect(bounds), false);
    assert!(!demand.is_conservative_full());
}

/// Clears the output, materializes `precomputed`'s shared backdrop by
/// replaying its scene prefix, then executes the optimized consumer graph
/// against it and reads the output back.
fn precomputed_variant_pixels(
    harness: &mut GlesEffectTestHarness,
    precomputed: &oblivion_one::effects::CompiledPrecomputedBackdropPlan,
    optimized_graph: &oblivion_one::effects::CompiledFrameGraph,
    full_damage: &EffectRegion,
    repaint: &RepaintPlan,
    output_bounds: EffectRect,
) -> Vec<u8> {
    unsafe {
        harness.gl.bind_framebuffer(
            glow::FRAMEBUFFER,
            harness.renderer.scene_state.active_output_framebuffer,
        );
        harness.gl.disable(glow::SCISSOR_TEST);
        harness.gl.clear_color(0.0, 0.0, 0.0, 0.0);
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
    }
    harness.renderer.establish_ordinary_scene_state();

    let build =
        oblivion_one::effects::compile_precomputed_backdrop_build(precomputed, output_bounds);
    let capture = harness
        .renderer
        .effect_runtime
        .effect_resources
        .acquire(
            &harness.gl,
            EffectTextureKey::new(
                build.capture.width,
                build.capture.height,
                EffectTextureFormat::Rgba8,
                EffectTextureFilter::Linear,
                build.capture.working_space,
            ),
        )
        .expect("shared capture texture allocates");
    let framebuffer = harness
        .renderer
        .effect_runtime
        .effect_resources
        .bind_draw_target(&harness.gl, &capture)
        .expect("shared capture framebuffer binds");
    harness
        .renderer
        .establish_scene_state_for_framebuffer(Some(framebuffer));
    unsafe {
        harness.gl.disable(glow::SCISSOR_TEST);
        harness.gl.disable(glow::BLEND);
        harness.gl.clear_color(0.0, 0.0, 0.0, 1.0);
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
    }
    let (prefix_end, _) = effects::composition_range_with_metadata(
        &harness.renderer.scene_state.commands,
        &harness.renderer.scene_state.capture_layers,
        &harness.renderer.scene_state.capture_visual_groups,
        precomputed.baseline_anchor,
        precomputed.baseline_visual_group,
        precomputed.baseline_anchor_scope,
    );
    let capture_result = {
        let mut context = harness.renderer.effect_execution_context();
        context.draw_effect_scene_range(
            &[OutputRect::new(
                0,
                0,
                output_bounds.width,
                output_bounds.height,
            )],
            0,
            prefix_end,
            OutputFramebufferOrigin::BottomLeft,
            EffectFramebufferTarget::new(Some(framebuffer)),
        )
    };
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .unbind_render_target(&harness.gl);
    harness.renderer.establish_ordinary_scene_state();
    capture_result.expect("baseline-only scene prefix replays to the shared capture");
    let (build_stats, shared_owner) = {
        let mut context = harness.renderer.effect_execution_context();
        effects::execute_precomputed_backdrop_build(
            &mut context,
            &build,
            capture,
            None,
            OutputFramebufferOrigin::BottomLeft,
        )
    }
    .expect("shared capture and four Kawase stages execute in real GLES");
    assert_eq!(build_stats.passes, 5);
    assert_eq!(build_stats.scene_captures, 1);
    assert_eq!(build_stats.blur_downsamples, 2);
    assert_eq!(build_stats.blur_upsamples, 2);
    assert_eq!(shared_owner.key.format, EffectTextureFormat::Rgba16Float);
    assert_eq!(
        shared_owner.key.working_space,
        EffectWorkingSpace::LinearSrgb
    );

    harness
        .renderer
        .effect_runtime
        .effect_resources
        .prepare_graph(&harness.gl, optimized_graph)
        .expect("optimized consumer graph preflights");
    let optimized_demand = plan_effect_execution_demand(optimized_graph, full_damage, true);
    let optimized_selection = effects::select_effect_execution(optimized_graph, &optimized_demand);
    let external_bindings = precomputed
        .consumers
        .iter()
        .map(|consumer| {
            (
                consumer.texture,
                effects::GraphTextureBinding::precomputed_backdrop_alias(shared_owner.clone()),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut prepared = {
        let mut context = harness.renderer.effect_execution_context();
        effects::prepare_effect_graph_execution_with_external_bindings(
            &mut context,
            optimized_graph,
            OutputFramebufferOrigin::BottomLeft,
            repaint,
            &optimized_demand,
            &optimized_selection,
            *effects::effect_debug_config(),
            None,
            external_bindings,
        )
        .expect("optimized graph prepares with non-owning shared aliases")
    };
    let optimized_result = {
        let mut context = harness.renderer.effect_execution_context();
        effects::execute_prepared_effect_graph_core(&mut context, &mut prepared)
    };
    {
        let mut context = harness.renderer.effect_execution_context();
        effects::finish_prepared_effect_graph_execution(&mut context, prepared, optimized_result)
    }
    .expect("optimized graph executes in real GLES");
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .release(shared_owner)
        .expect("single shared owner releases after graph execution");
    read_diagnostic_pixels(harness)
}

#[test]
fn precomputed_baseline_only_scene_content_matches_exact_gles_pixels() {
    const PIXEL_TOLERANCE: u8 = 2;

    let mut harness = GlesEffectTestHarness::new(320, 240);
    harness.install_texture_backed_output();
    let output_bounds = EffectRect::new(0, 0, 320, 240).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let a_group = oblivion_one::compositor::VisualGroupId::new(1).expect("A visual group");
    let b_group = oblivion_one::compositor::VisualGroupId::new(2).expect("B visual group");
    let a_bounds = EffectRect::new(12, 12, 32, 24).expect("A bounds");
    // With the 88 px Dual Kawase reach, B at (112,78) overlaps A's expanded
    // influence and gains a checkpoint dependency. Move B clear of A's domain.
    let b_bounds = EffectRect::new(232, 112, 32, 24).expect("B bounds");
    install_native_stacked_diagnostic_scene(
        &mut harness,
        NativeStackedBackdropSceneSpec {
            background_surface: 7,
            a_surface: 42,
            b_surface: 43,
            a_content_bounds: a_bounds,
            b_content_bounds: b_bounds,
            a_visual_group: a_group,
            b_visual_group: b_group,
        },
    );
    let program = oblivion_one::effects::builtin_background_material_program_with_passes(
        oblivion_one::material::MaterialConfiguration::default()
            .effective()
            .expect("default material validates"),
        2,
    );
    let program_id = program.program.id;
    let mut registry = oblivion_one::effects::EffectRegistry::empty();
    registry.insert(program).expect("built-in blur registry");
    let scene = ResolvedEffectScene::new(
        1,
        [
            (1_u64, 42_u32, a_bounds, a_group),
            (2_u64, 43_u32, b_bounds, b_group),
        ]
        .into_iter()
        .map(|(id, surface, bounds, visual_group)| {
            let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(surface);
            oblivion_one::compositor::ResolvedEffectInstance {
                composite_opacity: None,
                id: oblivion_one::effects::EffectInstanceId::new(id)
                    .expect("test effect id is non-zero"),
                program: program_id,
                anchor,
                target_bounds: bounds,
                region: EffectRegion::from_rect(bounds),
                parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
                coverage: None,
                signature: id,
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: Some(visual_group),
                anchor_scope: oblivion_one::compositor::EffectAnchorScope::VisualGroup,
                backdrop_reuse:
                    oblivion_one::compositor::EffectBackdropReuse::PrecomputedWindowCandidate,
                scene_order: oblivion_one::compositor::EffectSceneOrder::for_anchor(anchor),
            }
        })
        .collect(),
    );
    harness.renderer.set_effect_registry(registry.clone());
    let variants = oblivion_one::effects::compile_frame_execution_variants(
        &scene,
        &full_damage,
        output_bounds,
        &registry,
        true,
    )
    .expect("exact and precomputed graphs compile");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(exact_graph) = &variants.exact
    else {
        panic!("exact baseline scene compiles to an effect graph");
    };
    let optimized_graph = variants
        .precomputed_graph
        .as_ref()
        .expect("two candidates select the precomputed graph");
    let precomputed = variants
        .precomputed_backdrop
        .as_ref()
        .expect("safe exact baseline selects shared backdrop");
    for candidate in &scene.instances[1..] {
        let capture = exact_graph
            .passes
            .iter()
            .find(|pass| pass.instance == candidate.id && pass.kind == RenderPassKind::SceneCapture)
            .expect("candidate has an exact baseline capture");
        assert!(capture.checkpoint_dependencies.is_empty());
    }
    let repaint = diagnostic_repaint_plan_for_repairs_in_size(&[], true, (320, 240));

    let exact_demand = plan_effect_execution_demand(exact_graph, &full_damage, true);
    let exact_selection = effects::select_effect_execution(exact_graph, &exact_demand);
    effects::execute_effect_graph(
        &mut harness.renderer,
        exact_graph,
        OutputFramebufferOrigin::BottomLeft,
        &repaint,
        &exact_demand,
        &exact_selection,
    )
    .expect("exact graph executes in real GLES");
    let exact_pixels = read_diagnostic_pixels(&harness);

    let optimized_pixels = precomputed_variant_pixels(
        &mut harness,
        precomputed,
        optimized_graph,
        &full_damage,
        &repaint,
        output_bounds,
    );

    let mut compared_pixels = 0;
    for rect in [a_bounds, b_bounds] {
        for y in rect.y..rect.y + rect.height as i32 {
            for x in rect.x..rect.x + rect.width as i32 {
                // Readback rows are bottom-up.
                let row = 239 - y as usize;
                let offset = (row * 320 + x as usize) * 4;
                for channel in 0..4 {
                    let difference =
                        exact_pixels[offset + channel].abs_diff(optimized_pixels[offset + channel]);
                    assert!(
                        difference <= PIXEL_TOLERANCE,
                        "candidate pixel differs by {difference} at ({x}, {y}) channel {channel}"
                    );
                }
                compared_pixels += 1;
            }
        }
    }
    assert_eq!(
        compared_pixels,
        (a_bounds.width * a_bounds.height + b_bounds.width * b_bounds.height) as usize
    );
}

fn shell_backdrop_test_scene(
    program_id: oblivion_one::effects::EffectProgramId,
    a_bounds: EffectRect,
    b_bounds: EffectRect,
    a_group: oblivion_one::compositor::VisualGroupId,
    b_group: oblivion_one::compositor::VisualGroupId,
    shell_reuse: oblivion_one::compositor::EffectBackdropReuse,
) -> ResolvedEffectScene {
    // A is an ordinary window blur; B models a shell bar above it whose
    // region is clear of A's content.
    let instance =
        |id: u64, surface: u32, bounds: EffectRect, visual_group, anchor_scope, backdrop_reuse| {
            let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(surface);
            oblivion_one::compositor::ResolvedEffectInstance {
                composite_opacity: None,
                id: oblivion_one::effects::EffectInstanceId::new(id)
                    .expect("test effect id is non-zero"),
                program: program_id,
                anchor,
                target_bounds: bounds,
                region: EffectRegion::from_rect(bounds),
                parameter_block: oblivion_one::effects::EffectParameterBlock::default(),
                coverage: None,
                signature: id,
                frame_demand: EffectFrameDemand::OnDamage,
                visual_group: Some(visual_group),
                anchor_scope,
                backdrop_reuse,
                scene_order: oblivion_one::compositor::EffectSceneOrder::for_anchor(anchor),
            }
        };
    ResolvedEffectScene::new(
        1,
        vec![
            instance(
                1,
                42,
                a_bounds,
                a_group,
                oblivion_one::compositor::EffectAnchorScope::VisualGroup,
                oblivion_one::compositor::EffectBackdropReuse::Exact,
            ),
            instance(
                2,
                43,
                b_bounds,
                b_group,
                oblivion_one::compositor::EffectAnchorScope::Surface,
                shell_reuse,
            ),
        ],
    )
}

fn shell_backdrop_point(
    a_group: oblivion_one::compositor::VisualGroupId,
) -> oblivion_one::compositor::SharedBackdropPoint {
    let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);
    oblivion_one::compositor::SharedBackdropPoint {
        anchor,
        visual_group: a_group,
        scene_order: oblivion_one::compositor::EffectSceneOrder::for_anchor(anchor),
    }
}

#[test]
fn shell_blur_shares_backdrop_below_windows_only_with_a_point() {
    let output_bounds = EffectRect::new(0, 0, 320, 240).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let a_group = oblivion_one::compositor::VisualGroupId::new(1).expect("A visual group");
    let b_group = oblivion_one::compositor::VisualGroupId::new(2).expect("B visual group");
    let a_bounds = EffectRect::new(12, 12, 32, 24).expect("A bounds");
    let b_bounds = EffectRect::new(232, 112, 32, 24).expect("B bounds");
    let program = oblivion_one::effects::builtin_background_material_program_with_passes(
        oblivion_one::material::MaterialConfiguration::default()
            .effective()
            .expect("default material validates"),
        2,
    );
    let program_id = program.program.id;
    let mut registry = oblivion_one::effects::EffectRegistry::empty();
    registry.insert(program).expect("built-in blur registry");
    let compile = |scene: &ResolvedEffectScene| {
        oblivion_one::effects::compile_frame_execution_variants(
            scene,
            &full_damage,
            output_bounds,
            &registry,
            false,
        )
        .expect("variants compile")
    };
    let shell = oblivion_one::compositor::EffectBackdropReuse::PrecomputedShellCandidate;
    let base = shell_backdrop_test_scene(program_id, a_bounds, b_bounds, a_group, b_group, shell);

    // Without a point nothing proves the bar's backdrop is window-free.
    assert!(compile(&base).precomputed_backdrop.is_none());

    let point = shell_backdrop_point(a_group);
    let with_point = base.clone().with_shared_backdrop_point(Some(point));
    let variants = compile(&with_point);
    let plan = variants
        .precomputed_backdrop
        .as_ref()
        .expect("shell candidate shares the below-window backdrop");
    assert_eq!(plan.baseline_anchor, point.anchor);
    assert_eq!(plan.baseline_scene_order, point.scene_order);
    assert_eq!(
        plan.baseline_anchor_scope,
        oblivion_one::compositor::EffectAnchorScope::VisualGroup
    );
    assert_eq!(plan.baseline_visual_group, Some(a_group));
    assert_eq!(plan.consumers.len(), 1);
    assert_eq!(plan.consumers[0].instance, with_point.instances[1].id);

    // An ordinary window blur never becomes a shell consumer.
    let exact = oblivion_one::compositor::EffectBackdropReuse::Exact;
    let plain = shell_backdrop_test_scene(program_id, a_bounds, b_bounds, a_group, b_group, exact)
        .with_shared_backdrop_point(Some(point));
    assert!(compile(&plain).precomputed_backdrop.is_none());

    // A point above an effect would drop that effect's output from the
    // replayed backdrop, so it disables sharing.
    let above_anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(43);
    let above = oblivion_one::compositor::SharedBackdropPoint {
        anchor: above_anchor,
        visual_group: b_group,
        scene_order: oblivion_one::compositor::EffectSceneOrder::for_anchor(above_anchor),
    };
    let late = base.with_shared_backdrop_point(Some(above));
    if late.instances[0].scene_order < above.scene_order {
        assert!(compile(&late).precomputed_backdrop.is_none());
    }
}

#[test]
fn shell_backdrop_below_windows_matches_exact_gles_pixels() {
    const PIXEL_TOLERANCE: u8 = 2;

    let mut harness = GlesEffectTestHarness::new(320, 240);
    harness.install_texture_backed_output();
    let output_bounds = EffectRect::new(0, 0, 320, 240).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let a_group = oblivion_one::compositor::VisualGroupId::new(1).expect("A visual group");
    let b_group = oblivion_one::compositor::VisualGroupId::new(2).expect("B visual group");
    let a_bounds = EffectRect::new(12, 12, 32, 24).expect("A bounds");
    // Clear of A's 88 px blur reach, so below-window and exact backdrops
    // agree inside B.
    let b_bounds = EffectRect::new(232, 112, 32, 24).expect("B bounds");
    install_native_stacked_diagnostic_scene(
        &mut harness,
        NativeStackedBackdropSceneSpec {
            background_surface: 7,
            a_surface: 42,
            b_surface: 43,
            a_content_bounds: a_bounds,
            b_content_bounds: b_bounds,
            a_visual_group: a_group,
            b_visual_group: b_group,
        },
    );
    let program = oblivion_one::effects::builtin_background_material_program_with_passes(
        oblivion_one::material::MaterialConfiguration::default()
            .effective()
            .expect("default material validates"),
        2,
    );
    let program_id = program.program.id;
    let mut registry = oblivion_one::effects::EffectRegistry::empty();
    registry.insert(program).expect("built-in blur registry");
    harness.renderer.set_effect_registry(registry.clone());
    let scene = shell_backdrop_test_scene(
        program_id,
        a_bounds,
        b_bounds,
        a_group,
        b_group,
        oblivion_one::compositor::EffectBackdropReuse::PrecomputedShellCandidate,
    )
    .with_shared_backdrop_point(Some(shell_backdrop_point(a_group)));
    let variants = oblivion_one::effects::compile_frame_execution_variants(
        &scene,
        &full_damage,
        output_bounds,
        &registry,
        false,
    )
    .expect("exact and precomputed graphs compile");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(exact_graph) = &variants.exact
    else {
        panic!("exact scene compiles to an effect graph");
    };
    let optimized_graph = variants
        .precomputed_graph
        .as_ref()
        .expect("shell candidate selects the precomputed graph");
    let precomputed = variants
        .precomputed_backdrop
        .as_ref()
        .expect("shell candidate selects the shared backdrop");
    let repaint = diagnostic_repaint_plan_for_repairs_in_size(&[], true, (320, 240));

    let exact_demand = plan_effect_execution_demand(exact_graph, &full_damage, true);
    let exact_selection = effects::select_effect_execution(exact_graph, &exact_demand);
    effects::execute_effect_graph(
        &mut harness.renderer,
        exact_graph,
        OutputFramebufferOrigin::BottomLeft,
        &repaint,
        &exact_demand,
        &exact_selection,
    )
    .expect("exact graph executes in real GLES");
    let exact_pixels = read_diagnostic_pixels(&harness);
    let optimized_pixels = precomputed_variant_pixels(
        &mut harness,
        precomputed,
        optimized_graph,
        &full_damage,
        &repaint,
        output_bounds,
    );

    for rect in [a_bounds, b_bounds] {
        for y in rect.y..rect.y + rect.height as i32 {
            for x in rect.x..rect.x + rect.width as i32 {
                // Readback rows are bottom-up.
                let row = 239 - y as usize;
                let offset = (row * 320 + x as usize) * 4;
                for channel in 0..4 {
                    let difference =
                        exact_pixels[offset + channel].abs_diff(optimized_pixels[offset + channel]);
                    assert!(
                        difference <= PIXEL_TOLERANCE,
                        "pixel differs by {difference} at ({x}, {y}) channel {channel}"
                    );
                }
            }
        }
    }
}

#[test]
fn native_faithful_full_kawase_dock_control() {
    let fixture = native_dock_fixture();
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Full,
    );
    let (_, previous, candidate, events) = render_native_stacked_candidate(fixture, config, None);
    assert!(events.iter().any(|line| {
        line.contains("event=effect_pass_execute_end")
            && line.contains("kind=SceneCapture")
            && line.contains("checkpoints=1")
            && line.contains("capture_mode=framebuffer_blit")
            && line.contains("backdrop_capture_policy=replay")
            && line.contains("kawase_execution_policy=full")
    }));
    let full_current_reference = render_native_stacked_full_reference(fixture, config);
    let (outside, inside) = diagnostic_matrix_mismatch_counts_for_origin(
        &candidate,
        &previous,
        &full_current_reference,
        fixture.output_size.0,
        fixture.output_size.1,
        &[fixture.repair],
        2,
        OutputFramebufferOrigin::TopLeftScanout,
    );
    assert_eq!(
        inside, 0,
        "Full Kawase Dock candidate differs from reference"
    );
    assert_eq!(
        outside, 0,
        "Full Kawase Dock candidate changed outside repair"
    );

    let (_, _, poison_a, _) =
        render_native_stacked_candidate(fixture, config, Some([1.0, 0.0, 1.0, 1.0]));
    let (_, _, poison_b, _) =
        render_native_stacked_candidate(fixture, config, Some([0.0, 1.0, 0.0, 1.0]));
    assert_native_poison_independence(
        fixture,
        &poison_a,
        &poison_b,
        &full_current_reference,
        "Full Kawase Dock",
    );
}

#[test]
fn native_faithful_stacked_replay_baseline_matches_suffix_demand() {
    assert_native_baseline_matches_suffix_demand(native_dock_fixture(), "native Dock");
}

#[test]
fn replay_partial_whole_graph_with_linear_post_blur_sampling_is_poison_independent() {
    let output_bounds = EffectRect::new(0, 0, 512, 384).expect("diagnostic output bounds");
    let target_rect = EffectRect::new(145, 148, 321, 181).expect("diagnostic target bounds");
    let repair = OutputRect::new(292, 220, 5, 3);
    let visual_group = VisualGroupId::new(9).expect("diagnostic visual group");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );

    let render_candidate = |poison: [f32; 4]| {
        let mut candidate = GlesEffectTestHarness::new(512, 384);
        install_diagnostic_scene(&mut candidate, target_rect, visual_group);
        install_custom_linear_neighbor_shader(&mut candidate);
        let graph = diagnostic_graph_with_linear_neighbor_stage(
            target_rect,
            visual_group,
            &EffectRegion::empty(),
            output_bounds,
        );
        execute_diagnostic_frame(
            &mut candidate,
            &graph,
            &diagnostic_repaint_plan_for_repairs_in_size(&[repair], true, (512, 384)),
            EffectRegion::from_rect(output_bounds),
            true,
            config,
        );
        let previous = read_diagnostic_pixels(&candidate);
        assert!(
            candidate
                .renderer
                .effect_runtime
                .effect_resources
                .poison_cached_textures(&candidate.gl, poison)
                > 0,
            "custom diagnostic candidate must reuse a pooled texture"
        );
        execute_diagnostic_frame(
            &mut candidate,
            &graph,
            &diagnostic_repaint_plan_for_repairs_in_size(&[repair], false, (512, 384)),
            diagnostic_region(repair),
            false,
            config,
        );
        (previous, read_diagnostic_pixels(&candidate))
    };

    let (previous_a, actual_a) = render_candidate([1.0, 0.0, 1.0, 1.0]);
    let (previous_b, actual_b) = render_candidate([0.0, 1.0, 0.0, 1.0]);

    let mut reference = GlesEffectTestHarness::new(512, 384);
    install_diagnostic_scene(&mut reference, target_rect, visual_group);
    install_custom_linear_neighbor_shader(&mut reference);
    let reference_graph = diagnostic_graph_with_linear_neighbor_stage(
        target_rect,
        visual_group,
        &EffectRegion::from_rect(output_bounds),
        output_bounds,
    );
    execute_diagnostic_frame(
        &mut reference,
        &reference_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[repair], true, (512, 384)),
        EffectRegion::from_rect(output_bounds),
        true,
        config,
    );
    let full_reference = read_diagnostic_pixels(&reference);
    let (outside_a, inside_a) = diagnostic_matrix_mismatch_counts(
        &actual_a,
        &previous_a,
        &full_reference,
        512,
        384,
        &[repair],
        2,
    );
    let (outside_b, inside_b) = diagnostic_matrix_mismatch_counts(
        &actual_b,
        &previous_b,
        &full_reference,
        512,
        384,
        &[repair],
        2,
    );
    assert_eq!(
        outside_a, 0,
        "linear post-blur poison A changed outside repair"
    );
    assert_eq!(
        inside_a, 0,
        "linear post-blur poison A differs from reference"
    );
    assert_eq!(
        outside_b, 0,
        "linear post-blur poison B changed outside repair"
    );
    assert_eq!(
        inside_b, 0,
        "linear post-blur poison B differs from reference"
    );
    assert_eq!(
        actual_a, actual_b,
        "linear post-blur output depends on poison"
    );
}

#[test]
fn replay_partial_poison_sweep_finds_no_uninitialized_capture_samples() {
    let cases = [
        (
            (128, 96),
            EffectRect::new(24, 20, 80, 56).unwrap(),
            OutputRect::new(60, 44, 4, 4),
            1,
            1.0,
        ),
        (
            (256, 192),
            EffectRect::new(97, 61, 121, 87).unwrap(),
            OutputRect::new(151, 103, 5, 3),
            2,
            0.5,
        ),
        (
            (512, 384),
            EffectRect::new(145, 148, 321, 181).unwrap(),
            OutputRect::new(292, 220, 5, 3),
            3,
            0.75,
        ),
        (
            (1920, 1080),
            EffectRect::new(145, 148, 1112, 873).unwrap(),
            OutputRect::new(647, 901, 12, 10),
            2,
            0.5,
        ),
        (
            (512, 384),
            EffectRect::new(0, 0, 512, 384).unwrap(),
            OutputRect::new(257, 193, 3, 3),
            4,
            0.5,
        ),
        (
            (513, 385),
            EffectRect::new(17, 19, 479, 347).unwrap(),
            OutputRect::new(256, 192, 3, 3),
            4,
            0.0625,
        ),
        (
            (511, 383),
            EffectRect::new(3, 5, 503, 371).unwrap(),
            OutputRect::new(253, 191, 5, 3),
            3,
            0.9375,
        ),
    ];
    let visual_group = VisualGroupId::new(9).expect("diagnostic visual group");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );

    for (output_size, target_rect, repair, passes, scale) in cases {
        let output_bounds =
            EffectRect::new(0, 0, output_size.0, output_size.1).expect("diagnostic output bounds");
        let mut reference = GlesEffectTestHarness::new(output_size.0, output_size.1);
        install_diagnostic_scene(&mut reference, target_rect, visual_group);
        let reference_graph = diagnostic_graph_with_spec(
            target_rect,
            visual_group,
            &EffectRegion::empty(),
            output_bounds,
            4.0,
            passes,
            scale,
        );
        execute_diagnostic_frame(
            &mut reference,
            &reference_graph,
            &diagnostic_repaint_plan_for_repairs_in_size(&[repair], true, output_size),
            EffectRegion::from_rect(output_bounds),
            true,
            config,
        );
        let full_reference = read_diagnostic_pixels(&reference);
        drop(reference);
        let mut outputs = Vec::new();
        for poison in [[1.0, 0.0, 1.0, 1.0], [0.0, 1.0, 1.0, 1.0]] {
            let mut candidate = GlesEffectTestHarness::new(output_size.0, output_size.1);
            install_diagnostic_scene(&mut candidate, target_rect, visual_group);
            let graph = diagnostic_graph_with_spec(
                target_rect,
                visual_group,
                &EffectRegion::empty(),
                output_bounds,
                4.0,
                passes,
                scale,
            );
            execute_diagnostic_frame(
                &mut candidate,
                &graph,
                &diagnostic_repaint_plan_for_repairs_in_size(&[repair], true, output_size),
                EffectRegion::from_rect(output_bounds),
                true,
                config,
            );
            let previous = read_diagnostic_pixels(&candidate);
            assert!(
                candidate
                    .renderer
                    .effect_runtime
                    .effect_resources
                    .poison_cached_textures(&candidate.gl, poison)
                    > 0,
                "poison sweep candidate must reuse a pooled texture"
            );
            execute_diagnostic_frame(
                &mut candidate,
                &graph,
                &diagnostic_repaint_plan_for_repairs_in_size(&[repair], false, output_size),
                diagnostic_region(repair),
                false,
                config,
            );
            let actual = read_diagnostic_pixels(&candidate);
            let (outside, inside) = diagnostic_matrix_mismatch_counts(
                &actual,
                &previous,
                &full_reference,
                output_size.0,
                output_size.1,
                &[repair],
                2,
            );
            assert_eq!(
                outside, 0,
                "poison sweep changed outside repair for size={output_size:?} target={target_rect:?} passes={passes} scale={scale}"
            );
            assert_eq!(
                inside, 0,
                "poison sweep changed repair for size={output_size:?} target={target_rect:?} passes={passes} scale={scale}"
            );
            outputs.push(actual);
        }
        assert_eq!(
            outputs[0], outputs[1],
            "poison sweep output depends on pooled contents for size={output_size:?} target={target_rect:?} passes={passes} scale={scale}"
        );
    }
}

#[test]
fn replay_partial_fragmented_capture_materialization_is_a_non_regression() {
    let output_bounds = EffectRect::new(0, 0, 512, 384).expect("diagnostic output bounds");
    let target_rect = EffectRect::new(180, 130, 100, 80).expect("diagnostic target bounds");
    let repairs = [
        OutputRect::new(208, 150, 4, 4),
        OutputRect::new(248, 190, 4, 4),
    ];
    let visual_group = VisualGroupId::new(9).expect("diagnostic visual group");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );

    let mut reference = GlesEffectTestHarness::new(512, 384);
    install_diagnostic_scene(&mut reference, target_rect, visual_group);
    let reference_graph = diagnostic_graph(
        target_rect,
        visual_group,
        &EffectRegion::from_rect(output_bounds),
        output_bounds,
    );
    execute_diagnostic_frame(
        &mut reference,
        &reference_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, true, (512, 384)),
        EffectRegion::from_rect(output_bounds),
        true,
        config,
    );
    let full_reference = read_diagnostic_pixels(&reference);
    drop(reference);

    let mut candidate = GlesEffectTestHarness::new(512, 384);
    install_diagnostic_scene(&mut candidate, target_rect, visual_group);
    let graph = diagnostic_graph(
        target_rect,
        visual_group,
        &EffectRegion::empty(),
        output_bounds,
    );
    execute_diagnostic_frame(
        &mut candidate,
        &graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, true, (512, 384)),
        EffectRegion::from_rect(output_bounds),
        true,
        config,
    );
    let previous = read_diagnostic_pixels(&candidate);
    assert!(
        candidate
            .renderer
            .effect_runtime
            .effect_resources
            .poison_cached_textures(&candidate.gl, [1.0, 0.0, 1.0, 1.0])
            > 0,
        "fragmented materialization candidate must reuse a pooled texture"
    );
    execute_diagnostic_frame(
        &mut candidate,
        &graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, false, (512, 384)),
        diagnostic_region_for_repairs(&repairs),
        false,
        config,
    );
    let actual = read_diagnostic_pixels(&candidate);
    let (outside, inside) = diagnostic_matrix_mismatch_counts(
        &actual,
        &previous,
        &full_reference,
        512,
        384,
        &repairs,
        2,
    );
    assert_eq!(
        outside, 0,
        "fragmented replay changed pixels outside repair"
    );
    assert_eq!(inside, 0, "fragmented replay differs from full reference");
}

#[test]
fn replay_partial_translated_capture_domain_reuses_pool_without_stale_content() {
    let output_bounds = EffectRect::new(0, 0, 256, 192).expect("diagnostic output bounds");
    let rect_a = EffectRect::new(96, 72, 64, 48).expect("first blur bounds");
    // An 88 px Dual Kawase reach clips the translated domain at the right
    // edge when x=112; x=104 keeps both expanded domains the same width.
    let rect_b = EffectRect::new(104, 80, 64, 48).expect("translated blur bounds");
    let repairs = [
        OutputRect::new(rect_a.x, rect_a.y, rect_a.width, rect_a.height),
        OutputRect::new(rect_b.x, rect_b.y, rect_b.width, rect_b.height),
    ];
    let visual_group = VisualGroupId::new(9).expect("diagnostic visual group");
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );

    let mut candidate = GlesEffectTestHarness::new(256, 192);
    install_diagnostic_scene(&mut candidate, rect_a, visual_group);
    let graph_a = diagnostic_graph(rect_a, visual_group, &EffectRegion::empty(), output_bounds);
    execute_diagnostic_frame(
        &mut candidate,
        &graph_a,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, true, (256, 192)),
        EffectRegion::from_rect(output_bounds),
        true,
        config,
    );
    let previous = read_diagnostic_pixels(&candidate);

    install_diagnostic_scene(&mut candidate, rect_b, visual_group);
    let graph_b = diagnostic_graph(rect_b, visual_group, &EffectRegion::empty(), output_bounds);
    let capture_a = graph_a
        .textures
        .iter()
        .find(|texture| texture.source == GraphTextureSource::CapturedScene)
        .expect("first capture texture");
    let capture_b = graph_b
        .textures
        .iter()
        .find(|texture| texture.source == GraphTextureSource::CapturedScene)
        .expect("translated capture texture");
    assert_ne!(capture_a.domain, capture_b.domain);
    assert_eq!(
        (capture_a.width, capture_a.height),
        (capture_b.width, capture_b.height)
    );
    assert!(
        candidate
            .renderer
            .effect_runtime
            .effect_resources
            .poison_cached_textures(&candidate.gl, [0.0, 1.0, 1.0, 1.0])
            > 0,
        "translated candidate must reuse a pooled texture"
    );

    execute_diagnostic_frame(
        &mut candidate,
        &graph_b,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, false, (256, 192)),
        diagnostic_region_for_repairs(&repairs),
        false,
        config,
    );
    let actual = read_diagnostic_pixels(&candidate);
    drop(candidate);

    let mut reference = GlesEffectTestHarness::new(256, 192);
    install_diagnostic_scene(&mut reference, rect_b, visual_group);
    execute_diagnostic_frame(
        &mut reference,
        &graph_b,
        &diagnostic_repaint_plan_for_repairs_in_size(&repairs, true, (256, 192)),
        EffectRegion::from_rect(output_bounds),
        true,
        config,
    );
    let full_reference = read_diagnostic_pixels(&reference);
    let (outside, inside) = diagnostic_matrix_mismatch_counts(
        &actual,
        &previous,
        &full_reference,
        256,
        192,
        &repairs,
        2,
    );
    assert_eq!(
        outside, 0,
        "translated replay changed pixels outside repair"
    );
    assert_eq!(inside, 0, "translated replay differs from full reference");
}

#[test]
fn moving_blur_domain_reuses_real_gles_resources() {
    let mut harness = GlesEffectTestHarness::new(256, 192);
    let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let repaint_plan = RepaintPlan {
        render_damage: OutputDamage::Full,
        repair_damage: OutputDamage::Full,
        buffer_age: None,
        mode: RepaintMode::Full,
        fallback_reason: None,
        ..RepaintPlan::default()
    };
    let mut dimensions_by_domain_size = std::collections::HashMap::new();
    let mut first_allocation_count = None;
    let mut warm_checkpoint_entries = None;
    let mut warm_checkpoint_bytes_max = 0;
    let mut warm_pool_bytes_max = 0;
    let mut positions = Vec::with_capacity(512);
    for step in 0..512 {
        let phase = step % 16;
        let (x, y) = match phase {
            0 => (32, 32),
            1 => (33, 33),
            2 => (184, 32),
            3 => (183, 31),
            4 => (184, 144),
            5 => (183, 143),
            6 => (32, 144),
            7 => (33, 143),
            8 => (112, 80),
            9 => (113, 81),
            10 => (112, 80),
            11 => (32, 32),
            12 => (224, 168),
            13 => (0, 168),
            14 => (224, 0),
            _ => (0, 0),
        };
        positions.push((x, y));
    }

    for (step, (x, y)) in positions.into_iter().enumerate() {
        let rect = EffectRect::new(x, y, 32, 24).expect("moving blur rectangle");
        let (scene, registry) = moving_blur_scene(rect);
        let plan = oblivion_one::effects::compile_frame_execution_plan(
            &scene,
            &full_damage,
            output_bounds,
            &registry,
        )
        .expect("moving blur graph compiles");
        let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("moving blur must compile to an effect graph");
        };
        assert_eq!(
            graph
                .textures
                .iter()
                .filter(|texture| texture.source == GraphTextureSource::CapturedScene)
                .count(),
            1,
            "moving blur uses one persistent scene capture"
        );
        let demand = plan_effect_execution_demand(&graph, &full_damage, true);
        let selection = effects::select_effect_execution(&graph, &demand);

        effects::execute_effect_graph(
            &mut harness.renderer,
            &graph,
            OutputFramebufferOrigin::BottomLeft,
            &repaint_plan,
            &demand,
            &selection,
        )
        .expect("moving blur graph executes in real GLES");

        let mut first_gl_error = None;
        loop {
            let error = unsafe { harness.gl.get_error() };
            if error == glow::NO_ERROR {
                break;
            }
            first_gl_error.get_or_insert(error);
        }
        assert_eq!(first_gl_error, None, "step {step} left a GLES error");

        // Dual Kawase reach for radius 4, 2 passes, scale 1 is
        // 3*3*(4+5) + 2 + 2 + 3 = 88 px. Deriving it from the first capture
        // domain is wrong once that domain is clipped at the output edge.
        let (capture_radius_x, capture_radius_y) = (88, 88);
        let expected_left = (x - capture_radius_x).max(0);
        let expected_top = (y - capture_radius_y).max(0);
        let expected_right = (x + 32 + capture_radius_x).min(256);
        let expected_bottom = (y + 24 + capture_radius_y).min(192);
        let expected_capture = EffectRect::new(
            expected_left,
            expected_top,
            u32::try_from(expected_right - expected_left).expect("capture width"),
            u32::try_from(expected_bottom - expected_top).expect("capture height"),
        )
        .expect("expected capture domain");
        // The domain then snaps outward to the coarsest level's 4 px lattice.
        let expected_capture = oblivion_one::effects::align_domain_to_output_lattice(
            expected_capture,
            4,
            output_bounds,
        );
        for texture in &graph.textures {
            assert!(texture.width > 0 && texture.height > 0);
            if texture.source != GraphTextureSource::Output {
                assert_eq!(texture.domain, expected_capture, "step {step}");
            }
        }
        let dimensions = graph
            .textures
            .iter()
            .filter(|texture| texture.source != GraphTextureSource::Output)
            .map(|texture| (texture.width, texture.height))
            .collect::<Vec<_>>();
        let interior = expected_left == x - capture_radius_x
            && expected_top == y - capture_radius_y
            && expected_right == x + 32 + capture_radius_x
            && expected_bottom == y + 24 + capture_radius_y;
        if interior {
            // Translation alone never changes texture dimensions for a given
            // lattice-aligned domain size.
            let key = (expected_capture.width, expected_capture.height);
            if let Some(first) = dimensions_by_domain_size.get(&key) {
                assert_eq!(first, &dimensions, "translation changed texture dimensions");
            } else {
                dimensions_by_domain_size.insert(key, dimensions);
            }
        }

        let effect_resources = &harness.renderer.effect_runtime.effect_resources;
        let metrics = effect_resources.metrics();
        let (checkpoint_entries, checkpoint_bytes) = effect_resources.checkpoint_cache_stats();
        assert_eq!(
            checkpoint_entries, 1,
            "step {step}: one capture stays cached"
        );
        assert_eq!(
            metrics.checked_out_texture_count, checkpoint_entries,
            "step {step}: only persistent checkpoint owners remain checked out"
        );
        assert!(metrics.current_bytes <= metrics.budget_bytes, "step {step}");
        if step < 16 {
            warm_checkpoint_bytes_max = warm_checkpoint_bytes_max.max(checkpoint_bytes);
            warm_pool_bytes_max = warm_pool_bytes_max.max(metrics.current_bytes);
        }
        if step == 0 || step == 15 {
            eprintln!(
                "moving blur resources step {step}: checked_out={} checkpoint_entries={} checkpoint_bytes={} allocation_count={} current_bytes={}",
                metrics.checked_out_texture_count,
                checkpoint_entries,
                checkpoint_bytes,
                metrics.allocation_count,
                metrics.current_bytes,
            );
        }
        if step == 15 {
            first_allocation_count = Some(metrics.allocation_count);
            warm_checkpoint_entries = Some(checkpoint_entries);
        }
        if step >= 16 {
            assert_eq!(
                checkpoint_entries,
                warm_checkpoint_entries.expect("warm checkpoint entry count"),
                "step {step} grew the checkpoint cache"
            );
            assert!(
                checkpoint_bytes <= warm_checkpoint_bytes_max,
                "step {step} grew beyond the warm checkpoint-cache size bound"
            );
            assert!(
                metrics.current_bytes <= warm_pool_bytes_max,
                "step {step} grew beyond the warm resource-pool byte bound"
            );
            assert_eq!(
                metrics.allocation_count,
                first_allocation_count.expect("warm allocation count"),
                "step {step} allocated again"
            );
        }
        assert_eq!(metrics.eviction_count, 0, "translation evicted a resource");
        // Clipped domains at the output edges produce more distinct texture
        // sizes than the old 24 px reach, and lattice snapping adds edge
        // variants; the pool saturates at 44.
        assert!(
            metrics.cached_texture_count <= 44,
            "cached={}",
            metrics.cached_texture_count
        );
    }

    let effect_resources = &mut harness.renderer.effect_runtime.effect_resources;
    let metrics = effect_resources.metrics();
    let (checkpoint_entries, checkpoint_bytes) = effect_resources.checkpoint_cache_stats();
    assert_eq!(checkpoint_entries, 1);
    assert_eq!(metrics.checked_out_texture_count, checkpoint_entries);
    assert!(
        metrics.reuse_count > 0,
        "moving blur did not reuse pooled textures"
    );
    eprintln!(
        "moving blur resources before retirement: checked_out={} checkpoint_entries={} checkpoint_bytes={} allocation_count={} current_bytes={}",
        metrics.checked_out_texture_count,
        checkpoint_entries,
        checkpoint_bytes,
        metrics.allocation_count,
        metrics.current_bytes,
    );

    effect_resources.clear_checkpoint_capture_cache();
    let metrics = effect_resources.metrics();
    assert_eq!(effect_resources.checkpoint_cache_stats(), (0, 0));
    assert_eq!(metrics.checked_out_texture_count, 0);
    eprintln!(
        "moving blur resources after retirement: checked_out={} checkpoint_entries=0 checkpoint_bytes=0 allocation_count={} current_bytes={}",
        metrics.checked_out_texture_count, metrics.allocation_count, metrics.current_bytes,
    );
}

#[test]
fn moving_visual_group_blur_replays_only_background_below_target() {
    let mut harness = GlesEffectTestHarness::new(256, 192);
    const BACKGROUND_COLOR: u32 = 0xff20_4060;
    harness
        .renderer
        .resources
        .test_create_surface_texture(&harness.gl, 42, 32, 24, None)
        .expect("target scene texture creates");
    harness
        .renderer
        .resources
        .test_create_decoration_texture(
            &harness.gl,
            EglDrawLayer::SolidRgba(BACKGROUND_COLOR),
            1,
            1,
            None,
        )
        .expect("background scene texture creates");

    let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let repaint_plan = RepaintPlan {
        render_damage: OutputDamage::Full,
        repair_damage: OutputDamage::Full,
        buffer_age: None,
        mode: RepaintMode::Full,
        fallback_reason: None,
        ..RepaintPlan::default()
    };
    let visual_group = VisualGroupId::new(9).expect("visual group id");
    let (_, registry) = moving_blur_scene(output_bounds);
    let positions = [
        (32, 32),
        (33, 33),
        (184, 32),
        (183, 31),
        (224, 144),
        (240, 168),
        (240, 180),
        (0, 168),
        (-8, 180),
        (-8, -4),
        (0, 0),
        (112, 80),
        (113, 81),
        (112, 80),
        (32, 32),
        (184, 144),
    ];
    let mut dimensions_by_domain_size = std::collections::HashMap::new();
    let mut warm_allocation_count = None;
    let mut warm_checkpoint_entries = None;
    let mut warm_checkpoint_bytes_max = 0;
    let mut warm_pool_bytes_max = 0;

    for (step, (x, y)) in positions.iter().copied().cycle().take(512).enumerate() {
        let rect = EffectRect::new(x, y, 32, 24).expect("moving target rectangle");
        install_visual_group_scene_commands(&mut harness.renderer, rect, visual_group);
        let scene = moving_visual_group_blur_scene(rect, visual_group);
        let plan = oblivion_one::effects::compile_frame_execution_plan(
            &scene,
            &full_damage,
            output_bounds,
            &registry,
        )
        .expect("visual-group blur graph compiles");
        let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
            panic!("visual-group blur must compile to an effect graph");
        };
        assert_eq!(
            graph
                .textures
                .iter()
                .filter(|texture| texture.source == GraphTextureSource::CapturedScene)
                .count(),
            1,
            "visual-group blur uses one persistent scene capture"
        );
        let capture_pass = graph
            .passes
            .iter()
            .find(|pass| pass.kind == oblivion_one::effects::RenderPassKind::SceneCapture)
            .expect("visual-group blur capture pass");
        assert_eq!(
            capture_pass.anchor,
            oblivion_one::compositor::EffectAnchor::BeforeSurface(42)
        );
        assert_eq!(capture_pass.visual_group, Some(visual_group));
        assert_eq!(
            capture_pass.anchor_scope,
            oblivion_one::compositor::EffectAnchorScope::VisualGroup
        );
        assert!(capture_pass.checkpoint_dependencies.is_empty());

        let demand = plan_effect_execution_demand(&graph, &full_damage, true);
        let selection = effects::select_effect_execution(&graph, &demand);
        let before_replays = harness.renderer.last_frame_stats().draw_command_replays;
        effects::execute_effect_graph(
            &mut harness.renderer,
            &graph,
            OutputFramebufferOrigin::BottomLeft,
            &repaint_plan,
            &demand,
            &selection,
        )
        .expect("visual-group blur graph executes in real GLES");
        let after_replays = harness.renderer.last_frame_stats().draw_command_replays;
        let expected_replays = match effects::effect_debug_config().capture_mode() {
            effects::EffectDebugCaptureMode::Replay => 3,
            effects::EffectDebugCaptureMode::Framebuffer => 2,
        };
        assert_eq!(
            after_replays.saturating_sub(before_replays),
            expected_replays,
            "step {step}: capture policy {:?} must keep scene ordering without replaying the framebuffer capture",
            effects::effect_debug_config().capture_mode(),
        );

        let mut first_gl_error = None;
        loop {
            let error = unsafe { harness.gl.get_error() };
            if error == glow::NO_ERROR {
                break;
            }
            first_gl_error.get_or_insert(error);
        }
        assert_eq!(first_gl_error, None, "step {step} left a GLES error");

        let capture = graph
            .textures
            .iter()
            .find(|texture| texture.source == GraphTextureSource::CapturedScene)
            .expect("visual-group blur capture texture");
        assert!(capture.domain.width > 0 && capture.domain.height > 0);
        // Dual Kawase reach for radius 4, 2 passes, scale 1 is
        // 3*3*(4+5) + 2 + 2 + 3 = 88 px. Deriving it from the first capture
        // domain is wrong once that domain is clipped at the output edge.
        let (capture_radius_x, capture_radius_y) = (88, 88);
        let expected_left = (x - capture_radius_x).max(0);
        let expected_top = (y - capture_radius_y).max(0);
        let expected_right = (x + 32 + capture_radius_x).min(256);
        let expected_bottom = (y + 24 + capture_radius_y).min(192);
        let expected_capture = EffectRect::new(
            expected_left,
            expected_top,
            u32::try_from(expected_right - expected_left).expect("capture width"),
            u32::try_from(expected_bottom - expected_top).expect("capture height"),
        )
        .expect("expected capture domain");
        // The domain then snaps outward to the coarsest level's 4 px lattice.
        let expected_capture = oblivion_one::effects::align_domain_to_output_lattice(
            expected_capture,
            4,
            output_bounds,
        );
        assert_eq!(capture.domain, expected_capture, "step {step}");
        let dimensions = graph
            .textures
            .iter()
            .filter(|texture| texture.source != GraphTextureSource::Output)
            .map(|texture| (texture.width, texture.height))
            .collect::<Vec<_>>();
        let interior = expected_left == x - capture_radius_x
            && expected_top == y - capture_radius_y
            && expected_right == x + 32 + capture_radius_x
            && expected_bottom == y + 24 + capture_radius_y;
        if interior {
            // Translation alone never changes texture dimensions for a given
            // lattice-aligned domain size.
            let key = (expected_capture.width, expected_capture.height);
            if let Some(first) = dimensions_by_domain_size.get(&key) {
                assert_eq!(first, &dimensions, "translation changed texture dimensions");
            } else {
                dimensions_by_domain_size.insert(key, dimensions);
            }
        }

        let effect_resources = &harness.renderer.effect_runtime.effect_resources;
        let metrics = effect_resources.metrics();
        let (checkpoint_entries, checkpoint_bytes) = effect_resources.checkpoint_cache_stats();
        assert_eq!(
            checkpoint_entries, 1,
            "step {step}: one capture stays cached"
        );
        assert_eq!(
            metrics.checked_out_texture_count, checkpoint_entries,
            "step {step}: only persistent checkpoint owners remain checked out"
        );
        assert!(metrics.current_bytes <= metrics.budget_bytes, "step {step}");
        if step < positions.len() {
            warm_checkpoint_bytes_max = warm_checkpoint_bytes_max.max(checkpoint_bytes);
            warm_pool_bytes_max = warm_pool_bytes_max.max(metrics.current_bytes);
        }
        if step == 0 || step == positions.len() - 1 {
            eprintln!(
                "visual-group blur resources step {step}: checked_out={} checkpoint_entries={} checkpoint_bytes={} allocation_count={} current_bytes={}",
                metrics.checked_out_texture_count,
                checkpoint_entries,
                checkpoint_bytes,
                metrics.allocation_count,
                metrics.current_bytes,
            );
        }
        if step == positions.len() - 1 {
            warm_allocation_count = Some(metrics.allocation_count);
            warm_checkpoint_entries = Some(checkpoint_entries);
        }
        if step >= positions.len() {
            assert_eq!(
                checkpoint_entries,
                warm_checkpoint_entries.expect("warm checkpoint entry count"),
                "step {step} grew the checkpoint cache"
            );
            assert!(
                checkpoint_bytes <= warm_checkpoint_bytes_max,
                "step {step} grew beyond the warm checkpoint-cache size bound"
            );
            assert!(
                metrics.current_bytes <= warm_pool_bytes_max,
                "step {step} grew beyond the warm resource-pool byte bound"
            );
            assert_eq!(
                metrics.allocation_count,
                warm_allocation_count.expect("warm allocation count"),
                "step {step} allocated again"
            );
        }
        assert_eq!(metrics.eviction_count, 0, "translation evicted a resource");
        // Steady state is 48 cached textures with the 88 px Dual Kawase reach.
        // Lattice-snapped edge domains add size variants; the pool saturates at 52.
        assert!(
            metrics.cached_texture_count <= 52,
            "cached={}",
            metrics.cached_texture_count
        );
    }

    let effect_resources = &mut harness.renderer.effect_runtime.effect_resources;
    let metrics = effect_resources.metrics();
    let (checkpoint_entries, checkpoint_bytes) = effect_resources.checkpoint_cache_stats();
    assert_eq!(checkpoint_entries, 1);
    assert_eq!(metrics.checked_out_texture_count, checkpoint_entries);
    assert!(
        metrics.reuse_count > 0,
        "visual-group blur did not reuse pooled textures"
    );
    eprintln!(
        "visual-group blur resources before retirement: checked_out={} checkpoint_entries={} checkpoint_bytes={} allocation_count={} current_bytes={}",
        metrics.checked_out_texture_count,
        checkpoint_entries,
        checkpoint_bytes,
        metrics.allocation_count,
        metrics.current_bytes,
    );

    effect_resources.clear_checkpoint_capture_cache();
    let metrics = effect_resources.metrics();
    assert_eq!(effect_resources.checkpoint_cache_stats(), (0, 0));
    assert_eq!(metrics.checked_out_texture_count, 0);
    eprintln!(
        "visual-group blur resources after retirement: checked_out={} checkpoint_entries=0 checkpoint_bytes=0 allocation_count={} current_bytes={}",
        metrics.checked_out_texture_count, metrics.allocation_count, metrics.current_bytes,
    );
}

#[test]
fn effect_trace_bounds_visual_group_scene_and_final_replay() {
    let mut harness = GlesEffectTestHarness::new(256, 192);
    harness
        .renderer
        .resources
        .test_create_surface_texture(&harness.gl, 42, 32, 24, None)
        .expect("target scene texture creates");
    harness
        .renderer
        .resources
        .test_create_decoration_texture(
            &harness.gl,
            EglDrawLayer::SolidRgba(0xff20_4060),
            1,
            1,
            None,
        )
        .expect("background scene texture creates");
    harness.renderer.effect_runtime.effect_trace =
        effects::EffectExecutionTrace::enabled_for_test();
    effects::clear_effect_trace_test_events();

    let rect = EffectRect::new(32, 32, 32, 24).expect("visual group blur rectangle");
    let visual_group = VisualGroupId::new(9).expect("visual group id");
    install_visual_group_scene_commands(&mut harness.renderer, rect, visual_group);
    let (mut scene, registry) = moving_blur_scene(rect);
    let instance = scene.instances.first_mut().expect("moving blur instance");
    instance.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);
    instance.visual_group = Some(visual_group);
    instance.anchor_scope = oblivion_one::compositor::EffectAnchorScope::VisualGroup;
    instance.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(instance.anchor);
    let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let repaint_plan = RepaintPlan {
        render_damage: OutputDamage::Full,
        repair_damage: OutputDamage::Full,
        buffer_age: None,
        mode: RepaintMode::Full,
        fallback_reason: None,
        ..RepaintPlan::default()
    };
    let plan = oblivion_one::effects::compile_frame_execution_plan(
        &scene,
        &full_damage,
        output_bounds,
        &registry,
    )
    .expect("visual group blur graph compiles");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
        panic!("visual group blur must compile to an effect graph");
    };
    let demand = plan_effect_execution_demand(&graph, &full_damage, true);
    let selection = effects::select_effect_execution(&graph, &demand);

    effects::execute_effect_graph(
        &mut harness.renderer,
        &graph,
        OutputFramebufferOrigin::BottomLeft,
        &repaint_plan,
        &demand,
        &selection,
    )
    .expect("visual group blur graph executes in real GLES");
    let events = effects::take_effect_trace_test_events();

    let capture_execute_end = trace_event_index(
        &events,
        &["event=effect_pass_execute_end", "kind=SceneCapture"],
    );
    let expected_capture_mode = match effects::effect_debug_config().capture_mode() {
        effects::EffectDebugCaptureMode::Replay => "replay",
        effects::EffectDebugCaptureMode::Framebuffer => "framebuffer_blit",
    };
    let capture_execute_end_event = &events[capture_execute_end];
    assert!(capture_execute_end_event.contains(&format!("capture_mode={expected_capture_mode}")));
    assert!(capture_execute_end_event.contains(&format!(
        "backdrop_capture_policy={}",
        effects::effect_debug_config().capture_mode().as_str(),
    )));
    assert!(capture_execute_end_event.contains(&format!(
        "kawase_execution_policy={}",
        effects::effect_debug_config().kawase_mode().as_str(),
    )));
    let composite_resources_end = trace_event_index(
        &events,
        &["event=effect_pass_resources_end", "kind=Composite"],
    );
    let replay_begin = trace_event_index(
        &events,
        &[
            "event=effect_scene_replay_begin",
            "kind=Composite",
            "reason=composite_advance",
        ],
    );
    let replay_end = trace_event_index(
        &events,
        &[
            "event=effect_scene_replay_end",
            "kind=Composite",
            "reason=composite_advance",
        ],
    );
    let composite_validate_begin = trace_event_index(
        &events,
        &["event=effect_pass_validate_begin", "kind=Composite"],
    );
    let composite_end = trace_event_index(&events, &["event=effect_pass_end", "kind=Composite"]);
    let final_replay_begin = trace_event_index(&events, &["event=effect_final_scene_replay_begin"]);
    let final_replay_end = trace_event_index(&events, &["event=effect_final_scene_replay_end"]);
    let overlay_begin = trace_event_index(&events, &["event=effect_overlay_draw_begin"]);
    let overlay_end = trace_event_index(&events, &["event=effect_overlay_draw_end"]);
    let graph_execute_end = trace_event_index(&events, &["event=effect_graph_execute_end"]);

    let framebuffer_capture_advance = if matches!(
        effects::effect_debug_config().capture_mode(),
        effects::EffectDebugCaptureMode::Framebuffer
    ) {
        let begin = trace_event_index(
            &events,
            &[
                "event=effect_scene_replay_begin",
                "kind=SceneCapture",
                "reason=framebuffer_capture",
            ],
        );
        let end = trace_event_index(
            &events,
            &[
                "event=effect_scene_replay_end",
                "kind=SceneCapture",
                "reason=framebuffer_capture",
            ],
        );
        assert!(begin < end);
        assert!(end < capture_execute_end);
        Some((begin, end))
    } else {
        None
    };

    assert!(capture_execute_end < composite_resources_end);
    assert!(composite_resources_end < replay_begin);
    assert!(replay_begin < replay_end);
    assert!(replay_end < composite_validate_begin);
    assert!(composite_validate_begin < composite_end);
    assert!(composite_end < final_replay_begin);
    assert!(final_replay_begin < final_replay_end);
    assert!(final_replay_end < overlay_begin);
    assert!(overlay_begin < overlay_end);
    assert!(overlay_end < graph_execute_end);
    let expected_composite_cursor = if framebuffer_capture_advance.is_some() {
        (
            "scene_cursor_start=1",
            "scene_cursor_end=1",
            "command_count=0",
        )
    } else {
        (
            "scene_cursor_start=0",
            "scene_cursor_end=1",
            "command_count=1",
        )
    };
    assert!(events[replay_begin].contains(expected_composite_cursor.0));
    assert!(events[replay_begin].contains(expected_composite_cursor.1));
    assert!(events[replay_begin].contains(expected_composite_cursor.2));
    assert!(events[final_replay_begin].contains("scene_cursor_start=1"));
    assert!(events[final_replay_begin].contains("scene_cursor_end=2"));
    assert!(events[final_replay_begin].contains("command_count=1"));
}

#[test]
fn effect_trace_bounds_checkpoint_scene_advancement() {
    let mut harness = GlesEffectTestHarness::new(256, 192);
    harness.renderer.effect_runtime.effect_trace =
        effects::EffectExecutionTrace::enabled_for_test();
    effects::clear_effect_trace_test_events();
    for surface_id in [10, 20] {
        harness
            .renderer
            .resources
            .test_create_surface_texture(&harness.gl, surface_id, 1, 1, None)
            .expect("checkpoint scene texture creates");
    }
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::SolidRgba(0xff20_4060),
        EglRect::new(0.0, 0.0, 256.0, 192.0),
        256,
        192,
        OutputFramebufferOrigin::BottomLeft,
    );
    for (surface_id, y) in [(10, 0.0), (20, 96.0)] {
        push_draw_command(
            &mut harness.renderer.scene_state.vertices,
            &mut harness.renderer.scene_state.commands,
            EglDrawLayer::Surface(surface_id),
            EglRect::new(0.0, y, 256.0, 96.0),
            256,
            192,
            OutputFramebufferOrigin::BottomLeft,
        );
    }
    harness.renderer.scene_state.scene_geometry_dirty = true;

    let rect = EffectRect::new(32, 32, 64, 48).expect("checkpoint blur rectangle");
    let (scene, registry) = moving_blur_scene(rect);
    let mut first = scene.instances[0].clone();
    first.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(10);
    first.anchor_scope = oblivion_one::compositor::EffectAnchorScope::Surface;
    first.visual_group = None;
    first.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(first.anchor);
    let mut second = first.clone();
    second.id = oblivion_one::effects::EffectInstanceId::new(2).expect("second instance id");
    second.signature = second.signature.saturating_add(1);
    second.anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(20);
    second.scene_order = oblivion_one::compositor::EffectSceneOrder::for_anchor(second.anchor);
    let scene = ResolvedEffectScene::new(1, vec![first, second]);
    let output_bounds = EffectRect::new(0, 0, 256, 192).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let repaint_plan = RepaintPlan {
        render_damage: OutputDamage::Full,
        repair_damage: OutputDamage::Full,
        buffer_age: None,
        mode: RepaintMode::Full,
        fallback_reason: None,
        ..RepaintPlan::default()
    };
    let plan = oblivion_one::effects::compile_frame_execution_plan(
        &scene,
        &full_damage,
        output_bounds,
        &registry,
    )
    .expect("checkpoint blur graph compiles");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
        panic!("checkpoint blur must compile to an effect graph");
    };
    let checkpoint_pass = graph
        .passes
        .iter()
        .find(|pass| {
            pass.kind == oblivion_one::effects::RenderPassKind::SceneCapture
                && !pass.checkpoint_dependencies.is_empty()
        })
        .expect("checkpoint-dependent capture pass");
    let checkpoint_pass_id = checkpoint_pass.id.get().to_string();
    let demand = plan_effect_execution_demand(&graph, &full_damage, true);
    let selection = effects::select_effect_execution(&graph, &demand);

    effects::execute_effect_graph(
        &mut harness.renderer,
        &graph,
        OutputFramebufferOrigin::BottomLeft,
        &repaint_plan,
        &demand,
        &selection,
    )
    .expect("checkpoint blur graph executes in real GLES");
    let events = effects::take_effect_trace_test_events();

    let replay_begin = trace_event_index(
        &events,
        &[
            "event=effect_scene_replay_begin",
            "reason=checkpoint_dependency",
        ],
    );
    let replay_end = trace_event_index(
        &events,
        &[
            "event=effect_scene_replay_end",
            "reason=checkpoint_dependency",
        ],
    );
    let resources_end = trace_event_index(
        &events,
        &[
            "event=effect_pass_resources_end",
            &format!("pass={checkpoint_pass_id}"),
        ],
    );
    let validate_begin = trace_event_index(
        &events,
        &[
            "event=effect_pass_validate_begin",
            &format!("pass={checkpoint_pass_id}"),
        ],
    );

    assert!(events[replay_begin].contains(&format!("pass={checkpoint_pass_id}")));
    assert!(events[replay_end].contains(&format!("pass={checkpoint_pass_id}")));
    assert!(resources_end < replay_begin);
    assert!(replay_begin < replay_end);
    assert!(replay_end < validate_begin);
    assert!(events[replay_begin].contains("scene_cursor_start=1"));
    assert!(events[replay_begin].contains("scene_cursor_end=2"));
    assert!(events[replay_begin].contains("command_count=1"));
}

#[test]
fn same_invocation_mrt_reports_real_up2_and_fragment_store_provenance() {
    let even_bounds = (160, 120);
    let odd_bounds = (161, 121);
    let even_rect = EffectRect::new(18, 14, 96, 72).expect("even blur domain");
    let odd_rect = EffectRect::new(19, 13, 101, 75).expect("odd blur domain");
    let cases = [
        ("even_sat_1", even_bounds, even_rect, 1.0, 81_u32),
        ("even_sat_031", even_bounds, even_rect, 0.31, 82_u32),
        ("odd_sat_031", odd_bounds, odd_rect, 0.31, 83_u32),
    ];

    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        for (case_name, output_size, rect, saturation, group_id) in cases {
            for dither in [true, false] {
                let visual_group = VisualGroupId::new(group_id).expect("visual group");
                let material = oblivion_one::material::EffectiveMaterial {
                    blur: 0.37,
                    saturation,
                    noise: 0.0,
                    blur_passes: 3,
                };
                let (scene, registry) =
                    terminal_builtin_blur_scene(rect, visual_group, material, None);
                let bounds =
                    EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
                let graph = terminal_graph_variant(&scene, &registry, bounds, false);
                let tails = terminal_tail_capture_ids(&graph);
                assert_eq!(tails.len(), 1, "the provenance graph has one tail");
                let tail = tails[0];
                let up2_pass = graph
                    .passes
                    .iter()
                    .find(|pass| pass.id == tail.upsample)
                    .expect("legacy Up2 pass exists");
                let up2_input_id = up2_pass.inputs[0];
                let up2_input_pass = graph
                    .passes
                    .iter()
                    .find(|pass| pass.output == Some(up2_input_id))
                    .expect("legacy Up2 input has a producing pass");
                let capture_passes = [up2_input_pass.id, tail.upsample, tail.fragment];
                let mut harness = GlesEffectTestHarness::new(output_size.0, output_size.1);
                install_diagnostic_scene(&mut harness, rect, visual_group);
                let plan = diagnostic_repaint_plan_for_repairs_in_size(&[], true, output_size);
                let region = EffectRegion::from_rect(bounds);
                let config = effects::EffectDebugConfig::new(
                    effects::EffectDebugCaptureMode::Replay,
                    effects::EffectDebugKawaseMode::Partial,
                );
                let replay = effects::with_test_offscreen_dither_policy(Some(dither), || {
                    execute_diagnostic_frame_with_terminal_replay(
                        &mut harness,
                        &graph,
                        &plan,
                        region,
                        true,
                        config,
                        origin,
                        effects::TerminalBoundaryReplay::Legacy,
                        capture_passes,
                        [],
                    )
                });
                println!(
                    "STORE_PROVENANCE case={case_name} origin={origin:?} dither={dither} up2_input_filter=LINEAR"
                );
                run_same_invocation_boundary_probes(
                    &harness,
                    &replay,
                    up2_input_pass.id,
                    up2_pass,
                    tail.upsample,
                    tail.fragment,
                    saturation,
                    dither,
                    case_name,
                    origin,
                );
            }
        }
    }
}

#[test]
fn noise_free_terminal_matrix_reports_explicit_offscreen_dither_policies() {
    let odd_bounds = (161, 121);
    let odd_rect = EffectRect::new(19, 13, 101, 75).expect("odd translated blur");
    let even_rect = EffectRect::new(18, 14, 96, 72).expect("even blur");
    let rounded_triangle = oblivion_one::effects::EffectCoverage {
        rounded_rect: Some(oblivion_one::effects::EffectCoverageRoundedRect {
            x: odd_rect.x as f64,
            y: odd_rect.y as f64,
            width: odd_rect.width as f64,
            height: odd_rect.height as f64,
            radius: 13.5,
        }),
        triangle: Some(oblivion_one::effects::EffectCoverageTriangle {
            a: [27.0, 23.0],
            b: [108.0, 20.0],
            c: [78.0, 79.0],
        }),
    };

    for dither in [true, false] {
        let all_failures = effects::with_test_offscreen_dither_policy(Some(dither), || {
            let mut failures = Vec::new();
            for origin in [
                OutputFramebufferOrigin::BottomLeft,
                OutputFramebufferOrigin::TopLeftScanout,
            ] {
                for (name, size, rect, group, blur, saturation, coverage) in [
                    (
                        "single_sat_1_noise_0",
                        (160, 120),
                        even_rect,
                        91,
                        0.37,
                        1.0,
                        None,
                    ),
                    (
                        "single_sat_031_noise_0",
                        (160, 120),
                        even_rect,
                        92,
                        0.37,
                        0.31,
                        None,
                    ),
                    (
                        "odd_translated_sat_031_noise_0",
                        odd_bounds,
                        odd_rect,
                        93,
                        0.42,
                        0.31,
                        None,
                    ),
                    (
                        "even_translated_sat_1_noise_0",
                        (160, 120),
                        even_rect,
                        94,
                        0.37,
                        1.0,
                        None,
                    ),
                    (
                        "rounded_triangle_coverage_noise_0",
                        odd_bounds,
                        odd_rect,
                        95,
                        0.42,
                        0.31,
                        Some(rounded_triangle.clone()),
                    ),
                ] {
                    failures.extend(run_terminal_builtin_diagnostic_case(
                        name,
                        size,
                        rect,
                        VisualGroupId::new(group).expect("visual group"),
                        blur,
                        saturation,
                        0.0,
                        coverage,
                        origin,
                    ));
                }
                failures.extend(run_terminal_causal_diagnostic_case(0.0, origin));
            }
            failures
        });
        println!(
            "OFFSCREEN_DITHER policy={} matrix_exactness_failures={} failures={all_failures:?}",
            if dither { "enabled" } else { "disabled" },
            all_failures.len()
        );
    }
}

#[test]
fn canonical_staged_blur_matches_canonical_terminal_fusion_exactly() {
    let output_size = (64, 48);
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let rect = bounds;
    let visual_group = VisualGroupId::new(126).expect("visual group");
    let (scene, registry) = terminal_builtin_blur_scene(
        rect,
        visual_group,
        oblivion_one::material::EffectiveMaterial {
            blur: 0.5,
            saturation: 1.0,
            noise: 0.0,
            blur_passes: 3,
        },
        None,
    );
    let staged_graph = terminal_graph_variant(&scene, &registry, bounds, false);
    let fused_graph = terminal_graph_variant(&scene, &registry, bounds, true);
    let tails = terminal_tail_capture_ids(&staged_graph);
    assert_eq!(
        tails.len(),
        1,
        "canonical staged graph has one terminal tail"
    );
    assert!(
        fused_graph
            .passes
            .iter()
            .any(|pass| pass.terminal_kawase.is_some()),
        "canonical fused graph retains its terminal Composite"
    );
    assert_eq!(staged_graph.stats.terminal_blur_fusions, 0);
    assert_eq!(fused_graph.stats.terminal_blur_fusions, 1);

    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        let (_, fused, staged) = render_canonical_single_tail_pair(
            &staged_graph,
            &fused_graph,
            output_size,
            origin,
            |harness| install_diagnostic_scene(harness, rect, visual_group),
            "canonical_default_material",
        );
        assert_eq!(
            staged, fused,
            "production canonical C differs for {origin:?}"
        );
    }
}

#[test]
fn canonical_staged_blur_exact_matrix_and_legacy_impact() {
    // Canonical built-in qualification is Backdrop -> Kawase -> ColorMatrix -> Noise.
    // Synthetic Tint compiler coverage is isolated in its own topology test below.
    let odd_size = (161, 121);
    let odd_rect = EffectRect::new(19, 13, 101, 75).expect("odd translated blur");
    let even_size = (160, 120);
    let even_rect = EffectRect::new(18, 14, 96, 72).expect("even blur");
    let rounded = oblivion_one::effects::EffectCoverage {
        rounded_rect: Some(oblivion_one::effects::EffectCoverageRoundedRect {
            x: odd_rect.x as f64,
            y: odd_rect.y as f64,
            width: odd_rect.width as f64,
            height: odd_rect.height as f64,
            radius: 13.5,
        }),
        triangle: None,
    };
    let triangle = oblivion_one::effects::EffectCoverage {
        rounded_rect: None,
        triangle: Some(oblivion_one::effects::EffectCoverageTriangle {
            a: [27.0, 23.0],
            b: [108.0, 20.0],
            c: [78.0, 79.0],
        }),
    };
    let default_material = oblivion_one::material::MaterialConfiguration::default()
        .effective()
        .expect("production default material resolves");
    let cases = [
        (
            "even_sat1_noise0",
            even_size,
            even_rect,
            oblivion_one::material::EffectiveMaterial {
                blur: 0.37,
                saturation: 1.0,
                noise: 0.0,
                blur_passes: 3,
            },
            None,
            false,
        ),
        (
            "even_sat031_noise0",
            even_size,
            even_rect,
            oblivion_one::material::EffectiveMaterial {
                blur: 0.37,
                saturation: 0.31,
                noise: 0.0,
                blur_passes: 3,
            },
            None,
            true,
        ),
        (
            "odd_translated_sat031_noise0",
            odd_size,
            odd_rect,
            oblivion_one::material::EffectiveMaterial {
                blur: 0.42,
                saturation: 0.31,
                noise: 0.0,
                blur_passes: 3,
            },
            None,
            true,
        ),
        (
            "odd_translated_canonical_noise",
            odd_size,
            odd_rect,
            oblivion_one::material::EffectiveMaterial {
                blur: 0.42,
                saturation: 0.31,
                noise: 0.3,
                blur_passes: 3,
            },
            None,
            true,
        ),
        (
            "even_canonical_noise",
            even_size,
            even_rect,
            oblivion_one::material::EffectiveMaterial {
                blur: 0.37,
                saturation: 0.52,
                noise: 0.3,
                blur_passes: 3,
            },
            None,
            true,
        ),
        (
            "rounded_coverage",
            odd_size,
            odd_rect,
            oblivion_one::material::EffectiveMaterial {
                blur: 0.42,
                saturation: 0.52,
                noise: 0.0,
                blur_passes: 3,
            },
            Some(rounded),
            false,
        ),
        (
            "triangle_coverage",
            odd_size,
            odd_rect,
            oblivion_one::material::EffectiveMaterial {
                blur: 0.42,
                saturation: 0.52,
                noise: 0.0,
                blur_passes: 3,
            },
            Some(triangle),
            false,
        ),
        (
            "default_material",
            even_size,
            even_rect,
            default_material,
            None,
            true,
        ),
        (
            "high_noise_stress",
            even_size,
            even_rect,
            oblivion_one::material::EffectiveMaterial {
                blur: 0.37,
                saturation: 0.52,
                noise: 0.3,
                blur_passes: 3,
            },
            None,
            true,
        ),
    ];

    for (case_index, (case_name, output_size, rect, material, coverage, report_legacy)) in
        cases.into_iter().enumerate()
    {
        let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
        let visual_group = VisualGroupId::new(140 + case_index as u32).expect("visual group");
        let (scene, registry) = terminal_builtin_blur_scene(rect, visual_group, material, coverage);
        let staged_graph = terminal_graph_variant(&scene, &registry, bounds, false);
        let fused_graph = terminal_graph_variant(&scene, &registry, bounds, true);
        assert_eq!(staged_graph.stats.terminal_blur_fusions, 0);
        assert_eq!(fused_graph.stats.terminal_blur_fusions, 1);

        for origin in [
            OutputFramebufferOrigin::BottomLeft,
            OutputFramebufferOrigin::TopLeftScanout,
        ] {
            let (canonical_staged, _, production_canonical) = render_canonical_single_tail_pair(
                &staged_graph,
                &fused_graph,
                output_size,
                origin,
                |harness| install_diagnostic_scene(harness, rect, visual_group),
                case_name,
            );

            if case_name == "even_sat1_noise0" && origin == OutputFramebufferOrigin::BottomLeft {
                let (_, dither_on_pixels) =
                    effects::with_test_offscreen_dither_policy(Some(true), || {
                        render_terminal_replay_mode(
                            &staged_graph,
                            output_size,
                            origin,
                            |harness| install_diagnostic_scene(harness, rect, visual_group),
                            effects::TerminalBoundaryReplay::CanonicalStaged,
                            [],
                            [],
                        )
                    });
                assert_eq!(
                    canonical_staged, dither_on_pixels,
                    "explicit offscreen DITHER-on changed canonical C0"
                );
            }

            if report_legacy {
                let (_, old_pixels) = render_terminal_replay_mode(
                    &staged_graph,
                    output_size,
                    origin,
                    |harness| install_diagnostic_scene(harness, rect, visual_group),
                    effects::TerminalBoundaryReplay::Legacy,
                    [],
                    [],
                );
                let impact = report_rgba8_comparison(
                    &format!("old-vs-canonical {case_name} origin={origin:?}"),
                    &old_pixels,
                    &production_canonical,
                    output_size.0,
                    output_size.1,
                    origin,
                );
                println!("    compatibility impact: {impact:?}");
            }
        }
    }
}

#[test]
fn synthetic_tint_compiles_to_a_fail_closed_terminal_topology() {
    let output_size = (160, 120);
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("Tint bounds");
    let rect = EffectRect::new(18, 14, 96, 72).expect("Tint blur region");
    let tint_group = VisualGroupId::new(160).expect("Tint visual group");
    let tint_material = oblivion_one::material::EffectiveMaterial {
        blur: 0.37,
        saturation: 0.31,
        noise: 0.3,
        blur_passes: 3,
    };
    let tint = oblivion_one::effects::TintSpec::new([0.96, 0.82, 1.0, 1.0], 0.35)
        .expect("synthetic Tint validates");
    let (scene, registry) =
        terminal_builtin_blur_scene_with_tint(rect, tint_group, tint_material, tint);
    let graph = terminal_graph_variant_with_source_damage(
        &scene,
        &registry,
        bounds,
        &EffectRegion::from_rect(bounds),
        true,
    );

    assert_eq!(graph.stats.terminal_blur_fusions, 0);
    assert_eq!(
        graph.stats.terminal_blur_fusion_candidates, 0,
        "the extra Fragment prevents the optimizer from seeing the required three-pass tail"
    );
    let tail = graph
        .passes
        .windows(4)
        .find(|passes| {
            passes[0].kind == RenderPassKind::DualKawaseUpsample
                && passes[1].kind == RenderPassKind::Fragment
                && passes[2].kind == RenderPassKind::Fragment
                && passes[3].kind == RenderPassKind::Composite
        })
        .expect("synthetic Tint keeps two local Fragments between Up2 and Composite");
    assert!(matches!(
        tail[1].stage.as_ref(),
        Some(oblivion_one::effects::EffectNodeKind::ColorMatrix(_))
    ));
    assert_eq!(tail[1].fused_stages.len(), 1);
    assert!(matches!(
        tail[1].fused_stages.first(),
        Some(oblivion_one::effects::EffectNodeKind::Tint(_))
    ));
    assert!(matches!(
        tail[2].stage.as_ref(),
        Some(oblivion_one::effects::EffectNodeKind::Noise(_))
    ));
    assert!(tail[2].fused_stages.is_empty());
    assert_eq!(tail[3].kind, RenderPassKind::Composite);
    assert!(tail[3].terminal_kawase.is_none());
    println!(
        "synthetic Tint physical tail: DualKawaseUpsample -> Fragment(ColorMatrix + Tint) -> Fragment(Noise) -> Composite; terminal_blur_fusions=0"
    );
}

#[test]
fn canonical_causal_abc_noise_free_and_noise_are_exact() {
    let fixture = native_three_checkpoint_fixture();
    for (case_name, noise) in [("causal_abc_noise0", 0.0), ("causal_abc_noise", 0.3)] {
        let (mut scene, _) = native_three_checkpoint_backdrop_scene(fixture);
        let program = oblivion_one::effects::builtin_background_material_program(
            oblivion_one::material::EffectiveMaterial {
                blur: 0.4,
                saturation: 0.52,
                noise,
                blur_passes: 3,
            },
        );
        let program_id = program.program.id;
        for instance in &mut scene.instances {
            instance.program = program_id;
        }
        let mut registry = oblivion_one::effects::EffectRegistry::empty();
        registry.insert(program).expect("causal material inserts");
        let staged_graph = terminal_graph_variant(&scene, &registry, fixture.output_bounds, false);
        let fused_graph = terminal_graph_variant(&scene, &registry, fixture.output_bounds, true);
        assert_eq!(staged_graph.stats.terminal_blur_fusions, 0);
        assert_eq!(
            fused_graph.stats.terminal_blur_fusions, 3,
            "causal A/B/C should retain three independent tails"
        );

        for origin in [
            OutputFramebufferOrigin::BottomLeft,
            OutputFramebufferOrigin::TopLeftScanout,
        ] {
            let (canonical_staged, _) = render_canonical_multi_tail_pair(
                &staged_graph,
                &fused_graph,
                fixture.output_size,
                origin,
                |harness| {
                    install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene);
                },
                case_name,
            );
            if noise == 0.0 && origin == OutputFramebufferOrigin::BottomLeft {
                let (_, old_pixels) = render_terminal_replay_mode(
                    &staged_graph,
                    fixture.output_size,
                    origin,
                    |harness| {
                        install_native_three_checkpoint_diagnostic_scene(harness, fixture.scene);
                    },
                    effects::TerminalBoundaryReplay::Legacy,
                    [],
                    [],
                );
                let impact = report_rgba8_comparison(
                    "old-vs-canonical causal A/B/C",
                    &old_pixels,
                    &canonical_staged,
                    fixture.output_size.0,
                    fixture.output_size.1,
                    origin,
                );
                println!("    compatibility impact causal A/B/C: {impact:?}");
            }
        }
    }
}

#[test]
fn canonical_partial_repair_matches_full_render_for_staged_and_fused_paths() {
    let output_size = (161, 121);
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let rect = EffectRect::new(19, 13, 101, 75).expect("translated blur");
    let visual_group = VisualGroupId::new(170).expect("repair visual group");
    let (scene, registry) = terminal_builtin_blur_scene(
        rect,
        visual_group,
        oblivion_one::material::EffectiveMaterial {
            blur: 0.42,
            saturation: 0.31,
            noise: 0.3,
            blur_passes: 3,
        },
        None,
    );
    let full_source_damage = EffectRegion::from_rect(bounds);
    let staged_graph = terminal_graph_variant_with_source_damage(
        &scene,
        &registry,
        bounds,
        &full_source_damage,
        false,
    );
    let fused_graph = terminal_graph_variant_with_source_damage(
        &scene,
        &registry,
        bounds,
        &full_source_damage,
        true,
    );
    let tails = terminal_tail_capture_ids(&staged_graph);
    assert_eq!(tails.len(), 1);
    let fused_terminal = fused_graph
        .passes
        .iter()
        .find(|pass| pass.terminal_kawase.is_some())
        .expect("fused terminal pass");
    let repair = OutputRect::new(20, 14, 3, 2);
    let mut repair_failures = Vec::new();

    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        let staged_replay = render_terminal_replay_after_partial_repair(
            &scene,
            &registry,
            &staged_graph,
            output_size,
            origin,
            |harness| install_diagnostic_scene_with_origin(harness, rect, visual_group, origin),
            repair,
            false,
            effects::TerminalBoundaryReplay::ProductionCanonicalStaged,
            true,
        );
        let staged_source_impact = summarize_rgba8_difference(
            &staged_replay.initial_pixels,
            &staged_replay.full_reference_pixels,
            output_size.0,
            output_size.1,
            origin,
        );
        println!("source repair full-frame impact C0 {origin:?}: {staged_source_impact:?}");
        if let Some(failure) = repair_pixel_mismatch(
            "canonical C0 partial vs independent full reference",
            "full",
            "partial",
            &staged_replay.full_reference_pixels,
            &staged_replay.partial_pixels,
            Some(&staged_replay.initial_pixels),
            output_size,
            origin,
            &staged_replay.source_damage,
            &staged_replay.repair_graph,
        ) {
            repair_failures.push(failure);
        }

        let fused_replay = render_terminal_replay_after_partial_repair(
            &scene,
            &registry,
            &fused_graph,
            output_size,
            origin,
            |harness| install_diagnostic_scene_with_origin(harness, rect, visual_group, origin),
            repair,
            true,
            effects::TerminalBoundaryReplay::ProductionCanonicalFused,
            true,
        );
        if let Some(failure) = repair_pixel_mismatch(
            "canonical C1 partial vs independent full reference",
            "full",
            "partial",
            &fused_replay.full_reference_pixels,
            &fused_replay.partial_pixels,
            Some(&fused_replay.initial_pixels),
            output_size,
            origin,
            &fused_replay.source_damage,
            &fused_replay.repair_graph,
        ) {
            repair_failures.push(failure);
        }
        if let Some(failure) = repair_pixel_mismatch(
            "canonical full C0 vs C1",
            "C0 full",
            "C1 full",
            &staged_replay.full_reference_pixels,
            &fused_replay.full_reference_pixels,
            None,
            output_size,
            origin,
            &staged_replay.source_damage,
            &staged_replay.repair_graph,
        ) {
            repair_failures.push(failure);
        }
        if let Some(failure) = repair_pixel_mismatch(
            "canonical partial C0 vs C1",
            "C0 partial",
            "C1 partial",
            &staged_replay.partial_pixels,
            &fused_replay.partial_pixels,
            None,
            output_size,
            origin,
            &staged_replay.source_damage,
            &staged_replay.repair_graph,
        ) {
            repair_failures.push(failure);
        }
        assert!(fused_terminal.terminal_kawase.is_some());
    }
    assert!(
        repair_failures.is_empty(),
        "source-damage-aware partial repair mismatches:\n{}",
        repair_failures.join("\n\n")
    );
}

#[test]
fn canonical_noise_seed_fields_are_origin_and_translation_independent() {
    // Leave room for the conservative aggregate capture halo on every side so
    // the two translated captures have equal, unclipped physical domains. The
    // two-level material's capture domain snaps to a 4 px output lattice, so
    // the translation stays on that lattice.
    let output_size = (401, 321);
    let first_rect = EffectRect::new(128, 120, 101, 75).expect("first translated domain");
    let translated_rect = EffectRect::new(140, 128, 101, 75).expect("translated domain");
    let material = oblivion_one::material::EffectiveMaterial {
        blur: 0.42,
        saturation: 0.31,
        noise: 0.3,
        blur_passes: 3,
    };
    let first_group = VisualGroupId::new(180).expect("first noise group");
    let translated_group = VisualGroupId::new(181).expect("translated noise group");
    let (first_scene, first_registry) =
        terminal_builtin_blur_scene_with_passes(first_rect, first_group, material, None, 2);
    let (translated_scene, translated_registry) = terminal_builtin_blur_scene_with_passes(
        translated_rect,
        translated_group,
        material,
        None,
        2,
    );
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let first_graph = terminal_graph_variant(&first_scene, &first_registry, bounds, false);
    let translated_graph =
        terminal_graph_variant(&translated_scene, &translated_registry, bounds, false);
    let first_fused_graph = terminal_graph_variant(&first_scene, &first_registry, bounds, true);
    let translated_fused_graph =
        terminal_graph_variant(&translated_scene, &translated_registry, bounds, true);
    let first_tail = terminal_tail_capture_ids(&first_graph)[0];
    let translated_tail = terminal_tail_capture_ids(&translated_graph)[0];
    let first_fused_terminal = first_fused_graph
        .passes
        .iter()
        .find(|pass| pass.terminal_kawase.is_some())
        .expect("first fused graph retains its terminal pass");
    let translated_fused_terminal = translated_fused_graph
        .passes
        .iter()
        .find(|pass| pass.terminal_kawase.is_some())
        .expect("translated fused graph retains its terminal pass");
    let first_domain = terminal_stage_output_domain(&first_graph, first_tail.fragment);
    let translated_domain =
        terminal_stage_output_domain(&translated_graph, translated_tail.fragment);
    let first_bottom = capture_canonical_staged_noise_field(
        &first_graph,
        output_size,
        OutputFramebufferOrigin::BottomLeft,
        first_tail.fragment,
        first_domain,
        first_rect,
        |harness| install_diagnostic_scene(harness, first_rect, first_group),
    );
    let first_top = capture_canonical_staged_noise_field(
        &first_graph,
        output_size,
        OutputFramebufferOrigin::TopLeftScanout,
        first_tail.fragment,
        first_domain,
        first_rect,
        |harness| install_diagnostic_scene(harness, first_rect, first_group),
    );
    assert_eq!(
        first_bottom.pixels, first_top.pixels,
        "canonical offscreen Noise field changed with framebuffer origin"
    );
    let translated = capture_canonical_staged_noise_field(
        &translated_graph,
        output_size,
        OutputFramebufferOrigin::TopLeftScanout,
        translated_tail.fragment,
        translated_domain,
        translated_rect,
        |harness| install_diagnostic_scene(harness, translated_rect, translated_group),
    );
    let translated_bottom = capture_canonical_staged_noise_field(
        &translated_graph,
        output_size,
        OutputFramebufferOrigin::BottomLeft,
        translated_tail.fragment,
        translated_domain,
        translated_rect,
        |harness| install_diagnostic_scene(harness, translated_rect, translated_group),
    );
    let first_fused_bottom = capture_canonical_terminal_noise_field(
        &first_fused_graph,
        output_size,
        OutputFramebufferOrigin::BottomLeft,
        first_fused_terminal.id,
        first_rect,
        |harness| install_diagnostic_scene(harness, first_rect, first_group),
    );
    let first_fused_top = capture_canonical_terminal_noise_field(
        &first_fused_graph,
        output_size,
        OutputFramebufferOrigin::TopLeftScanout,
        first_fused_terminal.id,
        first_rect,
        |harness| install_diagnostic_scene(harness, first_rect, first_group),
    );
    let translated_fused_bottom = capture_canonical_terminal_noise_field(
        &translated_fused_graph,
        output_size,
        OutputFramebufferOrigin::BottomLeft,
        translated_fused_terminal.id,
        translated_rect,
        |harness| install_diagnostic_scene(harness, translated_rect, translated_group),
    );
    let translated_fused_top = capture_canonical_terminal_noise_field(
        &translated_fused_graph,
        output_size,
        OutputFramebufferOrigin::TopLeftScanout,
        translated_fused_terminal.id,
        translated_rect,
        |harness| install_diagnostic_scene(harness, translated_rect, translated_group),
    );
    for (capture_name, capture) in [
        ("first TopLeftScanout", &first_top),
        ("translated BottomLeft", &translated_bottom),
        ("translated TopLeftScanout", &translated),
    ] {
        assert_eq!(
            (capture.width, capture.height),
            (first_bottom.width, first_bottom.height),
            "translated Noise capture dimensions differ for {capture_name}"
        );
    }
    let edge_and_center_pixels = [
        ("top-left", 0, 0),
        ("top-right", first_bottom.width - 1, 0),
        ("bottom-left", 0, first_bottom.height - 1),
        (
            "bottom-right",
            first_bottom.width - 1,
            first_bottom.height - 1,
        ),
        ("center", first_bottom.width / 2, first_bottom.height / 2),
    ];
    let noise_captures = [
        ("first BottomLeft", &first_bottom, first_rect),
        ("first TopLeftScanout", &first_top, first_rect),
        ("translated BottomLeft", &translated_bottom, translated_rect),
        ("translated TopLeftScanout", &translated, translated_rect),
        ("first fused BottomLeft", &first_fused_bottom, first_rect),
        ("first fused TopLeftScanout", &first_fused_top, first_rect),
        (
            "translated fused BottomLeft",
            &translated_fused_bottom,
            translated_rect,
        ),
        (
            "translated fused TopLeftScanout",
            &translated_fused_top,
            translated_rect,
        ),
    ];
    for (edge, local_x, local_y) in edge_and_center_pixels {
        let expected =
            canonical_noise_bits_at_local_pixel(&first_bottom, first_rect, local_x, local_y);
        for (capture_name, capture, capture_domain) in noise_captures.iter().skip(1) {
            assert_eq!(
                canonical_noise_bits_at_local_pixel(capture, *capture_domain, local_x, local_y),
                expected,
                "Noise bits differ at {edge} local [{local_x},{local_y}] for {capture_name}"
            );
        }
    }
    let mut distinct_noise_values = std::collections::HashSet::new();
    for local_y in 0..first_bottom.height {
        for local_x in 0..first_bottom.width {
            let first_bits =
                canonical_noise_bits_at_local_pixel(&first_bottom, first_rect, local_x, local_y);
            for (capture_name, capture, capture_domain) in noise_captures.iter().skip(1) {
                assert_eq!(
                    canonical_noise_bits_at_local_pixel(capture, *capture_domain, local_x, local_y,),
                    first_bits,
                    "Noise changed at local [{local_x},{local_y}] for {capture_name}"
                );
            }
            distinct_noise_values.insert(first_bits);
        }
    }
    assert!(
        distinct_noise_values.len() > 1,
        "canonical Noise test field should contain varying seeds"
    );
}

#[test]
fn canonical_noise_field_origin_is_invariant_with_capture_clipped_to_output() {
    let output_size = (257, 193);
    let effect_rect = EffectRect::new(64, 48, 101, 75).expect("edge-adjacent domain");
    let material = oblivion_one::material::EffectiveMaterial {
        blur: 0.42,
        saturation: 0.31,
        noise: 0.3,
        blur_passes: 3,
    };
    let group = VisualGroupId::new(180).expect("noise group");
    let (scene, registry) = terminal_builtin_blur_scene(effect_rect, group, material, None);
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let graph = terminal_graph_variant(&scene, &registry, bounds, false);
    let fused_graph = terminal_graph_variant(&scene, &registry, bounds, true);
    let tail = terminal_tail_capture_ids(&graph)[0];
    let fused_terminal = fused_graph
        .passes
        .iter()
        .find(|pass| pass.terminal_kawase.is_some())
        .expect("fused graph retains its terminal pass");
    let domain = terminal_stage_output_domain(&graph, tail.fragment);
    assert_eq!(
        domain.x, 0,
        "the expanded capture clips at the left output edge"
    );
    assert_eq!(
        domain.y, 0,
        "the expanded capture clips at the top output edge"
    );

    let bottom_left = capture_canonical_staged_noise_field(
        &graph,
        output_size,
        OutputFramebufferOrigin::BottomLeft,
        tail.fragment,
        domain,
        effect_rect,
        |harness| install_diagnostic_scene(harness, effect_rect, group),
    );
    let top_left = capture_canonical_staged_noise_field(
        &graph,
        output_size,
        OutputFramebufferOrigin::TopLeftScanout,
        tail.fragment,
        domain,
        effect_rect,
        |harness| install_diagnostic_scene(harness, effect_rect, group),
    );
    let fused_bottom_left = capture_canonical_terminal_noise_field(
        &fused_graph,
        output_size,
        OutputFramebufferOrigin::BottomLeft,
        fused_terminal.id,
        effect_rect,
        |harness| install_diagnostic_scene(harness, effect_rect, group),
    );
    let fused_top_left = capture_canonical_terminal_noise_field(
        &fused_graph,
        output_size,
        OutputFramebufferOrigin::TopLeftScanout,
        fused_terminal.id,
        effect_rect,
        |harness| install_diagnostic_scene(harness, effect_rect, group),
    );
    assert_eq!(
        bottom_left.pixels, top_left.pixels,
        "canonical offscreen Noise field changed with framebuffer origin when capture is clipped"
    );
    assert_eq!(
        fused_bottom_left.pixels, fused_top_left.pixels,
        "canonical terminal Noise field changed with framebuffer origin when capture is clipped"
    );
    assert_eq!(
        bottom_left.pixels, fused_bottom_left.pixels,
        "canonical terminal Noise field differs from the canonical staged field"
    );
}

#[test]
fn shared_backdrop_capture_matches_the_output_scene_prefix() {
    let mut harness = GlesEffectTestHarness::new(320, 240);
    harness.install_texture_backed_output();
    let a_group = oblivion_one::compositor::VisualGroupId::new(1).expect("A visual group");
    let b_group = oblivion_one::compositor::VisualGroupId::new(2).expect("B visual group");
    install_native_stacked_diagnostic_scene(
        &mut harness,
        NativeStackedBackdropSceneSpec {
            background_surface: 7,
            a_surface: 42,
            b_surface: 43,
            a_content_bounds: EffectRect::new(12, 12, 32, 24).expect("A bounds"),
            b_content_bounds: EffectRect::new(232, 112, 32, 24).expect("B bounds"),
            a_visual_group: a_group,
            b_visual_group: b_group,
        },
    );
    harness.renderer.establish_ordinary_scene_state();
    let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);
    let scope = oblivion_one::compositor::EffectAnchorScope::VisualGroup;
    let (prefix_end, _) = effects::composition_range_with_metadata(
        &harness.renderer.scene_state.commands,
        &harness.renderer.scene_state.capture_layers,
        &harness.renderer.scene_state.capture_visual_groups,
        anchor,
        Some(a_group),
        scope,
    );
    assert!(prefix_end > 0, "the background is below the shared blur");

    unsafe {
        harness.gl.bind_framebuffer(
            glow::FRAMEBUFFER,
            harness.renderer.scene_state.active_output_framebuffer,
        );
        harness.gl.disable(glow::SCISSOR_TEST);
        harness.gl.clear_color(0.0, 0.0, 0.0, 1.0);
        harness.gl.clear(glow::COLOR_BUFFER_BIT);
    }
    {
        let output = harness.renderer.scene_state.active_output_framebuffer;
        let mut context = harness.renderer.effect_execution_context();
        context
            .draw_effect_scene_range(
                &[OutputRect::new(0, 0, 320, 240)],
                0,
                prefix_end,
                OutputFramebufferOrigin::BottomLeft,
                EffectFramebufferTarget::new(output),
            )
            .expect("scene prefix draws to the output");
    }
    let expected = read_diagnostic_pixels(&harness);

    let capture = harness
        .renderer
        .effect_runtime
        .effect_resources
        .acquire(
            &harness.gl,
            EffectTextureKey::new(
                320,
                240,
                EffectTextureFormat::Rgba8,
                EffectTextureFilter::Linear,
                oblivion_one::effects::EffectWorkingSpace::OutputEncodedSrgb,
            ),
        )
        .expect("shared capture texture allocates");
    {
        let origin = harness.renderer.scene_state.current_framebuffer_origin;
        let mut context = harness.renderer.effect_execution_context();
        context
            .replay_precomputed_backdrop_capture(&capture, anchor, Some(a_group), scope, origin)
            .expect("shared backdrop capture replays");
    }
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .bind_draw_target(&harness.gl, &capture)
        .map(|framebuffer| unsafe {
            harness
                .gl
                .bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
        })
        .expect("shared capture binds for readback");
    let captured = read_diagnostic_pixels(&harness);
    harness
        .renderer
        .effect_runtime
        .effect_resources
        .unbind_render_target(&harness.gl);

    let differing = expected
        .chunks_exact(4)
        .zip(captured.chunks_exact(4))
        .filter(|(left, right)| {
            left.iter()
                .zip(right.iter())
                .any(|(a, b)| a.abs_diff(*b) > 2)
        })
        .count();
    assert_eq!(
        differing,
        0,
        "shared capture must equal the output prefix (first expected {:?}, captured {:?})",
        &expected[..4],
        &captured[..4]
    );
}

#[test]
fn shared_backdrop_capture_is_bottom_left_on_top_left_scanout() {
    let mut harness = GlesEffectTestHarness::new(320, 240);
    harness.install_texture_backed_output();
    let a_group = oblivion_one::compositor::VisualGroupId::new(1).expect("A visual group");
    let b_group = oblivion_one::compositor::VisualGroupId::new(2).expect("B visual group");
    install_native_stacked_diagnostic_scene(
        &mut harness,
        NativeStackedBackdropSceneSpec {
            background_surface: 7,
            a_surface: 42,
            b_surface: 43,
            a_content_bounds: EffectRect::new(12, 12, 32, 24).expect("A bounds"),
            b_content_bounds: EffectRect::new(232, 112, 32, 24).expect("B bounds"),
            a_visual_group: a_group,
            b_visual_group: b_group,
        },
    );
    harness.renderer.establish_ordinary_scene_state();
    let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);
    let scope = oblivion_one::compositor::EffectAnchorScope::VisualGroup;
    let key = EffectTextureKey::new(
        320,
        240,
        EffectTextureFormat::Rgba8,
        EffectTextureFilter::Linear,
        oblivion_one::effects::EffectWorkingSpace::OutputEncodedSrgb,
    );
    let capture_and_read = |harness: &mut GlesEffectTestHarness| {
        let capture = harness
            .renderer
            .effect_runtime
            .effect_resources
            .acquire(&harness.gl, key)
            .expect("capture allocates");
        {
            let origin = harness.renderer.scene_state.current_framebuffer_origin;
            let mut context = harness.renderer.effect_execution_context();
            context
                .replay_precomputed_backdrop_capture(&capture, anchor, Some(a_group), scope, origin)
                .expect("shared backdrop capture replays");
        }
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .bind_draw_target(&harness.gl, &capture)
            .map(|framebuffer| unsafe {
                harness
                    .gl
                    .bind_framebuffer(glow::READ_FRAMEBUFFER, Some(framebuffer));
            })
            .expect("capture binds");
        let pixels = read_diagnostic_pixels(harness);
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .unbind_render_target(&harness.gl);
        harness
            .renderer
            .effect_runtime
            .effect_resources
            .release(capture)
            .expect("capture releases");
        pixels
    };
    let bottom_left = capture_and_read(&mut harness);

    // Same scene, built for a top-left scanout framebuffer.
    let commands = harness.renderer.scene_state.commands.clone();
    harness.renderer.scene_state.vertices.clear();
    harness.renderer.scene_state.commands.clear();
    for command in &commands {
        let index = harness.renderer.scene_state.commands.len();
        crate::egl_renderer::geometry::push_draw_command(
            &mut harness.renderer.scene_state.vertices,
            &mut harness.renderer.scene_state.commands,
            command.layer,
            command.bounds,
            320,
            240,
            OutputFramebufferOrigin::TopLeftScanout,
        );
        harness.renderer.scene_state.commands[index].visual_group = command.visual_group;
    }
    harness.renderer.scene_state.scene_geometry_dirty = true;
    harness.renderer.scene_state.current_framebuffer_origin =
        OutputFramebufferOrigin::TopLeftScanout;
    let top_left = capture_and_read(&mut harness);

    let differing = bottom_left
        .chunks_exact(4)
        .zip(top_left.chunks_exact(4))
        .filter(|(left, right)| {
            left.iter()
                .zip(right.iter())
                .any(|(a, b)| a.abs_diff(*b) > 2)
        })
        .count();
    assert_eq!(
        differing, 0,
        "a top-left scanout scene must store the same bottom-left capture"
    );
}

// A blur composite planned from a small source repair covers the repair
// expanded by the kernel footprint. When the frame repaints only the repair,
// the surfaces above the blur anchor are replayed only there, so any composite
// pixel outside it would show the backdrop without the window above it.
#[test]
fn replay_blur_composite_leaves_pixels_outside_the_repaint_unchanged() {
    let output_size = (160, 120);
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let rect = EffectRect::new(18, 14, 96, 72).expect("blur domain");
    let visual_group = VisualGroupId::new(79).expect("test visual group");
    let (scene, registry) = terminal_builtin_blur_scene(
        rect,
        visual_group,
        oblivion_one::material::EffectiveMaterial {
            blur: 0.37,
            saturation: 1.0,
            noise: 0.0,
            blur_passes: 3,
        },
        None,
    );
    let config = effects::EffectDebugConfig::new(
        effects::EffectDebugCaptureMode::Replay,
        effects::EffectDebugKawaseMode::Partial,
    );
    let mut harness = GlesEffectTestHarness::new(output_size.0, output_size.1);
    install_diagnostic_scene(&mut harness, rect, visual_group);
    let initial_graph = terminal_graph_variant(&scene, &registry, bounds, false);
    execute_diagnostic_frame_with_origin(
        &mut harness,
        &initial_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[], true, output_size),
        EffectRegion::from_rect(bounds),
        true,
        config,
        OutputFramebufferOrigin::BottomLeft,
    );
    let initial_pixels = read_diagnostic_pixels(&harness);

    let repair = OutputRect::new(60, 40, 4, 4);
    update_diagnostic_background_for_surface(&harness, 7, repair, [207, 43, 89, 255]);
    let source_damage = diagnostic_region(repair);
    let repair_graph =
        terminal_graph_variant_with_source_damage(&scene, &registry, bounds, &source_damage, false);
    assert!(
        repair_graph
            .final_damage
            .subtract(&source_damage)
            .rects()
            .iter()
            .any(|rect| rect.width > 0 && rect.height > 0),
        "the blur footprint must expand the composite beyond the repaint"
    );
    execute_diagnostic_frame_with_origin(
        &mut harness,
        &repair_graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[repair], false, output_size),
        source_damage,
        false,
        config,
        OutputFramebufferOrigin::BottomLeft,
    );
    let partial_pixels = read_diagnostic_pixels(&harness);

    let (width, height) = output_size;
    let mut changed_outside_repair = 0;
    for row in 0..height {
        // Rows are read bottom-up from a bottom-left framebuffer.
        let y = (height - 1 - row) as i32;
        for x in 0..width as i32 {
            let inside = x >= repair.x
                && x < repair.x + repair.width as i32
                && y >= repair.y
                && y < repair.y + repair.height as i32;
            if inside {
                continue;
            }
            let index = (row as usize * width as usize + x as usize) * 4;
            if initial_pixels[index..index + 4] != partial_pixels[index..index + 4] {
                changed_outside_repair += 1;
            }
        }
    }
    assert_eq!(
        changed_outside_repair, 0,
        "a partial frame must not change pixels outside its repaint"
    );
}

/// Render a full frame of a background-only pattern blurred over `rect` and
/// return the RGBA8 readback (bottom-left origin rows).
fn render_pattern_blur_frame(rect: EffectRect, passes: u8) -> Vec<u8> {
    let (width, height) = (320_u32, 240_u32);
    let output_bounds = EffectRect::new(0, 0, width, height).expect("output bounds");
    let mut harness = GlesEffectTestHarness::new(width, height);
    let mut background = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        for x in 0..width {
            // High-contrast strokes, like text behind a translucent panel.
            let ink = (x * 7 + y * 13) % 23 < 3 || (x / 5 + y / 3) % 11 == 0;
            let value: u8 = if ink { 16 } else { 236 };
            background.extend_from_slice(&[value, value, value.saturating_sub(20), 255]);
        }
    }
    harness
        .renderer
        .resources
        .test_create_surface_texture(&harness.gl, 7, width, height, Some(&background))
        .expect("pattern background texture creates");
    harness.renderer.scene_state.vertices.clear();
    harness.renderer.scene_state.commands.clear();
    push_draw_command(
        &mut harness.renderer.scene_state.vertices,
        &mut harness.renderer.scene_state.commands,
        EglDrawLayer::Surface(7),
        EglRect::new(0.0, 0.0, width as f32, height as f32),
        width,
        height,
        OutputFramebufferOrigin::BottomLeft,
    );
    harness.renderer.scene_state.scene_geometry_dirty = true;
    let (scene, registry) = moving_blur_scene_with_spec(rect, 4.0, passes, 1.0);
    harness.renderer.set_effect_registry(registry.clone());
    let plan = oblivion_one::effects::compile_frame_execution_plan(
        &scene,
        &EffectRegion::from_rect(output_bounds),
        output_bounds,
        &registry,
    )
    .expect("pattern blur graph compiles");
    let oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) = plan else {
        panic!("pattern blur must compile to an effect graph");
    };
    execute_diagnostic_frame(
        &mut harness,
        &graph,
        &diagnostic_repaint_plan_for_repairs_in_size(&[], true, (width, height)),
        EffectRegion::from_rect(output_bounds),
        true,
        effects::EffectDebugConfig::new(
            effects::EffectDebugCaptureMode::Replay,
            effects::EffectDebugKawaseMode::Partial,
        ),
    );
    read_diagnostic_pixels(&harness)
}

#[test]
fn growing_blur_region_keeps_blurred_pixels_on_a_fixed_screen_grid() {
    // A top-anchored panel growing by a few pixels (Spotlight's results list
    // animating its height) must not shift the Kawase sampling grid: pixels
    // well inside both regions blur to identical values.
    let (width, height) = (320_usize, 240_usize);
    for passes in [2_u8, 3] {
        let base = render_pattern_blur_frame(
            EffectRect::new(121, 33, 77, 90).expect("base panel"),
            passes,
        );
        for grow in 1..=3_u32 {
            let grown = render_pattern_blur_frame(
                EffectRect::new(121, 33, 77, 90 + grow).expect("grown panel"),
                passes,
            );
            let mut differing = 0;
            let mut max_delta = 0_u8;
            // Logical rows 33..123 are storage rows height-123..height-33.
            for storage_y in (height - 123)..(height - 33) {
                for x in 121..198 {
                    let index = (storage_y * width + x) * 4;
                    for channel in 0..3 {
                        let delta = base[index + channel].abs_diff(grown[index + channel]);
                        if delta != 0 {
                            differing += 1;
                            max_delta = max_delta.max(delta);
                        }
                    }
                }
            }
            assert_eq!(
                (differing, max_delta),
                (0, 0),
                "passes={passes} grow={grow}: blurred pixels moved with the region size"
            );
        }
    }
}
