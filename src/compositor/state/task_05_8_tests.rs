#![allow(clippy::module_inception)]

use super::*;

#[cfg(test)]
mod task_05_8_tests {
    use super::*;
    use crate::compositor::interaction::MAX_IN_FLIGHT_RESIZE_CONFIGURES;
    use crate::wm::{WindowManagementState, WorkspaceId, WorkspaceSwitchOutcome};
    use std::borrow::Cow;

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
            render_backend: SurfaceRenderBackend::NativeWayland,
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

    #[test]
    fn repeated_native_frame_resolution_borrows_stable_active_scene_view() {
        let mut state = CompositorState::new(None);
        let first = state.allocate_window_id().expect("first window id");
        let second = state.allocate_window_id().expect("second window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(first, 801))
            .expect("first window");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(second, 802))
            .expect("second window");
        state.window_mut(second).expect("second window").management =
            Some(WindowManagementState::new(
                crate::wm::WorkspaceLocation::Regular(WorkspaceId::new(2).expect("workspace two")),
            ));
        state.append_renderable_surface(test_surface(801, 16, 16));
        state.append_renderable_surface(test_surface(802, 16, 16));
        state.rebuild_active_scene_view();
        let rebuilds = state.active_scene_rebuild_count();

        for _ in 0..1000 {
            let surfaces = state.native_frame_renderable_surfaces();
            assert!(matches!(surfaces, Cow::Borrowed(_)));
            assert_eq!(
                surfaces
                    .iter()
                    .map(|surface| surface.surface_id)
                    .collect::<Vec<_>>(),
                [801]
            );
        }
        assert_eq!(state.active_scene_rebuild_count(), rebuilds);
        assert_eq!(state.active_scene_surface_update_count(), 0);
    }

    #[test]
    fn renderable_surface_index_survives_content_and_topology_mutations() {
        let mut state = CompositorState::new(None);
        state.append_renderable_surface(test_surface(801, 16, 16));
        state.append_renderable_surface(test_surface(802, 16, 16));
        state.assert_renderable_surface_index_invariant_for_test();

        let mut replacement = test_surface(801, 32, 32);
        replacement.generation = 2;
        state.replace_renderable_surface(801, replacement);
        state.assert_renderable_surface_index_invariant_for_test();

        state.remove_renderable_surface(802);
        state.assert_renderable_surface_index_invariant_for_test();

        state.append_renderable_surface(test_surface(803, 16, 16));
        state.renderable_surfaces.swap(0, 1);
        state.rebuild_renderable_surface_index();
        state.assert_renderable_surface_index_invariant_for_test();

        assert_eq!(
            state.renderable_surface(801).map(|surface| surface.width),
            Some(32)
        );
        assert_eq!(state.renderable_surface_index(803), Some(0));
    }

    #[test]
    fn active_scene_does_not_fallback_to_unindexed_renderables_when_empty() {
        let mut state = CompositorState::new(None);
        state.append_renderable_surface(test_surface(899, 16, 16));

        assert!(state.active_scene_surfaces().is_empty());
    }

    #[test]
    fn visible_special_workspace_is_an_overlay_selection_not_a_regular_workspace() {
        let mut state = CompositorState::new(None);
        let regular = state.allocate_window_id().expect("regular window id");
        let special = state.allocate_window_id().expect("special window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(regular, 901))
            .expect("regular window");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(special, 902))
            .expect("special window");
        let special_id = crate::wm::SpecialWorkspaceId::DEFAULT;
        state
            .window_mut(special)
            .expect("special window")
            .management = Some(WindowManagementState::new(
            crate::wm::WorkspaceLocation::Special(special_id),
        ));
        state.append_renderable_surface(test_surface(902, 16, 16));
        state.append_renderable_surface(test_surface(901, 16, 16));
        state.workspace_manager.toggle_special_workspace(special_id);
        state.rebuild_active_scene_view();

        assert_eq!(state.active_workspace(), WorkspaceId::new(1).unwrap());
        assert_eq!(
            state
                .active_scene_surfaces()
                .iter()
                .map(|surface| surface.surface_id)
                .collect::<Vec<_>>(),
            [901, 902]
        );
    }

