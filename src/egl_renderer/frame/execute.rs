use std::collections::HashMap;

use glow::HasContext;
use oblivion_one::effects::{
    CompiledFrameGraph, CompiledPrecomputedBackdropPlan, GraphTextureId, RenderPassKind,
    compile_precomputed_backdrop_build,
};

use super::super::checkpoint::CheckpointEffectCausalState;
use super::super::{
    CheckpointCausalState, EffectExecutionContext, EffectFailureReason, LifecycleRenderContext,
    RendererResult,
    damage::{OutputRect, RenderExecution, RepaintPlan},
    effects::{
        EffectDebugConfig, EffectExecutionSelection, EffectExecutionStats, EffectTextureFilter,
        EffectTextureFormat, EffectTextureKey, GraphTextureBinding, PooledEffectTexture,
        SceneReplayWorkMode, composition_range_with_metadata,
    },
    scene_state::{SceneRenderState, SceneTextureSources},
};
use super::{
    pipeline::FramePipeline,
    planning::PlannedFrame,
    telemetry,
    types::{EglFrameOutcome, EglSceneDrawRequest, EglSceneFrameCommit},
};

#[derive(Default)]
struct PrecomputedWindowBlurFrameTelemetry {
    consumers: usize,
    built: bool,
    fallback: bool,
    fallback_reason: &'static str,
    build_stats: Option<EffectExecutionStats>,
}

