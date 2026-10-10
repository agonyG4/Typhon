use super::*;
use crate::animation_control::{AnimationEffect, AnimationRuntimeCapabilities, AnimationSlot};
#[cfg(test)]
use crate::compositor::decoration::types::DecorationMode;
use crate::presentation_animation::PresentationRetainedVisualKind;
#[cfg(test)]
use crate::presentation_animation::{PresentationGroupTransform, TransitionId};
use crate::window_lifecycle_animation::{
    LifecycleDirection, LifecycleEffectKind, LifecycleFrameSnapshot, LifecycleMotionRequest,
    LifecycleRenderFallbackEntry, LifecycleSceneSample, LifecycleVisualGroup,
    LifecycleVisualSource, LifecycleVisualSourceKind, LifecycleWindowSample,
    ResolvedLifecycleTarget, resolve_lifecycle_target,
};
use std::num::NonZeroU64;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleAdmissionReason {
    EffectDisabled,
    RendererUnavailable,
    NoSceneNode,
    ClockUnavailable,
    MissingSourceGeometry,
    InvalidVisualGeometry,
    NoRenderableRoot,
    MissingPresentationGeneration,
    InvalidSurfaceTopology,
    SourceLeaseUnavailable,
    OffscreenVisual,
    RetainedCaptureFailed,
    ReservationFailed,
    MotionRejected,
    UnexpectedOwnerChange,
}