    #[test]
    fn visible_special_application_blocks_solitary_fullscreen_culling() {
        let mut state = CompositorState::new(None);
        let regular = state.allocate_window_id().expect("regular window id");
        let special = state.allocate_window_id().expect("special window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(regular, 903))
            .expect("regular window");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(special, 904))
            .expect("special window");
        state
            .window_mut(special)
            .expect("special window")
            .management = Some(WindowManagementState::new(
            crate::wm::WorkspaceLocation::Special(crate::wm::SpecialWorkspaceId::DEFAULT),
        ));
        state.append_renderable_surface(test_surface(903, 16, 16));
        state.append_renderable_surface(test_surface(904, 16, 16));

        state.toggle_default_special_workspace();
        assert!(state.has_visible_application_content_outside_fullscreen_owner(903));
        assert_eq!(
            state
                .active_scene_surfaces()
                .iter()
                .map(|surface| surface.surface_id)
                .collect::<Vec<_>>(),
            [903, 904]
        );

        state.toggle_default_special_workspace();
        assert!(!state.has_visible_application_content_outside_fullscreen_owner(903));
    }

    #[test]
    fn empty_special_selection_change_does_not_advance_scene_generation() {
        let mut state = CompositorState::new(None);
        let before_open = state.scene_render_generation;

        state.toggle_default_special_workspace();
        assert_eq!(state.scene_render_generation, before_open);

        state.toggle_default_special_workspace();
        assert_eq!(state.scene_render_generation, before_open);
    }

