use super::*;

fn terminal_builtin_blur_scene(
    rect: EffectRect,
    visual_group: VisualGroupId,
    material: oblivion_one::material::EffectiveMaterial,
    coverage: Option<oblivion_one::effects::EffectCoverage>,
) -> (ResolvedEffectScene, oblivion_one::effects::EffectRegistry) {
    let program = oblivion_one::effects::builtin_background_material_program(material);
    let program_id = program.program.id;
    let mut registry = oblivion_one::effects::EffectRegistry::empty();
    registry
        .insert(program)
        .expect("canonical material inserts");
    let anchor = oblivion_one::compositor::EffectAnchor::BeforeSurface(42);
    let instance = oblivion_one::compositor::ResolvedEffectInstance {
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

fn terminal_graph_variant(
    scene: &ResolvedEffectScene,
    registry: &oblivion_one::effects::EffectRegistry,
    bounds: EffectRect,
    enabled: bool,
) -> CompiledFrameGraph {
    let variants = oblivion_one::effects::compile_frame_execution_variants_with_options(
        scene,
        &EffectRegion::empty(),
        bounds,
        registry,
        oblivion_one::effects::FrameExecutionCompileOptions {
            precomputed_window_blur: false,
            terminal_kawase_fusion: enabled,
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
    read_diagnostic_pixels(&harness)
}

fn terminal_pixel_difference_summary(expected: &[u8], actual: &[u8], width: u32) -> String {
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
        let y = u32::try_from(pixel_index).unwrap() / width;
        bounds[0] = bounds[0].min(x);
        bounds[1] = bounds[1].min(y);
        bounds[2] = bounds[2].max(x);
        bounds[3] = bounds[3].max(y);
        for (expected, actual) in expected.iter().zip(actual) {
            max_channel_delta = max_channel_delta.max(expected.abs_diff(*actual));
        }
        if samples.len() < 8 {
            samples.push((x, y, expected.to_vec(), actual.to_vec()));
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
struct Rgba8DifferenceSummary {
    changed_pixels: usize,
    changed_ratio: f64,
    max_rgb_delta: u8,
    mean_absolute_rgb_error: f64,
    rms_rgb_error: f64,
    bounds: Option<[u32; 4]>,
    alpha_changed_pixels: usize,
}

fn summarize_rgba8_difference(
    expected: &[u8],
    actual: &[u8],
    width: u32,
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
            let y = (index as u32) / width;
            summary.bounds = Some(summary.bounds.map_or([x, y, x, y], |bounds| {
                [
                    bounds[0].min(x),
                    bounds[1].min(y),
                    bounds[2].max(x),
                    bounds[3].max(y),
                ]
            }));
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
) -> Rgba8DifferenceSummary {
    let summary = summarize_rgba8_difference(legacy, diagnostic, width);
    println!("    {label}: {summary:?}");
    summary
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
) {
    let material = oblivion_one::material::EffectiveMaterial {
        blur,
        saturation,
        noise,
    };
    let (scene, registry) = terminal_builtin_blur_scene(rect, visual_group, material, coverage);
    let bounds = EffectRect::new(0, 0, output_size.0, output_size.1).expect("output bounds");
    let legacy_graph = terminal_graph_variant(&scene, &registry, bounds, false);
    let terminal_graph = terminal_graph_variant(&scene, &registry, bounds, true);
    let tails = terminal_tail_capture_ids(&legacy_graph);
    assert_eq!(tails.len(), 1, "single blur has one canonical tail");
    let tail = tails[0];
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

    let a0 = l0.captures.get(&tail.upsample).expect("L0 captures A0");
    let b0 = l0.captures.get(&tail.fragment).expect("L0 captures B0");
    let a1 = kawase_probe
        .stage_probes
        .get(&(tail.upsample, effects::TerminalDiagnosticStage::Up2))
        .expect("A1 terminal-style Up2 probe");
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
    report_rgba16f_comparison("A0 vs A2", a0, a2);
    let a0_a3 = report_rgba16f_comparison("A0 vs A3", a0, a3);
    let b0_b1 = report_rgba16f_comparison("B0 vs B1", b0, b1);
    let b0_b2 = report_rgba16f_comparison("B0 vs B2", b0, b2);
    let b0_b3 = report_rgba16f_comparison("B0 vs B3", b0, b3);
    report_rgba8_comparison("C0 vs C1", &c0, &c1, output_size.0);
    report_rgba8_comparison("C0 vs C2", &c0, &c2, output_size.0);
    report_rgba8_comparison("C0 vs C_B2", &c0, &c_material_probe, output_size.0);
    report_rgba8_comparison("C0 vs C3", &c0, &c3, output_size.0);
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
}

fn run_terminal_causal_diagnostic_case(noise: f32, origin: OutputFramebufferOrigin) {
    let fixture = native_three_checkpoint_fixture();
    let (mut scene, _) = native_three_checkpoint_backdrop_scene(fixture);
    let program = oblivion_one::effects::builtin_background_material_program(
        oblivion_one::material::EffectiveMaterial {
            blur: 0.4,
            saturation: 0.52,
            noise,
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
        let b0 = l0.captures.get(&tail.fragment).expect("causal B0");
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
        report_rgba16f_comparison("A0 vs A1", a0, a1);
        report_rgba16f_comparison("A0 vs A2", a0, a2);
        report_rgba16f_comparison("A0 vs A3", a0, a3);
        report_rgba16f_comparison("B0 vs B1", b0, b1);
        report_rgba16f_comparison("B0 vs B2", b0, b2);
        report_rgba16f_comparison("B0 vs B3", b0, b3);
    }
    report_rgba8_comparison("C0 vs C1", &c0, &c1, fixture.output_size.0);
    report_rgba8_comparison("C0 vs C2", &c0, &c2, fixture.output_size.0);
    report_rgba8_comparison("C0 vs C_B2", &c0, &c_b2_probe, fixture.output_size.0);
    report_rgba8_comparison("C0 vs C3", &c0, &c3, fixture.output_size.0);
}

#[test]
fn terminal_boundary_rgba16f_noise_free_matrix_and_legacy_noise_attribution() {
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
        run_terminal_builtin_diagnostic_case(
            "single_sat_1_noise_0",
            (160, 120),
            even_rect,
            VisualGroupId::new(71).expect("visual group"),
            0.37,
            1.0,
            0.0,
            None,
            origin,
        );
        run_terminal_builtin_diagnostic_case(
            "single_sat_031_noise_0",
            (160, 120),
            even_rect,
            VisualGroupId::new(72).expect("visual group"),
            0.37,
            0.31,
            0.0,
            None,
            origin,
        );
        run_terminal_builtin_diagnostic_case(
            "odd_translated_sat_031_noise_0",
            odd_bounds,
            odd_rect,
            VisualGroupId::new(73).expect("visual group"),
            0.42,
            0.31,
            0.0,
            None,
            origin,
        );
        run_terminal_builtin_diagnostic_case(
            "even_translated_sat_1_noise_0",
            (160, 120),
            even_rect,
            VisualGroupId::new(74).expect("visual group"),
            0.37,
            1.0,
            0.0,
            None,
            origin,
        );
        run_terminal_builtin_diagnostic_case(
            "rounded_triangle_coverage_noise_0",
            odd_bounds,
            odd_rect,
            VisualGroupId::new(75).expect("visual group"),
            0.42,
            0.31,
            0.0,
            Some(rounded_triangle.clone()),
            origin,
        );
        run_terminal_causal_diagnostic_case(0.0, origin);
    }

    for origin in [
        OutputFramebufferOrigin::BottomLeft,
        OutputFramebufferOrigin::TopLeftScanout,
    ] {
        run_terminal_builtin_diagnostic_case(
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
        run_terminal_causal_diagnostic_case(0.3, origin);
    }
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
    let odd_diff = terminal_pixel_difference_summary(&odd_off_pixels, &odd_on_pixels, 161);

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
    let partial_diff = terminal_pixel_difference_summary(&odd_off_partial, &odd_on_partial, 161);

    let even_bounds = EffectRect::new(0, 0, 160, 120).expect("even output bounds");
    let even_rect = EffectRect::new(18, 14, 96, 72).expect("even blur domain");
    let (even_scene, even_registry) = terminal_builtin_blur_scene(
        even_rect,
        VisualGroupId::new(78).expect("even test visual group"),
        oblivion_one::material::EffectiveMaterial {
            blur: 0.37,
            saturation: 1.0,
            noise: 0.0,
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
    let even_diff = terminal_pixel_difference_summary(&even_off_pixels, &even_on_pixels, 160);

    // Keep the real A -> C -> B causal checkpoint chain while comparing the
    // same canonical local material under the two physical graph variants.
    let fixture = native_three_checkpoint_fixture();
    let (mut causal_scene, _) = native_three_checkpoint_backdrop_scene(fixture);
    let program = oblivion_one::effects::builtin_background_material_program(
        oblivion_one::material::EffectiveMaterial {
            blur: 0.4,
            saturation: 0.52,
            noise: 0.3,
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
    let program = oblivion_one::effects::builtin_background_blur_program();
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
    assert!(client_variants.precomputed_backdrop.is_none());
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

#[test]
fn precomputed_baseline_only_scene_content_matches_exact_gles_pixels() {
    const PIXEL_TOLERANCE: u8 = 2;

    let mut harness = GlesEffectTestHarness::new(160, 120);
    harness.install_texture_backed_output();
    let output_bounds = EffectRect::new(0, 0, 160, 120).expect("output bounds");
    let full_damage = EffectRegion::from_rect(output_bounds);
    let a_group = oblivion_one::compositor::VisualGroupId::new(1).expect("A visual group");
    let b_group = oblivion_one::compositor::VisualGroupId::new(2).expect("B visual group");
    let a_bounds = EffectRect::new(12, 12, 32, 24).expect("A bounds");
    let b_bounds = EffectRect::new(112, 78, 32, 24).expect("B bounds");
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
    let program = oblivion_one::effects::builtin_background_blur_program();
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
    let repaint = diagnostic_repaint_plan_for_repairs_in_size(&[], true, (160, 120));

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
            &[OutputRect::new(0, 0, 160, 120)],
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
    let optimized_demand = plan_effect_execution_demand(optimized_graph, &full_damage, true);
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
            &repaint,
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
    let optimized_pixels = read_diagnostic_pixels(&harness);

    let mut compared_pixels = 0;
    for rect in [a_bounds, b_bounds] {
        for y in rect.y..rect.y + rect.height as i32 {
            for x in rect.x..rect.x + rect.width as i32 {
                let offset = ((y as usize * 160 + x as usize) * 4) as usize;
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
    let rect_b = EffectRect::new(112, 80, 64, 48).expect("translated blur bounds");
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
    let mut first_dimensions = None;
    let mut first_allocation_count = None;
    let mut warm_checkpoint_entries = None;
    let mut warm_checkpoint_bytes_max = 0;
    let mut warm_pool_bytes_max = 0;
    let mut capture_radii = None;
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

        let capture = graph
            .textures
            .iter()
            .find(|texture| texture.source == GraphTextureSource::CapturedScene)
            .expect("moving blur capture texture");
        let (capture_radius_x, capture_radius_y) = *capture_radii.get_or_insert((
            x.saturating_sub(capture.domain.x).max(0),
            y.saturating_sub(capture.domain.y).max(0),
        ));
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
            if let Some(first) = &first_dimensions {
                assert_eq!(first, &dimensions, "translation changed texture dimensions");
            } else {
                first_dimensions = Some(dimensions);
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
        assert!(metrics.cached_texture_count <= 16);
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
    let mut first_dimensions = None;
    let mut warm_allocation_count = None;
    let mut warm_checkpoint_entries = None;
    let mut warm_checkpoint_bytes_max = 0;
    let mut warm_pool_bytes_max = 0;
    let mut capture_radii = None;

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
        let (capture_radius_x, capture_radius_y) = *capture_radii.get_or_insert((
            x.saturating_sub(capture.domain.x).max(0),
            y.saturating_sub(capture.domain.y).max(0),
        ));
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
            if let Some(first) = &first_dimensions {
                assert_eq!(first, &dimensions, "translation changed texture dimensions");
            } else {
                first_dimensions = Some(dimensions);
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
        assert!(metrics.cached_texture_count <= 32);
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