impl FramePipeline<'_> {
    pub(super) fn execute_frame(
        &mut self,
        request: &EglSceneDrawRequest<'_>,
        planned: &PlannedFrame,
        framebuffer_origin: super::super::OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        let compiled_graph = match &planned.execution_plan {
            oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) => Some(graph),
            oblivion_one::effects::FrameExecutionPlan::LegacyScene => None,
        };
        self.effects.effect_trace.frame_boundary(
            "renderer_draw_complete",
            "begin",
            telemetry::effect_trace_summary(
                self.scene,
                request.effects,
                Some(&planned.plan),
                compiled_graph,
                planned.selected_effect_count,
            ),
        );
        let mut precomputed_telemetry = PrecomputedWindowBlurFrameTelemetry {
            fallback_reason: "none",
            ..PrecomputedWindowBlurFrameTelemetry::default()
        };
        let draw_result = match &planned.execution_plan {
            oblivion_one::effects::FrameExecutionPlan::LegacyScene => {
                self.draw_textured_layers(&planned.plan, framebuffer_origin)
            }
            oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) => {
                let demand = planned
                    .effect_execution_demand
                    .as_ref()
                    .expect("effect graph execution must have an execution demand");
                let selection = planned
                    .effect_selection
                    .as_ref()
                    .expect("effect graph execution must have an execution selection");
                let selected_precomputed_consumers = planned
                    .precomputed_backdrop
                    .as_ref()
                    .map(|precomputed| {
                        precomputed
                            .consumers
                            .iter()
                            .filter(|consumer| {
                                selection.executed_instances.contains(&consumer.instance)
                            })
                            .count()
                    })
                    .unwrap_or(0);
                precomputed_telemetry.consumers = selected_precomputed_consumers;
                let execution_result = if selected_precomputed_consumers == 0 {
                    self.execute_effect_graph_with_overlays(
                        graph,
                        framebuffer_origin,
                        &planned.plan,
                        demand,
                        selection,
                    )
                } else {
                    let precomputed = planned
                        .precomputed_backdrop
                        .as_ref()
                        .expect("selected optimized consumers require a shared plan");
                    let preparation = self
                        .prepare_precomputed_backdrop(precomputed, framebuffer_origin)
                        .map_err(|_| ());
                    dispatch_shared_preparation(preparation, |outcome| match outcome {
                        SharedPreparationOutcome::Ready((shared_texture, build_stats)) => {
                            precomputed_telemetry.built = true;
                            precomputed_telemetry.build_stats = Some(build_stats);
                            if self
                                .effects
                                .effect_resources
                                .prepare_graph(self.gl, graph)
                                .is_err()
                            {
                                precomputed_telemetry.fallback = true;
                                precomputed_telemetry.fallback_reason =
                                    "optimized_preflight_failed";
                                let release = self.effects.effect_resources.release(shared_texture);
                                if let Err(error) = release {
                                    return Err(error.into());
                                }
                                let exact_graph = match planned.exact_execution_plan.as_ref() {
                                    Some(
                                        oblivion_one::effects::FrameExecutionPlan::EffectGraph(
                                            graph,
                                        ),
                                    ) => graph,
                                    _ => {
                                        return Err(std::io::Error::other(
                                            "precomputed fallback has no exact graph",
                                        )
                                        .into());
                                    }
                                };
                                let exact_demand = planned
                                    .exact_effect_execution_demand
                                    .as_ref()
                                    .expect("precomputed fallback requires exact demand");
                                let exact_selection = planned
                                    .exact_effect_selection
                                    .as_ref()
                                    .expect("precomputed fallback requires exact selection");
                                self.execute_effect_graph_with_overlays(
                                    exact_graph,
                                    framebuffer_origin,
                                    &planned.plan,
                                    exact_demand,
                                    exact_selection,
                                )
                            } else {
                                let optimized = self
                                    .execute_effect_graph_with_precomputed_backdrop(
                                        graph,
                                        precomputed,
                                        &shared_texture,
                                        framebuffer_origin,
                                        &planned.plan,
                                        demand,
                                        selection,
                                    );
                                let release = self.effects.effect_resources.release(shared_texture);
                                match (optimized, release) {
                                    (Ok(stats), Ok(())) => Ok(stats),
                                    (Err(error), _) => Err(error),
                                    (Ok(_), Err(error)) => Err(error.into()),
                                }
                            }
                        }
                        SharedPreparationOutcome::Failed => {
                            precomputed_telemetry.fallback = true;
                            precomputed_telemetry.fallback_reason = "shared_prepare_failed";
                            let exact_graph = match planned.exact_execution_plan.as_ref() {
                                Some(oblivion_one::effects::FrameExecutionPlan::EffectGraph(
                                    graph,
                                )) => graph,
                                _ => {
                                    return Err(std::io::Error::other(
                                        "precomputed fallback has no exact graph",
                                    )
                                    .into());
                                }
                            };
                            let exact_demand = planned
                                .exact_effect_execution_demand
                                .as_ref()
                                .expect("precomputed fallback requires exact demand");
                            let exact_selection = planned
                                .exact_effect_selection
                                .as_ref()
                                .expect("precomputed fallback requires exact selection");
                            self.execute_effect_graph_with_overlays(
                                exact_graph,
                                framebuffer_origin,
                                &planned.plan,
                                exact_demand,
                                exact_selection,
                            )
                        }
                    })
                };
                self.emit_precomputed_window_blur_telemetry(planned, &precomputed_telemetry);
                match execution_result {
                    Ok(execution_stats) => {
                        self.scene.frame_stats.effect_instances_executed =
                            execution_stats.instances;
                        self.scene.frame_stats.effect_passes_executed = execution_stats.passes;
                        self.scene.frame_stats.scene_replay_work_overflow_fallbacks =
                            execution_stats.scene_replay_work_overflow_fallbacks;
                        self.scene.frame_stats.blur_downsample_passes =
                            execution_stats.blur_downsamples;
                        self.scene.frame_stats.blur_upsample_passes =
                            execution_stats.blur_upsamples;
                        self.scene.frame_stats.effect_capture_pixels_executed =
                            execution_stats.capture_execution_pixels;
                        self.scene.frame_stats.effect_resource_acquisitions =
                            execution_stats.resource_acquisitions;
                        if let Some(build_stats) = &precomputed_telemetry.build_stats {
                            self.scene.frame_stats.effect_passes_executed = self
                                .scene
                                .frame_stats
                                .effect_passes_executed
                                .saturating_add(build_stats.passes);
                            self.scene.frame_stats.blur_downsample_passes = self
                                .scene
                                .frame_stats
                                .blur_downsample_passes
                                .saturating_add(build_stats.blur_downsamples);
                            self.scene.frame_stats.blur_upsample_passes = self
                                .scene
                                .frame_stats
                                .blur_upsample_passes
                                .saturating_add(build_stats.blur_upsamples);
                            self.scene.frame_stats.effect_capture_pixels_executed = self
                                .scene
                                .frame_stats
                                .effect_capture_pixels_executed
                                .saturating_add(build_stats.capture_execution_pixels);
                            self.scene.frame_stats.effect_resource_acquisitions = self
                                .scene
                                .frame_stats
                                .effect_resource_acquisitions
                                .saturating_add(build_stats.resource_acquisitions);
                        }
                        Ok(())
                    }
                    Err(error) => {
                        self.effects
                            .effect_resources
                            .invalidate_checkpoint_capture_contents();
                        self.scene.frame_stats.effect_fallbacks =
                            self.scene.frame_stats.effect_fallbacks.saturating_add(1);
                        self.scene.frame_stats.effect_instances_failed =
                            self.scene.frame_stats.effect_instances_visible;
                        self.scene.frame_stats.effect_failure_reason =
                            Some(EffectFailureReason::from_error(error.as_ref()));
                        if self.scene.frame_stats.effect_failure_reason
                            == Some(EffectFailureReason::ShaderUnavailable)
                        {
                            self.effects.failed_effect_generation =
                                Some(self.effects.effect_registry_generation);
                        }
                        self.draw_textured_layers(&planned.plan, framebuffer_origin)
                    }
                }
            }
        };
        if matches!(
            &planned.execution_plan,
            oblivion_one::effects::FrameExecutionPlan::LegacyScene
        ) {
            self.emit_precomputed_window_blur_telemetry(planned, &precomputed_telemetry);
        }
        if let Err(error) = draw_result {
            self.effects.effect_trace.frame_boundary(
                "renderer_draw_complete",
                "end",
                telemetry::effect_trace_summary(
                    self.scene,
                    request.effects,
                    Some(&planned.plan),
                    compiled_graph,
                    planned.selected_effect_count,
                ),
            );
            self.scene.repaint_planner.invalidate();
            return Err(error);
        }
        self.effects.effect_trace.frame_boundary(
            "renderer_draw_complete",
            "end",
            telemetry::effect_trace_summary(
                self.scene,
                request.effects,
                Some(&planned.plan),
                compiled_graph,
                planned.selected_effect_count,
            ),
        );
        Ok(())
    }

    pub(super) fn settle_frame(&mut self, planned: PlannedFrame) -> EglFrameOutcome {
        self.lifecycle.record_missing_evidence_fallbacks(
            f64::from(self.effects.effect_output_scale),
            self.scene.current_size,
        );
        let frame_survived = !self.lifecycle.has_fallbacks();
        self.lifecycle.trace_frame_events(frame_survived);
        if self.lifecycle.has_fallbacks() {
            self.scene.repaint_planner.invalidate();
            return EglFrameOutcome::LifecycleFallback {
                stats: self.scene.frame_stats,
                fallbacks: self.lifecycle.fallbacks(),
            };
        }
        telemetry::record_effect_resource_metrics(self.scene, self.effects);
        telemetry::record_repaint_stats(self.scene, &planned.plan);
        EglFrameOutcome::Rendered {
            commit: EglSceneFrameCommit {
                repaint_plan: planned.plan,
                damage_state: planned.damage_state,
                scene_key: planned.candidate_scene_key,
            },
            stats: self.scene.frame_stats,
            lifecycle_evidence: self.lifecycle.evidence(),
        }
    }

    pub(super) fn execute_effect_graph_with_overlays(
        &mut self,
        graph: &CompiledFrameGraph,
        framebuffer_origin: super::super::OutputFramebufferOrigin,
        repaint_plan: &RepaintPlan,
        demand: &oblivion_one::effects::EffectExecutionDemand,
        selection: &EffectExecutionSelection,
    ) -> RendererResult<EffectExecutionStats> {
        self.execute_effect_graph_with_overlays_config(
            graph,
            framebuffer_origin,
            repaint_plan,
            demand,
            selection,
            *super::super::effects::effect_debug_config(),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::egl_renderer) fn execute_effect_graph_with_overlays_config(
        &mut self,
        graph: &CompiledFrameGraph,
        framebuffer_origin: super::super::OutputFramebufferOrigin,
        repaint_plan: &RepaintPlan,
        demand: &oblivion_one::effects::EffectExecutionDemand,
        selection: &EffectExecutionSelection,
        debug_config: EffectDebugConfig,
        scene_replay_work_mode_override: Option<SceneReplayWorkMode>,
    ) -> RendererResult<EffectExecutionStats> {
        self.execute_effect_graph_with_overlays_config_and_bindings(
            graph,
            framebuffer_origin,
            repaint_plan,
            demand,
            selection,
            debug_config,
            scene_replay_work_mode_override,
            HashMap::new(),
        )
    }

    fn execute_effect_graph_with_overlays_config_and_bindings(
        &mut self,
        graph: &CompiledFrameGraph,
        framebuffer_origin: super::super::OutputFramebufferOrigin,
        repaint_plan: &RepaintPlan,
        demand: &oblivion_one::effects::EffectExecutionDemand,
        selection: &EffectExecutionSelection,
        debug_config: EffectDebugConfig,
        scene_replay_work_mode_override: Option<SceneReplayWorkMode>,
        external_bindings: HashMap<GraphTextureId, GraphTextureBinding>,
    ) -> RendererResult<EffectExecutionStats> {
        let mut prepared = {
            let mut context = self.effect_execution_context();
            super::super::effects::prepare_effect_graph_execution_with_external_bindings(
                &mut context,
                graph,
                framebuffer_origin,
                repaint_plan,
                demand,
                selection,
                debug_config,
                scene_replay_work_mode_override,
                external_bindings,
            )?
        };
        let promotes_checkpoint_cache = prepared.promotes_checkpoint_cache();
        let composition_target = prepared.composition_target();
        let mut result = {
            let mut context = self.effect_execution_context();
            super::super::effects::execute_prepared_effect_graph_core(&mut context, &mut prepared)
        };
        if result.is_ok() {
            let overlay_result = (|| {
                if self.effects.effect_trace.enabled() {
                    self.effects.effect_trace.overlay_boundary("begin");
                }
                self.draw_lifecycle_overlays(
                    prepared.overlay_rects(),
                    framebuffer_origin,
                    repaint_plan,
                )?;
                self.draw_effect_overlays(prepared.overlay_rects(), framebuffer_origin)?;
                if self.effects.effect_trace.enabled() {
                    self.effects.effect_trace.overlay_boundary("end");
                }
                self.scene
                    .establish_scene_state_for_framebuffer(self.gl, composition_target.framebuffer);
                Ok(())
            })();
            if let Err(error) = overlay_result {
                result = Err(error);
            }
        }
        let result = {
            let mut context = self.effect_execution_context();
            super::super::effects::finish_prepared_effect_graph_execution(
                &mut context,
                prepared,
                result,
            )
        };
        if result.is_ok() && promotes_checkpoint_cache {
            promote_checkpoint_cache_causal_state(self.scene, self.effects, graph);
        }
        result
    }

    fn execute_effect_graph_with_precomputed_backdrop(
        &mut self,
        graph: &CompiledFrameGraph,
        precomputed: &CompiledPrecomputedBackdropPlan,
        shared_texture: &PooledEffectTexture,
        framebuffer_origin: super::super::OutputFramebufferOrigin,
        repaint_plan: &RepaintPlan,
        demand: &oblivion_one::effects::EffectExecutionDemand,
        selection: &EffectExecutionSelection,
    ) -> RendererResult<EffectExecutionStats> {
        let bindings = precomputed
            .consumers
            .iter()
            .filter(|consumer| selection.executed_instances.contains(&consumer.instance))
            .map(|consumer| {
                (
                    consumer.texture,
                    GraphTextureBinding::precomputed_backdrop_alias(shared_texture.clone()),
                )
            })
            .collect();
        self.execute_effect_graph_with_overlays_config_and_bindings(
            graph,
            framebuffer_origin,
            repaint_plan,
            demand,
            selection,
            *super::super::effects::effect_debug_config(),
            None,
            bindings,
        )
    }

    fn prepare_precomputed_backdrop(
        &mut self,
        plan: &CompiledPrecomputedBackdropPlan,
        framebuffer_origin: super::super::OutputFramebufferOrigin,
    ) -> RendererResult<(PooledEffectTexture, EffectExecutionStats)> {
        let size = self.scene.current_size;
        let output_bounds = oblivion_one::effects::EffectRect::new(0, 0, size.0, size.1)
            .ok_or_else(|| std::io::Error::other("invalid precomputed backdrop output size"))?;
        let build = compile_precomputed_backdrop_build(plan, output_bounds);
        let capture_key = EffectTextureKey::new(
            build.capture.width,
            build.capture.height,
            EffectTextureFormat::Rgba8,
            EffectTextureFilter::Linear,
            build.capture.working_space,
        );
        let capture = self
            .effects
            .effect_resources
            .acquire(self.gl, capture_key)?;
        let mut context = self.effect_execution_context();
        let timing_scope = match context.replay_precomputed_backdrop_capture(
            &capture,
            plan.baseline_anchor,
            plan.baseline_visual_group,
            plan.baseline_anchor_scope,
            framebuffer_origin,
        ) {
            Ok(scope) => scope,
            Err(error) => {
                drop(context);
                let release = self.effects.effect_resources.release(capture);
                return match release {
                    Ok(()) => Err(error),
                    Err(release_error) => Err(release_error.into()),
                };
            }
        };
        let build_result = super::super::effects::execute_precomputed_backdrop_build(
            &mut context,
            &build,
            capture,
            timing_scope,
            framebuffer_origin,
        );
        match build_result {
            Ok((stats, texture)) => Ok((texture, stats)),
            Err(error) => Err(error),
        }
    }

    fn effect_execution_context(&mut self) -> EffectExecutionContext<'_> {
        let texture_sources: SceneTextureSources<'_> = self
            .lifecycle
            .texture_sources(self.resources.texture_view());
        EffectExecutionContext::new(self.gl, self.scene, self.effects, texture_sources)
    }

    fn emit_precomputed_window_blur_telemetry(
        &self,
        planned: &PlannedFrame,
        state: &PrecomputedWindowBlurFrameTelemetry,
    ) {
        if !self.effects.effect_gpu_profiler.cache_telemetry_enabled() {
            return;
        }
        let policy = self.effects.precomputed_window_blur_policy.as_str();
        let eligible = planned.precomputed_eligible_instances;
        let baseline = planned.precomputed_backdrop.as_ref().map_or_else(
            || "none".to_owned(),
            |plan| plan.baseline_instance.get().to_string(),
        );
        let build_passes = if state.built {
            planned
                .precomputed_backdrop
                .as_ref()
                .map_or(0, |plan| plan.build_passes)
        } else {
            0
        };
        let frame_id = self.effects.effect_trace.frame_id().unwrap_or_default();
        let (capture_pixels, downsample_pixels, upsample_pixels) = if state.built {
            planned
                .precomputed_backdrop
                .as_ref()
                .map(|plan| precomputed_blur_pixel_work(plan, self.scene.current_size))
                .unwrap_or_default()
        } else {
            (0, 0, 0)
        };
        let (
            saved_captures,
            saved_downsamples,
            saved_upsamples,
            saved_capture_pixels,
            saved_downsample_pixels,
            saved_upsample_pixels,
        ) = if state.built && !state.fallback {
            planned
                .precomputed_backdrop
                .as_ref()
                .and_then(|precomputed| {
                    let graph = match planned.exact_execution_plan.as_ref()? {
                        oblivion_one::effects::FrameExecutionPlan::EffectGraph(graph) => graph,
                        oblivion_one::effects::FrameExecutionPlan::LegacyScene => return None,
                    };
                    let selected = precomputed
                        .consumers
                        .iter()
                        .filter(|consumer| {
                            planned.effect_selection.as_ref().is_some_and(|selection| {
                                selection.executed_instances.contains(&consumer.instance)
                            })
                        })
                        .map(|consumer| consumer.instance)
                        .collect::<Vec<_>>();
                    let exact_captures =
                        graph_pass_work(graph, &selected, RenderPassKind::SceneCapture);
                    let exact_downsamples =
                        graph_pass_work(graph, &selected, RenderPassKind::DualKawaseDownsample);
                    let exact_upsamples =
                        graph_pass_work(graph, &selected, RenderPassKind::DualKawaseUpsample);
                    let (shared_capture, shared_down, shared_up) =
                        precomputed_blur_pixel_work(precomputed, self.scene.current_size);
                    let shared_down_count = usize::from(precomputed.blur.passes);
                    Some((
                        exact_captures.0.saturating_sub(1),
                        exact_downsamples.0.saturating_sub(shared_down_count),
                        exact_upsamples.0.saturating_sub(shared_down_count),
                        exact_captures.1.saturating_sub(shared_capture),
                        exact_downsamples.1.saturating_sub(shared_down),
                        exact_upsamples.1.saturating_sub(shared_up),
                    ))
                })
                .unwrap_or_default()
        } else {
            (0, 0, 0, 0, 0, 0)
        };
        eprintln!(
            "typhon effect: event=precomputed_window_blur precomputed_window_blur_frame_id={} precomputed_window_blur_policy={} precomputed_window_blur_eligible_instances={} precomputed_window_blur_consumers={} precomputed_window_blur_built={} precomputed_window_blur_fallback={} precomputed_window_blur_fallback_reason={} precomputed_window_blur_baseline_instance={} precomputed_window_blur_build_passes={} precomputed_window_blur_saved_scene_captures={} precomputed_window_blur_saved_kawase_downsamples={} precomputed_window_blur_saved_kawase_upsamples={} precomputed_window_blur_capture_pixels={} precomputed_window_blur_downsample_pixels={} precomputed_window_blur_upsample_pixels={} precomputed_window_blur_saved_capture_pixels={} precomputed_window_blur_saved_downsample_pixels={} precomputed_window_blur_saved_upsample_pixels={}",
            frame_id,
            policy,
            eligible,
            state.consumers,
            state.built,
            state.fallback,
            state.fallback_reason,
            baseline,
            build_passes,
            saved_captures,
            saved_downsamples,
            saved_upsamples,
            capture_pixels,
            downsample_pixels,
            upsample_pixels,
            saved_capture_pixels,
            saved_downsample_pixels,
            saved_upsample_pixels,
        );
    }

    fn draw_textured_layers(
        &mut self,
        plan: &RepaintPlan,
        framebuffer_origin: super::super::OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        self.scene
            .establish_scene_state_for_framebuffer(self.gl, self.scene.active_output_framebuffer);
        unsafe { self.gl.clear_color(0.0, 0.0, 0.0, 1.0) };

        let execution = plan
            .render_execution(
                self.scene.current_size.0,
                self.scene.current_size.1,
                framebuffer_origin,
            )
            .ok_or_else(|| std::io::Error::other("repaint execution conversion failed"))?;
        let region_plan_valid = self
            .scene
            .scene_region_plan
            .is_executable_for(&self.scene.commands);
        let region_execution_requested = self.scene.frame_stats.base_region_legacy_mode_frames == 0
            && self.scene.scene_region_plan.fallback.is_none();
        if region_execution_requested && !region_plan_valid {
            self.scene.frame_stats.base_region_legacy_fallbacks = self
                .scene
                .frame_stats
                .base_region_legacy_fallbacks
                .saturating_add(1);
        }
        let use_region_execution = region_execution_requested
            && region_plan_valid
            && !self.scene.scene_region_plan.repairs.is_empty();
        if use_region_execution {
            let repair_count = self.scene.scene_region_plan.repairs.len();
            for repair_index in 0..repair_count {
                let repair = self.scene.scene_region_plan.repairs[repair_index];
                self.scene.set_scene_scissor(self.gl, Some(repair));
                unsafe {
                    self.gl.clear(glow::COLOR_BUFFER_BIT);
                }
            }
            self.scene.set_scene_scissor(self.gl, None);
            self.effect_execution_context().draw_scene_region_plan()?;

            // Lifecycle source capture observes the complete repaired base
            // scene, matching the legacy phase boundary.
            self.prepare_lifecycle_visual_sources(plan, framebuffer_origin)?;
            for repair_index in 0..repair_count {
                let repair = self.scene.scene_region_plan.repairs[repair_index];
                self.draw_lamp_overlay(Some(repair))?;
                self.draw_squash_overlay(Some(repair))?;
                self.draw_command_batch(false, Some(repair))?;
            }
            self.scene.frame_stats.scissor_passes = repair_count;
        } else {
            match execution {
                RenderExecution::Full => {
                    unsafe {
                        self.gl.disable(glow::SCISSOR_TEST);
                        self.gl.clear(glow::COLOR_BUFFER_BIT);
                    }
                    self.draw_command_batch(true, None)?;
                    self.prepare_lifecycle_visual_sources(plan, framebuffer_origin)?;
                    self.draw_lamp_overlay(None)?;
                    self.draw_squash_overlay(None)?;
                    self.draw_command_batch(false, None)?;
                }
                RenderExecution::Scissored {
                    scissors,
                    disable_scissor_after,
                } => {
                    unsafe {
                        self.gl.enable(glow::SCISSOR_TEST);
                    }
                    let mut draw_result = Ok(());
                    for phase in legacy_scene_scissored_phase_plan(scissors.len()) {
                        if draw_result.is_err() {
                            break;
                        }
                        match phase {
                            LegacySceneScissoredPhase::BaseRepair(index) => {
                                let [x, y, width, height] = scissors[index];
                                unsafe {
                                    self.gl.scissor(x, y, width, height);
                                    self.gl.clear(glow::COLOR_BUFFER_BIT);
                                }
                                let output_rect = super::super::gl_scissor_to_output_rect(
                                    [x, y, width, height],
                                    self.scene.current_size.1,
                                    framebuffer_origin,
                                );
                                draw_result = self.draw_command_batch(true, output_rect);
                            }
                            LegacySceneScissoredPhase::PrepareLifecycleSources => {
                                draw_result =
                                    self.prepare_lifecycle_visual_sources(plan, framebuffer_origin);
                            }
                            LegacySceneScissoredPhase::RestoreRepairScissor(index) => {
                                let [x, y, width, height] = scissors[index];
                                unsafe {
                                    self.gl.enable(glow::SCISSOR_TEST);
                                    self.gl.scissor(x, y, width, height);
                                }
                            }
                            LegacySceneScissoredPhase::Lamp(index) => {
                                let [x, y, width, height] = scissors[index];
                                let output_rect = super::super::gl_scissor_to_output_rect(
                                    [x, y, width, height],
                                    self.scene.current_size.1,
                                    framebuffer_origin,
                                );
                                draw_result = self.draw_lamp_overlay(output_rect);
                            }
                            LegacySceneScissoredPhase::Squash(index) => {
                                let [x, y, width, height] = scissors[index];
                                let output_rect = super::super::gl_scissor_to_output_rect(
                                    [x, y, width, height],
                                    self.scene.current_size.1,
                                    framebuffer_origin,
                                );
                                draw_result = self.draw_squash_overlay(output_rect);
                            }
                            LegacySceneScissoredPhase::ExternalOverlays(index) => {
                                let [x, y, width, height] = scissors[index];
                                let output_rect = super::super::gl_scissor_to_output_rect(
                                    [x, y, width, height],
                                    self.scene.current_size.1,
                                    framebuffer_origin,
                                );
                                draw_result = self.draw_command_batch(false, output_rect);
                            }
                        }
                    }
                    if disable_scissor_after {
                        unsafe {
                            self.gl.disable(glow::SCISSOR_TEST);
                        }
                    }
                    draw_result?;
                    self.scene.frame_stats.scissor_passes = scissors.len();
                }
            }
        }

        unsafe {
            self.gl.disable(glow::SCISSOR_TEST);
            self.gl.bind_texture(glow::TEXTURE_2D, None);
        }
        Ok(())
    }

    fn prepare_lifecycle_visual_sources(
        &mut self,
        plan: &RepaintPlan,
        framebuffer_origin: super::super::OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        let Self {
            gl,
            scene,
            effects,
            resources,
            lifecycle,
            ..
        } = self;
        let mut context = LifecycleRenderContext::new(gl, scene, effects, resources.texture_view());
        lifecycle.prepare_visual_sources(&mut context, plan, framebuffer_origin)
    }

    fn draw_lamp_overlay(&mut self, scissor: Option<OutputRect>) -> RendererResult<()> {
        let Self {
            gl,
            scene,
            effects,
            resources,
            lifecycle,
            ..
        } = self;
        let mut context = LifecycleRenderContext::new(gl, scene, effects, resources.texture_view());
        lifecycle.draw_lamp_overlay(&mut context, scissor)
    }

    fn draw_squash_overlay(&mut self, scissor: Option<OutputRect>) -> RendererResult<()> {
        let Self {
            gl,
            scene,
            effects,
            resources,
            lifecycle,
            ..
        } = self;
        let mut context = LifecycleRenderContext::new(gl, scene, effects, resources.texture_view());
        lifecycle.draw_squash_overlay(&mut context, scissor)
    }

    fn draw_lifecycle_overlays(
        &mut self,
        rects: &[OutputRect],
        framebuffer_origin: super::super::OutputFramebufferOrigin,
        plan: &RepaintPlan,
    ) -> RendererResult<()> {
        let Self {
            gl,
            scene,
            effects,
            resources,
            lifecycle,
            ..
        } = self;
        let mut context = LifecycleRenderContext::new(gl, scene, effects, resources.texture_view());
        lifecycle.draw_overlays(&mut context, rects, framebuffer_origin, plan)
    }

    fn draw_effect_overlays(
        &mut self,
        rects: &[OutputRect],
        framebuffer_origin: super::super::OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        self.effect_execution_context()
            .draw_effect_overlays(rects, framebuffer_origin)
    }

    fn draw_command_batch(
        &mut self,
        scene: bool,
        scissor: Option<OutputRect>,
    ) -> RendererResult<()> {
        self.effect_execution_context()
            .draw_command_batch(scene, scissor)
    }
}

