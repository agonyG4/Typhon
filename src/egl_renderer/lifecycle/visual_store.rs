use super::super::geometry::{EglVisibilityDecision, plan_visibility};
use super::super::*;
use super::frame_state::LifecycleFrameState;
use super::geometry::{
    ndc_to_output_point, output_point_to_ndc, scaled_presentation_rect,
    surface_root_for_lifecycle_surface,
};
use oblivion_one::render_backend::buffer::SurfaceBufferSource;
use oblivion_one::window_lifecycle_animation::LifecycleDirection;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LifecycleRenderDiagnosticStage {
    SourceResolved,
    ResourcesReady,
    RepresentationCaptured,
    Drawn,
    FrameSurvived,
    Fallback,
}

impl LifecycleRenderDiagnosticStage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::SourceResolved => "source_resolved",
            Self::ResourcesReady => "resources_ready",
            Self::RepresentationCaptured => "representation_captured",
            Self::Drawn => "drawn",
            Self::FrameSurvived => "frame_survived",
            Self::Fallback => "fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LifecycleVisualSourceMetrics {
    pub(super) lifecycle_surface_count: usize,
    pub(super) buffer_source_shm: bool,
    pub(super) buffer_source_dmabuf: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleDiagnosticLayer {
    Surface,
    Decoration,
    DecorationAsset,
    ResolvedVisual,
}

impl LifecycleDiagnosticLayer {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Surface => "surface",
            Self::Decoration => "decoration",
            Self::DecorationAsset => "decoration_asset",
            Self::ResolvedVisual => "resolved_visual",
        }
    }
}

type MissingLifecycleSourceResource = (LifecycleDiagnosticLayer, Option<u32>, Option<u64>);

static LIFECYCLE_RENDER_DIAGNOSTICS_ENABLED: OnceLock<bool> = OnceLock::new();

fn lifecycle_render_diagnostics_enabled() -> bool {
    *LIFECYCLE_RENDER_DIAGNOSTICS_ENABLED.get_or_init(|| {
        matches!(
            std::env::var("ASTREA_LIFECYCLE_DIAGNOSTICS").as_deref(),
            Ok("1" | "true" | "on")
        )
    })
}

fn diagnostic_layer_for_draw_layer(layer: EglDrawLayer) -> LifecycleDiagnosticLayer {
    match layer {
        EglDrawLayer::Surface(_) => LifecycleDiagnosticLayer::Surface,
        EglDrawLayer::DecorationAsset(_) => LifecycleDiagnosticLayer::DecorationAsset,
        EglDrawLayer::SolidRgba(_) => LifecycleDiagnosticLayer::Decoration,
        EglDrawLayer::LifecycleResolvedVisual(_)
        | EglDrawLayer::Solid(_)
        | EglDrawLayer::Cursor => LifecycleDiagnosticLayer::ResolvedVisual,
    }
}

fn lifecycle_fallback_reason_label(reason: LifecycleRenderFallbackReason) -> &'static str {
    match reason {
        LifecycleRenderFallbackReason::ResolvedSourceAllocation => "resolved_source_allocation",
        LifecycleRenderFallbackReason::ResolvedSourceCapture => "resolved_source_capture",
        LifecycleRenderFallbackReason::MinimizedSnapshotUnavailable => {
            "minimized_snapshot_unavailable"
        }
        LifecycleRenderFallbackReason::LampProgramUnavailable => "lamp_program_unavailable",
        LifecycleRenderFallbackReason::LifecycleResourceUnavailable => {
            "lifecycle_resource_unavailable"
        }
        LifecycleRenderFallbackReason::MeshBudget => "mesh_budget",
        LifecycleRenderFallbackReason::NoConsumedRepresentation => "no_consumed_representation",
    }
}

#[derive(Clone, Copy, Debug)]
enum LifecycleSourceCaptureFailureStage {
    TargetAllocation,
    TargetClear,
    TargetRelease,
    ScratchAllocation,
    ScratchClear,
    ScratchTarget,
    MissingSourceCommands,
    GraphCompile,
    EffectExecution,
    TargetCopy,
    ScratchRelease,
}

impl LifecycleSourceCaptureFailureStage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::TargetAllocation => "target_allocation",
            Self::TargetClear => "target_clear",
            Self::TargetRelease => "target_release",
            Self::ScratchAllocation => "scratch_allocation",
            Self::ScratchClear => "scratch_clear",
            Self::ScratchTarget => "scratch_target",
            Self::MissingSourceCommands => "missing_source_commands",
            Self::GraphCompile => "graph_compile",
            Self::EffectExecution => "effect_execution",
            Self::TargetCopy => "target_copy",
            Self::ScratchRelease => "scratch_release",
        }
    }
}

#[derive(Debug)]
pub(super) struct LifecycleSourceCaptureFailure {
    stage: LifecycleSourceCaptureFailureStage,
    source: Box<dyn Error>,
}

impl LifecycleSourceCaptureFailure {
    fn new(stage: LifecycleSourceCaptureFailureStage, source: impl Into<Box<dyn Error>>) -> Self {
        Self {
            stage,
            source: source.into(),
        }
    }
}

pub(in crate::egl_renderer) struct LifecycleResolvedVisualResource {
    pub(super) texture: PooledEffectTexture,
    #[cfg(test)]
    pub(super) source_signature: u64,
}

impl LifecycleResolvedVisualResource {
    pub(in crate::egl_renderer) fn pooled_texture(&self) -> &PooledEffectTexture {
        &self.texture
    }
}

