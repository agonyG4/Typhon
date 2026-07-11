use super::*;

fn test_window_interaction(
    id: u64,
    kind: WindowInteractionKind,
    trigger_button: Option<u32>,
) -> WindowInteraction {
    WindowInteraction {
        id: WindowInteractionId::new(id),
        root_surface_id: 42,
        kind,
        source: WindowInteractionSource::NativeBinding,
        trigger_button,
        trigger_serial: None,
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
        ..Default::default()
    };

    state.clear_resize_state_for_surfaces(&[42]);

    assert!(!state.window_interaction_active());
}
