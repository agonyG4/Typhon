#![allow(clippy::module_inception)]

use super::*;

#[cfg(test)]
mod task_05_8_tests {
    use super::*;

    pub(in crate::compositor) fn test_surface(
        surface_id: u32,
        width: u32,
        height: u32,
    ) -> RenderableSurface {
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
            generation: 1,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: crate::render_backend::buffer::CommittedSurfaceBuffer::shm_snapshot(
                identity,
                BufferSize::new(width, height).expect("test size"),
                vec![0; width as usize * height as usize],
            ),
            viewport_source: None,
            damage: RenderableSurfaceDamage::Full,
        }
    }

    pub(in crate::compositor) fn test_resize_snapshot(
        _surface_id: u32,
        interaction_id: ResizeInteractionId,
        resizing: bool,
        width: u32,
        height: u32,
    ) -> ResizeCommitSnapshot {
        ResizeCommitSnapshot {
            serial: 7,
            sequence: 1,
            commit_sequence: 1,
            width,
            height,
            placement: SurfacePlacement::root_at(100, 100),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing,
            emitted_at: Instant::now(),
            committed_size: Some((width, height)),
            committed_window_geometry: None,
            buffer_id: None,
            interaction_id,
        }
    }

    fn install_captured_snapshot(
        state: &mut CompositorState,
        surface_id: u32,
        snapshot: ResizeCommitSnapshot,
    ) {
        let desired = PendingResizeConfigure {
            surface_id,
            width: snapshot.width,
            height: snapshot.height,
            placement: snapshot.placement,
            edges: snapshot.edges,
            resizing: snapshot.resizing,
            interaction_id: snapshot.interaction_id,
        };
        let flow = state.resize_configure_flows.entry(surface_id).or_default();
        flow.mark_sent(desired, snapshot.serial, snapshot.sequence);
        assert_eq!(flow.ack(snapshot.serial), ResizeAckDecision::Matched);
        let captured = flow
            .capture(snapshot.commit_sequence)
            .expect("snapshot should be captured before completion");
        assert_eq!(captured.sequence, snapshot.sequence);
    }

    #[test]
    pub(in crate::compositor) fn task_05_8_pointer_resize_changes_visual_box_not_surface_content() {
        let mut state = CompositorState::default();
        let surface_id = 42;
        let interaction_id = ResizeInteractionId::new(1);
        state
            .renderable_surfaces
            .push(test_surface(surface_id, 944, 502));

        assert!(state.preview_resize_root_window_to(
            surface_id,
            1100,
            650,
            SurfacePlacement::root_at(10, 20),
            ResizeEdges::BOTTOM_RIGHT,
            interaction_id,
        ));

        let visual = state
            .toplevel_visual_geometries
            .get(&surface_id)
            .expect("visual geometry");
        assert_eq!((visual.width, visual.height), (1100, 650));
        assert_eq!(visual.placement, SurfacePlacement::root_at(10, 20));
        let surface = &state.renderable_surfaces[0];
        assert_eq!((surface.width, surface.height), (944, 502));
        assert_eq!(
            surface.visual_clip,
            Some(render::SurfaceTargetRect::new(10, 20, 1100, 650))
        );
    }

    #[test]
    pub(in crate::compositor) fn task_05_8_csd_window_geometry_aligns_root_and_titlebar() {
        let mut state = CompositorState::default();
        let root_id = 42;
        let titlebar_id = 43;
        state
            .renderable_surfaces
            .push(test_surface(root_id, 944, 502));
        let mut titlebar = test_surface(titlebar_id, 944, 24);
        titlebar.placement = SurfacePlacement::subsurface(root_id, 0, -24);
        state
            .surface_placements
            .insert(titlebar_id, titlebar.placement);
        state.renderable_surfaces.push(titlebar);
        state
            .surface_window_geometries
            .insert(root_id, XdgWindowGeometry::new(0, -24, 944, 526));
        state.toplevel_visual_geometries.insert(
            root_id,
            ToplevelVisualGeometry {
                placement: SurfacePlacement::root_at(100, 100),
                width: 944,
                height: 526,
                active_resize: Some(ResizeInteractionId::new(1)),
            },
        );

        state.update_toplevel_visual_render_assignment(root_id);
        let origins = render::surface_origins(&state.renderable_surfaces);

        assert_eq!(
            origins[0],
            (
                render::FIRST_SURFACE_OFFSET.0 + 100,
                render::FIRST_SURFACE_OFFSET.1 + 124
            )
        );
        assert_eq!(
            origins[1],
            (
                render::FIRST_SURFACE_OFFSET.0 + 100,
                render::FIRST_SURFACE_OFFSET.1 + 100
            )
        );
        assert_eq!(
            (
                state.renderable_surfaces[0].width,
                state.renderable_surfaces[0].height
            ),
            (944, 502)
        );
    }

    #[test]
    pub(in crate::compositor) fn task_05_8_ack_waits_for_commit_before_freeing_configure_capacity()
    {
        let mut flow = ResizeConfigureFlow::default();
        let desired = PendingResizeConfigure {
            surface_id: 42,
            width: 1000,
            height: 700,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(1),
        };
        flow.mark_sent(desired, 10, 1);

        assert_eq!(flow.ack(10), ResizeAckDecision::Matched);
        assert_eq!(flow.retained_configure_count(), 1);
        assert_eq!(flow.captured_count(), 0);
        assert!(flow.queue(PendingResizeConfigure {
            surface_id: 42,
            width: 1200,
            height: 700,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(1),
        }));
        assert!(flow.take_sendable().is_none());
        let snapshot = flow.capture(90).expect("ACKed resize snapshot");
        assert!(flow.complete_applied(snapshot.sequence));
        assert!(flow.take_sendable().is_some());
    }

    #[test]
    pub(in crate::compositor) fn task_05_8_committed_snapshot_lives_outside_configure_flow() {
        let mut flow = ResizeConfigureFlow::default();
        let desired = PendingResizeConfigure {
            surface_id: 42,
            width: 1000,
            height: 620,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(1),
        };
        flow.mark_sent(desired, 10, 1);
        assert_eq!(flow.ack(10), ResizeAckDecision::Matched);
        let snapshot_a = flow.capture(90).expect("snapshot A");

        assert!(flow.complete_applied(snapshot_a.sequence));
        flow.mark_sent(
            PendingResizeConfigure {
                width: 1100,
                ..desired
            },
            11,
            2,
        );
        assert_eq!(flow.ack(11), ResizeAckDecision::Matched);
        let snapshot_b = flow.capture(91).expect("snapshot B");

        assert_eq!(snapshot_a.commit_sequence, 90);
        assert_eq!(snapshot_b.commit_sequence, 91);
        assert_eq!(flow.captured_count(), 1);
        assert_eq!(flow.retained_configure_count(), 1);
    }

    #[test]
    pub(in crate::compositor) fn task_05_8_intermediate_and_final_resize_lifecycle() {
        let mut state = CompositorState::default();
        let surface_id = 42;
        let interaction_id = ResizeInteractionId::new(1);
        state
            .renderable_surfaces
            .push(test_surface(surface_id, 944, 502));
        state.toplevel_visual_geometries.insert(
            surface_id,
            ToplevelVisualGeometry {
                placement: SurfacePlacement::root_at(100, 100),
                width: 1200,
                height: 700,
                active_resize: Some(interaction_id),
            },
        );
        state.active_toplevel_resizes.insert(
            surface_id,
            ActiveToplevelResize {
                interaction_id,
                flow_sequence: 1,
                edges: ResizeEdges::BOTTOM_RIGHT,
                activated_at: Instant::now(),
            },
        );

        let intermediate = test_resize_snapshot(surface_id, interaction_id, true, 1000, 620);
        install_captured_snapshot(&mut state, surface_id, intermediate);
        assert!(state.complete_pending_resize_from_current_geometry(surface_id, intermediate));
        let visual = state.toplevel_visual_geometries.get(&surface_id).unwrap();
        assert_eq!((visual.width, visual.height), (1200, 700));
        assert!(state.active_toplevel_resizes.contains_key(&surface_id));

        let final_snapshot = ResizeCommitSnapshot {
            sequence: 2,
            commit_sequence: 2,
            ..test_resize_snapshot(surface_id, interaction_id, false, 1000, 620)
        };
        install_captured_snapshot(&mut state, surface_id, final_snapshot);
        assert!(state.complete_pending_resize_from_current_geometry(surface_id, final_snapshot));
        assert!(!state.active_toplevel_resizes.contains_key(&surface_id));
        let visual = state.toplevel_visual_geometries.get(&surface_id).unwrap();
        assert_eq!((visual.width, visual.height), (1000, 620));
    }

    #[test]
    pub(in crate::compositor) fn task_05_8_move_updates_inactive_visual_geometry_and_render_origin()
    {
        let mut state = CompositorState::default();
        let surface_id = 42;
        state
            .renderable_surfaces
            .push(test_surface(surface_id, 944, 502));
        state.toplevel_visual_geometries.insert(
            surface_id,
            ToplevelVisualGeometry {
                placement: SurfacePlacement::root_at(100, 100),
                width: 944,
                height: 502,
                active_resize: None,
            },
        );
        state.update_toplevel_visual_render_assignment(surface_id);

        assert!(state.set_surface_placement_with_cause(
            surface_id,
            SurfacePlacement::root_at(160, 140),
            RenderGenerationCause::WindowMove,
        ));

        let visual = state.toplevel_visual_geometries.get(&surface_id).unwrap();
        assert_eq!(visual.placement, SurfacePlacement::root_at(160, 140));
        assert_eq!(
            state.renderable_surfaces[0].render_placement,
            Some(SurfacePlacement::root_at(160, 140))
        );
        assert_eq!(state.renderable_surfaces[0].visual_clip, None);
    }

    #[test]
    pub(in crate::compositor) fn inactive_visual_geometry_does_not_install_preview_clip() {
        let mut state = CompositorState::default();
        let surface_id = 42;
        state
            .renderable_surfaces
            .push(test_surface(surface_id, 944, 502));
        state.toplevel_visual_geometries.insert(
            surface_id,
            ToplevelVisualGeometry {
                placement: SurfacePlacement::root_at(100, 100),
                width: 944,
                height: 502,
                active_resize: None,
            },
        );

        state.update_toplevel_visual_render_assignment(surface_id);

        assert_eq!(state.renderable_surfaces[0].visual_clip, None);
        assert_eq!(
            state.renderable_surfaces[0].render_placement,
            Some(SurfacePlacement::root_at(100, 100))
        );
    }

    #[test]
    pub(in crate::compositor) fn final_resize_clears_preview_clip_and_keeps_root_render_placement()
    {
        let mut state = CompositorState::default();
        let surface_id = 42;
        let interaction_id = ResizeInteractionId::new(1);
        state
            .renderable_surfaces
            .push(test_surface(surface_id, 944, 502));
        state
            .surface_window_geometries
            .insert(surface_id, XdgWindowGeometry::new(16, 10, 944, 502));
        state.active_toplevel_resizes.insert(
            surface_id,
            ActiveToplevelResize {
                interaction_id,
                flow_sequence: 1,
                edges: ResizeEdges::new(false, false, false, true),
                activated_at: Instant::now(),
            },
        );
        assert!(state.preview_resize_root_window_to(
            surface_id,
            1000,
            502,
            SurfacePlacement::root_at(100, 80),
            ResizeEdges::new(false, false, false, true),
            interaction_id,
        ));

        let final_snapshot = test_resize_snapshot(surface_id, interaction_id, false, 1000, 502);
        install_captured_snapshot(&mut state, surface_id, final_snapshot);

        assert!(state.complete_pending_resize_from_current_geometry(surface_id, final_snapshot));
        assert_eq!(state.renderable_surfaces[0].visual_clip, None);
        assert_eq!(
            state.renderable_surfaces[0].render_placement,
            Some(SurfacePlacement::root_at(84, 90))
        );
    }

    #[test]
    pub(in crate::compositor) fn inactive_visual_geometry_clears_preview_clip_from_subsurfaces() {
        let mut state = CompositorState::default();
        let root_id = 42;
        let child_id = 43;
        let interaction_id = ResizeInteractionId::new(1);
        state
            .renderable_surfaces
            .push(test_surface(root_id, 944, 502));
        let mut child = test_surface(child_id, 100, 40);
        child.placement = SurfacePlacement::subsurface(root_id, 12, 8);
        state.surface_placements.insert(child_id, child.placement);
        state.renderable_surfaces.push(child);
        assert!(state.preview_resize_root_window_to(
            root_id,
            1000,
            520,
            SurfacePlacement::root_at(100, 80),
            ResizeEdges::BOTTOM_RIGHT,
            interaction_id,
        ));
        assert!(
            state
                .renderable_surfaces
                .iter()
                .all(|surface| surface.visual_clip.is_some())
        );
        state.toplevel_visual_geometries.insert(
            root_id,
            ToplevelVisualGeometry {
                placement: SurfacePlacement::root_at(100, 80),
                width: 1000,
                height: 520,
                active_resize: None,
            },
        );

        state.update_toplevel_visual_render_assignment(root_id);

        assert!(
            state
                .renderable_surfaces
                .iter()
                .all(|surface| surface.visual_clip.is_none())
        );
    }
}
