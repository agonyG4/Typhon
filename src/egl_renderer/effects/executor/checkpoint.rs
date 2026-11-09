use super::*;

/// `TYPHON_EFFECT_CAUSAL_TRACE=1` logs, every 30th frame, which scene command
/// ends the unchanged prefix that proves capture sources unchanged.
fn causal_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var_os("TYPHON_EFFECT_CAUSAL_TRACE").as_deref() == Some(std::ffi::OsStr::new("1"))
    })
}

pub(super) fn checkpoint_causal_stability_plan_for_owners(
    scene: &SceneRenderState,
    runtime: &EffectRuntime,
    graph: &CompiledFrameGraph,
) -> CheckpointCausalStabilityPlan {
    let mut plan = CheckpointCausalStabilityPlan::default();
    let frame_serial = runtime.effect_resources.checkpoint_frame_serial();
    let cache_baseline = runtime
        .effect_resources
        .checkpoint_causal_state_for_frame(frame_serial);
    let current_scene = scene.current_checkpoint_scene_causal_snapshot.as_ref();
    let scene_damage = scene.current_scene_damage.as_ref();
    let common_prefix_end = cache_baseline
        .zip(current_scene)
        .map(|(baseline, current)| baseline.scene.unchanged_command_prefix_len(current));
    if causal_trace_enabled()
        && let (Some(baseline), Some(current), Some(prefix_end)) =
            (cache_baseline, current_scene, common_prefix_end)
        && prefix_end < current.commands.len()
        && frame_serial % 30 == 0
    {
        eprintln!(
            "typhon effect: event=causal_prefix_diff frame_serial={frame_serial} unchanged_prefix={prefix_end} previous_commands={} current_commands={} difference={} previous_layer={} current_layer={}",
            baseline.scene.commands.len(),
            current.commands.len(),
            baseline.scene.command_difference(current, prefix_end),
            baseline.scene.command_layer_debug(prefix_end),
            current.command_layer_debug(prefix_end),
        );
    }
    let passes_by_id = graph
        .passes
        .iter()
        .map(|pass| (pass.id, pass))
        .collect::<std::collections::HashMap<_, _>>();

    for instance in &graph.instances {
        let scene_captures = graph
            .passes
            .iter()
            .filter(|pass| {
                pass.instance == instance.id && pass.kind == RenderPassKind::SceneCapture
            })
            .collect::<Vec<_>>();
        let has_surface_capture = graph.passes.iter().any(|pass| {
            pass.instance == instance.id && pass.kind == RenderPassKind::SurfaceCapture
        });
        let supported_topology = scene_captures.len() == 1 && !has_surface_capture;
        let previous_instance = cache_baseline.and_then(|state| state.effects.get(&instance.id));
        let primary_capture = scene_captures.first().copied();
        let dependency_ids = primary_capture.and_then(|pass| {
            pass.checkpoint_dependencies
                .iter()
                .map(|dependency| {
                    passes_by_id
                        .get(dependency)
                        .map(|dependency_pass| dependency_pass.instance)
                })
                .collect::<Option<Vec<_>>>()
        });
        let dependency_history_matches = previous_instance
            .zip(dependency_ids.as_ref())
            .is_some_and(|(previous, dependencies)| previous.dependencies == *dependencies);
        let dependency_outputs_unchanged = dependency_history_matches
            && dependency_ids.as_ref().is_some_and(|dependencies| {
                dependencies.iter().all(|dependency| {
                    plan.instances
                        .get(dependency)
                        .is_some_and(|stability| stability.output_unchanged)
                })
            });
        let current_composition_boundary = primary_capture.map(|pass| {
            let (draw_end, _) = composition_range_with_metadata(
                &scene.commands,
                &scene.capture_layers,
                &scene.capture_visual_groups,
                pass.anchor,
                pass.visual_group,
                pass.anchor_scope,
            );
            (draw_end, pass.anchor, pass.visual_group, pass.anchor_scope)
        });
        // Below the boundary either nothing changed, or only surface pixels
        // changed and the frame's scene damage stays outside every pixel the
        // instance samples. Only the instance's output relies on the second
        // proof; cached captures below still require an unchanged prefix.
        let ordinary_prefix_unchanged =
            current_composition_boundary.is_some_and(|(draw_end, _, _, _)| {
                common_prefix_end.is_some_and(|common_prefix_end| draw_end <= common_prefix_end)
                    || scene_damage_misses_unchanged_geometry(
                        cache_baseline,
                        current_scene,
                        scene_damage,
                        draw_end,
                        &instance.capture_region,
                    )
            });
        let composition_boundary_unchanged = previous_instance
            .zip(current_composition_boundary.as_ref())
            .is_some_and(|(previous, current)| {
                previous.composition_boundary.as_ref() == Some(current)
            });
        let capture_owner_unchanged = primary_capture
            .zip(previous_instance)
            .zip(current_scene)
            .is_some_and(|((pass, previous), current)| {
                previous.capture_owner_root
                    == current.presentation_owner_for_visual_group(pass.visual_group)
            });
        let source_unchanged = supported_topology
            && ordinary_prefix_unchanged
            && composition_boundary_unchanged
            && capture_owner_unchanged
            && dependency_outputs_unchanged;
        let semantic_state_unchanged = previous_instance.is_some_and(|previous| {
            previous.causal_backdrop_only
                && supported_topology
                && previous.semantic_signature == instance.semantic_signature
                && previous.frame_demand == instance.frame_demand
                && previous.frame_demand != EffectFrameDemand::Continuous
        });
        let output_unchanged = source_unchanged && semantic_state_unchanged;
        let instance_stability = EffectInstanceCausalStability {
            ordinary_prefix_unchanged,
            dependency_outputs_unchanged,
            source_unchanged,
            output_unchanged,
        };
        plan.instances.insert(instance.id, instance_stability);

        for pass in scene_captures {
            let pass_dependency_ids = pass
                .checkpoint_dependencies
                .iter()
                .map(|dependency| {
                    passes_by_id
                        .get(dependency)
                        .map(|dependency_pass| dependency_pass.instance)
                })
                .collect::<Option<Vec<_>>>();
            let pass_dependency_history_matches = previous_instance
                .zip(pass_dependency_ids.as_ref())
                .is_some_and(|(previous, dependencies)| previous.dependencies == *dependencies);
            let pass_dependencies_unchanged = pass_dependency_history_matches
                && pass_dependency_ids.as_ref().is_some_and(|dependencies| {
                    dependencies.iter().all(|dependency| {
                        plan.instances
                            .get(dependency)
                            .is_some_and(|stability| stability.output_unchanged)
                    })
                });
            let (draw_end, _) = composition_range_with_metadata(
                &scene.commands,
                &scene.capture_layers,
                &scene.capture_visual_groups,
                pass.anchor,
                pass.visual_group,
                pass.anchor_scope,
            );
            let pass_prefix_unchanged =
                common_prefix_end.is_some_and(|common_prefix_end| draw_end <= common_prefix_end);
            let pass_composition_boundary =
                (draw_end, pass.anchor, pass.visual_group, pass.anchor_scope);
            let pass_composition_boundary_unchanged = previous_instance.is_some_and(|previous| {
                previous.composition_boundary == Some(pass_composition_boundary)
            });
            let pass_owner_unchanged =
                previous_instance
                    .zip(current_scene)
                    .is_some_and(|(previous, current)| {
                        previous.capture_owner_root
                            == current.presentation_owner_for_visual_group(pass.visual_group)
                    });
            let pass_source_unchanged = supported_topology
                && pass_prefix_unchanged
                && pass_composition_boundary_unchanged
                && pass_owner_unchanged
                && pass_dependencies_unchanged;
            let unproven_reason = if pass_source_unchanged {
                None
            } else if cache_baseline.is_none() || current_scene.is_none() {
                Some(CheckpointCausalUnprovenReason::NoCacheBaseline)
            } else if !pass_prefix_unchanged
                || !pass_composition_boundary_unchanged
                || !pass_owner_unchanged
            {
                Some(CheckpointCausalUnprovenReason::ScenePrefixChanged)
            } else if !pass_dependencies_unchanged {
                Some(CheckpointCausalUnprovenReason::DependencyChanged)
            } else {
                Some(CheckpointCausalUnprovenReason::UnsupportedTopology)
            };
            plan.captures.insert(
                pass.id,
                CheckpointCaptureCausalStability {
                    ordinary_prefix_unchanged: pass_prefix_unchanged,
                    dependency_outputs_unchanged: pass_dependencies_unchanged,
                    source_unchanged: pass_source_unchanged,
                    unproven_reason,
                },
            );
        }
    }
    plan
}