    #[test]
    fn populated_special_selection_changes_scene_generation_once_each_direction() {
        let mut state = CompositorState::new(None);
        let special = state.allocate_window_id().expect("special window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(special, 905))
            .expect("special window");
        state
            .window_mut(special)
            .expect("special window")
            .management = Some(WindowManagementState::new(
            crate::wm::WorkspaceLocation::Special(crate::wm::SpecialWorkspaceId::DEFAULT),
        ));
        state.append_renderable_surface(test_surface(905, 16, 16));

        let before_open = state.scene_render_generation;
        state.toggle_default_special_workspace();
        assert_eq!(state.scene_render_generation, before_open + 1);

        let before_close = state.scene_render_generation;
        state.toggle_default_special_workspace();
        assert_eq!(state.scene_render_generation, before_close + 1);
    }

    #[test]
    fn active_scene_update_separates_selection_from_visual_change() {
        let mut state = CompositorState::new(None);
        let initial = state.rebuild_active_scene_view();
        assert!(!initial.selection_changed);
        assert!(!initial.visual_scene_changed);

        state
            .workspace_manager
            .toggle_special_workspace(crate::wm::SpecialWorkspaceId::DEFAULT);
        let empty_special = state.rebuild_active_scene_view();
        assert!(empty_special.selection_changed);
        assert!(!empty_special.visual_scene_changed);
    }

    #[test]
    fn hidden_surface_publication_advances_global_but_not_active_scene_generation() {
        let mut state = CompositorState::new(None);
        let first = state.allocate_window_id().expect("first window id");
        let second = state.allocate_window_id().expect("second window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(first, 811))
            .expect("first window");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(second, 812))
            .expect("second window");
        state.window_mut(second).expect("second window").management =
            Some(WindowManagementState::new(
                crate::wm::WorkspaceLocation::Regular(WorkspaceId::new(2).expect("workspace two")),
            ));
        state.append_renderable_surface(test_surface(811, 16, 16));
        state.append_renderable_surface(test_surface(812, 16, 16));
        state.rebuild_active_scene_view();
        let scene_before = state.scene_render_generation;
        let render_before = state.render_generation;

        for generation in 2..=4 {
            let surface = state
                .renderable_surfaces
                .iter_mut()
                .find(|surface| surface.surface_id == 812)
                .expect("hidden surface");
            surface.generation = generation;
            surface.commit_sequence = SurfaceCommitSequence(generation);
            state.publish_surface_generation(812, generation, RenderGenerationCause::SurfaceCommit);
        }

        assert!(state.render_generation > render_before);
        assert_eq!(state.scene_render_generation, scene_before);
        assert_eq!(state.active_scene_surfaces()[0].surface_id, 811);

        let workspace_two = WorkspaceId::new(2).expect("workspace two");
        assert!(matches!(
            state.activate_workspace(workspace_two),
            WorkspaceSwitchOutcome::Changed { .. }
        ));
        assert_eq!(state.scene_render_generation, scene_before + 1);
        assert_eq!(state.active_scene_surfaces()[0].surface_id, 812);
        assert_eq!(state.active_scene_surfaces()[0].generation, 4);
    }

    #[test]
    fn hidden_only_window_stack_reorder_does_not_advance_active_scene_generation() {
        let mut state = CompositorState::new(None);
        let active = state.allocate_window_id().expect("active window id");
        let hidden_first = state.allocate_window_id().expect("hidden window id");
        let hidden_second = state.allocate_window_id().expect("hidden window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(active, 921))
            .expect("active window");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(hidden_first, 922))
            .expect("hidden window");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(hidden_second, 923))
            .expect("hidden window");
        for window_id in [hidden_first, hidden_second] {
            state
                .window_mut(window_id)
                .expect("hidden window")
                .management = Some(WindowManagementState::new(
                crate::wm::WorkspaceLocation::Regular(WorkspaceId::new(2).unwrap()),
            ));
        }
        for surface in [
            test_surface(921, 16, 16),
            test_surface(922, 16, 16),
            test_surface(923, 16, 16),
        ] {
            state.append_renderable_surface(surface);
        }
        state.window_stacking = vec![hidden_second, hidden_first, active];
        state.rebuild_active_scene_view();
        let scene_before = state.scene_render_generation;

        assert!(state.reorder_renderable_surfaces_by_window_stack());
        assert_eq!(state.scene_render_generation, scene_before);
        assert_eq!(
            state
                .active_scene_surfaces()
                .iter()
                .map(|surface| surface.surface_id)
                .collect::<Vec<_>>(),
            [921]
        );
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
        state.append_renderable_surface(test_surface(surface_id, 944, 502));

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
            surface
                .visual_clip
                .as_ref()
                .map(|clip| clip.logical_target()),
            Some(render::SurfaceTargetRect::new(10, 20, 1100, 650))
        );
    }

    #[test]
    pub(in crate::compositor) fn task_05_8_csd_window_geometry_aligns_root_and_titlebar() {
        let mut state = CompositorState::default();
        let root_id = 42;
        let titlebar_id = 43;
        state.append_renderable_surface(test_surface(root_id, 944, 502));
        let mut titlebar = test_surface(titlebar_id, 944, 24);
        titlebar.placement = SurfacePlacement::subsurface(root_id, 0, -24);
        state
            .surface_placements
            .insert(titlebar_id, titlebar.placement);
        state.append_renderable_surface(titlebar);
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
                mode_transition: false,
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
    pub(in crate::compositor) fn task_05_8_configure_window_allows_pipelined_resize_targets() {
        let mut flow = ResizeConfigureFlow::default();
        let desired_a = PendingResizeConfigure {
            surface_id: 42,
            width: 1000,
            height: 700,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(1),
        };
        flow.mark_sent(desired_a, 10, 1);

        assert_eq!(flow.ack(10), ResizeAckDecision::Matched);
        assert_eq!(flow.retained_configure_count(), 1);
        assert_eq!(flow.captured_count(), 0);
        let desired_b = PendingResizeConfigure {
            surface_id: 42,
            width: 1200,
            height: 700,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(1),
        };
        assert!(flow.queue(desired_b));
        let desired_b = flow
            .take_sendable()
            .expect("ACKed content must not block the next bounded configure");
        flow.mark_sent(desired_b, 11, 2);

        let desired_c = PendingResizeConfigure {
            width: 1300,
            ..desired_b
        };
        assert!(flow.queue(desired_c));
        let desired_c = flow
            .take_sendable()
            .expect("the configure window should accept a third target");
        flow.mark_sent(desired_c, 12, 3);

        assert!(flow.queue(PendingResizeConfigure {
            width: 1400,
            ..desired_c
        }));
        assert!(
            flow.take_sendable().is_none(),
            "configure pressure must be bounded"
        );

        let snapshot = flow.capture(90).expect("ACKed resize snapshot");
        assert_eq!(snapshot.sequence, 1);
        assert!(flow.take_sendable().is_some());
    }

    #[test]
    pub(in crate::compositor) fn resize_flow_stall_does_not_block_future_interaction_forever() {
        let mut flow = ResizeConfigureFlow::default();
        let old = PendingResizeConfigure {
            surface_id: 42,
            width: 1000,
            height: 700,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(1),
        };
        flow.mark_sent(old, 10, 1);
        assert_eq!(flow.ack(10), ResizeAckDecision::Matched);
        let _snapshot = flow.capture(90).expect("old captured resize");

        let result = flow.begin_interaction(ResizeInteractionId::new(2));
        assert_eq!(result.obsolete_in_flight_discarded, 0);
        assert_eq!(flow.captured_count(), 1);
        assert!(flow.queue(PendingResizeConfigure {
            surface_id: 42,
            width: 1200,
            height: 760,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(2),
        }));

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

        let desired_b = PendingResizeConfigure {
            width: 1100,
            ..desired
        };
        assert!(flow.queue(desired_b));
        let desired_b = flow.take_sendable().expect("captured A is not pressure");
        flow.mark_sent(desired_b, 11, 2);
        assert_eq!(flow.ack(11), ResizeAckDecision::Matched);
        let snapshot_b = flow.capture(91).expect("snapshot B");

        assert_eq!(snapshot_a.commit_sequence, 90);
        assert_eq!(snapshot_b.commit_sequence, 91);
        assert_eq!(flow.captured_count(), 2);
        assert_eq!(flow.retained_configure_count(), 2);
    }

    #[test]
    pub(in crate::compositor) fn newer_ack_supersedes_older_outstanding_configures() {
        let mut flow = ResizeConfigureFlow::default();
        let target = PendingResizeConfigure {
            surface_id: 42,
            width: 1000,
            height: 700,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(1),
        };
        flow.mark_sent(target, 10, 1);
        flow.mark_sent(
            PendingResizeConfigure {
                width: 1100,
                ..target
            },
            11,
            2,
        );
        flow.mark_sent(
            PendingResizeConfigure {
                width: 1200,
                ..target
            },
            12,
            3,
        );

        assert_eq!(flow.ack(12), ResizeAckDecision::Matched);
        assert_eq!(flow.outstanding_count(), 0);
        assert_eq!(flow.acked_uncaptured_sequence(), Some(3));
        assert_eq!(flow.ack(10), ResizeAckDecision::Stale);
        assert_eq!(flow.ack(11), ResizeAckDecision::Stale);
        assert!(flow.capture(90).is_some());
    }

    #[test]
    pub(in crate::compositor) fn newer_ack_replaces_uncaptured_ack_before_commit() {
        let mut flow = ResizeConfigureFlow::default();
        let target = PendingResizeConfigure {
            surface_id: 42,
            width: 1000,
            height: 700,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(1),
        };
        flow.mark_sent(target, 10, 1);
        flow.mark_sent(
            PendingResizeConfigure {
                width: 1100,
                ..target
            },
            11,
            2,
        );

        assert_eq!(flow.ack(10), ResizeAckDecision::Matched);
        assert_eq!(flow.ack(11), ResizeAckDecision::Matched);
        assert_eq!(flow.acked_uncaptured_sequence(), Some(2));
        let captured = flow.capture(90).expect("latest ACK owns the commit");
        assert_eq!(captured.sequence, 2);
    }

    #[test]
    pub(in crate::compositor) fn slow_resize_client_keeps_protocol_pressure_bounded() {
        let mut flow = ResizeConfigureFlow::default();
        let base = PendingResizeConfigure {
            surface_id: 42,
            width: 1000,
            height: 700,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(1),
        };
        flow.mark_sent(base, 10, 1);

        for index in 0..1_000u32 {
            let desired = PendingResizeConfigure {
                width: 1001 + index,
                ..base
            };
            assert!(flow.queue(desired));
            if let Some(sendable) = flow.take_sendable() {
                flow.mark_sent(sendable, 11 + index, 2 + u64::from(index));
            }
        }

        assert!(flow.in_flight_configure_count() <= MAX_IN_FLIGHT_RESIZE_CONFIGURES);
        assert!(flow.queued_latest().is_some());
        assert!(flow.final_pending().is_none());
        assert!(flow.retained_configure_count() <= MAX_IN_FLIGHT_RESIZE_CONFIGURES + 1);
    }

    #[test]
    pub(in crate::compositor) fn final_resize_target_supersedes_queued_intermediate_target() {
        let mut flow = ResizeConfigureFlow::default();
        let base = PendingResizeConfigure {
            surface_id: 42,
            width: 1000,
            height: 700,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(1),
        };
        flow.mark_sent(base, 10, 1);
        flow.mark_sent(
            PendingResizeConfigure {
                width: 1100,
                ..base
            },
            11,
            2,
        );
        flow.mark_sent(
            PendingResizeConfigure {
                width: 1200,
                ..base
            },
            12,
            3,
        );
        assert!(flow.queue(PendingResizeConfigure {
            width: 1300,
            ..base
        }));

        let final_target = PendingResizeConfigure {
            width: 1400,
            resizing: false,
            ..base
        };
        assert!(flow.queue_final(final_target));
        assert_eq!(flow.queued_latest(), None);
        assert_eq!(flow.final_pending(), Some(final_target));

        assert!(flow.take_sendable().is_none());
        assert_eq!(flow.ack(10), ResizeAckDecision::Matched);
        assert!(flow.capture(90).is_some());
        assert_eq!(flow.take_sendable(), Some(final_target));
    }

    #[test]
    pub(in crate::compositor) fn task_05_8_intermediate_and_final_resize_lifecycle() {
        let mut state = CompositorState::default();
        let surface_id = 42;
        let interaction_id = ResizeInteractionId::new(1);
        state.append_renderable_surface(test_surface(surface_id, 944, 502));
        state.toplevel_visual_geometries.insert(
            surface_id,
            ToplevelVisualGeometry {
                placement: SurfacePlacement::root_at(100, 100),
                width: 1200,
                height: 700,
                active_resize: Some(interaction_id),
                mode_transition: false,
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
        state.append_renderable_surface(test_surface(surface_id, 944, 502));
        state.toplevel_visual_geometries.insert(
            surface_id,
            ToplevelVisualGeometry {
                placement: SurfacePlacement::root_at(100, 100),
                width: 944,
                height: 502,
                active_resize: None,
                mode_transition: false,
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
        state.append_renderable_surface(test_surface(surface_id, 944, 502));
        state.toplevel_visual_geometries.insert(
            surface_id,
            ToplevelVisualGeometry {
                placement: SurfacePlacement::root_at(100, 100),
                width: 944,
                height: 502,
                active_resize: None,
                mode_transition: false,
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
        state.append_renderable_surface(test_surface(surface_id, 944, 502));
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
        state.append_renderable_surface(test_surface(root_id, 944, 502));
        let mut child = test_surface(child_id, 100, 40);
        child.placement = SurfacePlacement::subsurface(root_id, 12, 8);
        state.surface_placements.insert(child_id, child.placement);
        state.append_renderable_surface(child);
        assert!(state.preview_resize_root_window_to(
            root_id,
            1000,
            520,
            SurfacePlacement::root_at(100, 80),
            ResizeEdges::BOTTOM_RIGHT,
            interaction_id,
        ));
        assert!(state.renderable_surfaces[0].visual_clip.is_some());
        assert!(state.renderable_surfaces[1].visual_clip.is_none());
        state.toplevel_visual_geometries.insert(
            root_id,
            ToplevelVisualGeometry {
                placement: SurfacePlacement::root_at(100, 80),
                width: 1000,
                height: 520,
                active_resize: None,
                mode_transition: false,
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
