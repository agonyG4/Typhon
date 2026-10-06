use glow::HasContext;
use oblivion_one::effects::CompiledFrameGraph;

use super::super::{
    EffectExecutionContext, EffectFailureReason, LifecycleRenderContext, RendererResult,
    damage::{OutputRect, RenderExecution, RepaintPlan},
    effects::{
        EffectDebugConfig, EffectExecutionSelection, EffectExecutionStats, SceneReplayWorkMode,
    },
    scene_state::{SceneRenderState, SceneTextureSources},
};
use super::{
    pipeline::FramePipeline,
    planning::PlannedFrame,
    telemetry,
    types::{EglFrameOutcome, EglSceneDrawRequest, EglSceneFrameCommit},
};

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
                match self.execute_effect_graph_with_overlays(
                    graph,
                    framebuffer_origin,
                    &planned.plan,
                    demand,
                    selection,
                ) {
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
        let mut prepared = {
            let mut context = self.effect_execution_context();
            super::super::effects::prepare_effect_graph_execution(
                &mut context,
                graph,
                framebuffer_origin,
                repaint_plan,
                demand,
                selection,
                debug_config,
                scene_replay_work_mode_override,
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

    fn effect_execution_context(&mut self) -> EffectExecutionContext<'_> {
        let texture_sources: SceneTextureSources<'_> = self
            .lifecycle
            .texture_sources(self.resources.texture_view());
        EffectExecutionContext::new(self.gl, self.scene, self.effects, texture_sources)
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
                for phase in super::super::legacy_scene_scissored_phase_plan(scissors.len()) {
                    if draw_result.is_err() {
                        break;
                    }
                    match phase {
                        super::super::LegacySceneScissoredPhase::BaseRepair(index) => {
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
                        super::super::LegacySceneScissoredPhase::PrepareLifecycleSources => {
                            draw_result =
                                self.prepare_lifecycle_visual_sources(plan, framebuffer_origin);
                        }
                        super::super::LegacySceneScissoredPhase::RestoreRepairScissor(index) => {
                            let [x, y, width, height] = scissors[index];
                            unsafe {
                                self.gl.enable(glow::SCISSOR_TEST);
                                self.gl.scissor(x, y, width, height);
                            }
                        }
                        super::super::LegacySceneScissoredPhase::Lamp(index) => {
                            let [x, y, width, height] = scissors[index];
                            let output_rect = super::super::gl_scissor_to_output_rect(
                                [x, y, width, height],
                                self.scene.current_size.1,
                                framebuffer_origin,
                            );
                            draw_result = self.draw_lamp_overlay(output_rect);
                        }
                        super::super::LegacySceneScissoredPhase::Squash(index) => {
                            let [x, y, width, height] = scissors[index];
                            let output_rect = super::super::gl_scissor_to_output_rect(
                                [x, y, width, height],
                                self.scene.current_size.1,
                                framebuffer_origin,
                            );
                            draw_result = self.draw_squash_overlay(output_rect);
                        }
                        super::super::LegacySceneScissoredPhase::ExternalOverlays(index) => {
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

pub(in crate::egl_renderer) fn promote_checkpoint_cache_causal_state(
    scene: &mut SceneRenderState,
    effects: &mut super::super::effects::EffectRuntime,
    graph: &CompiledFrameGraph,
) {
    let frame_serial = effects.effect_resources.checkpoint_frame_serial();
    let candidate_state =
        scene
            .current_checkpoint_scene_causal_snapshot
            .clone()
            .map(|scene_snapshot| {
                super::super::CheckpointCausalState::new(
                    scene_snapshot,
                    Some(graph),
                    &scene.commands,
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
