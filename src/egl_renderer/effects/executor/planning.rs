use super::*;

#[derive(Debug)]
pub(super) struct PreparedEffectRegion {
    pub(super) region: EffectRegion,
    pub(super) input_rect_count: usize,
    pub(super) duplicate_rects_removed: usize,
    pub(super) overlap_fragments_generated: usize,
    pub(super) fallback: Option<&'static str>,
}

pub(super) fn effect_region_pixels(region: &EffectRegion) -> u64 {
    region.rects().iter().fold(0u64, |total, rect| {
        total.saturating_add(u64::from(rect.width).saturating_mul(u64::from(rect.height)))
    })
}

pub(super) fn output_rects_pixels(rects: &[OutputRect]) -> u64 {
    rects.iter().fold(0u64, |total, rect| {
        total.saturating_add(u64::from(rect.width).saturating_mul(u64::from(rect.height)))
    })
}

pub(super) fn composite_scene_replay_work(
    command_start: usize,
    command_end: usize,
    scene_commands_total: usize,
    active_work: &[OutputRect],
    pending_checkpoint_requirements: usize,
) -> CompositeSceneReplayWork {
    let command_count = command_end.saturating_sub(command_start);
    let active_work_rects = active_work.len();
    CompositeSceneReplayWork {
        command_start,
        command_end,
        command_count,
        scene_commands_total,
        active_work_rects,
        active_work_pixels: output_rects_pixels(active_work),
        command_region_pairs: command_count.saturating_mul(active_work_rects),
        scene_scan_pairs: scene_commands_total.saturating_mul(active_work_rects),
        pending_checkpoint_requirements,
    }
}

pub(super) fn should_time_composite_scene_replay(
    pass_kind: RenderPassKind,
    graph_timing_active: bool,
    scene_cursor: usize,
    draw_end: usize,
    has_active_work: bool,
    capture_in_progress: bool,
) -> bool {
    pass_kind == RenderPassKind::Composite
        && graph_timing_active
        && draw_end > scene_cursor
        && has_active_work
        && !capture_in_progress
}

pub(super) fn monotonic_elapsed_ns(start: Option<Instant>) -> u64 {
    start.map_or(0, |start| {
        u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
    })
}

pub(super) fn composite_scene_replay_execution_detail(
    before: GlesSceneFrameStats,
    after: GlesSceneFrameStats,
    host_cpu_ns: u64,
) -> CompositeSceneReplayExecutionDetail {
    CompositeSceneReplayExecutionDetail {
        host_cpu_ns,
        commands_considered: after
            .commands_considered
            .saturating_sub(before.commands_considered),
        commands_executed: after
            .commands_executed
            .saturating_sub(before.commands_executed),
        draw_calls: after.draw_calls.saturating_sub(before.draw_calls),
        texture_binds: after.texture_binds.saturating_sub(before.texture_binds),
        scene_vbo_uploads: after
            .scene_vbo_uploads
            .saturating_sub(before.scene_vbo_uploads),
        scene_vbo_upload_bytes: after
            .scene_vbo_upload_bytes
            .saturating_sub(before.scene_vbo_upload_bytes),
    }
}

