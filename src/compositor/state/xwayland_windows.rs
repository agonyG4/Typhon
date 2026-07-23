use super::*;
use crate::xwayland::trace::{self, TraceFields};

impl CompositorState {
    pub(in crate::compositor) fn retire_xwayland_attachment(&mut self, surface_id: u32) {
        let mut retired_ids = self
            .surface_placements
            .keys()
            .copied()
            .filter(|candidate| self.root_surface_id_for_surface(*candidate) == surface_id)
            .collect::<std::collections::HashSet<_>>();
        retired_ids.insert(surface_id);
        self.xwayland
            .retired_surface_ids
            .extend(retired_ids.iter().copied());
        self.withdraw_xwayland_surface_content(surface_id);
        self.xwayland
            .buffer_ready_events
            .retain(|event| !retired_ids.contains(&event.surface_id));
        self.xwayland
            .buffer_level_events
            .retain(|(_, candidate)| !retired_ids.contains(candidate));
        for retired_id in retired_ids {
            self.cancel_pending_surface_trees_for_surface(
                retired_id,
                AcquireWatchCancelReason::SurfaceDestroyed,
            );
            let callbacks = self.cancel_pending_acquire_commits_for_surface(
                retired_id,
                AcquireWatchCancelReason::SurfaceDestroyed,
            );
            self.complete_frame_callbacks(callbacks);
            self.discard_pending_presentation_feedbacks_for_surface(retired_id);
            if let Some(feedbacks) = self
                .pending_surface_presentation_feedbacks
                .remove(&retired_id)
            {
                for feedback in feedbacks {
                    feedback.feedback.discarded();
                }
            }
            self.surface_publications.remove(&retired_id);
            self.surface_damage_journals.remove(&retired_id);
            self.presented_surface_commits.remove(&retired_id);
            self.surface_presentation_generations.remove(&retired_id);
            self.clear_resize_state_for_surfaces_with_reason(
                &[retired_id],
                WindowInteractionEndReason::SurfaceDestroyed,
            );
            if let Some(current) = self.current_surface_buffers.remove(&retired_id)
                && current.data.is_shm()
            {
                self.queue_buffer_release(current.resource);
            }
            if let Some(release) = self.active_dmabuf_buffers.remove(&retired_id) {
                self.queue_dmabuf_buffer_release(release);
            }
        }
    }

    pub(in crate::compositor) fn withdraw_xwayland_surface_content(
        &mut self,
        root_surface_id: u32,
    ) -> bool {
        self.pending_xwayland_visual_content
            .remove(&root_surface_id);
        let mut surface_ids = self
            .surface_placements
            .keys()
            .copied()
            .filter(|surface_id| self.root_surface_id_for_surface(*surface_id) == root_surface_id)
            .collect::<std::collections::HashSet<_>>();
        surface_ids.insert(root_surface_id);

        let withdrawn_ids = self
            .renderable_surfaces
            .iter()
            .filter_map(|surface| {
                surface_ids
                    .contains(&surface.surface_id)
                    .then_some(surface.surface_id)
            })
            .collect::<std::collections::HashSet<_>>();
        let minimized_ids_removed = self
            .window_id_for_surface(root_surface_id)
            .and_then(|window_id| self.window_mut(window_id))
            .map(|window| {
                let before = window.state.minimized_surfaces_len();
                window.state.remove_minimized_surface_ids(&surface_ids);
                before != window.state.minimized_surfaces_len()
            })
            .unwrap_or(false);

        if !withdrawn_ids.is_empty() {
            self.renderable_surfaces
                .retain(|surface| !withdrawn_ids.contains(&surface.surface_id));
            self.invalidate_surface_origin_cache();
            self.reconcile_all_surface_output_memberships();
            self.advance_render_generation(RenderGenerationCause::SurfaceUnmap);
        }

        !withdrawn_ids.is_empty() || minimized_ids_removed
    }

    pub(in crate::compositor) fn reapply_root_visual_assignment_after_surface_publication(
        &mut self,
        root_surface_id: u32,
    ) {
        if self
            .toplevel_visual_geometries
            .contains_key(&root_surface_id)
            || self.surface_placement(root_surface_id).root_mode == RootPlacementMode::Absolute
        {
            self.update_toplevel_visual_render_assignment(root_surface_id);
        }
    }

