use super::*;

fn test_window_interaction(
    id: u64,
    kind: WindowInteractionKind,
    trigger_button: Option<u32>,
) -> WindowInteraction {
    test_window_interaction_with_target(
        id,
        kind,
        WindowInteractionSource::NativeBinding,
        trigger_button,
        None,
    )
}

fn test_window_interaction_with_target(
    id: u64,
    kind: WindowInteractionKind,
    source: WindowInteractionSource,
    trigger_button: Option<u32>,
    pointer_motion_surface_id: Option<u32>,
) -> WindowInteraction {
    WindowInteraction {
        id: WindowInteractionId::new(id),
        root_surface_id: 42,
        kind,
        source,
        trigger_button,
        trigger_serial: None,
        pointer_motion_surface_id,
        start_pointer_x: 100.0,
        start_pointer_y: 100.0,
        start_placement: SurfacePlacement::root_at(10, 20),
        start_width: 300,
        start_height: 200,
        drag_committed: false,
        resize_interaction_id: matches!(kind, WindowInteractionKind::Resize(_))
            .then_some(ResizeInteractionId::new(id)),
    }
}

fn test_begin_window_interaction(
    root_surface_id: u32,
    pointer_motion_surface_id: Option<u32>,
    kind: WindowInteractionKind,
    source: WindowInteractionSource,
) -> BeginWindowInteraction {
    BeginWindowInteraction {
        root_surface_id,
        x: 100.0,
        y: 100.0,
        kind,
        source,
        trigger_button: None,
        trigger_serial: None,
        pointer_motion_surface_id,
    }
}

fn test_renderable_surface(surface_id: u32, width: u32, height: u32) -> RenderableSurface {
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

#[test]
fn failed_begin_does_not_capture_native_input() {
    let mut state = CompositorState::default();

    assert!(!state.begin_window_resize_at_with_trigger(100.0, 100.0, 0x111));

    assert!(!state.window_interaction_active());
}

#[test]
fn root_native_interaction_construction_records_root_motion_target() {
    let interaction = test_window_interaction_with_target(
        1,
        WindowInteractionKind::Move,
        WindowInteractionSource::NativeBinding,
        None,
        Some(42),
    );

    assert_eq!(interaction.pointer_motion_surface_id, Some(42));
}

#[test]
fn subsurface_native_interaction_construction_records_exact_motion_target() {
    let interaction = test_window_interaction_with_target(
        1,
        WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
        WindowInteractionSource::NativeBinding,
        Some(0x111),
        Some(84),
    );

    assert_eq!(interaction.root_surface_id, 42);
    assert_eq!(interaction.pointer_motion_surface_id, Some(84));
}

#[test]
fn xdg_move_and_resize_construction_records_exact_press_target() {
    let move_interaction = test_window_interaction_with_target(
        1,
        WindowInteractionKind::Move,
        WindowInteractionSource::XdgToplevelMove,
        Some(0x111),
        Some(84),
    );
    let resize_interaction = test_window_interaction_with_target(
        2,
        WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
        WindowInteractionSource::XdgToplevelResize,
        Some(0x112),
        Some(85),
    );

    assert_eq!(move_interaction.pointer_motion_surface_id, Some(84));
    assert_eq!(resize_interaction.pointer_motion_surface_id, Some(85));
}

#[test]
fn server_decoration_construction_has_no_motion_target() {
    let begin = test_begin_window_interaction(
        42,
        None,
        WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
        WindowInteractionSource::NativeBinding,
    );

    assert_eq!(begin.pointer_motion_surface_id, None);
}

#[test]
fn failed_begin_with_invalid_motion_target_does_not_capture_interaction() {
    let mut state = CompositorState::default();

    assert!(
        !state.begin_window_interaction_for_root(test_begin_window_interaction(
            42,
            Some(84),
            WindowInteractionKind::Move,
            WindowInteractionSource::NativeBinding,
        ))
    );

    assert!(!state.window_interaction_active());
}

#[test]
fn failed_begin_with_missing_resource_does_not_mutate_resize_flow() {
    let mut state = CompositorState::default();
    state
        .renderable_surfaces
        .push(test_renderable_surface(42, 300, 200));
    let mut flow = ResizeConfigureFlow::default();
    flow.mark_sent(
        PendingResizeConfigure {
            surface_id: 42,
            width: 320,
            height: 220,
            placement: SurfacePlacement::root(),
            edges: ResizeEdges::BOTTOM_RIGHT,
            resizing: true,
            interaction_id: ResizeInteractionId::new(1),
        },
        10,
        1,
    );
    state.resize_configure_flows.insert(42, flow);

    assert!(!state.begin_window_resize_at_with_trigger(
        f64::from(render::FIRST_SURFACE_OFFSET.0) + 299.0,
        f64::from(render::FIRST_SURFACE_OFFSET.1) + 199.0,
        0x111,
    ));

    let flow = state.resize_configure_flows.get(&42).expect("resize flow");
    assert_eq!(flow.retained_configure_count(), 1);
    assert_eq!(state.resize_flow_metrics.resize_interactions_started, 0);
    assert!(!state.window_interaction_active());
}

#[test]
fn active_resize_rejects_begin_move() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
            Some(0x111),
        )),
        ..Default::default()
    };

    assert!(!state.begin_window_interaction_at(
        100.0,
        100.0,
        WindowInteractionKind::Move,
        WindowInteractionSource::NativeBinding,
        Some(0x110),
        None,
    ));

    assert!(matches!(
        state.window_interaction.map(|interaction| interaction.kind),
        Some(WindowInteractionKind::Resize(_))
    ));
}