enum SharedPreparationOutcome<T> {
    Ready(T),
    Failed,
}

fn dispatch_shared_preparation<T, R>(
    result: Result<T, ()>,
    dispatch: impl FnOnce(SharedPreparationOutcome<T>) -> R,
) -> R {
    dispatch(match result {
        Ok(value) => SharedPreparationOutcome::Ready(value),
        Err(()) => SharedPreparationOutcome::Failed,
    })
}

fn graph_pass_work(
    graph: &CompiledFrameGraph,
    owners: &[oblivion_one::effects::EffectInstanceId],
    kind: RenderPassKind,
) -> (usize, u64) {
    graph
        .passes
        .iter()
        .filter(|pass| owners.contains(&pass.instance) && pass.kind == kind)
        .fold((0, 0), |(count, pixels), pass| {
            let pass_pixels = pass
                .output
                .and_then(|output| graph.textures.iter().find(|texture| texture.id == output))
                .map_or(0, |texture| {
                    u64::from(texture.width).saturating_mul(u64::from(texture.height))
                });
            (count.saturating_add(1), pixels.saturating_add(pass_pixels))
        })
}

fn precomputed_blur_pixel_work(
    plan: &CompiledPrecomputedBackdropPlan,
    size: (u32, u32),
) -> (u64, u64, u64) {
    let pixels = |width: u32, height: u32| u64::from(width).saturating_mul(u64::from(height));
    let processing_width = (f64::from(size.0) * f64::from(plan.blur.scale))
        .ceil()
        .max(1.0) as u32;
    let processing_height = (f64::from(size.1) * f64::from(plan.blur.scale))
        .ceil()
        .max(1.0) as u32;
    let ceil_div =
        |value: u32, divisor: u32| value.saturating_add(divisor.saturating_sub(1)) / divisor.max(1);
    let mut downsample = 0_u64;
    for level in 1..=plan.blur.passes {
        let divisor = 1_u32 << level.min(31);
        downsample = downsample.saturating_add(pixels(
            ceil_div(processing_width, divisor),
            ceil_div(processing_height, divisor),
        ));
    }
    let mut upsample = 0_u64;
    for level in (0..plan.blur.passes).rev() {
        let divisor = 1_u32 << level.min(31);
        upsample = upsample.saturating_add(pixels(
            ceil_div(processing_width, divisor),
            ceil_div(processing_height, divisor),
        ));
    }
    (pixels(size.0, size.1), downsample, upsample)
}