    pub(in crate::compositor) fn transfer_xwayland_visual_state_for_attachment_replacement(
        &mut self,
        old_surface_id: u32,
        replacement_surface_id: u32,
    ) {
        if self.pending_xwayland_visual_content.remove(&old_surface_id) {
            self.pending_xwayland_visual_content
                .insert(replacement_surface_id);
        }
        if let Some(visual) = self.toplevel_visual_geometries.remove(&old_surface_id) {
            self.toplevel_visual_geometries
                .insert(replacement_surface_id, visual);
        }
        if let Some(active_resize) = self.active_toplevel_resizes.remove(&old_surface_id) {
            self.active_toplevel_resizes
                .insert(replacement_surface_id, active_resize);
        }
        if let Some(flow) = self.resize_configure_flows.remove(&old_surface_id) {
            self.resize_configure_flows
                .insert(replacement_surface_id, flow);
        }
        if let Some(update) = self.pending_interactive_resize_update.as_mut()
            && update.root_surface_id == old_surface_id
        {
            update.root_surface_id = replacement_surface_id;
        }
        if let Some(interaction) = self.window_interaction.as_mut()
            && interaction.root_surface_id == old_surface_id
        {
            interaction.root_surface_id = replacement_surface_id;
        }
    }

    pub(in crate::compositor) fn update_pending_xwayland_visual_content(
        &mut self,
        root_surface_id: u32,
    ) {
        let content_matches_visual = self
            .toplevel_visual_geometries
            .get(&root_surface_id)
            .copied()
            .zip(
                self.renderable_surfaces
                    .iter()
                    .find(|surface| surface.surface_id == root_surface_id),
            )
            .is_some_and(|(visual, surface)| {
                surface.width == visual.width && surface.height == visual.height
            });
        if content_matches_visual {
            self.pending_xwayland_visual_content
                .remove(&root_surface_id);
        } else {
            self.pending_xwayland_visual_content.insert(root_surface_id);
        }
    }

    pub(in crate::compositor) fn clear_pending_xwayland_visual_content_if_matching(
        &mut self,
        root_surface_id: u32,
    ) {
        if !self
            .pending_xwayland_visual_content
            .contains(&root_surface_id)
        {
            return;
        }
        let content_matches_visual = self
            .toplevel_visual_geometries
            .get(&root_surface_id)
            .copied()
            .zip(
                self.renderable_surfaces
                    .iter()
                    .find(|surface| surface.surface_id == root_surface_id),
            )
            .is_some_and(|(visual, surface)| {
                surface.width == visual.width && surface.height == visual.height
            });
        if content_matches_visual {
            self.pending_xwayland_visual_content
                .remove(&root_surface_id);
        }
    }

    pub(in crate::compositor) fn adopt_current_xwayland_surface_content(
        &mut self,
        surface_id: u32,
    ) -> bool {
        if !matches!(self.surface_role(surface_id), SurfaceRole::Xwayland) {
            return false;
        }

        let Some(surface_generation) = self
            .xwayland
            .surface_states
            .get(&surface_id)
            .map(|state| state.generation)
        else {
            return false;
        };
        if self
            .xwayland
            .client_identity
            .as_ref()
            .is_none_or(|identity| identity.generation != surface_generation)
        {
            return false;
        }

        let Some(window_id) = self.window_id_for_surface(surface_id) else {
            return false;
        };
        if !self.window(window_id).is_some_and(|window| {
            matches!(
                window.backend,
                WindowBackend::X11(handle) if handle.generation() == surface_generation
            ) && !window.state.is_minimized()
        }) {
            return false;
        }

        let Some(current) = self.current_surface_buffers.get(&surface_id).cloned() else {
            return false;
        };
        let generation = self.next_render_generation_value();
        let placement = self.surface_placement(surface_id);
        let Ok(surface) = current.to_renderable_surface(
            surface_id,
            placement,
            generation,
            RenderableSurfaceDamage::Full,
        ) else {
            return false;
        };
        let buffer_size = surface.buffer_size();

        self.renderable_surfaces
            .retain(|existing| existing.surface_id != surface_id);
        self.renderable_surfaces.push(surface);
        self.clear_pending_xwayland_visual_content_if_matching(surface_id);
        self.record_surface_damage_commit(
            surface_id,
            RenderableSurfaceDamage::Full,
            buffer_size.width,
            buffer_size.height,
        );
        self.reorder_renderable_surfaces_by_committed_stack();
        self.reapply_root_visual_assignment_after_surface_publication(surface_id);
        self.set_render_generation(generation, RenderGenerationCause::SurfaceCommit);
        true
    }

