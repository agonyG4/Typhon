use super::*;
use crate::animation_control::{AnimationEffect, AnimationSlot, scale_curve};
use crate::compositor::PresentedCanonicalSceneSnapshot;
use crate::compositor::state::window_exit_retained::{
    PreparedWindowExit, WindowExitFrozenContent, WindowExitPayload, WindowExitPropertyRevisions,
};
use crate::presentation_animation::{
    AnimationCurve, AnimationTime, EasingCurve, PresentationGeometryMutation, PresentationOpacity,
    PresentationOpacityMutation, PresentationRect, PresentationRetainedVisualIdentity,
    PresentationRetainedVisualKind, PresentationTransactionMemberKind,
    PresentationTransactionRequest,
};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

const WINDOW_CLOSE_BASE_DURATION: Duration = Duration::from_millis(180);
const WINDOW_CLOSE_SCALE_FACTOR: f64 = 0.94;
const WINDOW_CLOSE_GLIDE_OFFSET_LOGICAL_PIXELS: f64 = 24.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WindowCloseAnimationPlan {
    pub(super) geometry_start: PresentationRect,
    pub(super) geometry_target: PresentationRect,
    pub(super) opacity_start: PresentationOpacity,
    pub(super) opacity_target: PresentationOpacity,
    pub(super) geometry_curve: AnimationCurve,
    pub(super) opacity_curve: AnimationCurve,
}

pub(super) fn window_close_animation_plan(
    effect: AnimationEffect,
    source_presented_rect: PresentationRect,
    source_presented_opacity: PresentationOpacity,
    speed: f64,
) -> Option<WindowCloseAnimationPlan> {
    if !speed.is_finite() || speed <= 0.0 {
        return None;
    }

    let geometry_target = match effect {
        AnimationEffect::WindowScale => {
            let width = source_presented_rect.width() * WINDOW_CLOSE_SCALE_FACTOR;
            let height = source_presented_rect.height() * WINDOW_CLOSE_SCALE_FACTOR;
            PresentationRect::new(
                source_presented_rect.x() + (source_presented_rect.width() - width) / 2.0,
                source_presented_rect.y() + (source_presented_rect.height() - height) / 2.0,
                width,
                height,
            )?
        }
        AnimationEffect::WindowGlide => PresentationRect::new(
            source_presented_rect.x(),
            source_presented_rect.y() + WINDOW_CLOSE_GLIDE_OFFSET_LOGICAL_PIXELS,
            source_presented_rect.width(),
            source_presented_rect.height(),
        )?,
        _ => return None,
    };
    let curve = || {
        scale_curve(
            AnimationCurve::easing(WINDOW_CLOSE_BASE_DURATION, EasingCurve::EaseInCubic),
            speed,
        )
    };

    Some(WindowCloseAnimationPlan {
        geometry_start: source_presented_rect,
        geometry_target,
        opacity_start: source_presented_opacity,
        opacity_target: PresentationOpacity::TRANSPARENT,
        geometry_curve: curve(),
        opacity_curve: curve(),
    })
}

impl CompositorState {
    /// Freeze one logical toplevel only when the current canonical content is
    /// exactly the content described by the latest promoted physical scene.
    /// DMA-BUF release tokens move into the dedicated prepared store before
    /// canonical teardown starts.
    pub(in crate::compositor) fn prepare_window_exit(&mut self, root_surface_id: u32) -> bool {
        let effect = self.animation_control.effective_effect(
            AnimationSlot::WindowClose,
            self.animation_runtime_capabilities(),
        );
        if !matches!(
            effect,
            AnimationEffect::WindowScale | AnimationEffect::WindowGlide
        ) {
            return false;
        }

        let Some(window_id) = self.window_id_for_surface(root_surface_id) else {
            return false;
        };
        let Some(window) = self.window(window_id) else {
            return false;
        };
        let valid_root_role = match window.backend {
            WindowBackend::Xdg(handle) => {
                handle.root_surface_id() == root_surface_id
                    && self.surface_role(root_surface_id) == SurfaceRole::XdgToplevel
            }
            WindowBackend::X11(_) => self.surface_role(root_surface_id) == SurfaceRole::Xwayland,
        };
        if !valid_root_role
            || window.root_surface_id != root_surface_id
            || !window.is_workspace_managed()
            || window.state.mode() != ToplevelMode::Normal
            || window.state.is_minimized()
            || !self.window_is_visible_in_active_scene(window_id)
            || !self.window_exit_payloads.can_prepare_root(root_surface_id)
        {
            return false;
        }

        let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id) else {
            return false;
        };
        for kind in [
            PresentationRetainedVisualKind::WindowLifecycle,
            PresentationRetainedVisualKind::WindowExit,
        ] {
            if self
                .presentation_animator
                .active_retained_visual(scene_node_id, kind)
                .is_some()
            {
                return false;
            }
        }
        if self.presentation_animator.has_track(scene_node_id) {
            return false;
        }