/// Proves the scene below `draw_end` unchanged inside `capture_region` when
/// the commands differ only in surface pixels and the frame's scene damage,
/// which covers every changed surface pixel since the last presented frame
/// (and so since the baseline frame), misses the sampled region.
fn scene_damage_misses_unchanged_geometry(
    baseline: Option<&crate::egl_renderer::checkpoint::CheckpointCausalState>,
    current: Option<&crate::egl_renderer::checkpoint::EglCheckpointSceneCausalSnapshot>,
    scene_damage: Option<&EffectRegion>,
    draw_end: usize,
    capture_region: &EffectRegion,
) -> bool {
    let (Some(baseline), Some(current), Some(scene_damage)) = (baseline, current, scene_damage)
    else {
        return false;
    };
    !capture_region.is_empty()
        && !scene_damage.intersects(capture_region)
        && baseline
            .scene
            .only_surface_content_changed_before(current, draw_end)
}

#[cfg(test)]
pub(crate) fn checkpoint_causal_stability_plan(
    renderer: &super::super::super::GlesSceneRenderer,
    graph: &CompiledFrameGraph,
) -> CheckpointCausalStabilityPlan {
    checkpoint_causal_stability_plan_for_owners(
        &renderer.scene_state,
        &renderer.effect_runtime,
        graph,
    )
}