    pub(in crate::compositor) fn commit_xwayland_surface_buffer(
        &mut self,
        surface_id: u32,
        pending: PendingSurfaceBuffer,
        frame_callbacks: Vec<wl_callback::WlCallback>,
        source: SurfacePublicationSource,
    ) {
        if self.xwayland.retired_surface_ids.contains(&surface_id) {
            pending.release_target().release();
            self.complete_frame_callbacks(frame_callbacks);
            return;
        }
        let root_surface_id = self.root_surface_id_for_surface(surface_id);
        if self.window_id_for_surface(root_surface_id).is_none() {
            self.commit_unassigned_surface_buffer(surface_id, pending, frame_callbacks, source);
            return;
        }
        let x11_window_minimized = self
            .window_id_for_surface(root_surface_id)
            .and_then(|window_id| self.window(window_id))
            .is_some_and(|window| {
                matches!(window.backend, WindowBackend::X11(_)) && window.state.is_minimized()
            });
        let commit_sequence = pending.commit_sequence;
        let buffer_id = pending.data.buffer_id();
        let association_serial = self
            .xwayland
            .associations
            .serial_for_surface(surface_id)
            .map(|(_, serial)| serial.get());
        let generation = self.next_render_generation_value();
        let placement = self.surface_placement(root_surface_id);
        let Ok(surface) = pending.to_renderable_surface(
            surface_id,
            placement,
            generation,
            RenderableSurfaceDamage::Full,
        ) else {
            pending.release_target().release();
            self.complete_frame_callbacks(frame_callbacks);
            return;
        };
        let old_render_placement = self
            .renderable_surfaces
            .iter()
            .find(|existing| existing.surface_id == surface_id)
            .and_then(|existing| existing.render_placement);
        let old_visual_clip = self
            .renderable_surfaces
            .iter()
            .find(|existing| existing.surface_id == surface_id)
            .and_then(|existing| existing.visual_clip);
        let visual_geometry = self
            .toplevel_visual_geometries
            .get(&root_surface_id)
            .copied();
        let active_resize = visual_geometry.and_then(|visual| visual.active_resize);
        let content_pending = self
            .pending_xwayland_visual_content
            .contains(&root_surface_id);
        let xid = self
            .window_id_for_surface(root_surface_id)
            .and_then(|window_id| self.window(window_id))
            .and_then(|window| match window.backend {
                WindowBackend::X11(handle) => Some(handle.xid()),
                WindowBackend::Xdg(_) => None,
            });
        let buffer_size = surface.buffer_size();
        let fresh_render_placement_before_reapply = surface.render_placement;
        let fresh_visual_clip_before_reapply = surface.visual_clip;
        trace::emit("xwayland_visual_assignment_before_reapply", || {
            TraceFields::new()
                .field("root_surface_id", root_surface_id)
                .field("surface_id", surface_id)
                .optional("xid", xid)
                .field("active_resize", format!("{active_resize:?}"))
                .field("content_pending", content_pending)
                .field("visual_geometry", format!("{visual_geometry:?}"))
                .field("committed_buffer_size", format!("{buffer_size:?}"))
                .field("old_render_placement", format!("{old_render_placement:?}"))
                .field("old_visual_clip", format!("{old_visual_clip:?}"))
                .field(
                    "fresh_render_placement_before_reapply",
                    format!("{fresh_render_placement_before_reapply:?}"),
                )
                .field(
                    "fresh_visual_clip_before_reapply",
                    format!("{fresh_visual_clip_before_reapply:?}"),
                )
        });
        self.track_committed_buffer_lifetime(surface_id, &pending);
        self.current_surface_buffers.insert(surface_id, pending);
        trace::emit("xwayland_commit_observed", || {
            TraceFields::new()
                .field("source", "wayland")
                .field("surface_id", surface_id)
                .field("commit_sequence", commit_sequence.get())
                .field("buffer_id", buffer_id.get())
                .field("buffer_width", buffer_size.width)
                .field("buffer_height", buffer_size.height)
                .optional("association_serial", association_serial)
                .field("buffer_ready_level", true)
        });
        self.renderable_surfaces
            .retain(|existing| existing.surface_id != surface_id);
        if !x11_window_minimized {
            self.renderable_surfaces.push(surface);
            self.clear_pending_xwayland_visual_content_if_matching(root_surface_id);
        }
        self.record_surface_publication(
            surface_id,
            root_surface_id,
            commit_sequence,
            Some(buffer_id),
            source,
            None,
        );
        self.record_surface_damage_commit(
            surface_id,
            RenderableSurfaceDamage::Full,
            buffer_size.width,
            buffer_size.height,
        );
        if !x11_window_minimized {
            self.reorder_renderable_surfaces_by_committed_stack();
            self.reapply_root_visual_assignment_after_surface_publication(root_surface_id);
            let render_placement_after_reapply = self
                .renderable_surfaces
                .iter()
                .find(|existing| existing.surface_id == surface_id)
                .and_then(|existing| existing.render_placement);
            let visual_clip_after_reapply = self
                .renderable_surfaces
                .iter()
                .find(|existing| existing.surface_id == surface_id)
                .and_then(|existing| existing.visual_clip);
            let content_pending_after_reapply = self
                .pending_xwayland_visual_content
                .contains(&root_surface_id);
            trace::emit("xwayland_visual_assignment_after_reapply", || {
                TraceFields::new()
                    .field("root_surface_id", root_surface_id)
                    .field("surface_id", surface_id)
                    .optional("xid", xid)
                    .field("active_resize", format!("{active_resize:?}"))
                    .field("content_pending", content_pending_after_reapply)
                    .field("visual_geometry", format!("{visual_geometry:?}"))
                    .field("committed_buffer_size", format!("{buffer_size:?}"))
                    .field("old_render_placement", format!("{old_render_placement:?}"))
                    .field("old_visual_clip", format!("{old_visual_clip:?}"))
                    .field(
                        "fresh_render_placement_before_reapply",
                        format!("{fresh_render_placement_before_reapply:?}"),
                    )
                    .field(
                        "fresh_visual_clip_before_reapply",
                        format!("{fresh_visual_clip_before_reapply:?}"),
                    )
                    .field(
                        "render_placement_after_reapply",
                        format!("{render_placement_after_reapply:?}"),
                    )
                    .field(
                        "visual_clip_after_reapply",
                        format!("{visual_clip_after_reapply:?}"),
                    )
            });
            self.set_render_generation(generation, RenderGenerationCause::SurfaceCommit);
        }
        self.note_xwayland_buffer_ready(surface_id);
        self.note_xwayland_commit_observed(
            surface_id,
            commit_sequence,
            Some(buffer_id),
            Some(buffer_size),
        );
        if x11_window_minimized {
            self.complete_frame_callbacks(frame_callbacks);
        } else {
            self.pending_frame_callbacks.extend(frame_callbacks);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_surface(surface_id: u32, width: u32, height: u32) -> RenderableSurface {
        let identity = BufferIdAllocator::default()
            .allocate()
            .expect("test buffer identity");
        RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width,
            height,
            placement: SurfacePlacement::root(),
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: crate::render_backend::buffer::CommittedSurfaceBuffer::shm_snapshot(
                identity,
                BufferSize::new(width, height).expect("test size"),
                vec![0; width as usize * height as usize],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::Full,
        }
    }

    #[test]
    fn xwayland_withdrawal_unpublishes_render_tree_without_forgetting_placement() {
        let mut state = CompositorState::default();
        let root_id = 42;
        let child_id = 43;
        state
            .renderable_surfaces
            .push(test_surface(root_id, 10, 10));
        state.renderable_surfaces.push(test_surface(child_id, 1, 1));
        assert!(state.set_surface_placement(root_id, SurfacePlacement::absolute_root_at(10, 10),));
        assert!(
            state.set_surface_placement(child_id, SurfacePlacement::subsurface(root_id, 1, 1),)
        );
        let generation = state.render_generation;

        assert!(state.withdraw_xwayland_surface_content(root_id));

        assert!(state.renderable_surfaces.is_empty());
        assert_eq!(
            state.surface_placement(root_id),
            SurfacePlacement::absolute_root_at(10, 10),
        );
        assert_eq!(
            state.surface_placement(child_id),
            SurfacePlacement::subsurface(root_id, 1, 1),
        );
        assert!(state.render_generation > generation);
        assert!(!state.withdraw_xwayland_surface_content(root_id));
    }
}