#[cfg(test)]
mod shared_preparation_tests {
    use super::*;

    #[test]
    fn shared_prepare_failure_selects_exact_before_optimized_output_mutation() {
        let mut events = Vec::new();
        let mut temporary_release_count = 0;
        let mut optimized_output_mutated = false;
        let mut exact_executed = false;

        events.push("precomputed_candidate_selected");
        events.push("shared_prepare_started");
        let preparation = Err::<(), ()>(());
        dispatch_shared_preparation(preparation, |outcome| match outcome {
            SharedPreparationOutcome::Ready(()) => {
                optimized_output_mutated = true;
                events.push("optimized_output_mutated");
            }
            SharedPreparationOutcome::Failed => {
                events.push("fallback_reason=shared_prepare_failed");
                temporary_release_count += 1;
                events.push("shared_temporary_released");
                events.push("exact_graph_selected");
                exact_executed = true;
                events.push("exact_graph_executed");
            }
        });

        assert_eq!(
            events,
            [
                "precomputed_candidate_selected",
                "shared_prepare_started",
                "fallback_reason=shared_prepare_failed",
                "shared_temporary_released",
                "exact_graph_selected",
                "exact_graph_executed",
            ]
        );
        assert!(!optimized_output_mutated);
        assert!(exact_executed);
        assert_eq!(temporary_release_count, 1);
    }
}