pub(super) fn replay_capture_host_timing_enabled(
    host_timing_enabled: bool,
    direct_capture: bool,
) -> bool {
    host_timing_enabled && !direct_capture
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct CheckpointSourceValidity {
    pub(super) required_rect_count: usize,
    pub(super) required_bounding_box: Option<(i32, i32, u32, u32)>,
    pub(super) valid_rect_count: usize,
    pub(super) valid_bounding_box: Option<(i32, i32, u32, u32)>,
    pub(super) missing: EffectRegion,
}

pub(super) fn checkpoint_source_validity(
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

pub(super) fn checkpoint_source_semantic_validity(
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

pub(super) fn output_rects_to_effect_region(rects: &[OutputRect]) -> EffectRegion {
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

pub(super) fn scene_valid_region_after_scene_advance(active_work: &[OutputRect]) -> EffectRegion {
    output_rects_to_effect_region(active_work)
}

pub(super) fn prepare_effect_execution_region(
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

pub(super) fn output_rect_pixels(rects: &[OutputRect]) -> u64 {
    rects.iter().fold(0u64, |total, rect| {
        total.saturating_add(u64::from(rect.width).saturating_mul(u64::from(rect.height)))
    })
}

pub(super) fn effective_pass_damage(
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

pub(super) fn pass_output_texture_domain(
    graph: &CompiledFrameGraph,
    pass: &CompiledRenderPass,
) -> EffectRegion {
    pass.output
        .and_then(|output| graph.textures.iter().find(|texture| texture.id == output))
        .map_or_else(EffectRegion::empty, |texture| {
            EffectRegion::from_rect(texture.domain)
        })
}

pub(super) fn checkpoint_dependency_influence_region(
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

pub(super) fn is_direct_framebuffer_capture(
    pass: &CompiledRenderPass,
    scene_baseline_authority: SceneBaselineAuthority,
    debug_config: EffectDebugConfig,
) -> bool {
    match pass.kind {
        RenderPassKind::SceneCapture => {
            scene_baseline_authority == SceneBaselineAuthority::PrecomposedFramebuffer
                || !pass.checkpoint_dependencies.is_empty()
                || debug_config.capture_mode() == EffectDebugCaptureMode::Framebuffer
        }
        RenderPassKind::SurfaceCapture => !pass.checkpoint_dependencies.is_empty(),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CheckpointCaptureExecutionPlan {
    pub(super) requested: Option<CheckpointCapturePath>,
    pub(super) executed: CaptureTimingMode,
    pub(super) fallback_reason: Option<CapturePathFallbackReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CaptureDownsampleFusion {
    pub(super) capture_pass: GraphPassId,
    pub(super) consumer_pass: GraphPassId,
    pub(super) capture_texture: GraphTextureId,
    pub(super) capture_domain: oblivion_one::effects::EffectRect,
}

#[derive(Debug, Default)]
pub(super) struct CaptureDownsampleFusionPlan {
    pub(super) by_capture: HashMap<GraphPassId, CaptureDownsampleFusion>,
    pub(super) by_consumer: HashMap<GraphPassId, CaptureDownsampleFusion>,
    pub(super) candidates: usize,
    pub(super) ineligible: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PersistentSceneCaptureMode {
    DependentFramebufferCheckpoint,
    DependencyFreeReplayCapture,
}

pub(super) fn persistent_scene_capture_mode(
    pass: &CompiledRenderPass,
    target: &GraphTextureBinding,
    scene_baseline_authority: SceneBaselineAuthority,
    debug_config: EffectDebugConfig,
    capture_plan: CheckpointCaptureExecutionPlan,
) -> Option<PersistentSceneCaptureMode> {
    if pass.kind != RenderPassKind::SceneCapture
        || !target.is_checkpoint_cache()
        || scene_baseline_authority != SceneBaselineAuthority::ReplayRequired
        || debug_config.capture_mode() != EffectDebugCaptureMode::Replay
    {
        return None;
    }
    match (
        pass.checkpoint_dependencies.is_empty(),
        capture_plan.executed,
    ) {
        (false, CaptureTimingMode::FramebufferShaderCopy) => {
            Some(PersistentSceneCaptureMode::DependentFramebufferCheckpoint)
        }
        (true, CaptureTimingMode::Replay) => {
            Some(PersistentSceneCaptureMode::DependencyFreeReplayCapture)
        }
        _ => None,
    }
}

pub(super) fn checkpoint_capture_execution_plan(
    pass_kind: RenderPassKind,
    checkpoint_dependency_count: usize,
    scene_baseline_authority: SceneBaselineAuthority,
    capture_mode: EffectDebugCaptureMode,
    requested_path: CheckpointCapturePath,
    active_output_texture_available: bool,
) -> CheckpointCaptureExecutionPlan {
    let direct_capture = match pass_kind {
        RenderPassKind::SceneCapture => {
            scene_baseline_authority == SceneBaselineAuthority::PrecomposedFramebuffer
                || checkpoint_dependency_count > 0
                || capture_mode == EffectDebugCaptureMode::Framebuffer
        }
        RenderPassKind::SurfaceCapture => checkpoint_dependency_count > 0,
        _ => false,
    };
    let checkpoint_direct_capture = pass_kind == RenderPassKind::SceneCapture
        && checkpoint_dependency_count > 0
        && scene_baseline_authority == SceneBaselineAuthority::ReplayRequired
        && capture_mode == EffectDebugCaptureMode::Replay;
    if checkpoint_direct_capture {
        let requested = Some(requested_path);
        return match requested_path {
            CheckpointCapturePath::FramebufferBlit => CheckpointCaptureExecutionPlan {
                requested,
                executed: CaptureTimingMode::FramebufferBlit,
                fallback_reason: None,
            },
            CheckpointCapturePath::FramebufferShaderCopy if active_output_texture_available => {
                CheckpointCaptureExecutionPlan {
                    requested,
                    executed: CaptureTimingMode::FramebufferShaderCopy,
                    fallback_reason: None,
                }
            }
            CheckpointCapturePath::FramebufferShaderCopy => CheckpointCaptureExecutionPlan {
                requested,
                executed: CaptureTimingMode::FramebufferBlit,
                fallback_reason: Some(CapturePathFallbackReason::NoSampleableOutputTexture),
            },
        };
    }

    CheckpointCaptureExecutionPlan {
        requested: None,
        executed: if direct_capture {
            CaptureTimingMode::FramebufferBlit
        } else {
            CaptureTimingMode::Replay
        },
        fallback_reason: None,
    }
}

pub(super) fn checkpoint_capture_execution_plan_for_pass(
    renderer: &EffectExecutionContext<'_>,
    pass: &CompiledRenderPass,
    scene_baseline_authority: SceneBaselineAuthority,
    debug_config: EffectDebugConfig,
) -> CheckpointCaptureExecutionPlan {
    checkpoint_capture_execution_plan(
        pass.kind,
        pass.checkpoint_dependencies.len(),
        scene_baseline_authority,
        debug_config.capture_mode(),
        debug_config.checkpoint_capture_path(),
        renderer.scene.active_output_texture.is_some(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_capture_downsample_fusions(
    graph: &CompiledFrameGraph,
    selection: &EffectExecutionSelection,
    preparations: &HashMap<GraphTextureId, CheckpointCapturePreparation>,
    active_output_texture_available: bool,
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
    scene_baseline_authority: SceneBaselineAuthority,
    debug_config: EffectDebugConfig,
) -> CaptureDownsampleFusionPlan {
    let mut plan = CaptureDownsampleFusionPlan::default();
    for (capture_index, capture) in graph.passes.iter().enumerate() {
        if capture.kind != RenderPassKind::SceneCapture {
            continue;
        }
        let Some(capture_texture) = capture.output else {
            continue;
        };
        let Some(preparation) = preparations.get(&capture_texture) else {
            continue;
        };
        if !preparation.newly_admitted || !preparation.identity_churn {
            continue;
        }
        plan.candidates = plan.candidates.saturating_add(1);
        let fusion = (|| {
            if capture.checkpoint_dependencies.is_empty()
                || !selection.executed_passes.contains(&capture.id)
                || !active_output_texture_available
            {
                return None;
            }
            let capture_execution = checkpoint_capture_execution_plan(
                capture.kind,
                capture.checkpoint_dependencies.len(),
                scene_baseline_authority,
                debug_config.capture_mode(),
                debug_config.checkpoint_capture_path(),
                active_output_texture_available,
            );
            if capture_execution.executed != CaptureTimingMode::FramebufferShaderCopy {
                return None;
            }
            let capture_plan = graph
                .textures
                .iter()
                .find(|texture| texture.id == capture_texture)?;
            if capture_plan.source != GraphTextureSource::CapturedScene
                || capture_plan.working_space
                    != oblivion_one::effects::EffectWorkingSpace::OutputEncodedSrgb
                || capture_plan.origin != oblivion_one::effects::GraphTextureOrigin::BottomLeft
                || capture_plan.width != capture_plan.domain.width
                || capture_plan.height != capture_plan.domain.height
                || plan_graph_texture_capture(
                    output_size,
                    capture_plan.domain,
                    (capture_plan.width, capture_plan.height),
                    framebuffer_origin,
                )
                .is_none()
            {
                return None;
            }
            let consumer_index = capture_index.checked_add(1)?;
            let consumer = graph.passes.get(consumer_index)?;
            if !selection.executed_passes.contains(&consumer.id)
                || consumer.instance != capture.instance
                || consumer.kind != RenderPassKind::DualKawaseDownsample
                || consumer.inputs.as_slice() != [capture_texture]
                || graph
                    .passes
                    .iter()
                    .filter(|candidate| candidate.inputs.contains(&capture_texture))
                    .count()
                    != 1
                || graph
                    .passes
                    .iter()
                    .find(|candidate| {
                        candidate.kind == RenderPassKind::DualKawaseDownsample
                            && candidate.instance == capture.instance
                    })
                    .is_none_or(|first| first.id != consumer.id)
            {
                return None;
            }
            let output = consumer.output?;
            if graph
                .textures
                .iter()
                .find(|texture| texture.id == output)
                .is_none_or(|texture| texture.source != GraphTextureSource::Intermediate)
            {
                return None;
            }
            Some(CaptureDownsampleFusion {
                capture_pass: capture.id,
                consumer_pass: consumer.id,
                capture_texture,
                capture_domain: capture_plan.domain,
            })
        })();
        if let Some(fusion) = fusion {
            plan.by_capture.insert(fusion.capture_pass, fusion);
            plan.by_consumer.insert(fusion.consumer_pass, fusion);
        } else {
            plan.ineligible = plan.ineligible.saturating_add(1);
        }
    }
    plan
}

pub(super) fn capture_timing_metadata(
    pass: &CompiledRenderPass,
    capture_plan: CheckpointCaptureExecutionPlan,
) -> Option<CaptureTimingMetadata> {
    matches!(
        pass.kind,
        RenderPassKind::SceneCapture | RenderPassKind::SurfaceCapture
    )
    .then_some(CaptureTimingMetadata {
        mode: capture_plan.executed,
        checkpoint_count: pass.checkpoint_dependencies.len(),
    })
}

pub(super) fn capture_execution_damage(
    graph: &CompiledFrameGraph,
    demand: &EffectExecutionDemand,
    pass: &CompiledRenderPass,
    scene_baseline_authority: SceneBaselineAuthority,
    debug_config: EffectDebugConfig,
) -> EffectRegion {
    if is_direct_framebuffer_capture(pass, scene_baseline_authority, debug_config) {
        pass_output_texture_domain(graph, pass)
    } else {
        effective_pass_damage(graph, demand, pass)
    }
}