impl LifecycleAdmissionReason {
    const fn id(self) -> &'static str {
        match self {
            Self::EffectDisabled => "effect_disabled",
            Self::RendererUnavailable => "renderer_unavailable",
            Self::NoSceneNode => "no_scene_node",
            Self::ClockUnavailable => "clock_unavailable",
            Self::MissingSourceGeometry => "missing_source_geometry",
            Self::InvalidVisualGeometry => "invalid_visual_geometry",
            Self::NoRenderableRoot => "no_renderable_root",
            Self::MissingPresentationGeneration => "missing_presentation_generation",
            Self::InvalidSurfaceTopology => "invalid_surface_topology",
            Self::SourceLeaseUnavailable => "source_lease_unavailable",
            Self::OffscreenVisual => "offscreen_visual",
            Self::RetainedCaptureFailed => "retained_capture_failed",
            Self::ReservationFailed => "reservation_failed",
            Self::MotionRejected => "motion_rejected",
            Self::UnexpectedOwnerChange => "unexpected_owner_change",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleAdmissionOutcome {
    Started,
    Skipped(LifecycleAdmissionReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LifecycleAdmissionFailure {
    reason: LifecycleAdmissionReason,
    target_provenance: Option<crate::window_lifecycle_animation::LifecycleTargetProvenance>,
}

impl LifecycleAdmissionFailure {
    const fn new(
        reason: LifecycleAdmissionReason,
        target_provenance: Option<crate::window_lifecycle_animation::LifecycleTargetProvenance>,
    ) -> Self {
        Self {
            reason,
            target_provenance,
        }
    }
}

static LIFECYCLE_DIAGNOSTICS_ENABLED: OnceLock<bool> = OnceLock::new();
static LIFECYCLE_DIAGNOSTICS_LAST_NS: AtomicU64 = AtomicU64::new(0);
const MAX_LIFECYCLE_RENDER_TERMINAL_DIAGNOSTICS: usize = 65_536;

fn lifecycle_diagnostics_enabled() -> bool {
    *LIFECYCLE_DIAGNOSTICS_ENABLED.get_or_init(|| {
        matches!(
            std::env::var("ASTREA_LIFECYCLE_DIAGNOSTICS").as_deref(),
            Ok("1" | "true" | "on")
        )
    })
}

fn lifecycle_diagnostic_budget_available() -> bool {
    let Ok(now_ns) = crate::native::event_loop::monotonic_now_ns() else {
        return false;
    };
    let mut previous = LIFECYCLE_DIAGNOSTICS_LAST_NS.load(Ordering::Relaxed);
    loop {
        if now_ns.saturating_sub(previous) < 100_000_000 {
            return false;
        }
        match LIFECYCLE_DIAGNOSTICS_LAST_NS.compare_exchange_weak(
            previous,
            now_ns,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(observed) => previous = observed,
        }
    }
}

impl CompositorState {
    pub(in crate::compositor) fn lifecycle_diagnostics_enabled(&self) -> bool {
        lifecycle_diagnostics_enabled()
    }

    fn trace_lifecycle_render_terminal_event(
        &mut self,
        stage: crate::compositor::LifecycleRenderTerminalEvent,
        window_id: WindowId,
        root_surface_id: u32,
        presentation_identity: crate::presentation_animation::PresentationRetainedVisualIdentity,
        payload_id: crate::compositor::PresentationRetainedVisualPayloadId,
        effect: LifecycleEffectKind,
        direction: LifecycleDirection,
    ) {
        if !lifecycle_diagnostics_enabled() {
            return;
        }
        if self.lifecycle_render_diagnostic_events.len()
            >= MAX_LIFECYCLE_RENDER_TERMINAL_DIAGNOSTICS
            || !self.lifecycle_render_diagnostic_events.insert((
                presentation_identity,
                payload_id,
                stage,
            ))
        {
            return;
        }
        let stage = match stage {
            crate::compositor::LifecycleRenderTerminalEvent::Presented => "presented",
            crate::compositor::LifecycleRenderTerminalEvent::Retired => "retired",
        };
        let effect = match effect {
            LifecycleEffectKind::Lamp => "lamp",
            LifecycleEffectKind::Squash => "squash",
        };
        let direction = match direction {
            LifecycleDirection::Minimize => "minimize",
            LifecycleDirection::Restore => "restore",
        };
        eprintln!(
            "typhon.lifecycle_render event={stage} window_id={window_id:?} root_surface_id={root_surface_id} presentation_identity={presentation_identity:?} payload_id={payload_id:?} effect={effect} direction={direction} backend=egl-gles"
        );
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one bounded lifecycle ownership diagnostic records the complete transition evidence"
    )]
    pub(in crate::compositor) fn trace_lifecycle_precapture_event(
        &mut self,
        event: super::lifecycle_retained::LifecyclePreCaptureEvent,
        identity: crate::presentation_animation::PresentationRetainedVisualIdentity,
        payload: &super::lifecycle_retained::RetainedLifecyclePayload,
        surface_count: usize,
        dmabuf_surface_count: usize,
        held_release_count: usize,
        frame_batch_id: Option<u64>,
        reason: Option<&str>,
    ) {
        if !lifecycle_diagnostics_enabled()
            || self.lifecycle_precapture_diagnostic_events.len()
                >= MAX_LIFECYCLE_RENDER_TERMINAL_DIAGNOSTICS
            || !self
                .lifecycle_precapture_diagnostic_events
                .insert((payload.payload_id, event))
        {
            return;
        }
        let event = match event {
            super::lifecycle_retained::LifecyclePreCaptureEvent::LeaseCreated => "lease_created",
            super::lifecycle_retained::LifecyclePreCaptureEvent::SourceWithdrawnWhileLeased => {
                "source_withdrawn_while_leased"
            }
            super::lifecycle_retained::LifecyclePreCaptureEvent::TransferredToFrame => {
                "transferred_to_frame"
            }
            super::lifecycle_retained::LifecyclePreCaptureEvent::ReleasedAfterCapture => {
                "released_after_capture"
            }
            super::lifecycle_retained::LifecyclePreCaptureEvent::RestoredToLiveOwner => {
                "restored_to_live_owner"
            }
            super::lifecycle_retained::LifecyclePreCaptureEvent::QueuedAfterCancel => {
                "queued_after_cancel"
            }
            super::lifecycle_retained::LifecyclePreCaptureEvent::DrainedOnShutdown => {
                "drained_on_shutdown"
            }
        };
        eprintln!(
            "typhon.lifecycle_precapture event={event} window_id={:?} root_surface_id={} presentation_identity={identity:?} payload_id={:?} surface_count={surface_count} dmabuf_surface_count={dmabuf_surface_count} held_release_count={held_release_count} frame_batch_id={} reason={}",
            payload.window_id,
            payload.root_surface_id,
            payload.payload_id,
            frame_batch_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "none".to_string()),
            reason.unwrap_or("none"),
        );
    }

    /// Freeze only the compositor-owned effect instances belonging to one
    /// lifecycle visual group before logical minimize removes that group from
    /// the canonical scene. Output-wide effects intentionally do not become a
    /// lifecycle source; they remain owned by the normal output composition.
    pub(in crate::compositor) fn resolved_effect_scene_for_lifecycle_root(
        &self,
        root_surface_id: u32,
    ) -> ResolvedEffectScene {
        let scene = self.resolved_effect_scene();
        let instances = scene
            .instances
            .into_iter()
            .filter(|instance| match instance.anchor {
                EffectAnchor::BeforeSurface(surface_id)
                | EffectAnchor::ReplaceSurface(surface_id)
                | EffectAnchor::AfterSurface(surface_id) => {
                    self.root_surface_id_for_surface(surface_id) == root_surface_id
                }
                EffectAnchor::OutputPostProcess => false,
            })
            .collect();
        ResolvedEffectScene::new(scene.generation, instances)
    }

    pub(in crate::compositor) fn map_effect_scene_to_presentation(
        scene: &ResolvedEffectScene,
        canonical_rect: PresentationRect,
        presented_rect: PresentationRect,
    ) -> ResolvedEffectScene {
        let transform = PresentationGroupTransform::with_scene_node(
            crate::core::SceneNodeId::from_raw(1).expect("effect scene node"),
            0,
            crate::presentation_animation::PresentationTransactionId::new(NonZeroU64::MIN),
            TransitionId::new(NonZeroU64::MIN),
            canonical_rect,
            presented_rect,
            false,
        );
        let instances = scene
            .instances
            .iter()
            .cloned()
            .map(|mut instance| {
                instance.region = super::super::effects::map_effect_region(
                    transform,
                    &instance.region,
                    instance.target_bounds,
                );
                if let Some(target_bounds) =
                    super::super::effects::map_effect_rect(transform, instance.target_bounds)
                {
                    instance.target_bounds = target_bounds;
                }
                instance.signature =
                    instance.signature.wrapping_mul(0x0000_0100_0000_01b3) ^ transform.signature();
                instance
            })
            .collect();
        ResolvedEffectScene::new(scene.generation, instances)
    }
}

#[cfg(test)]
#[path = "lifecycle_animation_tests.rs"]
mod tests;

impl CompositorState {
    pub(in crate::compositor) fn lifecycle_effect(
        &self,
        direction: LifecycleDirection,
    ) -> AnimationEffect {
        let slot = match direction {
            LifecycleDirection::Minimize => AnimationSlot::WindowMinimize,
            LifecycleDirection::Restore => AnimationSlot::WindowRestore,
        };
        self.animation_control
            .effective_effect(slot, self.animation_runtime_capabilities())
    }

    pub(in crate::compositor) fn animation_runtime_capabilities(
        &self,
    ) -> AnimationRuntimeCapabilities {
        AnimationRuntimeCapabilities {
            lamp_renderer: self.lifecycle_animation_renderer_available == Some(true),
            squash_renderer: self.lifecycle_squash_renderer_available == Some(true),
        }
    }

    pub(in crate::compositor) fn lifecycle_effect_kind_for_window(
        &self,
        window_id: WindowId,
        direction: LifecycleDirection,
    ) -> Option<LifecycleEffectKind> {
        let active_effect = self
            .scene_node_id_for_window_group(window_id)
            .and_then(|scene_node_id| {
                self.presentation_animator.active_retained_visual(
                    scene_node_id,
                    PresentationRetainedVisualKind::WindowLifecycle,
                )
            })
            .and_then(|identity| {
                let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
                self.window_lifecycle_animator
                    .sample(identity, now)
                    .map(|sample| sample.effect)
            });
        active_effect.or_else(|| {
            LifecycleEffectKind::from_animation_effect(self.lifecycle_effect(direction))
        })
    }

    fn lifecycle_effect_skip_reason(
        &self,
        direction: LifecycleDirection,
    ) -> LifecycleAdmissionReason {
        if !self.animation_control.enabled() {
            return LifecycleAdmissionReason::EffectDisabled;
        }
        let slot = match direction {
            LifecycleDirection::Minimize => AnimationSlot::WindowMinimize,
            LifecycleDirection::Restore => AnimationSlot::WindowRestore,
        };
        let capabilities = self.animation_runtime_capabilities();
        match self
            .animation_control
            .configuration()
            .requested_effect(slot)
            .0
        {
            AnimationEffect::MinimizeLamp if !capabilities.lamp_renderer => {
                LifecycleAdmissionReason::RendererUnavailable
            }
            AnimationEffect::MinimizeSquash if !capabilities.squash_renderer => {
                LifecycleAdmissionReason::RendererUnavailable
            }
            _ => LifecycleAdmissionReason::EffectDisabled,
        }
    }

    fn record_lifecycle_admission(
        &self,
        window_id: WindowId,
        root_surface_id: u32,
        direction: LifecycleDirection,
        selected_effect: Option<LifecycleEffectKind>,
        target_provenance: Option<crate::window_lifecycle_animation::LifecycleTargetProvenance>,
        reused_payload: bool,
        outcome: LifecycleAdmissionOutcome,
    ) {
        if !lifecycle_diagnostics_enabled() || !lifecycle_diagnostic_budget_available() {
            return;
        }
        let slot = match direction {
            LifecycleDirection::Minimize => AnimationSlot::WindowMinimize,
            LifecycleDirection::Restore => AnimationSlot::WindowRestore,
        };
        let configured_effect = self
            .animation_control
            .configuration()
            .requested_effect(slot)
            .0
            .id();
        let capabilities = self.animation_runtime_capabilities();
        let (status, reason) = match outcome {
            LifecycleAdmissionOutcome::Started => ("started", "none"),
            LifecycleAdmissionOutcome::Skipped(reason) => ("skipped", reason.id()),
        };
        let backend = self.window(window_id).map_or("missing", |window| {
            if matches!(window.backend, WindowBackend::X11(_)) {
                "x11"
            } else {
                "xdg"
            }
        });
        let operation = match direction {
            LifecycleDirection::Minimize => "minimize",
            LifecycleDirection::Restore => "restore",
        };
        let selected_effect = selected_effect.map_or("none", |effect| match effect {
            LifecycleEffectKind::Lamp => "lamp",
            LifecycleEffectKind::Squash => "squash",
        });
        let target_provenance = target_provenance.map_or("unavailable", |provenance| {
            match provenance {
                crate::window_lifecycle_animation::LifecycleTargetProvenance::DockPublished => {
                    "dock_published"
                }
                crate::window_lifecycle_animation::LifecycleTargetProvenance::RememberedDestination => {
                    "remembered_destination"
                }
                crate::window_lifecycle_animation::LifecycleTargetProvenance::PointerFallback => {
                    "pointer_fallback"
                }
                crate::window_lifecycle_animation::LifecycleTargetProvenance::OutputFallback => {
                    "output_fallback"
                }
            }
        });
        eprintln!(
            "typhon.lifecycle_admission event=admission window_id={} root_surface_id={} backend={} operation={} configured_effect={} selected_effect={} capability_lamp={} capability_squash={} target_provenance={} reused_payload={} outcome={} reason={}",
            window_id.get(),
            root_surface_id,
            backend,
            operation,
            configured_effect,
            selected_effect,
            capabilities.lamp_renderer,
            capabilities.squash_renderer,
            target_provenance,
            reused_payload,
            status,
            reason,
        );
    }

    pub(in crate::compositor) fn lifecycle_minimize_source_rect(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentationRect> {
        self.presented_window_geometry(root_surface_id)
            .map(PresentedWindowGeometry::presented_rect)
            .or_else(|| self.current_presentation_rect_for_root(root_surface_id))
    }

    fn published_lifecycle_anchor_rect(&self, window_id: WindowId) -> Option<PresentationRect> {
        let anchor = self.astrea_toplevel_publisher.minimize_anchor(window_id)?;
        PresentationRect::new(
            f64::from(anchor.x),
            f64::from(anchor.y),
            f64::from(anchor.width),
            f64::from(anchor.height),
        )
    }

    fn recent_lifecycle_pointer(&self) -> Option<(f64, f64)> {
        const POINTER_FRESHNESS_USEC: u64 = 30_000_000;
        let timestamp_usec = self.last_pointer_motion_usec?;
        let now_usec = crate::native::event_loop::monotonic_now_ns()
            .ok()?
            .checked_div(1_000)?;
        let age_usec = now_usec.checked_sub(timestamp_usec)?;
        (age_usec <= POINTER_FRESHNESS_USEC
            && self.last_pointer_x.is_finite()
            && self.last_pointer_y.is_finite())
        .then_some((self.last_pointer_x, self.last_pointer_y))
    }

    pub(in crate::compositor) fn resolve_lifecycle_target(
        &self,
        window_id: WindowId,
        presented_source_client_rect: PresentationRect,
    ) -> Option<ResolvedLifecycleTarget> {
        let remembered = self
            .window(window_id)
            .and_then(|window| window.state.minimized_lifecycle_target());
        resolve_lifecycle_target(
            self.published_lifecycle_anchor_rect(window_id),
            remembered,
            presented_source_client_rect,
            self.recent_lifecycle_pointer(),
            self.output_size.width,
            self.output_size.height,
        )
    }

    pub(in crate::compositor) fn lifecycle_window_rect(
        &self,
        root_surface_id: u32,
    ) -> Option<PresentationRect> {
        self.current_presentation_rect_for_root(root_surface_id)
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(in crate::compositor) fn begin_lifecycle_minimize(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        presented_source_client_rect: Option<PresentationRect>,
        canonical_client_rect: Option<PresentationRect>,
        visual_group: Option<LifecycleVisualGroup>,
        resolved_effect_scene: ResolvedEffectScene,
        lifecycle_decorations: Vec<DecorationRenderInstance>,
    ) {
        self.begin_lifecycle_minimize_with_preparation(
            window_id,
            root_surface_id,
            presented_source_client_rect,
            canonical_client_rect,
            visual_group,
            resolved_effect_scene,
            lifecycle_decorations,
            None,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn begin_lifecycle_minimize_with_preparation(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        presented_source_client_rect: Option<PresentationRect>,
        canonical_client_rect: Option<PresentationRect>,
        visual_group: Option<LifecycleVisualGroup>,
        resolved_effect_scene: ResolvedEffectScene,
        lifecycle_decorations: Vec<DecorationRenderInstance>,
        precapture_preparation: Option<
            Result<
                super::lifecycle_precapture::LifecyclePreCapturePreparation,
                LifecycleAdmissionReason,
            >,
        >,
    ) {
        let Some(effect) =
            self.lifecycle_effect_kind_for_window(window_id, LifecycleDirection::Minimize)
        else {
            self.record_lifecycle_admission(
                window_id,
                root_surface_id,
                LifecycleDirection::Minimize,
                None,
                self.window(window_id)
                    .and_then(|window| window.state.minimized_lifecycle_target())
                    .map(|target| target.provenance),
                false,
                LifecycleAdmissionOutcome::Skipped(
                    self.lifecycle_effect_skip_reason(LifecycleDirection::Minimize),
                ),
            );
            return;
        };
        let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id) else {
            self.record_lifecycle_admission(
                window_id,
                root_surface_id,
                LifecycleDirection::Minimize,
                Some(effect),
                visual_group.map(|group| group.anchor_provenance),
                false,
                LifecycleAdmissionOutcome::Skipped(LifecycleAdmissionReason::NoSceneNode),
            );
            return;
        };
        let Some(now) = AnimationTime::monotonic_now() else {
            self.record_lifecycle_admission(
                window_id,
                root_surface_id,
                LifecycleDirection::Minimize,
                Some(effect),
                visual_group.map(|group| group.anchor_provenance),
                false,
                LifecycleAdmissionOutcome::Skipped(LifecycleAdmissionReason::ClockUnavailable),
            );
            return;
        };
        let speed = self.animation_control.configuration().speed;
        let frozen_decoration = lifecycle_decorations
            .into_iter()
            .find(|decoration| decoration.root_surface_id() == root_surface_id);
        let admission = self.install_lifecycle_motion_with_preparation(
            scene_node_id,
            window_id,
            root_surface_id,
            presented_source_client_rect,
            canonical_client_rect,
            visual_group,
            LifecycleDirection::Minimize,
            effect,
            resolved_effect_scene,
            frozen_decoration,
            now,
            speed,
            precapture_preparation,
            None,
        );
        let (_identity, payload, reused_payload) = match admission {
            Ok(admission) => admission,
            Err(failure) => {
                self.record_lifecycle_admission(
                    window_id,
                    root_surface_id,
                    LifecycleDirection::Minimize,
                    Some(effect),
                    failure.target_provenance,
                    false,
                    LifecycleAdmissionOutcome::Skipped(failure.reason),
                );
                return;
            }
        };
        self.record_lifecycle_admission(
            window_id,
            root_surface_id,
            LifecycleDirection::Minimize,
            Some(effect),
            Some(payload.visual_group.anchor_provenance),
            reused_payload,
            LifecycleAdmissionOutcome::Started,
        );
        // Lamp takes over Group properties only after the retained transaction
        // and lifecycle executor have both accepted the same identity.
        self.presentation_animator.cancel_all(scene_node_id);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn install_lifecycle_motion(
        &mut self,
        scene_node_id: crate::core::SceneNodeId,
        window_id: WindowId,
        root_surface_id: u32,
        presented_source_client_rect: Option<PresentationRect>,
        canonical_client_rect: Option<PresentationRect>,
        visual_group: Option<LifecycleVisualGroup>,
        direction: LifecycleDirection,
        effect: LifecycleEffectKind,
        resolved_effect_scene: ResolvedEffectScene,
        frozen_decoration: Option<DecorationRenderInstance>,
        now: AnimationTime,
        speed: f64,
    ) -> Result<
        (
            crate::presentation_animation::PresentationRetainedVisualIdentity,
            std::sync::Arc<super::lifecycle_retained::RetainedLifecyclePayload>,
            bool,
        ),
        LifecycleAdmissionFailure,
    > {
        self.install_lifecycle_motion_with_preparation(
            scene_node_id,
            window_id,
            root_surface_id,
            presented_source_client_rect,
            canonical_client_rect,
            visual_group,
            direction,
            effect,
            resolved_effect_scene,
            frozen_decoration,
            now,
            speed,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn install_lifecycle_motion_with_preparation(
        &mut self,
        scene_node_id: crate::core::SceneNodeId,
        window_id: WindowId,
        root_surface_id: u32,
        presented_source_client_rect: Option<PresentationRect>,
        canonical_client_rect: Option<PresentationRect>,
        visual_group: Option<LifecycleVisualGroup>,
        direction: LifecycleDirection,
        effect: LifecycleEffectKind,
        resolved_effect_scene: ResolvedEffectScene,
        frozen_decoration: Option<DecorationRenderInstance>,
        now: AnimationTime,
        speed: f64,
        mut supplied_preparation: Option<
            Result<
                super::lifecycle_precapture::LifecyclePreCapturePreparation,
                LifecycleAdmissionReason,
            >,
        >,
        resolved_source_payload_id: Option<crate::compositor::PresentationRetainedVisualPayloadId>,
    ) -> Result<
        (
            crate::presentation_animation::PresentationRetainedVisualIdentity,
            std::sync::Arc<super::lifecycle_retained::RetainedLifecyclePayload>,
            bool,
        ),
        LifecycleAdmissionFailure,
    > {
        let previous_identity = self.presentation_animator.active_retained_visual(
            scene_node_id,
            PresentationRetainedVisualKind::WindowLifecycle,
        );
        let previous_payload = match previous_identity {
            Some(identity) => {
                let Some(payload) = self.retained_lifecycle_payloads.get_exact(identity) else {
                    return Err(LifecycleAdmissionFailure::new(
                        LifecycleAdmissionReason::UnexpectedOwnerChange,
                        None,
                    ));
                };
                if payload.window_id != window_id {
                    return Err(LifecycleAdmissionFailure::new(
                        LifecycleAdmissionReason::UnexpectedOwnerChange,
                        Some(payload.visual_group.anchor_provenance),
                    ));
                }
                Some(std::sync::Arc::clone(payload))
            }
            None => None,
        };
        let visual_group = if let Some(payload) = previous_payload.as_ref() {
            payload.visual_group
        } else if let Some(visual_group) = visual_group {
            visual_group
        } else {
            let canonical_client_rect = canonical_client_rect
                .or_else(|| self.lifecycle_window_rect(root_surface_id))
                .ok_or_else(|| {
                    LifecycleAdmissionFailure::new(
                        LifecycleAdmissionReason::MissingSourceGeometry,
                        None,
                    )
                })?;
            let presented_source_client_rect = match direction {
                LifecycleDirection::Minimize => presented_source_client_rect
                    .or_else(|| self.lifecycle_minimize_source_rect(root_surface_id))
                    .ok_or_else(|| {
                        LifecycleAdmissionFailure::new(
                            LifecycleAdmissionReason::MissingSourceGeometry,
                            None,
                        )
                    })?,
                LifecycleDirection::Restore => canonical_client_rect,
            };
            let target = self
                .resolve_lifecycle_target(window_id, presented_source_client_rect)
                .ok_or_else(|| {
                    LifecycleAdmissionFailure::new(
                        LifecycleAdmissionReason::InvalidVisualGeometry,
                        None,
                    )
                })?;
            LifecycleVisualGroup::from_resolved_effect_bounds(
                effect,
                canonical_client_rect,
                canonical_client_rect,
                presented_source_client_rect,
                target,
                self.output_size.width,
                self.output_size.height,
            )
            .ok_or_else(|| {
                LifecycleAdmissionFailure::new(
                    LifecycleAdmissionReason::InvalidVisualGeometry,
                    Some(target.provenance),
                )
            })?
        };
        let target_provenance = Some(visual_group.anchor_provenance);
        let visual_intersects_output =
            crate::window_lifecycle_animation::lifecycle_visual_bounds_intersect_output(
                effect,
                visual_group,
                0.0,
                self.output_size.width,
                self.output_size.height,
            );
        if !visual_intersects_output {
            return Err(LifecycleAdmissionFailure::new(
                LifecycleAdmissionReason::OffscreenVisual,
                target_provenance,
            ));
        }
        let mut fresh_source_preparation = if previous_payload.is_none()
            && resolved_source_payload_id.is_none()
        {
            let preparation = match supplied_preparation.take() {
                Some(Ok(preparation)) => preparation,
                Some(Err(reason)) => {
                    return Err(LifecycleAdmissionFailure::new(reason, target_provenance));
                }
                None => self
                    .capture_lifecycle_precapture_source(window_id, root_surface_id)
                    .map_err(|reason| LifecycleAdmissionFailure::new(reason, target_provenance))?,
            };
            Some(preparation)
        } else {
            None
        };

        let identity = self
            .presentation_animator
            .begin_retained_visual(
                scene_node_id,
                PresentationRetainedVisualKind::WindowLifecycle,
                now,
            )
            .map_err(|_| {
                LifecycleAdmissionFailure::new(
                    LifecycleAdmissionReason::ReservationFailed,
                    target_provenance,
                )
            })?;
        let activated_previous = match self
            .presentation_animator
            .activate_retained_visual_exact(identity)
        {
            Ok(previous) => previous,
            Err(_) => {
                self.presentation_animator
                    .retire_retained_visual_exact(identity);
                return Err(LifecycleAdmissionFailure::new(
                    LifecycleAdmissionReason::ReservationFailed,
                    target_provenance,
                ));
            }
        };
        if activated_previous != previous_identity {
            let restored = self
                .presentation_animator
                .restore_retained_visual_owner_exact(identity, activated_previous);
            let retired = self
                .presentation_animator
                .retire_retained_visual_exact(identity);
            debug_assert!(
                restored && retired,
                "unexpected lifecycle owner changed during start"
            );
            return Err(LifecycleAdmissionFailure::new(
                LifecycleAdmissionReason::UnexpectedOwnerChange,
                target_provenance,
            ));
        }

        let payload = if let Some(payload) = previous_payload {
            payload
        } else {
            let resource_payload_id = resolved_source_payload_id.unwrap_or_else(|| {
                crate::compositor::PresentationRetainedVisualPayloadId::from_origin_identity(
                    identity,
                )
            });
            let render_source = fresh_source_preparation
                .as_ref()
                .map(|preparation| {
                    super::lifecycle_retained::RetainedLifecycleRenderSource::new(
                        preparation.surface_presentation.clone(),
                        std::sync::Arc::clone(&preparation.render_layout),
                    )
                })
                .unwrap_or_else(super::lifecycle_retained::RetainedLifecycleRenderSource::empty)
                .with_resource_payload_id(resource_payload_id);
            let Some(payload) = super::lifecycle_retained::RetainedLifecyclePayload::capture(
                identity,
                window_id,
                root_surface_id,
                visual_group,
                resolved_effect_scene,
                frozen_decoration,
                render_source,
            ) else {
                self.rollback_lifecycle_reservation(identity, previous_identity);
                return Err(LifecycleAdmissionFailure::new(
                    LifecycleAdmissionReason::RetainedCaptureFailed,
                    target_provenance,
                ));
            };
            payload
        };
        let mapping_can_commit = if let Some(previous_identity) = previous_identity {
            self.retained_lifecycle_payloads.can_transfer_exact(
                previous_identity,
                identity,
                &payload,
            )
        } else {
            self.retained_lifecycle_payloads.can_publish_exact(identity)
        };
        if !mapping_can_commit {
            self.rollback_lifecycle_reservation(identity, previous_identity);
            return Err(LifecycleAdmissionFailure::new(
                LifecycleAdmissionReason::ReservationFailed,
                target_provenance,
            ));
        }

        let mut fresh_precapture_lease =
            if let Some(preparation) = fresh_source_preparation.as_mut() {
                let Some(mut lease) = super::lifecycle_retained::LifecyclePreCaptureLease::new(
                    std::mem::take(&mut preparation.surfaces),
                    Vec::new(),
                ) else {
                    self.rollback_lifecycle_reservation(identity, previous_identity);
                    return Err(LifecycleAdmissionFailure::new(
                        LifecycleAdmissionReason::SourceLeaseUnavailable,
                        target_provenance,
                    ));
                };
                let Some(held_release_obligations) = self.transfer_lifecycle_release_candidates(
                    std::mem::take(&mut preparation.release_candidates),
                ) else {
                    self.rollback_lifecycle_reservation(identity, previous_identity);
                    return Err(LifecycleAdmissionFailure::new(
                        LifecycleAdmissionReason::SourceLeaseUnavailable,
                        target_provenance,
                    ));
                };
                lease.held_release_obligations = held_release_obligations;
                Some(lease)
            } else {
                None
            };
        let started = self.window_lifecycle_animator.start_or_reverse(
            LifecycleMotionRequest {
                presentation_identity: identity,
                direction,
                effect,
                canonical_opacity: self
                    .window(window_id)
                    .map(|window| window.canonical_opacity().get())
                    .unwrap_or(1.0),
            },
            previous_identity,
            now,
            speed,
        );
        if started != Some(identity) {
            if let Some(lease) = fresh_precapture_lease.take() {
                self.restore_lifecycle_precapture_lease(
                    identity,
                    &payload,
                    lease,
                    "motion_rejected",
                );
            }
            self.rollback_lifecycle_reservation(identity, previous_identity);
            return Err(LifecycleAdmissionFailure::new(
                LifecycleAdmissionReason::MotionRejected,
                target_provenance,
            ));
        }

        let published = if let Some(previous_identity) = previous_identity {
            self.retained_lifecycle_payloads
                .transfer_exact(previous_identity, identity, &payload)
        } else {
            let entry = super::lifecycle_retained::RetainedLifecycleEntry {
                payload: std::sync::Arc::clone(&payload),
                pre_capture: fresh_precapture_lease.take(),
            };
            match self
                .retained_lifecycle_payloads
                .publish_exact(identity, entry)
            {
                Ok(()) => true,
                Err(mut entry) => {
                    if let Some(lease) = entry.pre_capture.take() {
                        self.restore_lifecycle_precapture_lease(
                            identity,
                            &entry.payload,
                            lease,
                            "publish_failed",
                        );
                    }
                    false
                }
            }
        };
        if !published {
            self.window_lifecycle_animator.cancel(identity);
            self.rollback_lifecycle_reservation(identity, previous_identity);
            return Err(LifecycleAdmissionFailure::new(
                LifecycleAdmissionReason::ReservationFailed,
                target_provenance,
            ));
        }
        let precapture_trace = (previous_identity.is_none())
            .then(|| self.retained_lifecycle_payloads.get_entry_exact(identity))
            .flatten()
            .and_then(|entry| {
                entry.pre_capture.as_ref().map(|lease| {
                    (
                        std::sync::Arc::clone(&entry.payload),
                        lease.surfaces.len(),
                        lease.dmabuf_surface_count(),
                        lease.held_release_obligations.len(),
                    )
                })
            });
        if let Some((payload, surface_count, dmabuf_surface_count, held_release_count)) =
            precapture_trace
        {
            self.trace_lifecycle_precapture_event(
                super::lifecycle_retained::LifecyclePreCaptureEvent::LeaseCreated,
                identity,
                &payload,
                surface_count,
                dmabuf_surface_count,
                held_release_count,
                None,
                None,
            );
        }
        if let Some(previous_identity) = previous_identity {
            debug_assert!(
                self.presentation_animator
                    .retire_retained_visual_exact(previous_identity)
            );
        }
        Ok((identity, payload, previous_identity.is_some()))
    }

    fn rollback_lifecycle_reservation(
        &mut self,
        identity: crate::presentation_animation::PresentationRetainedVisualIdentity,
        previous_identity: Option<
            crate::presentation_animation::PresentationRetainedVisualIdentity,
        >,
    ) {
        let restored = self
            .presentation_animator
            .restore_retained_visual_owner_exact(identity, previous_identity);
        let retired = self
            .presentation_animator
            .retire_retained_visual_exact(identity);
        debug_assert!(
            restored && retired,
            "failed lifecycle start restores its previous owner"
        );
    }

    pub(in crate::compositor) fn begin_lifecycle_restore(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        visual_group: Option<LifecycleVisualGroup>,
        resolved_effect_scene: ResolvedEffectScene,
        lifecycle_decorations: Vec<DecorationRenderInstance>,
    ) {
        self.begin_lifecycle_restore_with_source(
            window_id,
            root_surface_id,
            visual_group,
            resolved_effect_scene,
            lifecycle_decorations,
            None,
        );
    }

    pub(in crate::compositor) fn begin_lifecycle_restore_with_source(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        visual_group: Option<LifecycleVisualGroup>,
        resolved_effect_scene: ResolvedEffectScene,
        lifecycle_decorations: Vec<DecorationRenderInstance>,
        resolved_source_payload_id: Option<crate::compositor::PresentationRetainedVisualPayloadId>,
    ) {
        let Some(effect) =
            self.lifecycle_effect_kind_for_window(window_id, LifecycleDirection::Restore)
        else {
            self.record_lifecycle_admission(
                window_id,
                root_surface_id,
                LifecycleDirection::Restore,
                None,
                visual_group
                    .map(|group| group.anchor_provenance)
                    .or_else(|| {
                        self.window(window_id)
                            .and_then(|window| window.state.minimized_lifecycle_target())
                            .map(|target| target.provenance)
                    }),
                false,
                LifecycleAdmissionOutcome::Skipped(
                    self.lifecycle_effect_skip_reason(LifecycleDirection::Restore),
                ),
            );
            return;
        };
        let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id) else {
            self.record_lifecycle_admission(
                window_id,
                root_surface_id,
                LifecycleDirection::Restore,
                Some(effect),
                visual_group.map(|group| group.anchor_provenance),
                false,
                LifecycleAdmissionOutcome::Skipped(LifecycleAdmissionReason::NoSceneNode),
            );
            return;
        };
        let Some(now) = AnimationTime::monotonic_now() else {
            self.record_lifecycle_admission(
                window_id,
                root_surface_id,
                LifecycleDirection::Restore,
                Some(effect),
                visual_group.map(|group| group.anchor_provenance),
                false,
                LifecycleAdmissionOutcome::Skipped(LifecycleAdmissionReason::ClockUnavailable),
            );
            return;
        };
        let speed = self.animation_control.configuration().speed;
        let frozen_decoration = lifecycle_decorations
            .into_iter()
            .find(|decoration| decoration.root_surface_id() == root_surface_id);
        let admission = self.install_lifecycle_motion_with_preparation(
            scene_node_id,
            window_id,
            root_surface_id,
            None,
            None,
            visual_group,
            LifecycleDirection::Restore,
            effect,
            resolved_effect_scene,
            frozen_decoration,
            now,
            speed,
            None,
            resolved_source_payload_id,
        );
        let (_identity, payload, reused_payload) = match admission {
            Ok(admission) => admission,
            Err(failure) => {
                self.record_lifecycle_admission(
                    window_id,
                    root_surface_id,
                    LifecycleDirection::Restore,
                    Some(effect),
                    failure.target_provenance,
                    false,
                    LifecycleAdmissionOutcome::Skipped(failure.reason),
                );
                return;
            }
        };
        self.record_lifecycle_admission(
            window_id,
            root_surface_id,
            LifecycleDirection::Restore,
            Some(effect),
            Some(payload.visual_group.anchor_provenance),
            reused_payload,
            LifecycleAdmissionOutcome::Started,
        );
    }

    pub(in crate::compositor) fn lifecycle_scene_sample_at(
        &self,
        at: AnimationTime,
    ) -> LifecycleSceneSample {
        let active_identities = self
            .presentation_animator
            .active_retained_visuals(PresentationRetainedVisualKind::WindowLifecycle);
        let mut samples = Vec::with_capacity(active_identities.len());
        for identity in active_identities {
            let Some(motion) = self.window_lifecycle_animator.sample(identity, at) else {
                continue;
            };
            let Some(payload) = self.retained_lifecycle_payloads.get_exact(identity) else {
                continue;
            };
            if motion.presentation_identity != identity {
                continue;
            }
            let kind = if payload.effect_scene.is_empty() {
                LifecycleVisualSourceKind::NoOwnedEffects
            } else {
                LifecycleVisualSourceKind::ResolvedOwnedEffects
            };
            let visual_source = LifecycleVisualSource {
                window_id: payload.window_id,
                root_surface_id: payload.root_surface_id,
                presentation_identity: identity,
                payload_id: payload.payload_id,
                resource_payload_id: payload.resource_payload_id,
                kind,
                effect_scene: std::sync::Arc::clone(&payload.effect_scene),
                render_layout: Some(std::sync::Arc::clone(&payload.render_layout)),
            };
            let stable_order = payload.window_id.get() as usize;
            let (scene_band, layer_rank, stack_position, stable_order) =
                self.renderable_root_stack_key(payload.root_surface_id, stable_order);
            samples.push(LifecycleWindowSample {
                window_id: payload.window_id,
                root_surface_id: payload.root_surface_id,
                presentation_identity: identity,
                payload_id: payload.payload_id,
                painter_order: crate::window_lifecycle_animation::LifecyclePainterOrder {
                    scene_band,
                    layer_rank,
                    stack_position,
                    stable_order: stable_order as u64,
                },
                visual_group: payload.visual_group,
                visual_source,
                effect: motion.effect,
                progress: motion.progress,
                effect_opacity: motion.effect_opacity,
                direction: motion.direction,
                mathematically_settled: motion.mathematically_settled,
            });
        }
        LifecycleSceneSample {
            sampled_at: at,
            samples,
            minimized_visual_resource_payload_ids: self
                .desktop_windows
                .values()
                .filter(|window| window.state.is_minimized())
                .filter_map(|window| {
                    window
                        .state
                        .minimized_visual_snapshot()
                        .map(|snapshot| snapshot.resource_payload_id)
                })
                .collect(),
        }
    }

    pub(in crate::compositor) fn lifecycle_visual_group_for_scene_node(
        &self,
        scene_node_id: crate::core::SceneNodeId,
    ) -> Option<LifecycleVisualGroup> {
        let identity = self.presentation_animator.active_retained_visual(
            scene_node_id,
            PresentationRetainedVisualKind::WindowLifecycle,
        )?;
        self.retained_lifecycle_payloads
            .get_exact(identity)
            .map(|payload| payload.visual_group)
    }

    pub(in crate::compositor) fn lifecycle_decoration_render_instances(
        &self,
        sample: &LifecycleSceneSample,
        surfaces: &[RenderableSurface],
    ) -> Vec<DecorationRenderInstance> {
        let roots = sample
            .samples
            .iter()
            .map(|lamp| lamp.root_surface_id)
            .collect::<HashSet<_>>();
        let mut frozen = Vec::new();
        let mut frozen_roots = HashSet::new();
        for lifecycle_sample in &sample.samples {
            if self.presentation_animator.active_retained_visual(
                lifecycle_sample.presentation_identity.scene_node_id(),
                PresentationRetainedVisualKind::WindowLifecycle,
            ) != Some(lifecycle_sample.presentation_identity)
            {
                continue;
            }
            let Some(payload) = self
                .retained_lifecycle_payloads
                .get_exact(lifecycle_sample.presentation_identity)
            else {
                continue;
            };
            if payload.payload_id != lifecycle_sample.payload_id
                || payload.root_surface_id != lifecycle_sample.root_surface_id
                || payload.window_id != lifecycle_sample.window_id
            {
                continue;
            }
            if let Some(decoration) = payload.frozen_decoration.as_ref().filter(|decoration| {
                decoration.root_surface_id() == lifecycle_sample.root_surface_id
            }) && frozen_roots.insert(lifecycle_sample.root_surface_id)
            {
                frozen.push(decoration.clone());
            }
        }
        frozen.extend(
            self.native_decoration_render_instances_for_scale(surfaces, 1.0)
                .into_iter()
                .filter(|decoration| {
                    roots.contains(&decoration.root_surface_id())
                        && !frozen_roots.contains(&decoration.root_surface_id())
                }),
        );
        frozen
    }

    pub(in crate::compositor) fn lifecycle_root_restore_suppressed(
        &self,
        root_surface_id: u32,
    ) -> bool {
        let at = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        self.lifecycle_scene_sample_at(at)
            .restore_suppresses_root(root_surface_id)
    }

    fn lifecycle_sample_has_visible_pixels(&self, sample: &LifecycleWindowSample) -> bool {
        crate::window_lifecycle_animation::lifecycle_sample_requires_physical_ack(
            sample.effect,
            sample.visual_group,
            sample.progress,
            sample.effect_opacity,
            sample.mathematically_settled,
            self.output_size.width,
            self.output_size.height,
        )
    }

    pub(in crate::compositor) fn settle_lifecycle_no_visual_change(&mut self) -> bool {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        let candidates = self
            .lifecycle_scene_sample_at(now)
            .samples
            .into_iter()
            .filter(|sample| !self.lifecycle_sample_has_visible_pixels(sample))
            .filter(|sample| {
                !self
                    .presented_lifecycle_physical
                    .has_pending_visible_scene_node(
                        sample.presentation_identity.scene_node_id(),
                        self.output_size.width,
                        self.output_size.height,
                    )
            })
            .map(|sample| (sample.presentation_identity, sample.payload_id))
            .collect::<Vec<_>>();
        let mut settled = false;
        for (identity, payload_id) in candidates {
            if self
                .presentation_animator
                .active_retained_visual(identity.scene_node_id(), identity.kind())
                != Some(identity)
            {
                continue;
            }
            let Some(payload) = self.retained_lifecycle_payloads.get_exact(identity) else {
                continue;
            };
            if payload.payload_id != payload_id {
                continue;
            }
            if let Some(retired_identity) = self
                .window_lifecycle_animator
                .settle_no_visual_change(identity)
            {
                if !self
                    .presentation_animator
                    .retire_active_retained_visual_exact(retired_identity)
                {
                    continue;
                }
                let _ = self.retire_lifecycle_payload_exact(identity, "no_visual_change");
                settled = true;
            }
        }
        settled
    }

    pub(in crate::compositor) fn lifecycle_animation_has_pending_visible(&self) -> bool {
        let active_intersects = self
            .lifecycle_scene_sample_at(
                AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0)),
            )
            .samples
            .iter()
            .any(|sample| self.lifecycle_sample_has_visible_pixels(sample));
        active_intersects
            || self
                .presented_lifecycle_physical
                .has_pending_visible(self.output_size.width, self.output_size.height)
    }

    #[cfg(test)]
    pub(in crate::compositor) fn publish_presented_lifecycle(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
    ) {
        self.publish_presented_lifecycle_snapshot(
            frame_id,
            snapshot,
            crate::compositor::PresentedLifecycleScene::Initial,
        );
    }

    #[cfg(test)]
    pub(in crate::compositor) fn publish_presented_lifecycle_with_replacements(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
        canonical_root_surface_ids: &[u32],
        rendered_scene_replacement: bool,
    ) {
        let scene = if rendered_scene_replacement {
            crate::compositor::PresentedLifecycleScene::RenderedSceneReplacement {
                canonical_root_surface_ids,
            }
        } else {
            crate::compositor::PresentedLifecycleScene::Initial
        };
        self.publish_presented_lifecycle_snapshot(frame_id, snapshot, scene);
    }

    pub(in crate::compositor) fn publish_presented_lifecycle_snapshot(
        &mut self,
        frame_id: u64,
        snapshot: &LifecycleFrameSnapshot,
        scene: crate::compositor::PresentedLifecycleScene<'_>,
    ) {
        self.presented_lifecycle_physical.publish(
            frame_id,
            snapshot,
            scene,
            self.output_size.width,
            self.output_size.height,
        );
        for lifecycle_sample in &snapshot.samples {
            if self.presentation_animator.active_retained_visual(
                lifecycle_sample.presentation_identity.scene_node_id(),
                lifecycle_sample.presentation_identity.kind(),
            ) != Some(lifecycle_sample.presentation_identity)
            {
                continue;
            }
            if !self
                .retained_lifecycle_payloads
                .get_exact(lifecycle_sample.presentation_identity)
                .is_some_and(|payload| {
                    payload.payload_id == lifecycle_sample.payload_id
                        && payload.root_surface_id == lifecycle_sample.root_surface_id
                        && payload.window_id == lifecycle_sample.window_id
                })
            {
                continue;
            }
            self.trace_lifecycle_render_terminal_event(
                crate::compositor::LifecycleRenderTerminalEvent::Presented,
                lifecycle_sample.window_id,
                lifecycle_sample.root_surface_id,
                lifecycle_sample.presentation_identity,
                lifecycle_sample.payload_id,
                lifecycle_sample.effect,
                lifecycle_sample.direction,
            );
            let acknowledged = self.window_lifecycle_animator.acknowledge(
                lifecycle_sample.presentation_identity,
                lifecycle_sample.mathematically_settled,
            );
            let retired = acknowledged.is_some_and(|identity| {
                self.presentation_animator
                    .retire_active_retained_visual_exact(identity)
            });
            if retired {
                if lifecycle_sample.direction == LifecycleDirection::Minimize
                    && lifecycle_sample.mathematically_settled
                    && let Some(payload) = self
                        .retained_lifecycle_payloads
                        .get_exact(lifecycle_sample.presentation_identity)
                {
                    let minimized_snapshot = super::window_state::MinimizedVisualSnapshot {
                        resource_payload_id: payload.resource_payload_id,
                        source_client_rect: payload.visual_group.presented_source_client_rect,
                        source_visual_rect: payload.visual_group.presented_source_visual_rect,
                    };
                    if let Some(window) = self.window_mut(lifecycle_sample.window_id)
                        && window
                            .state
                            .promote_minimized_visual_snapshot(minimized_snapshot)
                        && lifecycle_diagnostics_enabled()
                    {
                        eprintln!(
                            "typhon.minimized_visual event=promoted window_id={} source_payload_id={} source_rect={:?}",
                            lifecycle_sample.window_id.get(),
                            minimized_snapshot.resource_payload_id.get(),
                            minimized_snapshot.source_visual_rect,
                        );
                    }
                }
                let _ = self.retire_lifecycle_payload_exact(
                    lifecycle_sample.presentation_identity,
                    "presented",
                );
                self.trace_lifecycle_render_terminal_event(
                    crate::compositor::LifecycleRenderTerminalEvent::Retired,
                    lifecycle_sample.window_id,
                    lifecycle_sample.root_surface_id,
                    lifecycle_sample.presentation_identity,
                    lifecycle_sample.payload_id,
                    lifecycle_sample.effect,
                    lifecycle_sample.direction,
                );
            }
        }
        self.advance_pointer_hit_generation();
    }

    pub(in crate::compositor) fn set_lifecycle_animation_enabled(&mut self, enabled: bool) {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        let active = self
            .presentation_animator
            .active_retained_visuals(PresentationRetainedVisualKind::WindowLifecycle);
        self.window_lifecycle_animator
            .set_enabled(enabled, &active, now);
    }

    pub(in crate::compositor) fn reconcile_lifecycle_animation_policy(&mut self) {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        if !self.animation_control.configuration().enabled {
            let active = self.lifecycle_scene_sample_at(now).samples;
            for sample in active {
                self.window_lifecycle_animator
                    .snap_to_endpoint(sample.presentation_identity, now);
            }
        }
    }

    pub(in crate::compositor) fn apply_lifecycle_render_fallback(
        &mut self,
        fallback: LifecycleRenderFallbackEntry,
    ) -> bool {
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        if self.presentation_animator.active_retained_visual(
            fallback.presentation_identity.scene_node_id(),
            fallback.presentation_identity.kind(),
        ) != Some(fallback.presentation_identity)
        {
            return false;
        }
        let Some(current) = self
            .window_lifecycle_animator
            .sample(fallback.presentation_identity, now)
        else {
            return false;
        };
        let Some(payload) = self
            .retained_lifecycle_payloads
            .get_exact(fallback.presentation_identity)
        else {
            return false;
        };
        if payload.root_surface_id != fallback.root_surface_id
            || payload.window_id != fallback.window_id
            || payload.payload_id != fallback.payload_id
            || current.presentation_identity != fallback.presentation_identity
            || current.effect != fallback.effect
        {
            return false;
        }
        let Some(identity) = self
            .window_lifecycle_animator
            .retire_render_fallback(fallback.presentation_identity)
        else {
            return false;
        };
        if !self
            .presentation_animator
            .retire_active_retained_visual_exact(identity)
        {
            return false;
        }
        let _ = self
            .retire_lifecycle_payload_exact(fallback.presentation_identity, "renderer_fallback");
        self.trace_lifecycle_render_terminal_event(
            crate::compositor::LifecycleRenderTerminalEvent::Retired,
            fallback.window_id,
            fallback.root_surface_id,
            fallback.presentation_identity,
            fallback.payload_id,
            fallback.effect,
            current.direction,
        );
        true
    }

    pub(in crate::compositor) fn set_lifecycle_animation_renderer_available(
        &mut self,
        available: bool,
    ) {
        self.set_lifecycle_animation_renderer_capabilities(
            available,
            self.lifecycle_squash_renderer_available.unwrap_or(false),
        );
    }

    pub(in crate::compositor) fn set_lifecycle_animation_renderer_capabilities(
        &mut self,
        lamp_available: bool,
        squash_available: bool,
    ) {
        self.lifecycle_animation_renderer_available = Some(lamp_available);
        self.lifecycle_squash_renderer_available = Some(squash_available);
        let unavailable = self
            .presentation_animator
            .active_retained_visuals(PresentationRetainedVisualKind::WindowLifecycle)
            .into_iter()
            .filter(|identity| {
                self.window_lifecycle_animator
                    .sample(*identity, AnimationTime::from_nanos(0))
                    .is_some_and(|sample| match sample.effect {
                        LifecycleEffectKind::Lamp => !lamp_available,
                        LifecycleEffectKind::Squash => !squash_available,
                    })
            })
            .collect::<Vec<_>>();
        for identity in unavailable {
            self.window_lifecycle_animator.cancel(identity);
            self.presentation_animator
                .retire_active_retained_visual_exact(identity);
            let _ = self.retire_lifecycle_payload_exact(identity, "renderer_unavailable");
        }
    }

    pub(in crate::compositor) fn lifecycle_cancel_window(&mut self, window_id: WindowId) {
        let scene_node_id = self.scene_node_id_for_window_group(window_id);
        let active_identity = scene_node_id.and_then(|scene_node_id| {
            self.presentation_animator.active_retained_visual(
                scene_node_id,
                PresentationRetainedVisualKind::WindowLifecycle,
            )
        });
        if let Some(identity) = active_identity {
            self.window_lifecycle_animator.cancel(identity);
            self.presentation_animator
                .retire_active_retained_visual_exact(identity);
            let _ = self.retire_lifecycle_payload_exact(identity, "window_cancelled");
        }
        let orphaned_executions = scene_node_id
            .map(|scene_node_id| {
                self.window_lifecycle_animator
                    .cancel_scene_executions(scene_node_id)
            })
            .unwrap_or_default();
        for identity in orphaned_executions {
            self.presentation_animator
                .retire_retained_visual_exact(identity);
            let _ = self.retire_lifecycle_payload_exact(identity, "window_cancelled");
        }
        let orphaned_payloads = scene_node_id
            .map(|scene_node_id| {
                self.retained_lifecycle_payloads
                    .remove_scene_node(scene_node_id)
            })
            .unwrap_or_default();
        for (identity, entry) in orphaned_payloads {
            self.settle_retired_lifecycle_entry(identity, entry, "window_cancelled");
            self.presentation_animator
                .retire_retained_visual_exact(identity);
            self.window_lifecycle_animator.cancel(identity);
        }
    }

    pub(in crate::compositor) fn lifecycle_teardown_window(&mut self, window_id: WindowId) {
        self.lifecycle_cancel_window(window_id);
        self.presented_lifecycle_physical.remove_window(window_id);
    }

    pub(in crate::compositor) const fn presented_lifecycle_frame_id(&self) -> u64 {
        self.presented_lifecycle_physical.frame_id()
    }
}
