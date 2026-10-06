use std::collections::HashMap;

use glow::HasContext;
use khronos_egl as egl;
use oblivion_one::{
    compositor::{self, RenderableSurface},
    effects::{EffectExecutionDemand, FrameExecutionPlan, compile_frame_execution_plan},
};

use super::super::{
    DamageTraceSnapshot, EffectRepaintProvenanceSnapshot, EglCheckpointSceneCausalSnapshot,
    EglSceneCacheKey, FrameTraceSummary, RendererResult, RepaintPlanTraceSnapshot,
    damage::{
        BufferAge, ClientCursorDamageState, DamageComplexityShadow, EglOutputDamageTracker,
        EglPresentedDamageState, OutputDamage, RepaintMode, RepaintPlan, merge_effect_damage,
        resolve_effect_execution_for_repaint_plan,
        resolve_effect_execution_for_repaint_plan_with_diagnostics,
    },
    effects::{EffectExecutionSelection, graph_metrics},
    geometry::{
        SurfaceConsumerPlan, add_surface_consumers_for_command_range, plan_surface_consumers,
    },
    resources::{ResourceTelemetry, surface::SurfaceResourceInputs},
};
use super::{
    pipeline::FramePipeline,
    telemetry,
    types::{EglFrameOutcome, EglSceneDrawRequest, FrameSkipReason},
};

pub(super) struct FrameBeginState {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) output_scale_key: u32,
    pub(super) scaled_visual_state: compositor::DesktopVisualState,
    input_damage_trace: Option<DamageTraceSnapshot>,
}

#[derive(Clone, Copy)]
struct FrameDamageTraceState {
    input: Option<DamageTraceSnapshot>,
    scene: Option<DamageTraceSnapshot>,
    merged: Option<DamageTraceSnapshot>,
    initial_repaint: Option<RepaintPlanTraceSnapshot>,
    complexity_shadow: Option<DamageComplexityShadow>,
}

pub(super) struct PreparedSceneFrame {
    pub(super) output_damage: OutputDamage,
    pub(super) damage_state: EglPresentedDamageState,
    pub(super) candidate_scene_key: EglSceneCacheKey,
    trace: FrameDamageTraceState,
}

pub(super) enum FramePlanOutcome {
    Skipped(EglFrameOutcome),
    Planned(PlannedFrame),
}

pub(super) struct PlannedFrame {
    pub(super) plan: RepaintPlan,
    pub(super) execution_plan: FrameExecutionPlan,
    pub(super) damage_state: EglPresentedDamageState,
    pub(super) candidate_scene_key: EglSceneCacheKey,
    pub(super) effect_execution_demand: Option<EffectExecutionDemand>,
    pub(super) selected_effect_count: Option<usize>,
    pub(super) effect_selection: Option<EffectExecutionSelection>,
    trace: FrameDamageTraceState,
}