pub(in crate::egl_renderer) fn promote_checkpoint_cache_causal_state(
    scene: &mut SceneRenderState,
    effects: &mut super::super::effects::EffectRuntime,
    graph: &CompiledFrameGraph,
) {
    scene.ensure_capture_command_metadata();
    let frame_serial = effects.effect_resources.checkpoint_frame_serial();
    let candidate_state =
        scene
            .current_checkpoint_scene_causal_snapshot
            .clone()
            .map(|scene_snapshot| {
                build_checkpoint_causal_state(
                    scene_snapshot,
                    Some(graph),
                    &scene.commands,
                    &scene.capture_layers,
                    &scene.capture_visual_groups,
                )
            });
    if let Some(state) = candidate_state {
        effects
            .effect_resources
            .promote_checkpoint_causal_state(frame_serial, state);
    } else {
        effects
            .effect_resources
            .invalidate_checkpoint_causal_state();
    }
}

fn build_checkpoint_causal_state(
    scene: super::super::EglCheckpointSceneCausalSnapshot,
    graph: Option<&CompiledFrameGraph>,
    commands: &[super::super::geometry::EglDrawCommand],
    capture_layers: &[super::super::effects::CaptureLayer],
    capture_visual_groups: &[Option<oblivion_one::compositor::VisualGroupId>],
) -> CheckpointCausalState {
    let mut effects = HashMap::new();
    if let Some(graph) = graph {
        let passes_by_id = graph
            .passes
            .iter()
            .map(|pass| (pass.id, pass))
            .collect::<HashMap<_, _>>();
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
            let capture_dependencies = scene_captures
                .first()
                .and_then(|pass| {
                    pass.checkpoint_dependencies
                        .iter()
                        .map(|dependency| {
                            passes_by_id
                                .get(dependency)
                                .map(|dependency_pass| dependency_pass.instance)
                        })
                        .collect::<Option<Vec<_>>>()
                })
                .unwrap_or_default();
            effects.insert(
                instance.id,
                CheckpointEffectCausalState {
                    semantic_signature: instance.semantic_signature,
                    frame_demand: instance.frame_demand,
                    causal_backdrop_only: scene_captures.len() == 1 && !has_surface_capture,
                    capture_owner_root: scene_captures.first().and_then(|pass| {
                        scene.presentation_owner_for_visual_group(pass.visual_group)
                    }),
                    composition_boundary: scene_captures.first().map(|pass| {
                        let (draw_end, _) = composition_range_with_metadata(
                            commands,
                            capture_layers,
                            capture_visual_groups,
                            pass.anchor,
                            pass.visual_group,
                            pass.anchor_scope,
                        );
                        (draw_end, pass.anchor, pass.visual_group, pass.anchor_scope)
                    }),
                    dependencies: capture_dependencies,
                },
            );
        }
    }
    CheckpointCausalState { scene, effects }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::egl_renderer) enum LegacySceneScissoredPhase {
    BaseRepair(usize),
    PrepareLifecycleSources,
    RestoreRepairScissor(usize),
    Lamp(usize),
    Squash(usize),
    ExternalOverlays(usize),
}