#[test]
fn active_move_rejects_begin_resize() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Move,
            Some(0x110),
        )),
        ..Default::default()
    };

    assert!(!state.begin_window_interaction_at(
        100.0,
        100.0,
        WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
        WindowInteractionSource::NativeBinding,
        Some(0x111),
        None,
    ));

    assert_eq!(
        state.window_interaction.map(|interaction| interaction.kind),
        Some(WindowInteractionKind::Move)
    );
}

#[test]
fn stale_interaction_update_is_ignored() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            2,
            WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
            Some(0x111),
        )),
        ..Default::default()
    };

    assert!(!state.update_window_interaction_by_id(WindowInteractionId::new(1), 150.0, 150.0));

    assert!(state.pending_interactive_resize_update.is_none());
}

#[test]
fn stale_interaction_end_is_ignored() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            2,
            WindowInteractionKind::Move,
            Some(0x110),
        )),
        ..Default::default()
    };

    assert!(!state.end_window_interaction_by_id(WindowInteractionId::new(1)));

    assert_eq!(
        state.active_window_interaction_id(),
        Some(WindowInteractionId::new(2))
    );
}

#[test]
fn non_trigger_button_release_does_not_end_resize() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
            Some(0x111),
        )),
        ..Default::default()
    };

    assert!(!state.end_window_interaction_for_button(0x110));

    assert!(state.window_interaction_active());
}

#[test]
fn xdg_resize_trigger_button_release_ends_interaction() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
            Some(0x110),
        )),
        ..Default::default()
    };

    assert!(state.end_window_interaction_for_button(0x110));

    assert!(!state.window_interaction_active());
}

#[test]
fn destroyed_target_cancels_active_interaction() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Move,
            Some(0x110),
        )),
        interaction_cursor_override: Some(InteractionCursorOverride {
            shape: InteractionCursorShape::Move,
        }),
        ..Default::default()
    };

    state.clear_resize_state_for_surfaces(&[42]);

    assert!(!state.window_interaction_active());
    assert!(state.interaction_cursor_override.is_none());
}

#[test]
fn interaction_cursor_shape_maps_every_window_resize_edge() {
    assert_eq!(
        InteractionCursorShape::for_window_interaction(WindowInteractionKind::Move),
        InteractionCursorShape::Move
    );
    assert_eq!(
        InteractionCursorShape::for_window_interaction(WindowInteractionKind::Resize(
            ResizeEdges::new(false, false, true, false),
        )),
        InteractionCursorShape::ResizeHorizontal
    );
    assert_eq!(
        InteractionCursorShape::for_window_interaction(WindowInteractionKind::Resize(
            ResizeEdges::new(false, false, false, true),
        )),
        InteractionCursorShape::ResizeHorizontal
    );
    assert_eq!(
        InteractionCursorShape::for_window_interaction(WindowInteractionKind::Resize(
            ResizeEdges::new(true, false, false, false),
        )),
        InteractionCursorShape::ResizeVertical
    );
    assert_eq!(
        InteractionCursorShape::for_window_interaction(WindowInteractionKind::Resize(
            ResizeEdges::new(false, true, false, false),
        )),
        InteractionCursorShape::ResizeVertical
    );
    assert_eq!(
        InteractionCursorShape::for_window_interaction(WindowInteractionKind::Resize(
            ResizeEdges::new(true, false, true, false),
        )),
        InteractionCursorShape::ResizeDiagonalNwSe
    );
    assert_eq!(
        InteractionCursorShape::for_window_interaction(WindowInteractionKind::Resize(
            ResizeEdges::BOTTOM_RIGHT,
        )),
        InteractionCursorShape::ResizeDiagonalNwSe
    );
    assert_eq!(
        InteractionCursorShape::for_window_interaction(WindowInteractionKind::Resize(
            ResizeEdges::new(true, false, false, true),
        )),
        InteractionCursorShape::ResizeDiagonalNeSw
    );
    assert_eq!(
        InteractionCursorShape::for_window_interaction(WindowInteractionKind::Resize(
            ResizeEdges::new(false, true, true, false),
        )),
        InteractionCursorShape::ResizeDiagonalNeSw
    );
}

#[test]
fn failed_interaction_begin_does_not_activate_cursor_override() {
    let mut state = CompositorState::default();

    assert!(!state.begin_window_resize_at_with_trigger(100.0, 100.0, 0x111));

    assert!(state.interaction_cursor_override.is_none());
}