pub(super) fn checkpoint_update_materialization_plan(
    frame_damage: &EffectRegion,
    capture_domain: oblivion_one::effects::EffectRect,
    output_size: (u32, u32),
) -> CaptureMaterializationPlan {
    let clipped = frame_damage.intersect_rect(capture_domain);
    if clipped.is_empty() {
        return CaptureMaterializationPlan {
            region: EffectRegion::empty(),
            output_rects: Vec::new(),
        };
    }
    let disjoint = clipped.disjoint_bounded();
    let region = if disjoint.overflowed {
        EffectRegion::from_rect(capture_domain)
    } else {
        disjoint.region
    };
    let output_rects = effect_capture_output_rects(&region, Some(capture_domain), output_size);
    CaptureMaterializationPlan {
        region,
        output_rects,
    }
}

#[cfg(test)]
pub(crate) fn checkpoint_update_rects_for_test(
    frame_damage: &EffectRegion,
    capture_domain: oblivion_one::effects::EffectRect,
    output_size: (u32, u32),
    target: &oblivion_one::effects::GraphTexturePlan,
) -> (Vec<OutputRect>, Vec<OutputRect>) {
    let plan = checkpoint_update_materialization_plan(frame_damage, capture_domain, output_size);
    let texture_rects = plan.texture_rects(target);
    (plan.output_rects, texture_rects)
}

pub(super) fn effect_rect_to_texture_rect(
    rect: oblivion_one::effects::EffectRect,
    target: &oblivion_one::effects::GraphTexturePlan,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<OutputRect> {
    let coverage = logical_rect_to_physical_coverage(rect, target)?;
    let width = coverage.width();
    let height = coverage.height();
    let y = match target.origin {
        oblivion_one::effects::GraphTextureOrigin::BottomLeft => match target.source {
            GraphTextureSource::Output => match framebuffer_origin {
                OutputFramebufferOrigin::BottomLeft => {
                    target.height.saturating_sub(coverage.bottom)
                }
                OutputFramebufferOrigin::TopLeftScanout => coverage.top,
            },
            _ => target.height.saturating_sub(coverage.bottom),
        },
    };
    Some(OutputRect::new(
        i32::try_from(coverage.left).ok()?,
        i32::try_from(y).ok()?,
        width as u32,
        height as u32,
    ))
}