#[derive(Debug, Clone, Copy)]
pub(in crate::egl_renderer) struct LegacySceneScissoredPhasePlan {
    repair_count: usize,
    index: usize,
}

impl Iterator for LegacySceneScissoredPhasePlan {
    type Item = LegacySceneScissoredPhase;

    fn next(&mut self) -> Option<Self::Item> {
        let phase = if self.index < self.repair_count {
            LegacySceneScissoredPhase::BaseRepair(self.index)
        } else if self.index == self.repair_count {
            LegacySceneScissoredPhase::PrepareLifecycleSources
        } else {
            let offset = self.index - self.repair_count - 1;
            let repair = offset / 4;
            if repair >= self.repair_count {
                return None;
            }
            match offset % 4 {
                0 => LegacySceneScissoredPhase::RestoreRepairScissor(repair),
                1 => LegacySceneScissoredPhase::Lamp(repair),
                2 => LegacySceneScissoredPhase::Squash(repair),
                _ => LegacySceneScissoredPhase::ExternalOverlays(repair),
            }
        };
        self.index = self.index.saturating_add(1);
        Some(phase)
    }
}

pub(in crate::egl_renderer) fn legacy_scene_scissored_phase_plan(
    repair_count: usize,
) -> LegacySceneScissoredPhasePlan {
    LegacySceneScissoredPhasePlan {
        repair_count,
        index: 0,
    }
}