#[test]
fn locked_client_rejects_window_interaction_begin_without_cursor_override() {
    let mut state = CompositorState::default();
    state
        .pointer_constraint
        .activate(PointerConstraintMode::Locked, 42);

    assert!(!state.begin_window_interaction_at(
        100.0,
        100.0,
        WindowInteractionKind::Move,
        WindowInteractionSource::NativeBinding,
        Some(0x110),
        None,
    ));
    assert!(state.interaction_cursor_override.is_none());
}

#[test]
fn ending_window_interaction_clears_cursor_override_and_only_advances_cursor_generation() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Move,
            Some(0x110),
        )),
        interaction_cursor_override: Some(InteractionCursorOverride {
            shape: InteractionCursorShape::Move,
        }),
        ..Default::default()
    };
    let before_render_generation = state.render_generation;
    let before_scene_generation = state.scene_render_generation;

    assert!(state.end_window_interaction_for_button(0x110));

    assert!(state.interaction_cursor_override.is_none());
    assert!(state.render_generation > before_render_generation);
    assert_eq!(state.scene_render_generation, before_scene_generation);
    assert_eq!(
        state.render_generation_cause,
        RenderGenerationCause::CursorState
    );
}

#[test]
fn interaction_cursor_motion_advances_cursor_generation_only() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Move,
            Some(0x110),
        )),
        interaction_cursor_override: Some(InteractionCursorOverride {
            shape: InteractionCursorShape::Move,
        }),
        ..Default::default()
    };
    let before_render_generation = state.render_generation;
    let before_scene_generation = state.scene_render_generation;

    assert!(state.update_pointer_position_without_client_dispatch(150.0, 125.0));

    assert!(state.render_generation > before_render_generation);
    assert_eq!(state.scene_render_generation, before_scene_generation);
    assert_eq!(
        state.render_generation_cause,
        RenderGenerationCause::CursorMotion
    );
}

#[test]
fn fullscreen_transition_clears_interaction_cursor_override() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
            Some(0x110),
        )),
        interaction_cursor_override: Some(InteractionCursorOverride {
            shape: InteractionCursorShape::ResizeDiagonalNwSe,
        }),
        ..Default::default()
    };

    state.clear_resize_state_for_surfaces(&[42]);

    assert!(!state.window_interaction_active());
    assert!(state.interaction_cursor_override.is_none());
}

#[test]
fn maximize_transition_clears_interaction_cursor_override() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Resize(ResizeEdges::BOTTOM_RIGHT),
            Some(0x110),
        )),
        interaction_cursor_override: Some(InteractionCursorOverride {
            shape: InteractionCursorShape::ResizeDiagonalNwSe,
        }),
        ..Default::default()
    };

    state.clear_resize_state_for_surfaces(&[42]);

    assert!(!state.window_interaction_active());
    assert!(state.interaction_cursor_override.is_none());
}

#[test]
fn client_disconnect_cleanup_path_clears_interaction_cursor_override() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Move,
            Some(0x110),
        )),
        interaction_cursor_override: Some(InteractionCursorOverride {
            shape: InteractionCursorShape::Move,
        }),
        ..Default::default()
    };

    state.clear_resize_state_for_surfaces(&[42]);

    assert!(!state.window_interaction_active());
    assert!(state.interaction_cursor_override.is_none());
}

#[test]
fn explicit_interaction_cancel_clears_interaction_cursor_override() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Move,
            Some(0x110),
        )),
        interaction_cursor_override: Some(InteractionCursorOverride {
            shape: InteractionCursorShape::Move,
        }),
        ..Default::default()
    };

    assert!(state.clear_window_interaction_state());
    assert!(!state.window_interaction_active());
    assert!(state.interaction_cursor_override.is_none());
}

#[test]
fn focus_loss_clears_interaction_cursor_override() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Move,
            Some(0x110),
        )),
        interaction_cursor_override: Some(InteractionCursorOverride {
            shape: InteractionCursorShape::Move,
        }),
        ..Default::default()
    };

    state.clear_pointer_focus();

    assert!(!state.window_interaction_active());
    assert!(state.interaction_cursor_override.is_none());
}

#[test]
fn interaction_cancel_with_pointer_constraint_clears_override_without_clearing_confinement() {
    let mut state = CompositorState {
        window_interaction: Some(test_window_interaction(
            1,
            WindowInteractionKind::Move,
            Some(0x110),
        )),
        interaction_cursor_override: Some(InteractionCursorOverride {
            shape: InteractionCursorShape::Move,
        }),
        ..Default::default()
    };
    state
        .pointer_constraint
        .activate(PointerConstraintMode::Confined, 42);

    assert!(state.clear_window_interaction_state());

    assert!(state.interaction_cursor_override.is_none());
    assert_eq!(
        state.pointer_constraint.mode(),
        PointerConstraintMode::Confined
    );
}
