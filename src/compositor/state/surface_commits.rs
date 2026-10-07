#![allow(clippy::too_many_arguments)]

use super::*;

impl CompositorState {
    pub(in crate::compositor) fn complete_pending_resize_from_current_geometry(
        &mut self,
        surface_id: u32,
        resize: ResizeCommitSnapshot,
    ) -> bool {
        if self.committed_xdg_geometry_is_invalid(surface_id) {
            return false;
        }
        if self.surface_tree_generation.is_some() {
            self.surface_tree_pending_resize_completions
                .push((surface_id, resize));
            return true;
        }
        let committed_size = resize
            .effective_xdg_window_geometry
            .or_else(|| {
                self.effective_xdg_window_geometry(surface_id)
                    .map(|geometry| geometry.geometry)
            })
            .map(|geometry| BufferSize {
                width: geometry.width as u32,
                height: geometry.height as u32,
            })
            .or_else(|| {
                resize
                    .committed_size
                    .map(|(width, height)| BufferSize { width, height })
            })
            .or_else(|| self.current_committed_surface_content_size(surface_id));
        let Some(committed_size) = committed_size else {
            return false;
        };
        let placement =
            resize.placement_for_committed_size(committed_size.width, committed_size.height);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize commit surface={surface_id} decision=accepted reason=geometry-only serial={} requested={}x{} actual={}x{} placement={},{}",
                resize.serial,
                resize.width,
                resize.height,
                committed_size.width,
                committed_size.height,
                placement.local_x,
                placement.local_y,
            );
        }
        if !resize.resizing {
            let completes_active = self
                .active_toplevel_resizes
                .get(&surface_id)
                .is_some_and(|active| active.interaction_id == resize.interaction_id);
            if completes_active {
                self.active_toplevel_resizes.remove(&surface_id);
                self.toplevel_visual_geometries.insert(
                    surface_id,
                    ToplevelVisualGeometry {
                        placement,
                        width: committed_size.width,
                        height: committed_size.height,
                        active_resize: None,
                        mode_transition: false,
                        xdg_mode_transition_fence: None,
                    },
                );
                self.update_toplevel_visual_render_assignment(surface_id);
            }
            self.store_surface_placement(surface_id, placement);
            self.advance_render_generation_with_scene_effect(
                RenderGenerationCause::WindowResize,
                self.surface_is_visible_in_active_scene(surface_id),
            );
        }
        self.complete_applied_resize_transaction(surface_id, resize);
        true
    }

    pub(in crate::compositor) fn hide_renderable_surface_subtree(
        &mut self,
        surface_id: u32,
    ) -> bool {
        let renderable_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let removed_surface_ids = renderable_ids
            .into_iter()
            .filter(|candidate_id| self.surface_is_descendant_of(*candidate_id, surface_id))
            .collect::<Vec<_>>();
        self.reconcile_hidden_surface_ids(removed_surface_ids)
    }

    fn reconcile_hidden_surface_ids(&mut self, mut removed_surface_ids: Vec<u32>) -> bool {
        removed_surface_ids.sort_unstable();
        removed_surface_ids.dedup();
        if removed_surface_ids.is_empty() {
            self.reconcile_idle_inhibition();
            return false;
        }

        let scene_effect = removed_surface_ids
            .iter()
            .any(|surface_id| self.surface_is_visible_in_active_scene(*surface_id));
        self.cleanup_hidden_surface_ids(&removed_surface_ids);
        self.reconcile_hidden_surface_output_memberships(&removed_surface_ids);
        self.publish_hidden_surface_ids(scene_effect);
        true
    }

    pub(in crate::compositor) fn cleanup_hidden_surface_ids(
        &mut self,
        removed_surface_ids: &[u32],
    ) {
        self.retain_renderable_surfaces(|surface| {
            !removed_surface_ids.contains(&surface.surface_id)
        });
        self.clear_resize_state_for_surfaces_with_reason(
            removed_surface_ids,
            WindowInteractionEndReason::SurfaceUnmapped,
        );
        self.clear_popup_grab_for_surface_ids(removed_surface_ids);
        self.popup_grab_stack
            .retain(|surface_id| !removed_surface_ids.contains(surface_id));
        self.recent_input_serials
            .retain(|input| !removed_surface_ids.contains(&compositor_surface_id(&input.surface)));
        self.clear_pointer_button_state_for_removed_surfaces(
            removed_surface_ids,
            "surface-destroyed",
        );
        self.reconcile_idle_inhibition();
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.clear_pointer_focus();
        }
        if self
            .focused_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.focused_surface = None;
            self.focused_window_id = None;
            if self.keyboard_surface.as_ref().is_some_and(|surface| {
                removed_surface_ids.contains(&compositor_surface_id(surface))
            }) {
                self.clear_keyboard_focus();
            }
            let _ = self.focus_topmost_renderable_toplevel();
        }
        self.invalidate_surface_origin_cache();
    }

    pub(in crate::compositor) fn reconcile_hidden_surface_output_memberships(
        &mut self,
        removed_surface_ids: &[u32],
    ) {
        for removed_surface_id in removed_surface_ids {
            if let Some(surface) = self.surface_resource_by_id(*removed_surface_id) {
                self.reconcile_surface_output_membership(&surface);
            }
        }
    }

    fn publish_hidden_surface_ids(&mut self, scene_effect: bool) {
        if scene_effect {
            self.rebuild_active_scene_view();
        } else {
            self.refresh_active_scene_surface_order();
        }
        self.advance_render_generation_with_scene_effect(
            RenderGenerationCause::SurfaceUnmap,
            scene_effect,
        );
    }

    pub(in crate::compositor) fn unmap_surface_content(&mut self, surface_id: u32) -> bool {
        let renderable_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let current_buffer_ids = self
            .current_surface_buffers
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let mut removed_surface_ids = renderable_ids
            .into_iter()
            .filter(|candidate_id| self.surface_is_descendant_of(*candidate_id, surface_id))
            .collect::<Vec<_>>();
        removed_surface_ids.extend(
            current_buffer_ids
                .into_iter()
                .filter(|candidate_id| self.surface_is_descendant_of(*candidate_id, surface_id)),
        );
        if removed_surface_ids.is_empty() {
            self.reconcile_idle_inhibition();
            return false;
        }

        for removed_surface_id in &removed_surface_ids {
            self.clear_current_surface_content(*removed_surface_id);
        }
        self.reconcile_hidden_surface_ids(removed_surface_ids)
    }

    pub(in crate::compositor) fn unmap_xdg_role_surfaces(&mut self, surface_id: u32) -> bool {
        if self.root_surface_id_for_surface(surface_id) == surface_id {
            self.cancel_pending_normal_restore(surface_id, "root_unmapped");
            self.cancel_window_open_presentation_ownership(surface_id);
        }
        let renderable_ids = self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>();
        let mut removed_surface_ids = renderable_ids
            .into_iter()
            .filter(|candidate_id| self.surface_is_descendant_of(*candidate_id, surface_id))
            .collect::<Vec<_>>();
        removed_surface_ids.push(surface_id);
        removed_surface_ids.sort_unstable();
        removed_surface_ids.dedup();

        for removed_surface_id in &removed_surface_ids {
            self.remove_current_surface_buffer(*removed_surface_id);
            if let Some(buffer) = self.active_dmabuf_buffers.remove(removed_surface_id) {
                self.queue_dmabuf_buffer_release(buffer);
            }
        }
        let previous_renderable_count = self.renderable_surfaces.len();
        self.retain_renderable_surfaces(|surface| {
            !removed_surface_ids.contains(&surface.surface_id)
        });
        for removed_surface_id in &removed_surface_ids {
            if let Some(surface) = self.surface_resource_by_id(*removed_surface_id) {
                self.reconcile_surface_output_membership(&surface);
            }
        }
        self.clear_popup_grab_for_surface_ids(&removed_surface_ids);
        self.popup_grab_stack
            .retain(|surface_id| !removed_surface_ids.contains(surface_id));
        self.recent_input_serials
            .retain(|input| !removed_surface_ids.contains(&compositor_surface_id(&input.surface)));
        self.clear_resize_state_for_surfaces_with_reason(
            &removed_surface_ids,
            WindowInteractionEndReason::SurfaceUnmapped,
        );
        self.clear_pointer_button_state_for_removed_surfaces(
            &removed_surface_ids,
            "surface-destroyed",
        );
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.clear_pointer_focus();
        }
        if self
            .focused_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.focused_surface = None;
            self.focused_window_id = None;
            if self.keyboard_surface.as_ref().is_some_and(|surface| {
                removed_surface_ids.contains(&compositor_surface_id(surface))
            }) {
                self.clear_keyboard_focus();
            }
            let _ = self.focus_topmost_renderable_toplevel();
        }

        if self.renderable_surfaces.len() == previous_renderable_count {
            return false;
        }

        let scene_effect = removed_surface_ids
            .iter()
            .any(|surface_id| self.surface_is_visible_in_active_scene(*surface_id));
        if scene_effect {
            self.rebuild_active_scene_view();
        } else {
            self.refresh_active_scene_surface_order();
        }
        self.invalidate_surface_origin_cache();
        self.advance_render_generation_with_scene_effect(
            RenderGenerationCause::SurfaceUnmap,
            scene_effect,
        );
        true
    }

    pub(in crate::compositor) fn clear_resize_state_for_surfaces(&mut self, surface_ids: &[u32]) {
        self.clear_resize_state_for_surfaces_with_reason(
            surface_ids,
            WindowInteractionEndReason::SurfaceDestroyed,
        );
    }

    pub(in crate::compositor) fn clear_resize_state_for_surfaces_with_reason(
        &mut self,
        surface_ids: &[u32],
        reason: WindowInteractionEndReason,
    ) {
        let before_flows = self.resize_configure_flows.len();
        self.resize_configure_flows
            .retain(|surface_id, _| !surface_ids.contains(surface_id));
        let removed_flows = before_flows.saturating_sub(self.resize_configure_flows.len());
        self.surface_transactions
            .clear_resize_state_for_surfaces(surface_ids);
        let before_previews = self.active_toplevel_resizes.len();
        self.active_toplevel_resizes
            .retain(|surface_id, _| !surface_ids.contains(surface_id));
        self.pending_xwayland_visual_content
            .retain(|id| !surface_ids.contains(id));
        let removed_previews = before_previews.saturating_sub(self.active_toplevel_resizes.len());
        let visual_ids = self
            .toplevel_visual_geometries
            .keys()
            .copied()
            .filter(|surface_id| surface_ids.contains(surface_id))
            .collect::<Vec<_>>();
        for surface_id in visual_ids {
            self.toplevel_visual_geometries.remove(&surface_id);
            self.clear_toplevel_visual_render_assignment(surface_id);
        }
        let interaction_cleared = self
            .window_interaction
            .is_some_and(|interaction| surface_ids.contains(&interaction.root_surface_id));
        if interaction_cleared {
            let visual_root_surface_id = self
                .window_interaction
                .map(|interaction| interaction.root_surface_id);
            self.clear_window_interaction_state(reason);
            self.refresh_pointer_focus_after_window_interaction(visual_root_surface_id);
        }
        debug_assert!(
            self.window_interaction.is_some() || self.interaction_cursor_override.is_none()
        );
        if removed_flows > 0 || removed_previews > 0 {
            self.resize_flow_metrics.resize_interactions_canceled = self
                .resize_flow_metrics
                .resize_interactions_canceled
                .saturating_add(1);
        }
    }
    pub(in crate::compositor) fn note_shm_materialization_failure(
        &mut self,
        pending: &PendingSurfaceBuffer,
    ) {
        if pending.data.is_shm() {
            self.shm_buffer_lifetime_metrics
                .shm_materialization_failures_total = self
                .shm_buffer_lifetime_metrics
                .shm_materialization_failures_total
                .saturating_add(1);
        }
    }

    pub(in crate::compositor) fn release_unmaterialized_pending_buffer(
        &mut self,
        pending: PendingSurfaceBuffer,
        superseded: bool,
    ) {
        if pending.data.is_shm() {
            if superseded {
                self.shm_buffer_lifetime_metrics
                    .shm_releases_superseded_without_read_total = self
                    .shm_buffer_lifetime_metrics
                    .shm_releases_superseded_without_read_total
                    .saturating_add(1);
            } else {
                self.shm_buffer_lifetime_metrics
                    .shm_releases_deferred_unmaterialized_total = self
                    .shm_buffer_lifetime_metrics
                    .shm_releases_deferred_unmaterialized_total
                    .saturating_add(1);
            }
        }
        self.release_pending_surface_buffer(pending);
    }

    pub(in crate::compositor) fn release_pending_surface_buffer(
        &mut self,
        pending: PendingSurfaceBuffer,
    ) {
        if pending.data.is_dmabuf() || pending.explicit_release.is_some() {
            let obligation = DmabufReleaseObligation {
                buffer_id: pending.data.buffer_id(),
                release: pending.release_target(),
            };
            let _ = self.complete_dmabuf_release_if_inactive(
                CompositorFrameBatchId::for_shutdown(),
                0,
                obligation,
            );
        } else {
            self.release_wl_buffer_direct(pending.resource);
        }
    }

    pub(in crate::compositor) fn release_wl_buffer_direct(&mut self, buffer: wl_buffer::WlBuffer) {
        if !buffer.is_alive() {
            self.buffer_release_metrics.buffer_releases_discarded = self
                .buffer_release_metrics
                .buffer_releases_discarded
                .saturating_add(1);
            return;
        }
        if buffer.send_event(wl_buffer::Event::Release).is_ok() {
            self.buffer_release_metrics.buffer_releases_completed = self
                .buffer_release_metrics
                .buffer_releases_completed
                .saturating_add(1);
        } else {
            self.buffer_release_metrics.buffer_releases_discarded = self
                .buffer_release_metrics
                .buffer_releases_discarded
                .saturating_add(1);
        }
    }

    pub(in crate::compositor) fn release_materialized_shm(
        &mut self,
        release: SafeShmRelease,
        copy_to_release_us: u64,
    ) {
        self.complete_materialized_shm_release(release);
        self.shm_buffer_lifetime_metrics.shm_materializations_total = self
            .shm_buffer_lifetime_metrics
            .shm_materializations_total
            .saturating_add(1);
        self.shm_buffer_lifetime_metrics
            .shm_releases_after_materialization_total = self
            .shm_buffer_lifetime_metrics
            .shm_releases_after_materialization_total
            .saturating_add(1);
        self.shm_buffer_lifetime_metrics.shm_copy_to_release_us = self
            .shm_buffer_lifetime_metrics
            .shm_copy_to_release_us
            .saturating_add(copy_to_release_us);
    }

    pub(in crate::compositor) fn replace_current_surface_buffer(
        &mut self,
        surface_id: u32,
        current: CurrentSurfaceBuffer,
    ) {
        if let Some(CurrentSurfaceBuffer::Unmaterialized(pending)) =
            self.current_surface_buffers.insert(surface_id, current)
            && pending.data.is_shm()
        {
            self.release_unmaterialized_pending_buffer(pending, true);
        }
    }

    pub(in crate::compositor) fn remove_current_surface_buffer(&mut self, surface_id: u32) {
        let Some(previous) = self.current_surface_buffers.remove(&surface_id) else {
            return;
        };
        if let CurrentSurfaceBuffer::Unmaterialized(pending) = previous
            && pending.data.is_shm()
        {
            self.release_unmaterialized_pending_buffer(pending, false);
        }
    }

    pub(in crate::compositor) fn track_committed_buffer_lifetime(
        &mut self,
        surface_id: u32,
        pending: &PendingSurfaceBuffer,
    ) {
        if pending.data.is_shm() {
            if let Some(release) = self.active_dmabuf_buffers.remove(&surface_id) {
                self.queue_dmabuf_buffer_release(release);
            }
            return;
        }

        let new_release = DmabufReleaseObligation {
            buffer_id: pending.data.buffer_id(),
            release: pending.release_target(),
        };
        self.reclassify_reactivated_dmabuf_release(&new_release);
        if let Some(previous) = self
            .active_dmabuf_buffers
            .insert(surface_id, new_release.clone())
            && !previous.same_release_token(&new_release)
        {
            self.queue_dmabuf_buffer_release(previous);
        }
    }

    pub(in crate::compositor) fn queue_dmabuf_buffer_release(
        &mut self,
        obligation: DmabufReleaseObligation,
    ) {
        // Published-buffer replacement and surface removal/destruction both converge here. Only
        // the exact same completion token is a duplicate; a reused wl_buffer with a newer
        // explicit-sync timeline point is a distinct use.
        if self.buffer_release_is_owned(&obligation) {
            self.note_buffer_release_duplicate_attempt();
            return;
        }
        self.pending_dmabuf_buffer_releases.push(obligation);
    }
}