#[derive(Default)]
pub(super) struct LifecycleVisualStore {
    pub(super) source_vertices:
        HashMap<compositor::PresentationRetainedVisualPayloadId, Vec<EglTexturedVertex>>,
    pub(super) source_commands:
        HashMap<compositor::PresentationRetainedVisualPayloadId, Vec<EglDrawCommand>>,
    pub(super) source_metrics:
        HashMap<compositor::PresentationRetainedVisualPayloadId, LifecycleVisualSourceMetrics>,
    missing_source_layers:
        HashMap<compositor::PresentationRetainedVisualPayloadId, MissingLifecycleSourceResource>,
    diagnostics_emitted: HashSet<(
        compositor::PresentationRetainedVisualPayloadId,
        LifecycleRenderDiagnosticStage,
    )>,
    pub(super) resolved_visual_resources:
        HashMap<compositor::PresentationRetainedVisualPayloadId, LifecycleResolvedVisualResource>,
    captured_payloads: HashSet<compositor::PresentationRetainedVisualPayloadId>,
    minimized_snapshot_payloads: HashSet<compositor::PresentationRetainedVisualPayloadId>,
    pub(super) visual_sources: HashMap<
        oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
        LifecycleVisualSource,
    >,
}

impl LifecycleVisualStore {
    #[cfg(test)]
    pub(super) fn is_ready(
        &self,
        payload_id: compositor::PresentationRetainedVisualPayloadId,
        runtime: &EffectRuntime,
    ) -> bool {
        self.resolved_visual_resources
            .get(&payload_id)
            .is_some_and(|resource| {
                runtime
                    .effect_resources
                    .texture(&resource.texture)
                    .is_some()
            })
    }

    pub(super) fn begin_frame(&mut self, snapshot: &LifecycleSceneSample) {
        self.visual_sources.clear();
        self.visual_sources.extend(
            snapshot
                .samples
                .iter()
                .map(|sample| sample.visual_source.clone())
                .map(|source| (source.presentation_identity, source)),
        );
        self.minimized_snapshot_payloads = snapshot
            .minimized_visual_resource_payload_ids
            .iter()
            .copied()
            .collect();
    }

    pub(super) fn rebuild_source_commands(
        &mut self,
        lifecycle: &LifecycleSceneSample,
        lifecycle_surfaces: &[RenderableSurface],
        lifecycle_decorations: &[DecorationRenderInstance],
        output_scale: f64,
        output_size: (u32, u32),
        framebuffer_origin: OutputFramebufferOrigin,
    ) {
        if !lifecycle.samples.iter().any(|sample| {
            sample.visual_source.resource_payload_id == sample.payload_id
                && !self.source_vertices.contains_key(&sample.payload_id)
        }) {
            return;
        }
        let dynamic_assignments = lifecycle
            .samples
            .iter()
            .any(|sample| sample.visual_source.render_layout.is_none())
            .then(|| {
                compositor::surface_render_space_assignments(lifecycle_surfaces, output_scale)
            });
        for sample in &lifecycle.samples {
            let source = &sample.visual_source;
            if source.root_surface_id != sample.root_surface_id
                || source.payload_id != sample.payload_id
                || source.presentation_identity != sample.presentation_identity
                || source.resource_payload_id != source.payload_id
                || self.source_vertices.contains_key(&source.payload_id)
            {
                continue;
            }
            let mut vertices = Vec::new();
            let mut commands = Vec::new();
            let visual_group = source
                .effect_scene
                .instances
                .first()
                .and_then(|instance| instance.visual_group)
                .or_else(|| VisualGroupId::new(1));
            for (surface_index, surface) in lifecycle_surfaces.iter().enumerate() {
                if surface_root_for_lifecycle_surface(surface, lifecycle_surfaces, lifecycle)
                    != sample.root_surface_id
                {
                    continue;
                }
                let assignment = if let Some(layout) = source.render_layout.as_ref() {
                    layout.assignment_for(surface, output_scale)
                } else {
                    dynamic_assignments
                        .as_ref()
                        .and_then(|assignments| assignments.get(surface_index))
                        .cloned()
                };
                let Some(assignment) = assignment else {
                    continue;
                };
                push_egl_surface_commands(
                    &mut vertices,
                    &mut commands,
                    output_size.0,
                    output_size.1,
                    surface,
                    assignment,
                    framebuffer_origin,
                    visual_group,
                );
            }
            for decoration in lifecycle_decorations
                .iter()
                .filter(|decoration| decoration.root_surface_id() == sample.root_surface_id)
            {
                push_egl_decoration_instance(
                    &mut vertices,
                    &mut commands,
                    output_size.0,
                    output_size.1,
                    decoration,
                    output_scale,
                    framebuffer_origin,
                    visual_group,
                );
            }
            reproject_lifecycle_source_commands(
                &mut vertices,
                &mut commands,
                sample.visual_group.canonical_client_rect,
                sample.visual_group.presented_source_client_rect,
                output_scale,
                output_size,
                framebuffer_origin,
            );
            let source_surfaces = lifecycle_surfaces
                .iter()
                .filter(|surface| {
                    surface_root_for_lifecycle_surface(surface, lifecycle_surfaces, lifecycle)
                        == sample.root_surface_id
                })
                .collect::<Vec<_>>();
            self.source_metrics.insert(
                source.payload_id,
                LifecycleVisualSourceMetrics {
                    lifecycle_surface_count: source_surfaces.len(),
                    buffer_source_shm: source_surfaces
                        .iter()
                        .any(|surface| surface.buffer_source() == SurfaceBufferSource::Shm),
                    buffer_source_dmabuf: source_surfaces
                        .iter()
                        .any(|surface| surface.buffer_source() == SurfaceBufferSource::Dmabuf),
                },
            );
            self.source_vertices.insert(source.payload_id, vertices);
            self.source_commands.insert(source.payload_id, commands);
        }
    }