impl FramePipeline<'_> {
    pub(super) fn begin_frame(
        &mut self,
        request: &EglSceneDrawRequest<'_>,
        framebuffer_origin: super::super::OutputFramebufferOrigin,
    ) -> RendererResult<FrameBeginState> {
        self.effects.effect_trace = self.effects.effect_trace.with_frame_context(
            request.frame_id,
            request.render_generation,
            Some(request.scene_generation),
            Some(request.scene_signature),
        );
        if !self.effects.capture_in_progress {
            self.effects
                .effect_gpu_profiler
                .collect(self.gl, &self.effects.effect_trace);
        }
        self.effects.effect_trace.frame_boundary(
            "effect_scene_resolve",
            "begin",
            FrameTraceSummary::default(),
        );
        let width = request.width.max(1);
        let height = request.height.max(1);
        let input_damage_trace = self.effects.effect_trace.enabled().then(|| {
            DamageTraceSnapshot::from_optional(request.current_damage.as_ref(), width, height)
        });
        self.scene.current_framebuffer_origin = framebuffer_origin;
        self.lifecycle.begin_frame(request.lifecycle);
        let output_scale_key = compositor::output_scale_key(request.output_scale);
        let mut scaled_visual_state =
            compositor::scale_desktop_visual_state(request.visual_state, request.output_scale);
        if request.client_cursor.is_some() {
            scaled_visual_state.cursor = None;
        }
        self.scene.frame_stats = super::types::GlesSceneFrameStats::default();
        let effect_time = self.effects.effect_clock_elapsed_seconds();
        self.effects.effect_delta_seconds = if self.effects.effect_time_seconds == 0.0 {
            0.0
        } else {
            (effect_time - self.effects.effect_time_seconds).clamp(0.0, 0.25)
        };
        self.effects.effect_time_seconds = effect_time;
        self.effects.effect_output_scale = request.output_scale.max(0.0) as f32;
        self.ensure_output_size(width, height)?;
        self.lifecycle.release_stale_visual_resources(self.effects);
        self.scene.frame_stats.effect_instances_visible = request
            .effects
            .instances
            .iter()
            .filter(|instance| !instance.region.is_empty())
            .count();
        self.effects.effect_trace.frame_boundary(
            "effect_scene_resolve",
            "end",
            FrameTraceSummary {
                scene_generation: Some(request.scene_generation),
                scene_signature: Some(request.scene_signature),
                visible_effect_count: Some(self.scene.frame_stats.effect_instances_visible),
                ..FrameTraceSummary::default()
            },
        );
        Ok(FrameBeginState {
            width,
            height,
            output_scale_key,
            scaled_visual_state,
            input_damage_trace,
        })
    }

    fn ensure_output_size(&mut self, width: u32, height: u32) -> RendererResult<()> {
        if self.scene.current_size == (width, height) {
            return Ok(());
        }

        self.lifecycle.release_all_visual_resources(self.effects);
        self.scene.current_size = (width, height);
        self.scene.repaint_planner.resize((width, height));
        self.scene.scene_cache_key = None;
        self.effects.effect_resources.cleanup_size_history(self.gl);
        unsafe {
            self.gl.viewport(0, 0, width as i32, height as i32);
        }
        Ok(())
    }

    pub(super) fn prepare_resources(
        &mut self,
        egl: &super::super::EglInstance,
        egl_display: egl::Display,
        request: &EglSceneDrawRequest<'_>,
        frame: &FrameBeginState,
    ) -> RendererResult<()> {
        self.resources.ensure_frame_resources(self.gl)?;
        self.resources.ensure_decoration_resources(
            self.gl,
            egl,
            egl_display,
            request
                .decoration_instances
                .iter()
                .chain(request.lifecycle_decorations.iter()),
        )?;
        if frame.scaled_visual_state.cursor.is_some() {
            self.resources
                .ensure_cursor_resource(self.gl, egl, egl_display, self.cursor_image)?;
        }
        {
            let mut telemetry = ResourceTelemetry::new(&mut self.scene.frame_stats);
            self.resources.reconcile_surface_resource_lifetimes(
                self.gl,
                egl,
                egl_display,
                request.surfaces,
                request.lifecycle_surfaces,
                request.client_cursor.map(|cursor| cursor.surface),
                &mut telemetry,
            )?;
        }
        if let Some(cursor) = request.client_cursor.map(|cursor| cursor.surface) {
            let mut cursor_consumers = SurfaceConsumerPlan::default();
            cursor_consumers.add_surface(cursor.surface_id);
            cursor_consumers.finish();
            let mut telemetry = ResourceTelemetry::new(&mut self.scene.frame_stats);
            self.resources.realize_surface_resources_for_consumers(
                self.gl,
                egl,
                egl_display,
                SurfaceResourceInputs {
                    canonical: request.surfaces,
                    lifecycle: request.lifecycle_surfaces,
                    client_cursor: Some(cursor),
                },
                &cursor_consumers,
                &request.surface_resource_sync_states,
                &mut telemetry,
            )?;
        }
        Ok(())
    }

    pub(super) fn prepare_scene(
        &mut self,
        request: &mut EglSceneDrawRequest<'_>,
        frame: &FrameBeginState,
        framebuffer_origin: super::super::OutputFramebufferOrigin,
    ) -> PreparedSceneFrame {
        let (base_surfaces, overlay_surfaces) = super::super::split_external_overlay_surfaces(
            request.surfaces,
            request.external_overlay_surface_ids,
        );
        let presentation_owner_roots_by_surface = request
            .surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .zip(request.presentation_owner_root_surface_ids.iter().copied())
            .collect::<HashMap<_, _>>();
        let scene_surfaces = if request.external_overlay_surface_ids.is_empty() {
            request.surfaces
        } else {
            base_surfaces.as_slice()
        };
        let surface_signatures = super::super::egl_scene_surface_signatures(request.surfaces);
        let candidate_scene_key = EglSceneCacheKey::new_with_decorations_and_external_overlay_ids(
            frame.width,
            frame.height,
            request.content_generation,
            frame.output_scale_key,
            &surface_signatures,
            request.presentation_visual_signature,
            request.external_overlay_surface_ids,
            request.decoration_instances,
            request.popup_surface_ids,
            framebuffer_origin,
        );
        let scene_changed = self.scene.presented_scene_key != Some(candidate_scene_key);
        let commands_changed = !self.scene.scene_cache_is_current(
            frame.width,
            frame.height,
            request.content_generation,
            frame.output_scale_key,
            &surface_signatures,
            request.external_overlay_surface_ids,
            request.decoration_instances,
            request.popup_surface_ids,
            request.presentation_visual_signature,
            framebuffer_origin,
        );
        let client_cursor_damage = request.client_cursor.map(|cursor| {
            ClientCursorDamageState::new(
                compositor::scale_logical_coordinate(
                    cursor.logical_x.saturating_add(cursor.surface.x),
                    request.output_scale,
                ),
                compositor::scale_logical_coordinate(
                    cursor.logical_y.saturating_add(cursor.surface.y),
                    request.output_scale,
                ),
                compositor::scale_logical_extent(cursor.surface.width, request.output_scale),
                compositor::scale_logical_extent(cursor.surface.height, request.output_scale),
                cursor.surface.generation,
                frame.width,
                frame.height,
            )
        });
        let damage_authority_available = request.current_damage.is_some();
        let output_damage = self.scene.damage_tracker.damage_for_frame(
            frame.width,
            frame.height,
            scene_changed,
            request.current_damage.take(),
            frame.scaled_visual_state,
            client_cursor_damage,
        );
        let output_damage = output_damage.union(
            self.lifecycle.damage_for_snapshot(
                request.lifecycle,
                (frame.width, frame.height),
                request.output_scale,
            ),
            frame.width,
            frame.height,
        );
        let (output_damage, contradictory_empty_damage) =
            super::super::resolve_scene_damage_authority(
                scene_changed,
                damage_authority_available,
                output_damage,
            );
        let scene_damage_trace =
            self.effects.effect_trace.enabled().then(|| {
                DamageTraceSnapshot::from_damage(&output_damage, frame.width, frame.height)
            });
        self.scene.frame_stats.contradictory_empty_damage = contradictory_empty_damage;
        let damage_state = EglOutputDamageTracker::candidate_state(
            frame.width,
            frame.height,
            frame.scaled_visual_state,
            client_cursor_damage,
            self.cursor_image,
        );

        if commands_changed {
            self.scene.frame_stats.scene_rebuilt = true;
            self.scene.rebuild_scene_commands(
                frame.width,
                frame.height,
                scene_surfaces,
                request.decoration_instances,
                request.popup_surface_ids,
                request.content_generation,
                request.output_scale,
                frame.output_scale_key,
                &surface_signatures,
                request.external_overlay_surface_ids,
                request.presentation_visual_signature,
                request.presentation_opacities,
                request.presentation_clips,
                &presentation_owner_roots_by_surface,
                framebuffer_origin,
            );
        }
        self.scene.rebuild_overlay_commands(
            frame.width,
            frame.height,
            frame.scaled_visual_state,
            &overlay_surfaces,
            request.client_cursor,
            request.output_scale,
            framebuffer_origin,
            self.cursor_image,
            self.resources.texture_view().cursor_size(),
        );
        self.scene.current_checkpoint_scene_causal_snapshot =
            Some(EglCheckpointSceneCausalSnapshot::new(
                (frame.width, frame.height),
                &self.scene.commands,
                &self.scene.vertices,
                &surface_signatures,
                &self.scene.presentation_opacities,
                &self.scene.presentation_visual_group_owners,
            ));
        self.lifecycle.rebuild_lamp_commands(
            request.lifecycle,
            request.lifecycle_surfaces,
            request.lifecycle_decorations,
            request.output_scale,
            self.scene.current_size,
            framebuffer_origin,
        );
        self.lifecycle.rebuild_squash_commands(
            request.lifecycle,
            request.lifecycle_surfaces,
            request.lifecycle_decorations,
            request.output_scale,
            self.scene.current_size,
            framebuffer_origin,
        );
        PreparedSceneFrame {
            output_damage,
            damage_state,
            candidate_scene_key,
            trace: FrameDamageTraceState {
                input: frame.input_damage_trace,
                scene: scene_damage_trace,
                merged: None,
                initial_repaint: None,
                complexity_shadow: None,
            },
        }
    }

    pub(super) fn plan_frame(
        &mut self,
        request: &EglSceneDrawRequest<'_>,
        prepared: PreparedSceneFrame,
        buffer_age: BufferAge,
    ) -> RendererResult<FramePlanOutcome> {
        let width = self.scene.current_size.0;
        let height = self.scene.current_size.1;
        let effect_source_damage =
            super::super::effect_region_from_output_damage(&prepared.output_damage, width, height);
        let output_bounds = oblivion_one::effects::EffectRect::new(0, 0, width, height)
            .expect("non-zero renderer dimensions must form valid effect bounds");
        self.effects.effect_trace.frame_boundary(
            "effect_graph_compile",
            "begin",
            telemetry::effect_trace_summary(self.scene, request.effects, None, None, None),
        );
        let execution_plan = if self.effects.failed_effect_generation
            == Some(self.effects.effect_registry_generation)
            && self.scene.frame_stats.effect_instances_visible != 0
        {
            self.scene.frame_stats.effect_fallbacks =
                self.scene.frame_stats.effect_fallbacks.saturating_add(1);
            self.scene.frame_stats.effect_instances_failed =
                self.scene.frame_stats.effect_instances_visible;
            self.scene.frame_stats.effect_failure_reason =
                Some(super::super::effects::EffectFailureReason::GraphCompile);
            FrameExecutionPlan::LegacyScene
        } else {
            match compile_frame_execution_plan(
                request.effects,
                &effect_source_damage,
                output_bounds,
                &self.effects.effect_registry,
            ) {
                Ok(FrameExecutionPlan::LegacyScene) => FrameExecutionPlan::LegacyScene,
                Ok(FrameExecutionPlan::EffectGraph(graph)) => {
                    telemetry::record_effect_graph_metrics(self.scene, graph_metrics(&graph));
                    FrameExecutionPlan::EffectGraph(graph)
                }
                Err(_) => {
                    self.scene.frame_stats.effect_fallbacks =
                        self.scene.frame_stats.effect_fallbacks.saturating_add(1);
                    self.scene.frame_stats.effect_instances_failed =
                        self.scene.frame_stats.effect_instances_visible;
                    self.scene.frame_stats.effect_failure_reason =
                        Some(super::super::effects::EffectFailureReason::GraphCompile);
                    self.effects.failed_effect_generation =
                        Some(self.effects.effect_registry_generation);
                    FrameExecutionPlan::LegacyScene
                }
            }
        };
        if matches!(&execution_plan, FrameExecutionPlan::LegacyScene) {
            self.effects
                .effect_resources
                .clear_checkpoint_capture_cache();
        }
        let compiled_graph = match &execution_plan {
            FrameExecutionPlan::EffectGraph(graph) => Some(graph),
            FrameExecutionPlan::LegacyScene => None,
        };
        self.effects.effect_trace.frame_boundary(
            "effect_graph_compile",
            "end",
            telemetry::effect_trace_summary(
                self.scene,
                request.effects,
                None,
                compiled_graph,
                None,
            ),
        );
        let output_damage = match &execution_plan {
            FrameExecutionPlan::LegacyScene => prepared.output_damage,
            FrameExecutionPlan::EffectGraph(graph) => {
                merge_effect_damage(prepared.output_damage, &graph.final_damage, width, height)
            }
        };
        let merged_damage_trace = self
            .effects
            .effect_trace
            .enabled()
            .then(|| DamageTraceSnapshot::from_damage(&output_damage, width, height));
        let (plan, complexity_shadow_trace) = if self.effects.effect_trace.enabled() {
            let (plan, shadow) = self
                .scene
                .repaint_planner
                .plan_with_damage_complexity_shadow(output_damage, buffer_age);
            (plan, Some(shadow))
        } else {
            (
                self.scene.repaint_planner.plan(output_damage, buffer_age),
                None,
            )
        };
        let trace = FrameDamageTraceState {
            merged: merged_damage_trace,
            complexity_shadow: complexity_shadow_trace,
            ..prepared.trace
        };
        if plan.mode == RepaintMode::Skip {
            self.scene.frame_stats.surface_resource_candidates = request.surfaces.len();
            self.scene.frame_stats.surface_resource_deferred = request.surfaces.len();
            telemetry::record_effect_resource_metrics(self.scene, self.effects);
            telemetry::record_repaint_stats(self.scene, &plan);
            return Ok(FramePlanOutcome::Skipped(EglFrameOutcome::Skipped {
                reason: FrameSkipReason::NoLogicalDamage,
                stats: self.scene.frame_stats,
            }));
        }
        let initial_repaint_trace = self
            .effects
            .effect_trace
            .enabled()
            .then(|| RepaintPlanTraceSnapshot::from_plan(&plan, width, height));
        Ok(FramePlanOutcome::Planned(PlannedFrame {
            plan,
            execution_plan,
            damage_state: prepared.damage_state,
            candidate_scene_key: prepared.candidate_scene_key,
            effect_execution_demand: None,
            selected_effect_count: None,
            effect_selection: None,
            trace: FrameDamageTraceState {
                initial_repaint: initial_repaint_trace,
                ..trace
            },
        }))
    }

    pub(super) fn plan_effect_demand(
        &mut self,
        request: &EglSceneDrawRequest<'_>,
        planned: &mut PlannedFrame,
    ) {
        let width = self.scene.current_size.0;
        let height = self.scene.current_size.1;
        let compiled_graph = match &planned.execution_plan {
            FrameExecutionPlan::EffectGraph(graph) => Some(graph),
            FrameExecutionPlan::LegacyScene => None,
        };
        let demand_trace_seed = self
            .effects
            .effect_trace
            .enabled()
            .then(|| {
                compiled_graph.map(|graph| oblivion_one::effects::EffectDemandPlanStats {
                    repair_rect_count: planned.plan.repair_damage.rect_count(),
                    dependency_edge_count: graph.instances.iter().fold(0, |count, instance| {
                        count.saturating_add(instance.dependencies.len())
                    }),
                    dependency_propagations: 0,
                    max_instance_region_rect_count: 0,
                    conservative_full: planned.plan.mode == RepaintMode::Full,
                    ..oblivion_one::effects::EffectDemandPlanStats::default()
                })
            })
            .flatten();
        let mut demand_trace_begin_summary = telemetry::effect_trace_summary(
            self.scene,
            request.effects,
            Some(&planned.plan),
            compiled_graph,
            None,
        );
        demand_trace_begin_summary.demand_plan = demand_trace_seed;
        self.effects.effect_trace.frame_boundary(
            "effect_demand_plan",
            "begin",
            demand_trace_begin_summary,
        );
        let demand = match &planned.execution_plan {
            FrameExecutionPlan::LegacyScene => None,
            FrameExecutionPlan::EffectGraph(graph) => {
                Some(if self.effects.effect_trace.enabled() {
                    let (demand, snapshot) =
                        resolve_effect_execution_for_repaint_plan_with_diagnostics(
                            &self.scene.repaint_planner,
                            graph,
                            &mut planned.plan,
                            width,
                            height,
                        );
                    self.effects
                        .effect_trace
                        .effect_execution_resolution(|| snapshot);
                    demand
                } else {
                    resolve_effect_execution_for_repaint_plan(
                        &self.scene.repaint_planner,
                        graph,
                        &mut planned.plan,
                        width,
                        height,
                    )
                })
            }
        };
        let selected_effect_count = demand.as_ref().map(|demand| demand.instances.len());
        let demand_trace_stats = self
            .effects
            .effect_trace
            .enabled()
            .then(|| demand.as_ref().map(EffectExecutionDemand::plan_stats))
            .flatten();
        let mut demand_trace_end_summary = telemetry::effect_trace_summary(
            self.scene,
            request.effects,
            Some(&planned.plan),
            compiled_graph,
            selected_effect_count,
        );
        demand_trace_end_summary.demand_plan = demand_trace_stats;
        self.effects.effect_trace.frame_boundary(
            "effect_demand_plan",
            "end",
            demand_trace_end_summary,
        );
        self.effects.effect_trace.effect_repaint_provenance(|| {
            EffectRepaintProvenanceSnapshot::new(
                planned
                    .trace
                    .input
                    .expect("enabled effect trace must capture input damage"),
                planned
                    .trace
                    .scene
                    .expect("enabled effect trace must capture scene damage"),
                planned
                    .trace
                    .merged
                    .expect("enabled effect trace must capture merged damage"),
                planned
                    .trace
                    .initial_repaint
                    .expect("enabled effect trace must capture initial repaint"),
                RepaintPlanTraceSnapshot::from_plan(&planned.plan, width, height),
                planned
                    .trace
                    .complexity_shadow
                    .expect("enabled effect trace must capture damage complexity shadow"),
            )
        });
        if let Some(demand) = &demand {
            self.scene.frame_stats.effect_instances_pruned = self
                .scene
                .frame_stats
                .effect_instances_visible
                .saturating_sub(demand.instances.len());
        }
        planned.effect_execution_demand = demand;
        planned.selected_effect_count = selected_effect_count;
    }

    pub(super) fn plan_consumers(
        &mut self,
        request: &EglSceneDrawRequest<'_>,
        planned: &mut PlannedFrame,
    ) -> SurfaceConsumerPlan {
        let (width, height) = self.scene.current_size;
        let repair_rects = super::super::repaint_plan_output_rects(&planned.plan, width, height);
        let mut consumer_plan = plan_surface_consumers(&self.scene.commands, &repair_rects);
        self.lifecycle
            .extend_surface_consumers(&mut consumer_plan, &repair_rects);
        add_surface_consumers_for_command_range(
            &mut consumer_plan,
            &self.scene.cursor_commands,
            0,
            self.scene.cursor_commands.len(),
            &repair_rects,
        );
        let effect_selection = match (&planned.execution_plan, &planned.effect_execution_demand) {
            (FrameExecutionPlan::EffectGraph(graph), Some(demand)) => {
                let selection = super::super::effects::select_effect_execution(graph, demand);
                consumer_plan.extend(&super::super::effects::plan_effect_surface_consumers(
                    graph,
                    demand,
                    &selection,
                    &self.scene.commands,
                    &repair_rects,
                    (width, height),
                ));
                Some(selection)
            }
            _ => None,
        };
        consumer_plan.finish();
        self.scene.frame_stats.surface_resource_candidates = request.surfaces.len();
        self.scene.frame_stats.surface_resource_consumers = consumer_plan
            .surface_ids()
            .iter()
            .filter(|surface_id| {
                request
                    .surfaces
                    .iter()
                    .any(|surface| surface.surface_id == **surface_id)
            })
            .count();
        self.scene.frame_stats.surface_resource_deferred = self
            .scene
            .frame_stats
            .surface_resource_candidates
            .saturating_sub(self.scene.frame_stats.surface_resource_consumers);
        planned.effect_selection = effect_selection;
        consumer_plan
    }

    pub(super) fn realize_consumers(
        &mut self,
        egl: &super::super::EglInstance,
        egl_display: egl::Display,
        request: &EglSceneDrawRequest<'_>,
        _planned: &PlannedFrame,
        consumer_plan: &SurfaceConsumerPlan,
    ) -> RendererResult<()> {
        let mut telemetry = ResourceTelemetry::new(&mut self.scene.frame_stats);
        self.resources.realize_surface_resources_for_consumers(
            self.gl,
            egl,
            egl_display,
            SurfaceResourceInputs {
                canonical: request.surfaces,
                lifecycle: request.lifecycle_surfaces,
                client_cursor: request.client_cursor.map(|cursor| cursor.surface),
            },
            consumer_plan,
            &request.surface_resource_sync_states,
            &mut telemetry,
        )
    }
}

pub(in crate::egl_renderer) fn split_external_overlay_surfaces(
    surfaces: &[RenderableSurface],
    external_overlay_surface_ids: &[u32],
) -> (Vec<RenderableSurface>, Vec<RenderableSurface>) {
    if external_overlay_surface_ids.is_empty() {
        return (Vec::new(), Vec::new());
    }
    surfaces
        .iter()
        .cloned()
        .partition(|surface| !external_overlay_surface_ids.contains(&surface.surface_id))
}

pub(in crate::egl_renderer) fn resolve_scene_damage_authority(
    scene_changed: bool,
    damage_authority_available: bool,
    output_damage: OutputDamage,
) -> (OutputDamage, bool) {
    if scene_changed && !damage_authority_available && output_damage == OutputDamage::Empty {
        (OutputDamage::Full, true)
    } else {
        (output_damage, false)
    }
}