        let Some(output_id) = self.native_output_id() else {
            return false;
        };
        let Some(promoted) = self.presented_canonical_scene.as_ref() else {
            return false;
        };
        let Some(physical_geometry) = self.presented_window_geometry(root_surface_id) else {
            return false;
        };
        if physical_geometry.scene_node_id() != scene_node_id {
            return false;
        }
        let Some(physical_frame) = self.presented_presentation.as_ref() else {
            return false;
        };
        let Some(physical_opacity) = physical_frame
            .opacities
            .iter()
            .find(|opacity| {
                opacity.scene_node_id == scene_node_id && opacity.root_surface_id == root_surface_id
            })
            .map(|opacity| opacity.opacity)
        else {
            return false;
        };
        let Some(physical_clip) = physical_frame
            .clips
            .iter()
            .find(|clip| {
                clip.scene_node_id == scene_node_id && clip.root_surface_id == root_surface_id
            })
            .map(|clip| clip.clip)
        else {
            return false;
        };

        let (surfaces, scene_nodes, fullscreen_plan, _) =
            self.native_frame_renderable_surfaces_with_scene_nodes_and_composition_plan();
        let owner_roots = surfaces
            .iter()
            .map(|surface| self.presentation_owner_root_for_surface(surface.surface_id))
            .collect::<Vec<_>>();
        if surfaces.len() != scene_nodes.len() || surfaces.len() != owner_roots.len() {
            return false;
        }
        let mut current_group_surfaces = Vec::new();
        let mut current_group_nodes = Vec::new();
        for ((surface, scene_node), owner_root) in surfaces
            .iter()
            .zip(scene_nodes.iter().copied())
            .zip(owner_roots.iter().copied())
        {
            if owner_root == root_surface_id {
                current_group_surfaces.push(surface.clone());
                current_group_nodes.push(scene_node);
            }
        }
        if current_group_surfaces.is_empty() {
            return false;
        }

        let Some(now) = AnimationTime::monotonic_now() else {
            return false;
        };
        let targets = self.native_frame_presentation_targets(surfaces.as_ref());
        let sample = self.presentation_scene_sample_for_targets_at_with_source(
            now,
            crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback,
            &targets,
        );
        let lifecycle = self.lifecycle_scene_sample_at(now);
        let current_effects = self.resolved_effect_scene_with_presentation_and_lifecycle(
            &sample,
            &fullscreen_plan,
            &lifecycle,
        );
        let current_render_generation = self.scene_render_generation;
        let visual_root_by_surface = crate::compositor::visual_stack_groups(
            surfaces.as_ref(),
            self.active_scene_view.popup_surface_ids(),
        )
        .into_iter()
        .flat_map(|group| {
            group
                .surface_indices()
                .iter()
                .map(|index| (surfaces[*index].surface_id, group.root_surface_id()))
                .collect::<Vec<_>>()
        })
        .collect::<std::collections::HashMap<_, _>>();
        let visual_root_surface_ids = surfaces
            .iter()
            .map(|surface| {
                visual_root_by_surface
                    .get(&surface.surface_id)
                    .copied()
                    .unwrap_or(surface.surface_id)
            })
            .collect::<Vec<_>>();
        let presentation_keys = surfaces
            .iter()
            .map(|surface| self.surface_presentation_key_for_surface(surface.surface_id))
            .collect::<Vec<_>>();
        let Some(current_evidence) = PresentedCanonicalSceneSnapshot::capture(
            output_id,
            current_render_generation,
            current_effects.signature,
            surfaces.as_ref(),
            scene_nodes.as_ref(),
            &owner_roots,
            &presentation_keys,
            &visual_root_surface_ids,
        ) else {
            return false;
        };
        if !super::window_exit_physical::prove_window_exit_source(
            Some(promoted),
            output_id,
            root_surface_id,
            current_render_generation,
            current_effects.signature,
            &current_evidence.surfaces,
            true,
        ) {
            return false;
        }