    pub(super) fn extend_surface_consumers(
        &self,
        consumer_plan: &mut SurfaceConsumerPlan,
        _repairs: &[OutputRect],
    ) {
        for (&payload_id, commands) in &self.source_commands {
            if self.captured_payloads.contains(&payload_id) {
                continue;
            }
            for command in commands {
                if let EglDrawLayer::Surface(surface_id) = command.layer {
                    consumer_plan.add_surface(surface_id);
                }
            }
        }
    }

    pub(super) fn release_stale(&mut self, runtime: &mut EffectRuntime) {
        let live_payload_ids = self
            .visual_sources
            .values()
            .map(|source| source.payload_id)
            .collect::<HashSet<_>>();
        let mut live_resource_payload_ids = self
            .visual_sources
            .values()
            .map(|source| source.resource_payload_id)
            .collect::<HashSet<_>>();
        live_resource_payload_ids.extend(self.minimized_snapshot_payloads.iter().copied());
        let stale = self
            .resolved_visual_resources
            .keys()
            .copied()
            .filter(|payload_id| !live_resource_payload_ids.contains(payload_id))
            .collect::<Vec<_>>();
        for payload_id in stale {
            if let Some(resource) = self.resolved_visual_resources.remove(&payload_id) {
                let _ = runtime.effect_resources.release(resource.texture);
            }
        }
        self.source_vertices
            .retain(|payload_id, _| live_payload_ids.contains(payload_id));
        self.source_commands
            .retain(|payload_id, _| live_payload_ids.contains(payload_id));
        self.source_metrics
            .retain(|payload_id, _| live_payload_ids.contains(payload_id));
        self.missing_source_layers
            .retain(|payload_id, _| live_payload_ids.contains(payload_id));
        self.captured_payloads
            .retain(|payload_id| live_payload_ids.contains(payload_id));
        self.diagnostics_emitted
            .retain(|(payload_id, _)| live_payload_ids.contains(payload_id));
    }

    pub(super) fn release_all(&mut self, runtime: &mut EffectRuntime) {
        for (_, resource) in self.resolved_visual_resources.drain() {
            let _ = runtime.effect_resources.release(resource.texture);
        }
    }
}

impl LifecycleVisualStore {
    pub(super) fn prepare_lifecycle_visual_sources(
        &mut self,
        frame: &mut LifecycleFrameState,
        context: &mut LifecycleRenderContext<'_>,
        _plan: &RepaintPlan,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> RendererResult<()> {
        self.release_stale(context.effect_runtime);
        let output_scale = context.effect_runtime.effect_output_scale.max(1.0) as f64;
        let sources = self.visual_sources.values().cloned().collect::<Vec<_>>();

        for source in sources {
            let Some(lamp) = frame
                .samples
                .iter()
                .find(|sample| {
                    sample.presentation_identity == source.presentation_identity
                        && sample.root_surface_id == source.root_surface_id
                        && sample.payload_id == source.payload_id
                })
                .copied()
            else {
                continue;
            };
            self.trace_source_event(
                LifecycleRenderDiagnosticStage::SourceResolved,
                &source,
                lamp,
                None,
            );
            let source_visual_rect = scaled_presentation_rect(
                lamp.visual_group.presented_source_visual_rect,
                output_scale,
            );
            let Some((width, height)) = lifecycle_visual_texture_size(source_visual_rect) else {
                frame.record_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::ResolvedSourceAllocation,
                );
                continue;
            };
            let ready = self
                .resolved_visual_resources
                .get(&source.resource_payload_id)
                .is_some_and(|resource| {
                    context
                        .effect_runtime
                        .effect_resources
                        .texture(&resource.texture)
                        .is_some()
                });
            if ready {
                self.trace_source_event(
                    LifecycleRenderDiagnosticStage::ResourcesReady,
                    &source,
                    lamp,
                    None,
                );
                continue;
            }

            if source.resource_payload_id != source.payload_id {
                frame.record_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::MinimizedSnapshotUnavailable,
                );
                let report_missing_snapshot = lifecycle_render_diagnostics_enabled()
                    && !self
                        .diagnostics_emitted
                        .contains(&(source.payload_id, LifecycleRenderDiagnosticStage::Fallback));
                self.trace_source_event(
                    LifecycleRenderDiagnosticStage::Fallback,
                    &source,
                    lamp,
                    Some(LifecycleRenderFallbackReason::MinimizedSnapshotUnavailable),
                );
                if report_missing_snapshot {
                    eprintln!(
                        "typhon.minimized_visual event=missing_resource window_id={} source_payload_id={} restore_payload_id={} reason=minimized_snapshot_unavailable",
                        source.window_id.get(),
                        source.resource_payload_id.get(),
                        source.payload_id.get(),
                    );
                }
                continue;
            }

            if self.captured_payloads.contains(&source.payload_id) {
                self.missing_source_layers.insert(
                    source.payload_id,
                    (LifecycleDiagnosticLayer::ResolvedVisual, None, None),
                );
                frame.record_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::LifecycleResourceUnavailable,
                );
                continue;
            }

