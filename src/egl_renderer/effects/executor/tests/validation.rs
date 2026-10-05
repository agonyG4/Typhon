use super::*;

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
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
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
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
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
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
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
            0.5,
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
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
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
        requested.push(oblivion_one::effects::EffectRect::new(0, index * 2 + 1, 128, 1).unwrap());
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
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
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
        requested.push(oblivion_one::effects::EffectRect::new(0, index * 2 + 1, 128, 1).unwrap());
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
    assert!(NORMALIZE_FRAGMENT_SHADER.contains("(output_position - u_effect_input_domain.xy) /"));
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
                semantic_signature: 0,
                frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
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
                semantic_signature: 0,
                frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
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
            presentation_output_region: EffectRegion::from_rect(
                oblivion_one::effects::EffectRect::new(0, 0, 10, 10).unwrap(),
            ),
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
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
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
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
            id: instance,
            output_influence_region: EffectRegion::from_rect(visible),
            capture_region: EffectRegion::from_rect(capture_domain),
            dependencies: Vec::new(),
        }],
        final_damage: EffectRegion::empty(),
        stats: Default::default(),
    };
    let demand =
        oblivion_one::effects::plan_effect_execution_demand(&graph, &EffectRegion::empty(), true);

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
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
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
        (rect.x..rect.x + rect.width as i32)
            .all(|x| (rect.y..rect.y + rect.height as i32).all(|y| visible.contains_point(x, y)))
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
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
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
            semantic_signature: 0,
            frame_demand: oblivion_one::effects::EffectFrameDemand::OnDamage,
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