        let canonical_geometry = window
            .x11_geometry
            .as_ref()
            .map(|geometry| geometry.frame)
            .or_else(|| self.current_root_window_geometry(root_surface_id));
        let Some(canonical_rect) = canonical_geometry
            .and_then(|geometry| self.presentation_rect_for_geometry(root_surface_id, geometry))
        else {
            return false;
        };
        let Some(target) = targets
            .windows()
            .iter()
            .copied()
            .find(|target| target.root_surface_id() == root_surface_id)
        else {
            return false;
        };
        if target.scene_node_id() != scene_node_id || target.canonical_rect() != canonical_rect {
            return false;
        }

        let root_position = owner_roots
            .iter()
            .position(|owner_root| *owner_root == root_surface_id)
            .unwrap_or(usize::MAX);
        let (scene_band, layer_rank, stack_position, _) =
            self.renderable_root_stack_key(root_surface_id, root_position);
        let speed = self.animation_control.configuration().speed;
        let Some(close_plan) = window_close_animation_plan(
            effect,
            physical_geometry.presented_rect(),
            physical_opacity,
            speed,
        ) else {
            return false;
        };
        let current_group_id_set = current_group_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<HashSet<_>>();
        let raw_effects = self.resolved_effect_scene_for_composition_plan_with_lifecycle(
            &fullscreen_plan,
            &lifecycle,
        );
        let frozen_effects = raw_effects
            .instances
            .into_iter()
            .filter(|instance| {
                let surface_id = match instance.anchor {
                    crate::compositor::EffectAnchor::BeforeSurface(surface_id)
                    | crate::compositor::EffectAnchor::ReplaceSurface(surface_id)
                    | crate::compositor::EffectAnchor::AfterSurface(surface_id) => surface_id,
                    crate::compositor::EffectAnchor::OutputPostProcess => return false,
                };
                current_group_id_set.contains(&surface_id)
            })
            .collect::<Vec<_>>();
        let effect_scene = Arc::new(ResolvedEffectScene::new(
            current_effects.generation,
            frozen_effects,
        ));
        let frozen_decoration = self
            .native_decoration_render_instances_for_scale(&current_group_surfaces, 1.0)
            .into_iter()
            .find(|decoration| decoration.root_surface_id() == root_surface_id);

        let evidence_by_surface = current_evidence
            .surfaces_for_owner(root_surface_id)
            .into_iter()
            .map(|evidence| (evidence.key.surface_id, evidence))
            .collect::<std::collections::HashMap<_, _>>();
        let Some(visual_root_surface_ids) = current_group_surfaces
            .iter()
            .map(|surface| evidence_by_surface.get(&surface.surface_id))
            .collect::<Option<Vec<_>>>()
            .map(|evidence| {
                evidence
                    .into_iter()
                    .map(|evidence| evidence.visual_root_surface_id)
                    .collect::<Vec<_>>()
            })
        else {
            return false;
        };

        let content = Arc::new(WindowExitFrozenContent {
            window_id,
            root_surface_id,
            scene_node_id,
            surfaces: current_group_surfaces,
            surface_scene_node_ids: current_group_nodes,
            visual_root_surface_ids,
            presentation_owner_root_surface_ids: vec![root_surface_id; evidence_by_surface.len()],
            canonical_rect,
            close_geometry_target: close_plan.geometry_target,
            close_curve: close_plan.geometry_curve,
            source_presented_rect: physical_geometry.presented_rect(),
            source_presented_opacity: physical_opacity,
            source_presented_clip: physical_clip,
            frozen_decoration,
            effect_scene,
            painter_order: super::window_exit_retained::WindowExitPainterOrder {
                scene_band,
                layer_rank,
                stack_position,
            },
            render_generation: current_render_generation,
            effect_identity_signature: current_effects.signature,
        });

        let mut held_release_obligations = Vec::new();
        for surface in &content.surfaces {
            if surface.dmabuf_handle().is_none() {
                continue;
            }
            let Some(obligation) = self.active_dmabuf_buffers.get(&surface.surface_id).cloned()
            else {
                return false;
            };
            if obligation.buffer_id != surface.buffer_id()
                || self.buffer_release_is_owned(&obligation)
                || self
                    .active_dmabuf_buffers
                    .iter()
                    .any(|(owner_surface_id, other)| {
                        *owner_surface_id != surface.surface_id
                            && other.same_release_token(&obligation)
                    })
                || held_release_obligations.iter().any(
                    |held: &super::window_exit_retained::WindowExitReleaseObligation| {
                        held.obligation.same_release_token(&obligation)
                    },
                )
            {
                return false;
            }
            held_release_obligations.push(
                super::window_exit_retained::WindowExitReleaseObligation {
                    surface_id: surface.surface_id,
                    obligation,
                },
            );
        }