            #[cfg(test)]
            let source_signature = lifecycle_visual_source_signature(&source, lamp, output_scale);
            let texture_key = EffectTextureKey::new(
                width,
                height,
                EffectTextureFormat::Rgba8,
                EffectTextureFilter::Linear,
                EffectWorkingSpace::OutputEncodedSrgb,
            );
            let texture = match context
                .effect_runtime
                .effect_resources
                .acquire(&context.gl, texture_key)
            {
                Ok(texture) => texture,
                Err(error) => {
                    Self::record_source_capture_failure(
                        context,
                        &LifecycleSourceCaptureFailure::new(
                            LifecycleSourceCaptureFailureStage::TargetAllocation,
                            error,
                        ),
                    );
                    frame.record_fallback(
                        lamp,
                        LifecycleRenderFallbackReason::ResolvedSourceAllocation,
                    );
                    continue;
                }
            };
            if let Some(missing_layer) =
                self.first_missing_source_texture(context, &source, lamp, source_visual_rect)
            {
                let _ = context.effect_runtime.effect_resources.release(texture);
                self.missing_source_layers
                    .insert(source.payload_id, missing_layer);
                frame.record_fallback(
                    lamp,
                    LifecycleRenderFallbackReason::LifecycleResourceUnavailable,
                );
                continue;
            }
            if let Err(error) = self.capture_visual_source(
                context,
                &source,
                lamp,
                texture.clone(),
                framebuffer_origin,
            ) {
                Self::record_source_capture_failure(context, &error);
                if let Err(release_error) = context.effect_runtime.effect_resources.release(texture)
                {
                    Self::record_source_capture_failure(
                        context,
                        &LifecycleSourceCaptureFailure::new(
                            LifecycleSourceCaptureFailureStage::TargetRelease,
                            Box::new(release_error),
                        ),
                    );
                }
                frame.record_fallback(lamp, LifecycleRenderFallbackReason::ResolvedSourceCapture);
                continue;
            }
            self.resolved_visual_resources.insert(
                source.resource_payload_id,
                LifecycleResolvedVisualResource {
                    texture,
                    #[cfg(test)]
                    source_signature,
                },
            );
            self.captured_payloads.insert(source.payload_id);
            self.trace_source_event(
                LifecycleRenderDiagnosticStage::RepresentationCaptured,
                &source,
                lamp,
                None,
            );
            self.trace_source_event(
                LifecycleRenderDiagnosticStage::ResourcesReady,
                &source,
                lamp,
                None,
            );
        }
        Ok(())
    }

    fn first_missing_source_texture(
        &self,
        context: &LifecycleRenderContext<'_>,
        source: &LifecycleVisualSource,
        lamp: LifecycleFrameSample,
        source_visual_rect: compositor::PresentationRect,
    ) -> Option<MissingLifecycleSourceResource> {
        let Some(commands) = self.source_commands.get(&source.payload_id) else {
            return Some((LifecycleDiagnosticLayer::ResolvedVisual, None, None));
        };
        let Some(vertices) = self.source_vertices.get(&source.payload_id) else {
            return Some((LifecycleDiagnosticLayer::ResolvedVisual, None, None));
        };
        if commands.is_empty() || vertices.is_empty() {
            return Some((
                LifecycleDiagnosticLayer::Surface,
                Some(source.root_surface_id),
                None,
            ));
        }
        if !commands.iter().any(|command| {
            matches!(command.layer, EglDrawLayer::Surface(surface_id) if surface_id == source.root_surface_id)
        }) {
            return Some((
                LifecycleDiagnosticLayer::Surface,
                Some(source.root_surface_id),
                None,
            ));
        }
        let Some(source_damage) = presentation_rect_to_effect_rect(source_visual_rect) else {
            return Some((LifecycleDiagnosticLayer::ResolvedVisual, None, None));
        };
        let source_bounds = EglRect::new(
            source_damage.x as f32,
            source_damage.y as f32,
            source_damage.width as f32,
            source_damage.height as f32,
        );
        let mut decisions = Vec::new();
        plan_visibility(commands, source_bounds, &mut decisions);
        let missing = commands
            .iter()
            .zip(decisions)
            .filter(|(_, decision)| *decision == EglVisibilityDecision::Drawable)
            .find_map(|(command, _)| {
                context
                    .texture_for_layer(command.layer, &self.resolved_visual_resources)
                    .is_none()
                    .then(|| {
                        (
                            diagnostic_layer_for_draw_layer(command.layer),
                            match command.layer {
                                EglDrawLayer::Surface(surface_id) => Some(surface_id),
                                _ => None,
                            },
                            match command.layer {
                                EglDrawLayer::DecorationAsset(asset_id) => Some(asset_id),
                                _ => None,
                            },
                        )
                    })
            })
            .or_else(|| {
                (source.root_surface_id != lamp.root_surface_id).then_some((
                    LifecycleDiagnosticLayer::ResolvedVisual,
                    None,
                    None,
                ))
            });
        missing
    }

    pub(super) fn trace_frame_events(&mut self, frame: &LifecycleFrameState, frame_survived: bool) {
        if !lifecycle_render_diagnostics_enabled() {
            return;
        }
        let failed_payloads = frame
            .fallbacks
            .failed
            .iter()
            .map(|entry| entry.payload_id)
            .collect::<HashSet<_>>();
        for fallback in &frame.fallbacks.failed {
            let Some(sample) = frame.sample_for_identity(fallback.presentation_identity) else {
                continue;
            };
            self.trace_diagnostic(
                LifecycleRenderDiagnosticStage::Fallback,
                fallback.window_id,
                fallback.root_surface_id,
                fallback.presentation_identity,
                fallback.payload_id,
                fallback.effect,
                sample.direction,
                self.visual_sources
                    .get(&fallback.presentation_identity)
                    .cloned(),
                Some(fallback.reason),
            );
        }
        for evidence in &frame.evidence.consumed {
            if failed_payloads.contains(&evidence.payload_id) {
                continue;
            }
            let Some(sample) = frame.sample_for_identity(evidence.presentation_identity) else {
                continue;
            };
            let source = self
                .visual_sources
                .get(&evidence.presentation_identity)
                .cloned();
            self.trace_diagnostic(
                LifecycleRenderDiagnosticStage::Drawn,
                evidence.window_id,
                evidence.root_surface_id,
                evidence.presentation_identity,
                evidence.payload_id,
                sample.effect,
                sample.direction,
                source.clone(),
                None,
            );
            if frame_survived {
                self.trace_diagnostic(
                    LifecycleRenderDiagnosticStage::FrameSurvived,
                    evidence.window_id,
                    evidence.root_surface_id,
                    evidence.presentation_identity,
                    evidence.payload_id,
                    sample.effect,
                    sample.direction,
                    source,
                    None,
                );
            }
        }
    }

    fn trace_source_event(
        &mut self,
        stage: LifecycleRenderDiagnosticStage,
        source: &LifecycleVisualSource,
        sample: LifecycleFrameSample,
        reason: Option<LifecycleRenderFallbackReason>,
    ) {
        if !lifecycle_render_diagnostics_enabled() {
            return;
        }
        self.trace_diagnostic(
            stage,
            source.window_id,
            source.root_surface_id,
            source.presentation_identity,
            source.payload_id,
            sample.effect,
            sample.direction,
            Some(source.clone()),
            reason,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn trace_diagnostic(
        &mut self,
        stage: LifecycleRenderDiagnosticStage,
        window_id: compositor::WindowId,
        root_surface_id: u32,
        presentation_identity: oblivion_one::presentation_animation::PresentationRetainedVisualIdentity,
        payload_id: compositor::PresentationRetainedVisualPayloadId,
        effect: LifecycleEffectKind,
        direction: LifecycleDirection,
        source: Option<LifecycleVisualSource>,
        reason: Option<LifecycleRenderFallbackReason>,
    ) {
        if !self.diagnostics_emitted.insert((payload_id, stage)) {
            return;
        }
        let metrics = self.source_metrics.get(&payload_id).copied();
        let commands = self.source_commands.get(&payload_id);
        let surface_command_count = commands.map_or(0, |commands| {
            commands
                .iter()
                .filter(|command| matches!(command.layer, EglDrawLayer::Surface(_)))
                .count()
        });
        let decoration_command_count = commands.map_or(0, |commands| {
            commands
                .iter()
                .filter(|command| {
                    matches!(
                        command.layer,
                        EglDrawLayer::DecorationAsset(_) | EglDrawLayer::SolidRgba(_)
                    )
                })
                .count()
        });
        let (missing_layer, missing_surface_id, missing_decoration_asset_id) = self
            .missing_source_layers
            .get(&payload_id)
            .copied()
            .map_or(("none", "none".to_owned(), "none".to_owned()), |missing| {
                (
                    missing.0.as_str(),
                    missing
                        .1
                        .map_or_else(|| "none".to_owned(), |id| id.to_string()),
                    missing
                        .2
                        .map_or_else(|| "none".to_owned(), |id| id.to_string()),
                )
            });
        let buffer_sources = metrics.map_or_else(String::new, |metrics| {
            [
                (metrics.buffer_source_shm, "shm"),
                (metrics.buffer_source_dmabuf, "dmabuf"),
            ]
            .into_iter()
            .filter_map(|(present, name)| present.then_some(name))
            .collect::<Vec<_>>()
            .join(",")
        });
        let visual_source_kind = source
            .as_ref()
            .map_or("unknown", |source| match source.kind {
                LifecycleVisualSourceKind::NoOwnedEffects => "no_owned_effects",
                LifecycleVisualSourceKind::ResolvedOwnedEffects => "resolved_owned_effects",
            });
        let reason = reason.map_or("none", lifecycle_fallback_reason_label);
        let effect = match effect {
            LifecycleEffectKind::Lamp => "lamp",
            LifecycleEffectKind::Squash => "squash",
        };
        let direction = match direction {
            LifecycleDirection::Minimize => "minimize",
            LifecycleDirection::Restore => "restore",
        };
        eprintln!(
            "typhon.lifecycle_render event={} window_id={window_id:?} root_surface_id={root_surface_id} presentation_identity={presentation_identity:?} payload_id={payload_id:?} effect={effect} direction={direction} backend=egl-gles reason={reason} visual_source_kind={visual_source_kind} lifecycle_surface_count={} surface_command_count={surface_command_count} decoration_command_count={decoration_command_count} buffer_sources={buffer_sources} missing_layer={missing_layer} missing_surface_id={missing_surface_id} missing_decoration_asset_id={missing_decoration_asset_id}",
            stage.as_str(),
            metrics.map_or(0, |metrics| metrics.lifecycle_surface_count),
        );
    }

    pub(super) fn capture_visual_source(
        &self,
        context: &mut LifecycleRenderContext<'_>,
        source: &LifecycleVisualSource,
        lamp: LifecycleFrameSample,
        target: PooledEffectTexture,
        framebuffer_origin: OutputFramebufferOrigin,
    ) -> Result<(), LifecycleSourceCaptureFailure> {
        clear_effect_texture(context, &target).map_err(|error| {
            LifecycleSourceCaptureFailure::new(
                LifecycleSourceCaptureFailureStage::TargetClear,
                error,
            )
        })?;
        let scratch_key = EffectTextureKey::new(
            context.scene_state.current_size.0.max(1),
            context.scene_state.current_size.1.max(1),
            EffectTextureFormat::Rgba8,
            EffectTextureFilter::Linear,
            EffectWorkingSpace::OutputEncodedSrgb,
        );
        let scratch = context
            .effect_runtime
            .effect_resources
            .acquire(&context.gl, scratch_key)
            .map_err(|error| {
                LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::ScratchAllocation,
                    error,
                )
            })?;
        let result = (|| {
            clear_effect_texture(context, &scratch).map_err(|error| {
                LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::ScratchClear,
                    error,
                )
            })?;
            let scratch_framebuffer = context
                .effect_runtime
                .effect_resources
                .bind_lifecycle_composition_target(&context.gl, &scratch)
                .map_err(|error| {
                    LifecycleSourceCaptureFailure::new(
                        LifecycleSourceCaptureFailureStage::ScratchTarget,
                        error,
                    )
                })?;
            let targets = EffectExecutionTargets {
                baseline_read: EffectFramebufferTarget::new(
                    context.scene_state.active_output_framebuffer,
                ),
                composition_draw: EffectFramebufferTarget::new(Some(scratch_framebuffer)),
            };
            if !targets.uses_separate_targets() {
                return Err(LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::ScratchTarget,
                    io::Error::other("lifecycle scratch aliases the active output framebuffer"),
                ));
            }

            let source_vertices = self
                .source_vertices
                .get(&source.payload_id)
                .cloned()
                .unwrap_or_default();
            let source_commands = self
                .source_commands
                .get(&source.payload_id)
                .cloned()
                .unwrap_or_default();
            if source_vertices.is_empty() || source_commands.is_empty() {
                return Err(LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::MissingSourceCommands,
                    io::Error::other("lifecycle visual source has no draw commands"),
                ));
            }

            let saved_vertices =
                std::mem::replace(&mut context.scene_state.vertices, source_vertices);
            let saved_commands =
                std::mem::replace(&mut context.scene_state.commands, source_commands);
            let saved_output_framebuffer = context.scene_state.active_output_framebuffer;
            let saved_output_texture = context.scene_state.active_output_texture;
            context.scene_state.scene_geometry_dirty = true;
            let draw_result = (|| {
                let source_damage =
                    lifecycle_visual_effect_damage(lamp.visual_group.presented_source_visual_rect);
                let output_bounds = EffectRect::new(
                    0,
                    0,
                    context.scene_state.current_size.0.max(1),
                    context.scene_state.current_size.1.max(1),
                )
                .expect("non-zero renderer dimensions must form valid effect bounds");
                let plan = compile_frame_execution_plan(
                    &source.effect_scene,
                    &source_damage,
                    output_bounds,
                    &context.effect_runtime.effect_registry,
                )
                .map_err(|error| {
                    LifecycleSourceCaptureFailure::new(
                        LifecycleSourceCaptureFailureStage::GraphCompile,
                        error,
                    )
                })?;
                let source_output_rect = lifecycle_visual_output_rect(
                    lamp.visual_group.presented_source_visual_rect,
                    output_bounds,
                );
                match plan {
                    FrameExecutionPlan::EffectGraph(graph) => {
                        let demand = plan_effect_execution_demand(&graph, &source_damage, true);
                        let selection = effects::select_effect_execution(&graph, &demand);
                        {
                            let mut execution_context =
                                context.effect_execution_context(&self.resolved_visual_resources);
                            effects::execute_effect_graph_for_lifecycle(
                                &mut execution_context,
                                &graph,
                                targets,
                                framebuffer_origin,
                                &[source_output_rect],
                                &demand,
                                &selection,
                            )
                        }
                        .map_err(|error| {
                            LifecycleSourceCaptureFailure::new(
                                LifecycleSourceCaptureFailureStage::EffectExecution,
                                error,
                            )
                        })?;
                    }
                    FrameExecutionPlan::LegacyScene => {
                        let scratch_texture = context
                            .effect_runtime
                            .effect_resources
                            .texture(&scratch)
                            .ok_or_else(|| {
                                LifecycleSourceCaptureFailure::new(
                                    LifecycleSourceCaptureFailureStage::ScratchTarget,
                                    io::Error::other("lifecycle scratch texture is unavailable"),
                                )
                            })?;
                        context.scene_state.active_output_framebuffer = Some(scratch_framebuffer);
                        context.scene_state.active_output_texture = Some(scratch_texture);
                        context.establish_ordinary_scene_state();
                        {
                            let mut execution_context =
                                context.effect_execution_context(&self.resolved_visual_resources);
                            execution_context.draw_command_batch(true, Some(source_output_rect))
                        }
                        .map_err(|error| {
                            LifecycleSourceCaptureFailure::new(
                                LifecycleSourceCaptureFailureStage::EffectExecution,
                                error,
                            )
                        })?;
                    }
                }
                copy_framebuffer_region_to_texture(
                    context,
                    &target,
                    lamp.visual_group.presented_source_visual_rect,
                    framebuffer_origin,
                    targets.composition_draw,
                    targets.composition_draw,
                )
                .map_err(|error| {
                    LifecycleSourceCaptureFailure::new(
                        LifecycleSourceCaptureFailureStage::TargetCopy,
                        error,
                    )
                })?;
                Ok(())
            })();
            context.scene_state.active_output_framebuffer = saved_output_framebuffer;
            context.scene_state.active_output_texture = saved_output_texture;
            context.scene_state.vertices = saved_vertices;
            context.scene_state.commands = saved_commands;
            // The temporary source draw replaced the scene VBO contents.
            // Force the normal scene cache to upload its unchanged geometry
            // before the next ordinary scene draw.
            context.scene_state.scene_geometry_dirty = true;
            draw_result
        })();
        context.establish_ordinary_scene_state();
        let release_result = context
            .effect_runtime
            .effect_resources
            .release(scratch)
            .map_err(|error| {
                LifecycleSourceCaptureFailure::new(
                    LifecycleSourceCaptureFailureStage::ScratchRelease,
                    Box::new(error),
                )
            });
        match (result, release_result) {
            (Err(error), Err(release_error)) => {
                Self::record_source_capture_failure(context, &release_error);
                Err(error)
            }
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

impl LifecycleVisualStore {
    fn record_source_capture_failure(
        context: &mut LifecycleRenderContext<'_>,
        failure: &LifecycleSourceCaptureFailure,
    ) {
        context.effect_runtime.effect_trace.event(|| {
            format!(
                "event=lifecycle_source_capture_failure stage={} error={}",
                failure.stage.as_str(),
                failure.source,
            )
        });
    }
}

fn reproject_lifecycle_source_commands(
    vertices: &mut [EglTexturedVertex],
    commands: &mut [EglDrawCommand],
    canonical_client_rect: compositor::PresentationRect,
    presented_source_client_rect: compositor::PresentationRect,
    output_scale: f64,
    output_size: (u32, u32),
    framebuffer_origin: OutputFramebufferOrigin,
) {
    let full = scaled_presentation_rect(canonical_client_rect, output_scale);
    let source = scaled_presentation_rect(presented_source_client_rect, output_scale);
    let map_point = |point: [f64; 2]| {
        [
            source.x() + (point[0] - full.x()) * source.width() / full.width(),
            source.y() + (point[1] - full.y()) * source.height() / full.height(),
        ]
    };
    for vertex in vertices {
        let output_point = ndc_to_output_point(vertex.position, output_size, framebuffer_origin);
        let mapped = map_point(output_point);
        vertex.position = output_point_to_ndc(mapped, output_size, framebuffer_origin);
    }
    for command in commands {
        let left = f64::from(command.bounds.x());
        let top = f64::from(command.bounds.y());
        let right = left + f64::from(command.bounds.width());
        let bottom = top + f64::from(command.bounds.height());
        let mapped_top_left = map_point([left, top]);
        let mapped_bottom_right = map_point([right, bottom]);
        command.bounds = EglRect::new(
            mapped_top_left[0] as f32,
            mapped_top_left[1] as f32,
            (mapped_bottom_right[0] - mapped_top_left[0]) as f32,
            (mapped_bottom_right[1] - mapped_top_left[1]) as f32,
        );
        // A lifecycle source is a non-linear visual-group input. It must not
        // contribute a rectangular opaque region to ordinary occlusion.
        command.opaque_regions.clear();
    }
}

#[cfg(test)]
fn lifecycle_visual_source_signature(
    source: &LifecycleVisualSource,
    lamp: LifecycleFrameSample,
    output_scale: f64,
) -> u64 {
    let mut signature = source.effect_scene.signature ^ source.payload_id.get();
    for value in [
        lamp.visual_group.canonical_client_rect.x().to_bits(),
        lamp.visual_group.canonical_client_rect.y().to_bits(),
        lamp.visual_group.canonical_client_rect.width().to_bits(),
        lamp.visual_group.canonical_client_rect.height().to_bits(),
        lamp.visual_group.canonical_visual_rect.x().to_bits(),
        lamp.visual_group.canonical_visual_rect.y().to_bits(),
        lamp.visual_group.canonical_visual_rect.width().to_bits(),
        lamp.visual_group.canonical_visual_rect.height().to_bits(),
        lamp.visual_group.presented_source_client_rect.x().to_bits(),
        lamp.visual_group.presented_source_client_rect.y().to_bits(),
        lamp.visual_group
            .presented_source_client_rect
            .width()
            .to_bits(),
        lamp.visual_group
            .presented_source_client_rect
            .height()
            .to_bits(),
        lamp.visual_group.presented_source_visual_rect.x().to_bits(),
        lamp.visual_group.presented_source_visual_rect.y().to_bits(),
        lamp.visual_group
            .presented_source_visual_rect
            .width()
            .to_bits(),
        lamp.visual_group
            .presented_source_visual_rect
            .height()
            .to_bits(),
        output_scale.to_bits(),
    ] {
        signature ^= value;
        signature = signature.wrapping_mul(0x1000_0000_01b3);
    }
    signature
}

fn lifecycle_visual_texture_size(rect: compositor::PresentationRect) -> Option<(u32, u32)> {
    let width = rect.width().ceil();
    let height = rect.height().ceil();
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let width = width.min(f64::from(u32::MAX)) as u32;
    let height = height.min(f64::from(u32::MAX)) as u32;
    (width != 0 && height != 0).then_some((width, height))
}

fn lifecycle_visual_effect_damage(rect: compositor::PresentationRect) -> EffectRegion {
    presentation_rect_to_effect_rect(rect)
        .map(EffectRegion::from_rect)
        .unwrap_or_default()
}

fn presentation_rect_to_effect_rect(rect: compositor::PresentationRect) -> Option<EffectRect> {
    let x = rect.x().floor();
    let y = rect.y().floor();
    let right = (rect.x() + rect.width()).ceil();
    let bottom = (rect.y() + rect.height()).ceil();
    if !x.is_finite()
        || !y.is_finite()
        || !right.is_finite()
        || !bottom.is_finite()
        || x < f64::from(i32::MIN)
        || y < f64::from(i32::MIN)
        || right > f64::from(i32::MAX)
        || bottom > f64::from(i32::MAX)
    {
        return None;
    }
    EffectRect::new(
        x as i32,
        y as i32,
        (right - x).max(1.0).min(f64::from(u32::MAX)) as u32,
        (bottom - y).max(1.0).min(f64::from(u32::MAX)) as u32,
    )
}

fn lifecycle_visual_output_rect(
    rect: compositor::PresentationRect,
    output_bounds: EffectRect,
) -> OutputRect {
    let Some(rect) =
        presentation_rect_to_effect_rect(rect).and_then(|rect| rect.intersect(output_bounds))
    else {
        return OutputRect::new(0, 0, 0, 0);
    };
    OutputRect::new(rect.x, rect.y, rect.width, rect.height)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GlBlitRect {
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
}

impl GlBlitRect {
    const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LifecycleOutputBlitPlan {
    output: GlBlitRect,
    texture: GlBlitRect,
}

fn lifecycle_output_copy_region(
    output_size: (u32, u32),
    rect: compositor::PresentationRect,
    texture_size: (u32, u32),
    output_scale: f64,
    framebuffer_origin: OutputFramebufferOrigin,
) -> Option<LifecycleOutputBlitPlan> {
    if output_size.0 == 0 || output_size.1 == 0 || texture_size.0 == 0 || texture_size.1 == 0 {
        return None;
    }
    let scale = output_scale.max(1.0);
    let left = (rect.x() * scale).floor();
    let top = (rect.y() * scale).floor();
    let right = ((rect.x() + rect.width()) * scale).ceil();
    let bottom = ((rect.y() + rect.height()) * scale).ceil();
    if !left.is_finite()
        || !top.is_finite()
        || !right.is_finite()
        || !bottom.is_finite()
        || right <= left
        || bottom <= top
        || left < f64::from(i32::MIN)
        || top < f64::from(i32::MIN)
        || right > f64::from(i32::MAX)
        || bottom > f64::from(i32::MAX)
    {
        return None;
    }
    let left = left as i32;
    let top = top as i32;
    let right = right as i32;
    let bottom = bottom as i32;
    let visible_left = i64::from(left).max(0).min(i64::from(output_size.0));
    let visible_top = i64::from(top).max(0).min(i64::from(output_size.1));
    let visible_right = i64::from(right).max(0).min(i64::from(output_size.0));
    let visible_bottom = i64::from(bottom).max(0).min(i64::from(output_size.1));
    if visible_right <= visible_left || visible_bottom <= visible_top {
        return None;
    }

    let visible_left = i32::try_from(visible_left).ok()?;
    let visible_top = i32::try_from(visible_top).ok()?;
    let visible_right = i32::try_from(visible_right).ok()?;
    let visible_bottom = i32::try_from(visible_bottom).ok()?;
    let output_height = i32::try_from(output_size.1).ok()?;
    let texture_width = i32::try_from(texture_size.0).ok()?;
    let texture_height = i32::try_from(texture_size.1).ok()?;
    let texture_left = visible_left.checked_sub(left)?;
    let texture_right = texture_left.checked_add(visible_right.checked_sub(visible_left)?)?;
    let logical_texture_top = visible_top.checked_sub(top)?;
    let logical_texture_bottom =
        logical_texture_top.checked_add(visible_bottom.checked_sub(visible_top)?)?;
    let texture_low_y = texture_height.checked_sub(logical_texture_bottom)?;
    let texture_high_y = texture_height.checked_sub(logical_texture_top)?;
    if texture_left < 0
        || texture_right > texture_width
        || texture_low_y < 0
        || texture_high_y > texture_height
        || texture_right <= texture_left
        || texture_high_y <= texture_low_y
    {
        return None;
    }

    let output = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => GlBlitRect::new(
            visible_left,
            output_height.checked_sub(visible_bottom)?,
            visible_right,
            output_height.checked_sub(visible_top)?,
        ),
        OutputFramebufferOrigin::TopLeftScanout => {
            GlBlitRect::new(visible_left, visible_top, visible_right, visible_bottom)
        }
    };
    let texture = match framebuffer_origin {
        OutputFramebufferOrigin::BottomLeft => {
            GlBlitRect::new(texture_left, texture_low_y, texture_right, texture_high_y)
        }
        OutputFramebufferOrigin::TopLeftScanout => {
            GlBlitRect::new(texture_left, texture_high_y, texture_right, texture_low_y)
        }
    };
    Some(LifecycleOutputBlitPlan { output, texture })
}

fn clear_effect_texture(
    context: &mut LifecycleRenderContext<'_>,
    target: &PooledEffectTexture,
) -> RendererResult<()> {
    let result = (|| {
        context
            .effect_runtime
            .effect_resources
            .bind_render_target(context.gl, target)?;
        unsafe {
            context
                .gl
                .viewport(0, 0, target.key.width as i32, target.key.height as i32);
            context.gl.disable(glow::SCISSOR_TEST);
            context.gl.clear_color(0.0, 0.0, 0.0, 0.0);
            context.gl.clear(glow::COLOR_BUFFER_BIT);
        }
        context
            .effect_runtime
            .effect_resources
            .unbind_render_target(context.gl);
        Ok(())
    })();
    context.establish_ordinary_scene_state();
    result
}

fn copy_framebuffer_region_to_texture(
    context: &mut LifecycleRenderContext<'_>,
    target: &PooledEffectTexture,
    rect: compositor::PresentationRect,
    framebuffer_origin: OutputFramebufferOrigin,
    source: EffectFramebufferTarget,
    restore_target: EffectFramebufferTarget,
) -> RendererResult<()> {
    let Some(plan) = lifecycle_output_copy_region(
        context.scene_state.current_size,
        rect,
        (target.key.width, target.key.height),
        f64::from(context.effect_runtime.effect_output_scale),
        framebuffer_origin,
    ) else {
        return Ok(());
    };
    let result = (|| {
        let draw_framebuffer = context
            .effect_runtime
            .effect_resources
            .bind_draw_target(context.gl, target)?;
        if source.framebuffer == Some(draw_framebuffer) {
            return Err(io::Error::other("lifecycle source and texture targets alias").into());
        }
        unsafe {
            context.gl.disable(glow::SCISSOR_TEST);
            context
                .gl
                .bind_framebuffer(glow::READ_FRAMEBUFFER, source.framebuffer);
            context
                .gl
                .bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(draw_framebuffer));
            context.gl.blit_framebuffer(
                plan.output.x0,
                plan.output.y0,
                plan.output.x1,
                plan.output.y1,
                plan.texture.x0,
                plan.texture.y0,
                plan.texture.x1,
                plan.texture.y1,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
        }
        Ok(())
    })();
    context.establish_effect_composition_state(restore_target);
    result
}

#[cfg(test)]
mod tests;