        let prepared = PreparedWindowExit {
            content,
            held_release_obligations,
        };
        if self
            .window_exit_payloads
            .prepare_candidate_exact(root_surface_id, prepared)
            .is_err()
        {
            return false;
        }
        let prepared = self
            .window_exit_payloads
            .take_prepared_root(root_surface_id)
            .expect("prepared Window Exit candidate must remain registered");
        for held in prepared.held_release_obligations.clone() {
            let Some(active) = self.active_dmabuf_buffers.remove(&held.surface_id) else {
                self.restore_window_exit_candidate(prepared);
                return false;
            };
            if !active.same_release_token(&held.obligation)
                || active.buffer_id != held.obligation.buffer_id
            {
                self.active_dmabuf_buffers.insert(held.surface_id, active);
                self.restore_window_exit_candidate(prepared);
                return false;
            }
        }
        match self
            .window_exit_payloads
            .prepare_candidate_exact(root_surface_id, prepared)
        {
            Ok(()) => true,
            Err(prepared) => {
                self.restore_window_exit_candidate(prepared);
                false
            }
        }
    }

    fn restore_window_exit_candidate(&mut self, prepared: PreparedWindowExit) {
        for held in prepared.held_release_obligations {
            let still_current = self
                .current_surface_buffers
                .get(&held.surface_id)
                .is_some_and(|current| current.buffer_id() == held.obligation.buffer_id);
            match self.active_dmabuf_buffers.get(&held.surface_id) {
                Some(active) if active.same_release_token(&held.obligation) => {}
                Some(_) => self.queue_dmabuf_buffer_release(held.obligation),
                None if still_current => {
                    self.active_dmabuf_buffers
                        .insert(held.surface_id, held.obligation);
                }
                None => self.queue_dmabuf_buffer_release(held.obligation),
            }
        }
    }

    pub(in crate::compositor) fn discard_prepared_window_exit(
        &mut self,
        root_surface_id: u32,
    ) -> bool {
        let Some(prepared) = self
            .window_exit_payloads
            .take_prepared_root(root_surface_id)
        else {
            return false;
        };
        self.restore_window_exit_candidate(prepared);
        true
    }

    /// Activate a prepared payload only after canonical teardown completed.
    /// Geometry and Opacity commit as one PE transaction; the retained owner
    /// becomes visible only after both exact revisions are recorded.
    pub(in crate::compositor) fn activate_prepared_window_exit(
        &mut self,
        root_surface_id: u32,
    ) -> bool {
        let Some(prepared) = self
            .window_exit_payloads
            .take_prepared_root(root_surface_id)
        else {
            return false;
        };
        let scene_node_id = prepared.content.scene_node_id;
        let content = &prepared.content;
        let plan = WindowCloseAnimationPlan {
            geometry_start: content.source_presented_rect,
            geometry_target: content.close_geometry_target,
            opacity_start: content.source_presented_opacity,
            opacity_target: PresentationOpacity::TRANSPARENT,
            geometry_curve: content.close_curve,
            opacity_curve: content.close_curve,
        };
        let Some(now) = AnimationTime::monotonic_now() else {
            self.restore_window_exit_candidate(prepared);
            return false;
        };
        let identity = match self.presentation_animator.begin_retained_visual(
            scene_node_id,
            PresentationRetainedVisualKind::WindowExit,
            now,
        ) {
            Ok(identity) => identity,
            Err(_) => {
                self.restore_window_exit_candidate(prepared);
                return false;
            }
        };
        let payload = match WindowExitPayload::new(identity, prepared, now) {
            Ok(payload) => payload,
            Err(prepared) => {
                let _ = self
                    .presentation_animator
                    .retire_retained_visual_exact(identity);
                self.restore_window_exit_candidate(prepared);
                return false;
            }
        };
        if let Err(mut payload) = self
            .window_exit_payloads
            .publish_candidate_exact(identity, payload)
        {
            let _ = self
                .presentation_animator
                .retire_retained_visual_exact(identity);
            for held in payload.take_held_release_obligations() {
                self.queue_dmabuf_buffer_release(held.obligation);
            }
            return false;
        }

        let committed = self
            .presentation_animator
            .commit(PresentationTransactionRequest::mixed(
                now,
                vec![PresentationGeometryMutation::new(
                    scene_node_id,
                    plan.geometry_start,
                    plan.geometry_target,
                    plan.geometry_curve,
                )],
                vec![PresentationOpacityMutation::new(
                    scene_node_id,
                    plan.opacity_start,
                    plan.opacity_target,
                    plan.opacity_curve,
                )],
            ));
        let committed = match committed {
            Ok(committed) => committed,
            Err(_) => {
                let _ = self
                    .presentation_animator
                    .retire_retained_visual_exact(identity);
                if let Some(payload) = self.window_exit_payloads.retire_exact(identity) {
                    self.release_retired_window_exit(payload);
                }
                return false;
            }
        };
        let mut geometry_revision_id = None;
        let mut opacity_revision_id = None;
        for member in committed.members() {
            if member.scene_node_id() != scene_node_id || member.transaction_id() != committed.id()
            {
                continue;
            }
            match member.kind() {
                PresentationTransactionMemberKind::Property(
                    crate::presentation_animation::PresentationPropertyKind::Geometry,
                ) => geometry_revision_id = Some(member.revision_id()),
                PresentationTransactionMemberKind::Property(
                    crate::presentation_animation::PresentationPropertyKind::Opacity,
                ) => opacity_revision_id = Some(member.revision_id()),
                _ => {}
            }
        }
        let revisions = geometry_revision_id.zip(opacity_revision_id).map(
            |(geometry_revision_id, opacity_revision_id)| WindowExitPropertyRevisions {
                transaction_id: committed.id(),
                geometry_revision_id,
                opacity_revision_id,
            },
        );
        let Some(revisions) = revisions else {
            self.rollback_window_exit_activation(identity, committed.id(), None);
            return false;
        };
        if !self
            .window_exit_payloads
            .set_property_revisions_before_activation(identity, revisions)
        {
            self.rollback_window_exit_activation(identity, committed.id(), Some(revisions));
            return false;
        }
        match self
            .presentation_animator
            .activate_retained_visual_exact(identity)
        {
            Ok(None) => {}
            Ok(Some(previous)) => {
                let _ = self
                    .presentation_animator
                    .restore_retained_visual_owner_exact(identity, Some(previous));
                self.rollback_window_exit_activation(identity, committed.id(), Some(revisions));
                return false;
            }
            Err(_) => {
                self.rollback_window_exit_activation(identity, committed.id(), Some(revisions));
                return false;
            }
        }
        self.advance_scene_render_generation_for_window_exit();
        true
    }

    fn rollback_window_exit_activation(
        &mut self,
        identity: PresentationRetainedVisualIdentity,
        transaction_id: crate::presentation_animation::PresentationTransactionId,
        revisions: Option<WindowExitPropertyRevisions>,
    ) {
        let cancelled_exact_pair = revisions.is_some_and(|revisions| {
            self.presentation_animator.cancel_property_pair_exact(
                identity.scene_node_id(),
                revisions.transaction_id,
                revisions.geometry_revision_id,
                revisions.opacity_revision_id,
            )
        });
        if !cancelled_exact_pair {
            self.presentation_animator
                .cancel_transaction_exact(identity.scene_node_id(), transaction_id);
        }
        let _ = self
            .presentation_animator
            .retire_active_retained_visual_exact(identity);
        let _ = self
            .presentation_animator
            .retire_retained_visual_exact(identity);
        if let Some(payload) = self.window_exit_payloads.retire_exact(identity) {
            self.release_retired_window_exit(payload);
        }
    }

    pub(in crate::compositor) fn release_retired_window_exit(
        &mut self,
        mut payload: WindowExitPayload,
    ) {
        for held in payload.take_held_release_obligations() {
            self.queue_dmabuf_buffer_release(held.obligation);
        }
    }

    pub(in crate::compositor) fn retire_window_exit_for_root(
        &mut self,
        root_surface_id: u32,
    ) -> bool {
        if let Some(prepared) = self
            .window_exit_payloads
            .take_prepared_root(root_surface_id)
        {
            self.restore_window_exit_candidate(prepared);
            return true;
        }
        let Some(identity) = self.window_exit_payloads.identities().find(|identity| {
            self.window_exit_payloads
                .get_exact(*identity)
                .is_some_and(|payload| payload.content.root_surface_id == root_surface_id)
        }) else {
            return false;
        };
        if let Some(revisions) = self
            .window_exit_payloads
            .get_exact(identity)
            .and_then(|payload| payload.property_revisions)
        {
            let cancelled_pair = self.presentation_animator.cancel_property_pair_exact(
                identity.scene_node_id(),
                revisions.transaction_id,
                revisions.geometry_revision_id,
                revisions.opacity_revision_id,
            );
            if !cancelled_pair {
                self.presentation_animator
                    .cancel_transaction_exact(identity.scene_node_id(), revisions.transaction_id);
            }
        }
        if !self
            .presentation_animator
            .retire_active_retained_visual_exact(identity)
        {
            return false;
        }
        let Some(payload) = self.window_exit_payloads.retire_exact(identity) else {
            return false;
        };
        self.release_retired_window_exit(payload);
        self.advance_scene_render_generation_for_window_exit();
        true
    }

    /// Canonical XDG role destruction can follow a null-buffer unmap that
    /// already began Window Exit. `remove_desktop_window` correctly cancels
    /// every property track; rebase that one logical Exit from the latest
    /// physically presented values after the terminal teardown.
    pub(in crate::compositor) fn resume_window_exit_after_canonical_teardown(
        &mut self,
        root_surface_id: u32,
    ) -> bool {
        let Some(identity) = self.window_exit_payloads.identities().find(|identity| {
            self.window_exit_payloads
                .get_exact(*identity)
                .is_some_and(|payload| payload.content.root_surface_id == root_surface_id)
        }) else {
            return false;
        };
        if self.presentation_animator.active_retained_visual(
            identity.scene_node_id(),
            PresentationRetainedVisualKind::WindowExit,
        ) != Some(identity)
        {
            return false;
        }
        let Some(now) = AnimationTime::monotonic_now() else {
            return false;
        };
        let Some(mut old_payload) = self.window_exit_payloads.retire_exact(identity) else {
            return false;
        };
        let motion_started_at = old_payload.motion_started_at;
        let content = Arc::make_mut(&mut old_payload.content);
        if let Some(physical_geometry) = self.presented_window_geometry(root_surface_id)
            && physical_geometry.scene_node_id() == content.scene_node_id
        {
            content.source_presented_rect = physical_geometry.presented_rect();
        }
        if let Some(frame) = self.presented_presentation.as_ref() {
            if let Some(opacity) = frame.opacities.iter().find(|opacity| {
                opacity.root_surface_id == root_surface_id
                    && opacity.scene_node_id == content.scene_node_id
            }) {
                content.source_presented_opacity = opacity.opacity;
            }
            if let Some(clip) = frame.clips.iter().find(|clip| {
                clip.root_surface_id == root_surface_id
                    && clip.scene_node_id == content.scene_node_id
            }) {
                content.source_presented_clip = clip.clip;
            }
        }
        let remaining_curve = match content.close_curve {
            AnimationCurve::Easing { duration, curve } => {
                let elapsed_nanos = now.as_nanos().saturating_sub(motion_started_at.as_nanos());
                let remaining_nanos = duration
                    .as_nanos()
                    .saturating_sub(u128::from(elapsed_nanos));
                AnimationCurve::easing(
                    Duration::from_nanos(remaining_nanos.max(1_000_000) as u64),
                    curve,
                )
            }
            AnimationCurve::Spring(spec) => AnimationCurve::spring(spec),
        };
        content.close_curve = remaining_curve;
        let new_content = Arc::new(content.clone());
        let held_release_obligations = old_payload.take_held_release_obligations();
        let _ = self
            .presentation_animator
            .retire_active_retained_visual_exact(identity);
        let _ = self
            .presentation_animator
            .retire_retained_visual_exact(identity);
        let prepared = PreparedWindowExit {
            content: new_content,
            held_release_obligations,
        };
        match self
            .window_exit_payloads
            .prepare_candidate_exact(root_surface_id, prepared)
        {
            Ok(()) => {
                self.advance_scene_render_generation_for_window_exit();
                self.activate_prepared_window_exit(root_surface_id)
            }
            Err(prepared) => {
                for held in prepared.take_release_obligations() {
                    self.queue_dmabuf_buffer_release(held.obligation);
                }
                false
            }
        }
    }

    pub(in crate::compositor) fn advance_scene_render_generation_for_window_exit(&mut self) {
        self.scene_render_generation = self.scene_render_generation.saturating_add(1);
        self.render_generation = self.render_generation.saturating_add(1);
    }
}
