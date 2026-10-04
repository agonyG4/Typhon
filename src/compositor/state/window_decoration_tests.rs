use super::window_open_animation_tests::{
    set_window_open_preset, set_window_open_preset_with_maximized_policy,
};
use super::*;
use crate::animation_control::{
    AnimationEffect, AnimationPreset, AnimationRuntimeCapabilities, AnimationSlot,
};
use crate::compositor::decoration::{
    layout::DecorationLayout,
    render_plan::DecorationRenderPrimitive,
    types::{
        CapturedXdgDecorationCommitState, DecorationHit, DecorationMode, DecorationPreference,
    },
};
use crate::presentation_animation::PresentationOpacity;
use crate::render_backend::buffer::{BufferIdAllocator, BufferSize, CommittedSurfaceBuffer};
use crate::xwayland::xwm::{X11FrameExtents, X11MotifDecorationHint};
use crate::xwayland::{X11WindowHandle, XwaylandGeneration};
use std::{num::NonZeroU64, time::Instant};

const SURFACE_WIDTH: u32 = 300;
const SURFACE_HEIGHT: u32 = 200;
const SURFACE_PIXEL: u32 = 0xff12_3456;

fn test_surface(surface_id: u32) -> RenderableSurface {
    let identity = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer identity");
    RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width: SURFACE_WIDTH,
        height: SURFACE_HEIGHT,
        placement: SurfacePlacement::root(),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            identity,
            BufferSize::new(SURFACE_WIDTH, SURFACE_HEIGHT).expect("test size"),
            vec![SURFACE_PIXEL; (SURFACE_WIDTH * SURFACE_HEIGHT) as usize],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wl_output::Transform::Normal,
        damage: RenderableSurfaceDamage::Full,
    }
}

fn xdg_state(
    surface: RenderableSurface,
    preference: DecorationPreference,
    mode: ToplevelMode,
) -> CompositorState {
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("window id");
    let mut window = DesktopWindow::new_xdg(window_id, surface.surface_id);
    window.state.set_mode(mode);
    state
        .insert_desktop_window(window)
        .expect("insert XDG window");
    let mut decoration_state = WindowDecorationState::new();
    decoration_state.set_preference(preference);
    decoration_state
        .apply_configured_mode(preference.effective_mode(true, mode == ToplevelMode::Fullscreen));
    state
        .xdg_decoration_states
        .insert(surface.surface_id, decoration_state);
    state.append_renderable_surface(surface);
    state.rebuild_active_scene_view();
    state
}

fn zero_sized_xdg_mode_transition_state(surface_id: u32, configure_serial: u32) -> CompositorState {
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let target = SurfacePlacement::absolute_root_at(120, 90);
    state
        .xdg_surface_lifecycles
        .entry(surface_id)
        .or_default()
        .record_configure(configure_serial);
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
    );
    state.set_surface_placement(surface_id, target);
    state.install_xdg_mode_transition_visual_geometry(
        surface_id,
        WindowGeometry::new(target, 0, 0),
        VisualGeometryTransition::Immediate,
        Some(configure_serial),
    );
    state
}

fn install_test_toplevel_role(
    state: &mut CompositorState,
    surface_id: u32,
) -> (
    wayland_server::Display<CompositorState>,
    wayland_server::Client,
) {
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = std::os::unix::net::UnixStream::pair().expect("test socket");
    let client = display_handle
        .insert_client(server_end, std::sync::Arc::new(()))
        .expect("test client");
    let surface =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let xdg_surface = client
        .create_resource::<
            wayland_protocols::xdg::shell::server::xdg_surface::XdgSurface,
            XdgSurfaceData,
            CompositorState,
        >(
            &display_handle,
            20,
            XdgSurfaceData {
                surface: surface.clone(),
                reservation: XdgAssociationReservation::Fresh,
            },
        )
        .expect("test XDG surface");
    let toplevel = client
        .create_resource::<
            wayland_protocols::xdg::shell::server::xdg_toplevel::XdgToplevel,
            XdgToplevelData,
            CompositorState,
        >(
            &display_handle,
            21,
            XdgToplevelData { surface },
        )
        .expect("test XDG toplevel");
    let window_id = state
        .window_id_for_surface(surface_id)
        .expect("test window");
    state.toplevel_surfaces.insert(
        surface_id,
        ToplevelSurface {
            window_id,
            xdg_surface,
            toplevel,
            pending_constraints: None,
            wm_capabilities_sent: false,
        },
    );
    (display, client)
}

fn acknowledge_test_xdg_configure(state: &mut CompositorState, surface_id: u32, serial: u32) {
    let acknowledgement = state
        .acknowledge_xdg_configure(surface_id, serial)
        .expect("valid test configure ACK");
    state.ack_xdg_surface_configure(surface_id, acknowledgement);
}

fn assert_unpresented_xdg_mode_admission_uses_window_open(
    surface_id: u32,
    mode: ToplevelMode,
    animate_maximized_window_open: bool,
) {
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    set_window_open_preset_with_maximized_policy(
        &mut state,
        AnimationPreset::Astrea,
        animate_maximized_window_open,
    );
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
    );
    state.xdg_surface_lifecycles.entry(surface_id).or_default();
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    state.presentation_animator.set_enabled(true);
    let window_id = state
        .window_id_for_surface(surface_id)
        .expect("test window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    let target_geometry = state.window_geometry_for_surface_mode(surface_id, mode);
    let canonical_opacity = state
        .window(window_id)
        .expect("test window")
        .canonical_opacity();

    assert!(state.presented_window_geometry(surface_id).is_none());
    assert!(state.set_root_window_mode(surface_id, mode));
    assert_eq!(
        state.window(window_id).expect("test window").state.mode(),
        mode
    );
    let visual = state
        .toplevel_visual_geometries
        .get(&surface_id)
        .expect("XDG mode visual remains installed behind its response fence");
    assert!(visual.mode_transition);
    let fence = visual
        .xdg_mode_transition_fence
        .expect("mode configure response fence");
    assert_eq!(
        Some(fence.configure_serial),
        state
            .xdg_configure_serials
            .get(&surface_id)
            .map(|serials| serials.latest_sent)
    );
    assert_eq!(visual.window_geometry(), target_geometry);
    assert!(
        !state
            .presentation_animator
            .has_geometry_track(scene_node_id),
        "an unpresented XDG mode request must not install a mode geometry track"
    );

    let target_rect = state
        .presentation_rect_for_geometry(surface_id, target_geometry)
        .expect("final XDG mode target rect");
    let transaction_count_before_open = state.presentation_animator.transaction_count();
    let should_animate = mode != ToplevelMode::Maximized || animate_maximized_window_open;
    assert_eq!(
        state.maybe_begin_window_open_animation(surface_id),
        should_animate
    );
    assert_eq!(
        state
            .presentation_animator
            .has_geometry_track(scene_node_id),
        should_animate
    );
    assert!(!state.presentation_animator.has_opacity_track(scene_node_id));
    assert_eq!(
        state
            .presentation_animator
            .opacity_track_transaction(scene_node_id),
        None
    );
    assert_eq!(
        state.presentation_animator.transaction_count(),
        transaction_count_before_open + if should_animate { 1 } else { 0 }
    );
    assert_eq!(
        state.window(window_id).expect("test window").state.mode(),
        mode
    );
    assert_eq!(
        state
            .window(window_id)
            .expect("test window")
            .canonical_opacity(),
        canonical_opacity
    );
    if should_animate {
        assert_eq!(
            state
                .presentation_animator
                .sample_for_scene_node(scene_node_id, AnimationTime::from_nanos(u64::MAX))
                .expect("WindowOpen sample")
                .rect,
            target_rect
        );
    } else {
        assert_eq!(
            state.current_visual_root_window_geometry(surface_id),
            Some(target_geometry)
        );
        assert_eq!(
            state
                .presentation_animator
                .sample_for_scene_node(scene_node_id, AnimationTime::from_nanos(u64::MAX))
                .expect("canonical scene sample")
                .rect,
            target_rect
        );
    }
}

#[test]
fn unpresented_xdg_fullscreen_admission_uses_window_open_geometry() {
    assert_unpresented_xdg_mode_admission_uses_window_open(90, ToplevelMode::Fullscreen, true);
}

#[test]
fn unpresented_xdg_maximized_admission_uses_window_open_geometry() {
    assert_unpresented_xdg_mode_admission_uses_window_open(91, ToplevelMode::Maximized, true);
}

#[test]
fn disabled_maximized_window_open_policy_installs_no_presentation_transaction() {
    assert_unpresented_xdg_mode_admission_uses_window_open(94, ToplevelMode::Maximized, false);
}

#[test]
fn disabled_maximized_window_open_policy_keeps_fullscreen_admission() {
    assert_unpresented_xdg_mode_admission_uses_window_open(95, ToplevelMode::Fullscreen, false);
}

#[test]
fn unpresented_xdg_mode_change_retargets_active_window_open_geometry() {
    let surface_id = 92;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    set_window_open_preset_with_maximized_policy(&mut state, AnimationPreset::Astrea, false);
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
    );
    state.xdg_surface_lifecycles.entry(surface_id).or_default();
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    state.presentation_animator.set_enabled(true);
    let window_id = state
        .window_id_for_surface(surface_id)
        .expect("test window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    let target_geometry =
        state.window_geometry_for_surface_mode(surface_id, ToplevelMode::Fullscreen);

    assert!(state.maybe_begin_window_open_animation(surface_id));
    let open_transaction = state
        .presentation_animator
        .track_transaction(scene_node_id)
        .expect("initial WindowOpen geometry transaction");
    assert_eq!(
        state
            .presentation_animator
            .opacity_track_transaction(scene_node_id),
        None
    );
    assert!(state.presented_window_geometry(surface_id).is_none());

    assert!(state.set_root_window_mode(surface_id, ToplevelMode::Fullscreen));

    assert!(
        state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert!(!state.presentation_animator.has_opacity_track(scene_node_id));
    let retargeted_transaction = state
        .presentation_animator
        .track_transaction(scene_node_id)
        .expect("retargeted WindowOpen geometry transaction");
    assert_ne!(retargeted_transaction, open_transaction);
    assert_eq!(
        state
            .presentation_animator
            .opacity_track_transaction(scene_node_id),
        None
    );
    assert_eq!(
        state
            .presentation_animator
            .sample_for_scene_node(scene_node_id, AnimationTime::from_nanos(u64::MAX))
            .expect("retargeted WindowOpen sample")
            .rect,
        state
            .presentation_rect_for_geometry(surface_id, target_geometry)
            .expect("fullscreen target rect")
    );
}

#[test]
fn unpresented_xdg_normal_to_maximized_cancels_window_open_without_restarting_it() {
    let surface_id = 96;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    set_window_open_preset_with_maximized_policy(&mut state, AnimationPreset::Astrea, false);
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
    );
    state.xdg_surface_lifecycles.entry(surface_id).or_default();
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    state.presentation_animator.set_enabled(true);
    let window_id = state
        .window_id_for_surface(surface_id)
        .expect("test window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    let canonical_opacity = state
        .window(window_id)
        .expect("test window")
        .canonical_opacity();

    assert!(state.maybe_begin_window_open_animation(surface_id));
    let _first_open_transaction = state
        .presentation_animator
        .track_transaction(scene_node_id)
        .expect("initial WindowOpen geometry transaction");
    assert!(state.window_open_geometry_track_active(surface_id));
    let transaction_count = state.presentation_animator.transaction_count();
    assert!(state.presented_window_geometry(surface_id).is_none());

    let maximized_geometry =
        state.window_geometry_for_surface_mode(surface_id, ToplevelMode::Maximized);
    assert!(state.set_root_window_mode(surface_id, ToplevelMode::Maximized));

    assert_eq!(
        state.window(window_id).expect("test window").state.mode(),
        ToplevelMode::Maximized
    );
    assert!(!state.window_open_geometry_track_active(surface_id));
    assert!(
        !state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert!(!state.presentation_animator.has_opacity_track(scene_node_id));
    assert_eq!(
        state.presentation_animator.track_transaction(scene_node_id),
        None
    );
    assert_eq!(
        state.presentation_animator.transaction_count(),
        transaction_count
    );
    assert_eq!(
        state.current_visual_root_window_geometry(surface_id),
        Some(maximized_geometry)
    );
    assert_eq!(
        state
            .window(window_id)
            .expect("test window")
            .canonical_opacity(),
        canonical_opacity
    );
    assert!(!state.maybe_begin_window_open_animation(surface_id));
    assert_eq!(
        state.presentation_animator.transaction_count(),
        transaction_count
    );
    assert_eq!(
        state.presentation_animator.track_transaction(scene_node_id),
        None
    );
}

#[test]
fn unpresented_xdg_fullscreen_then_normal_is_an_admission_correction() {
    let surface_id = 93;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
    );
    state.xdg_surface_lifecycles.entry(surface_id).or_default();
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    state.presentation_animator.set_enabled(true);
    let window_id = state
        .window_id_for_surface(surface_id)
        .expect("test window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    let normal_geometry = state
        .current_visual_root_window_geometry(surface_id)
        .expect("initial normal geometry");

    assert!(state.set_root_window_mode(surface_id, ToplevelMode::Fullscreen));
    assert_eq!(
        state.window(window_id).expect("test window").state.mode(),
        ToplevelMode::Fullscreen
    );
    assert!(
        !state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert!(state.restore_normal_root_window(surface_id));
    assert_eq!(
        state.window(window_id).expect("test window").state.mode(),
        ToplevelMode::Normal
    );
    assert!(
        !state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert_eq!(
        state.current_visual_root_window_geometry(surface_id),
        Some(normal_geometry)
    );
}

#[test]
fn unpresented_xdg_maximized_then_fullscreen_opens_on_fullscreen_geometry() {
    let surface_id = 94;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    set_window_open_preset(&mut state, AnimationPreset::Astrea);
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
    );
    state.xdg_surface_lifecycles.entry(surface_id).or_default();
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    state.presentation_animator.set_enabled(true);
    let window_id = state
        .window_id_for_surface(surface_id)
        .expect("test window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");

    assert!(state.set_root_window_mode(surface_id, ToplevelMode::Maximized));
    assert!(
        !state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert!(state.set_root_window_mode(surface_id, ToplevelMode::Fullscreen));
    assert_eq!(
        state.window(window_id).expect("test window").state.mode(),
        ToplevelMode::Fullscreen
    );
    assert!(
        !state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    let final_geometry =
        state.window_geometry_for_surface_mode(surface_id, ToplevelMode::Fullscreen);
    assert_eq!(
        state.current_visual_root_window_geometry(surface_id),
        Some(final_geometry)
    );
    let target_rect = state
        .presentation_rect_for_geometry(surface_id, final_geometry)
        .expect("fullscreen target rect");

    assert!(state.maybe_begin_window_open_animation(surface_id));
    assert!(
        state
            .presentation_animator
            .has_geometry_track(scene_node_id)
    );
    assert!(!state.presentation_animator.has_opacity_track(scene_node_id));
    assert_eq!(
        state
            .presentation_animator
            .opacity_track_transaction(scene_node_id),
        None
    );
    assert_eq!(
        state
            .presentation_animator
            .sample_for_scene_node(scene_node_id, AnimationTime::from_nanos(u64::MAX))
            .expect("WindowOpen sample")
            .rect,
        target_rect
    );
}

#[test]
fn physically_presented_xdg_mode_changes_keep_enter_and_exit_animations() {
    for (surface_id, mode, enter, exit) in [
        (
            95,
            ToplevelMode::Fullscreen,
            PresentationAnimationKind::FullscreenEnter,
            PresentationAnimationKind::FullscreenExit,
        ),
        (
            96,
            ToplevelMode::Maximized,
            PresentationAnimationKind::MaximizeEnter,
            PresentationAnimationKind::MaximizeExit,
        ),
    ] {
        let mut state = xdg_state(
            test_surface(surface_id),
            DecorationPreference::ServerSide,
            ToplevelMode::Normal,
        );
        state.set_test_effective_xdg_window_geometry(
            surface_id,
            XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
        );
        state.xdg_surface_lifecycles.entry(surface_id).or_default();
        let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
        state.presentation_animator.set_enabled(true);
        let window_id = state
            .window_id_for_surface(surface_id)
            .expect("test window");
        let scene_node_id = state
            .scene_node_id_for_window_group(window_id)
            .expect("window group node");
        let presented_rect = state
            .current_presentation_rect_for_root(surface_id)
            .expect("normal presentation rect");
        state.publish_presented_window_geometry(
            1,
            PresentedWindowGeometry::new(surface_id, presented_rect),
        );
        assert!(state.set_root_window_mode(surface_id, mode));
        assert_eq!(
            state.presentation_animator.track_curve(scene_node_id),
            state.animation_control.curve_for(enter)
        );
        assert!(state.restore_normal_root_window(surface_id));
        assert_eq!(
            state.presentation_animator.track_curve(scene_node_id),
            state.animation_control.curve_for(exit)
        );
    }
}

fn x11_state(surface: RenderableSurface) -> CompositorState {
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("window id");
    let mut window = DesktopWindow::new_xdg(window_id, surface.surface_id);
    window.backend = WindowBackend::X11(X11WindowHandle::new(
        XwaylandGeneration::new(NonZeroU64::new(1).expect("generation")),
        0x100,
    ));
    state
        .insert_desktop_window(window)
        .expect("insert XWayland window");
    state.append_renderable_surface(surface);
    state.rebuild_active_scene_view();
    state
}

fn x11_test_handle() -> X11WindowHandle {
    X11WindowHandle::new(
        XwaylandGeneration::new(NonZeroU64::new(1).expect("generation")),
        0x100,
    )
}

fn decoration_instances(state: &CompositorState) -> Vec<DecorationRenderInstance> {
    state.native_decoration_render_instances(&state.renderable_surfaces)
}

#[test]
fn x11_decoration_mode_precedence_controls_rendering_hit_testing_and_extents() {
    let mut state = x11_state(test_surface(50));
    let handle = x11_test_handle();
    let window_id = state.window_id_for_x11_handle(handle).expect("X11 window");

    assert_eq!(
        state.x11_effective_decoration_mode(handle),
        DecorationMode::ServerSide
    );
    assert_eq!(state.x11_decoration_frame_extents(handle), [0, 0, 26, 0]);
    let instances = decoration_instances(&state);
    assert_eq!(instances.len(), 1);
    assert_eq!(
        instances[0].scene_node_id(),
        state
            .scene_node_id_for_server_decoration(window_id)
            .expect("server decoration scene node")
    );
    assert!(matches!(
        state.decoration_hit_for_root_at(50, (0, 0), 100.0, -10.0),
        Some(DecorationHit::Titlebar)
    ));

    state
        .window_mut(window_id)
        .expect("X11 window")
        .x11_decoration_hints
        .gtk_frame_extents = Some(X11FrameExtents {
        left: 1,
        right: 0,
        top: 0,
        bottom: 0,
    });
    assert_eq!(
        state.x11_effective_decoration_mode(handle),
        DecorationMode::ClientSide
    );
    assert_eq!(state.x11_decoration_frame_extents(handle), [0; 4]);
    assert!(decoration_instances(&state).is_empty());
    assert!(
        state
            .decoration_hit_for_root_at(50, (0, 0), 100.0, -10.0)
            .is_none()
    );

    state
        .window_mut(window_id)
        .expect("X11 window")
        .x11_decoration_hints
        .motif = X11MotifDecorationHint::Undecorated;
    assert_eq!(
        state.x11_effective_decoration_mode(handle),
        DecorationMode::ClientSide
    );

    state
        .window_mut(window_id)
        .expect("X11 window")
        .x11_decoration_hints
        .gtk_frame_extents = None;
    assert_eq!(
        state.x11_effective_decoration_mode(handle),
        DecorationMode::None
    );

    state
        .window_mut(window_id)
        .expect("X11 window")
        .x11_decoration_hints
        .motif = X11MotifDecorationHint::Decorated;
    assert_eq!(
        state.x11_effective_decoration_mode(handle),
        DecorationMode::ServerSide
    );
    state
        .window_mut(window_id)
        .expect("X11 window")
        .state
        .set_mode(ToplevelMode::Fullscreen);
    assert_eq!(
        state.x11_effective_decoration_mode(handle),
        DecorationMode::None
    );
}

#[test]
fn server_decoration_scene_node_survives_visibility_mode_transitions() {
    let mut state = x11_state(test_surface(55));
    let handle = x11_test_handle();
    let window_id = state.window_id_for_x11_handle(handle).expect("X11 window");
    let node = state
        .scene_node_id_for_server_decoration(window_id)
        .expect("server decoration scene node");

    assert_eq!(
        state.scene_node_id_for_server_decoration(window_id),
        Some(node)
    );

    state
        .window_mut(window_id)
        .expect("X11 window")
        .x11_decoration_hints
        .gtk_frame_extents = Some(X11FrameExtents {
        left: 1,
        right: 0,
        top: 0,
        bottom: 0,
    });
    assert!(decoration_instances(&state).is_empty());

    state
        .window_mut(window_id)
        .expect("X11 window")
        .state
        .set_mode(ToplevelMode::Fullscreen);
    assert!(decoration_instances(&state).is_empty());

    state
        .window_mut(window_id)
        .expect("X11 window")
        .state
        .set_mode(ToplevelMode::Normal);
    state
        .window_mut(window_id)
        .expect("X11 window")
        .x11_decoration_hints
        .gtk_frame_extents = None;
    assert_eq!(
        state.scene_node_id_for_server_decoration(window_id),
        Some(node)
    );
}

#[test]
fn x11_decoration_transition_clears_stale_native_interaction_state() {
    let mut state = x11_state(test_surface(51));
    let handle = x11_test_handle();
    let window_id = state.window_id_for_x11_handle(handle).expect("X11 window");

    state.decoration_button_capture = Some(DecorationButtonCapture {
        window_id,
        root_surface_id: 51,
        kind: crate::compositor::decoration::types::DecorationButtonKind::Close,
        button: 0x110,
    });
    state.decoration_button_hover = Some((
        window_id,
        crate::compositor::decoration::types::DecorationButtonKind::Close,
    ));
    state.decoration_titlebar_click_capture = Some((window_id, 0x110));
    state.decoration_last_titlebar_click = Some((window_id, Instant::now(), 10.0, 10.0));
    state.window_interaction = Some(WindowInteraction {
        id: WindowInteractionId::new(1),
        window_id,
        root_surface_id: 51,
        kind: WindowInteractionKind::Move,
        source: WindowInteractionSource::NativeBinding,
        trigger_button: Some(0x110),
        trigger_serial: None,
        pointer_motion_surface_id: None,
        start_pointer_x: 10.0,
        start_pointer_y: 10.0,
        start_placement: SurfacePlacement::root(),
        start_width: 300,
        start_height: 200,
        drag_committed: false,
        first_move_geometry_logged: false,
        resize_interaction_id: None,
        tiled_resize: false,
        decoration_owned: true,
    });
    assert!(state.window_interaction_debug_snapshot().is_some());

    state.reconcile_x11_decoration_transition(
        handle,
        DecorationMode::ServerSide,
        DecorationMode::ClientSide,
    );

    assert!(state.decoration_button_capture.is_none());
    assert!(state.decoration_button_hover.is_none());
    assert!(state.decoration_titlebar_click_capture.is_none());
    assert!(state.decoration_last_titlebar_click.is_none());
    assert!(state.window_interaction_debug_snapshot().is_none());
}

#[test]
fn x11_decoration_transition_preserves_native_client_content_interaction() {
    let mut state = x11_state(test_surface(52));
    let handle = x11_test_handle();
    let window_id = state.window_id_for_x11_handle(handle).expect("X11 window");
    state.window_interaction = Some(WindowInteraction {
        id: WindowInteractionId::new(2),
        window_id,
        root_surface_id: 52,
        kind: WindowInteractionKind::Move,
        source: WindowInteractionSource::NativeBinding,
        trigger_button: Some(0x110),
        trigger_serial: None,
        pointer_motion_surface_id: Some(52),
        start_pointer_x: 10.0,
        start_pointer_y: 10.0,
        start_placement: SurfacePlacement::root(),
        start_width: 300,
        start_height: 200,
        drag_committed: false,
        first_move_geometry_logged: false,
        resize_interaction_id: None,
        tiled_resize: false,
        decoration_owned: false,
    });

    state.reconcile_x11_decoration_transition(
        handle,
        DecorationMode::ServerSide,
        DecorationMode::ClientSide,
    );

    assert!(state.window_interaction_debug_snapshot().is_some());
}

#[test]
fn renderable_surface_order_follows_authoritative_window_stacking() {
    let mut state = CompositorState::new(None);
    let first_surface = test_surface(41);
    let second_surface = test_surface(42);
    let first_id = state.allocate_window_id().expect("first window id");
    let second_id = state.allocate_window_id().expect("second window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(first_id, first_surface.surface_id))
        .expect("first window");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(second_id, second_surface.surface_id))
        .expect("second window");
    state.append_renderable_surface(first_surface);
    state.append_renderable_surface(second_surface);
    state.window_stacking = vec![second_id, first_id];

    assert!(state.normalize_window_stacking());
    assert_eq!(
        state
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .collect::<Vec<_>>(),
        vec![42, 41]
    );
}

#[test]
fn pointer_scene_hit_returns_top_window_decoration_before_lower_client() {
    let mut state = CompositorState::new(None);
    let mut rear = test_surface(41);
    rear.placement = SurfacePlacement::absolute_root_at(100, 70);
    let mut front = test_surface(42);
    front.placement = SurfacePlacement::absolute_root_at(100, 100);
    let rear_id = state.allocate_window_id().expect("rear window id");
    let front_id = state.allocate_window_id().expect("front window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(rear_id, rear.surface_id))
        .expect("rear window");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(front_id, front.surface_id))
        .expect("front window");
    let mut rear_decoration = WindowDecorationState::new();
    rear_decoration.set_preference(DecorationPreference::ServerSide);
    rear_decoration.apply_configured_mode(DecorationMode::ServerSide);
    state
        .xdg_decoration_states
        .insert(rear.surface_id, rear_decoration);
    let mut front_decoration = WindowDecorationState::new();
    front_decoration.set_preference(DecorationPreference::ServerSide);
    front_decoration.apply_configured_mode(DecorationMode::ServerSide);
    state
        .xdg_decoration_states
        .insert(front.surface_id, front_decoration);
    state.append_renderable_surface(rear);
    state.append_renderable_surface(front);
    state.window_stacking = vec![rear_id, front_id];
    state.rebuild_active_scene_view();

    let origins = render::surface_origins(&state.renderable_surfaces);
    let front_origin = origins[1];
    let hit = state.pointer_scene_hit_at(
        f64::from(front_origin.0 + 80),
        f64::from(front_origin.1 - 13),
    );
    assert!(matches!(
        hit,
        PointerSceneHit::Decoration {
            window_id,
            root_surface_id: 42,
            hit: DecorationHit::Titlebar,
        } if window_id == front_id
    ));

    let resize_hit =
        state.pointer_scene_hit_at(f64::from(front_origin.0 - 2), f64::from(front_origin.1 - 2));
    assert!(matches!(
        resize_hit,
        PointerSceneHit::Decoration {
            window_id,
            root_surface_id: 42,
            hit: DecorationHit::Resize(_),
        } if window_id == front_id
    ));
}

#[test]
fn pointer_scene_hit_keeps_ssd_above_an_ordinary_subsurface() {
    let mut state = xdg_state(
        test_surface(42),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let mut child = test_surface(43);
    child.placement = SurfacePlacement::subsurface(42, 10, -20);
    state.append_renderable_surface(child);
    state.rebuild_active_scene_view();

    let root_origin = render::surface_origins(&state.renderable_surfaces)[0];
    let point = (f64::from(root_origin.0 + 20), f64::from(root_origin.1 - 13));
    for _ in 0..1_000 {
        let hit = state.pointer_scene_hit_at(point.0, point.1);
        assert!(matches!(
            hit,
            PointerSceneHit::Decoration {
                root_surface_id: 42,
                hit: DecorationHit::Titlebar,
                ..
            }
        ));
    }
    assert_eq!(state.visual_stack_groups_cache.len(), 1);
    assert_eq!(
        state.visual_stack_groups_cache[0].surface_indices(),
        &[0, 1]
    );
}

#[test]
fn pointer_scene_hit_cache_requires_current_pointer_hit_generation() {
    let mut state = CompositorState::new(None);
    state.scene_render_generation = 7;
    state.pointer_hit_generation = 10;
    let _ = state.pointer_scene_hit_at(40.0, 30.0);
    state.pointer_hit_generation = 11;

    let hit = state.pointer_scene_hit_at(40.0, 30.0);

    assert!(matches!(hit, PointerSceneHit::None));
    assert_eq!(
        state
            .pointer_scene_hit_cache
            .as_ref()
            .expect("hit-test must refresh the stale cache")
            .pointer_hit_generation(),
        11
    );
}

#[test]
fn pointer_scene_hit_metrics_cover_repeated_positions_without_hot_path_clones() {
    let mut state = xdg_state(
        test_surface(42),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    state.pointer_hit_instrumentation_enabled = true;
    let client = (100.0, 100.0);
    let titlebar = (100.0, -13.0);
    let button = (145.0, -13.0);

    let _ = state.pointer_scene_hit_at(client.0, client.1);
    let _ = state.pointer_scene_hit_at(client.0, client.1);
    state.advance_pointer_hit_generation();
    let _ = state.pointer_scene_hit_at(client.0, client.1);
    for _ in 0..2_500 {
        for (x, y) in [client, titlebar, button, client] {
            let _ = state.pointer_scene_hit_at(x, y);
        }
    }

    let metrics = state.pointer_hit_metrics;
    assert_eq!(metrics.pointer_scene_hit_calls, 10_003);
    assert!(metrics.pointer_scene_hit_cache_hits >= 1);
    assert!(metrics.pointer_scene_hit_cache_misses >= 7_500);
    assert_eq!(
        metrics.full_scene_hit_scans,
        metrics.pointer_scene_hit_cache_misses
    );
    assert_eq!(metrics.owner_locality_fast_hits, 0);
    assert!(metrics.pointer_scene_hit_groups_inspected > 0);
    assert!(metrics.pointer_scene_hit_surfaces_inspected > 0);
    assert_eq!(metrics.pointer_scene_hit_origin_cache_clones, 0);
    assert_eq!(metrics.pointer_scene_hit_root_linear_searches, 0);
    assert!(metrics.pointer_scene_hit_cpu_nanos > 0);
    assert!(state.pointer_scene_hit_cache.is_some());
}

#[test]
fn pointer_scene_hit_owner_locality_reuses_scene_owner_for_nearby_decoration_points() {
    let mut state = xdg_state(
        test_surface(42),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    state.pointer_hit_instrumentation_enabled = true;
    let origin = render::surface_origins(&state.renderable_surfaces)[0];

    let first = state.pointer_scene_hit_at(f64::from(origin.0 + 100), f64::from(origin.1 - 13));
    let second = state.pointer_scene_hit_at(
        f64::from(origin.0 + 101) + 0.25,
        f64::from(origin.1 - 13) + 0.5,
    );

    let PointerSceneHit::Decoration { hit: first_hit, .. } = first else {
        panic!("expected decoration hit for first point");
    };
    let PointerSceneHit::Decoration {
        hit: second_hit, ..
    } = second
    else {
        panic!("expected decoration hit for second point");
    };
    assert_eq!(first_hit, DecorationHit::Titlebar);
    assert_eq!(second_hit, DecorationHit::Titlebar);
    assert_eq!(state.pointer_hit_metrics.full_scene_hit_scans, 1);
    assert_eq!(state.pointer_hit_metrics.owner_locality_fast_hits, 1);
    assert_eq!(state.pointer_hit_metrics.active_scene_index_hits, 1);
}

#[test]
fn content_only_pointer_hit_does_not_refresh_global_origin_cache() {
    let mut state = xdg_state(
        test_surface(42),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    state.refresh_surface_origin_cache();
    let cached_generation = state.surface_origin_cache_generation;
    let origin = state.active_scene_surface_origins()[0];

    let _ = state.pointer_scene_hit_at(f64::from(origin.0 + 40), f64::from(origin.1 - 13));
    state.render_generation = state.render_generation.saturating_add(1);
    let hit = state.pointer_scene_hit_at(
        f64::from(origin.0 + 41) + 0.25,
        f64::from(origin.1 - 13) + 0.5,
    );

    assert!(matches!(hit, PointerSceneHit::Decoration { .. }));
    assert_eq!(state.surface_origin_cache_generation, cached_generation);
    assert_eq!(state.pointer_hit_metrics.global_origin_cache_recomputes, 1);
    let PointerSceneHit::Decoration { hit, .. } = hit else {
        panic!("content-only pointer motion should retain the same owner");
    };
    assert_eq!(hit, DecorationHit::Titlebar);
}

#[test]
fn pointer_scene_hit_cache_does_not_survive_destroyed_window() {
    let mut state = xdg_state(
        test_surface(42),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let window_id = state
        .desktop_windows
        .keys()
        .next()
        .copied()
        .expect("test window");
    let x = 40.0;
    let y = -13.0;
    state.pointer_scene_hit_cache = Some(PointerSceneHitCache::new_for_test(
        x,
        y,
        state.scene_render_generation,
        state.pointer_hit_generation,
        PointerSceneHit::Decoration {
            window_id,
            root_surface_id: 42,
            hit: DecorationHit::Titlebar,
        },
    ));
    assert!(matches!(
        state.pointer_scene_hit_at(x, y),
        PointerSceneHit::Decoration { window_id: id, .. } if id == window_id
    ));

    state.remove_desktop_window(window_id);
    state.retain_renderable_surfaces(|_| false);
    state.invalidate_surface_origin_cache();

    assert!(matches!(
        state.pointer_scene_hit_at(x, y),
        PointerSceneHit::None
    ));
}

fn rgba_to_pixel(color: [u8; 4]) -> u32 {
    (u32::from(color[3]) << 24)
        | (u32::from(color[0]) << 16)
        | (u32::from(color[1]) << 8)
        | u32::from(color[2])
}

#[test]
fn ssd_render_uses_resolved_cascaded_root_origin_for_titlebar_and_content() {
    let state = xdg_state(
        test_surface(41),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let surfaces = state.renderable_surfaces.clone();
    let instances = decoration_instances(&state);
    let origins = render::surface_origins(&surfaces);
    let instance = instances.first().expect("SSD instance");

    assert_eq!(origins, vec![render::FIRST_SURFACE_OFFSET]);
    assert_eq!(instance.origin(), (72, 46));

    let titlebar_color = match instance.primitives().first().expect("titlebar primitive") {
        DecorationRenderPrimitive::SolidRect { color, .. } => *color,
        _ => panic!("titlebar must be a solid primitive"),
    };
    let mut renderer = render::DesktopSceneRenderer::default();
    renderer.set_decoration_instances(&instances);
    let mut frame = vec![0; 400 * 350];
    renderer.compose_request(DesktopComposeRequest {
        frame: &mut frame,
        frame_width: 400,
        frame_height: 350,
        output_scale: 1.0,
        surfaces: &surfaces,
        external_overlay_surface_ids: Vec::new(),
        content_generation: 1,
        visual_state: DesktopVisualState::wallpaper_only(),
        client_cursor: None,
    });

    let titlebar_x = instance.origin().0 + 5;
    let titlebar_y = instance.origin().1 + 5;
    assert_eq!(
        frame[titlebar_y as usize * 400 + titlebar_x as usize],
        rgba_to_pixel(titlebar_color)
    );
    assert_eq!(
        frame[origins[0].1 as usize * 400 + origins[0].0 as usize],
        SURFACE_PIXEL
    );
    assert_ne!(
        frame[5 * 400 + 5],
        rgba_to_pixel(titlebar_color),
        "the titlebar must not be painted at output origin"
    );
}

#[test]
fn ssd_render_follows_absolute_move_and_active_render_placement() {
    let mut surface = test_surface(42);
    surface.placement = SurfacePlacement::absolute_root_at(200, 140);
    let mut state = xdg_state(
        surface,
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );

    assert_eq!(decoration_instances(&state)[0].origin(), (200, 114));

    state.renderable_surfaces[0].placement = SurfacePlacement::root_at(40, 50);
    state.renderable_surfaces[0].render_placement = None;
    assert_eq!(decoration_instances(&state)[0].origin(), (112, 96));

    state.renderable_surfaces[0].render_placement =
        Some(SurfacePlacement::absolute_root_at(300, 220));
    assert_eq!(decoration_instances(&state)[0].origin(), (300, 194));
}

#[test]
fn xwayland_ssd_render_follows_actual_frame_content_placement() {
    let mut surface = test_surface(43);
    surface.render_backend = SurfaceRenderBackend::Xwayland;
    surface.placement = SurfacePlacement::absolute_root_at(320, 180);
    let state = x11_state(surface);

    assert_eq!(decoration_instances(&state)[0].origin(), (320, 154));
}

#[test]
fn rendered_ssd_button_centers_hit_the_same_buttons() {
    let mut state = xdg_state(
        test_surface(44),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let surfaces = state.renderable_surfaces.clone();
    let instances = decoration_instances(&state);
    let instance = instances.first().expect("SSD instance");
    let layout = DecorationLayout::for_window(
        SURFACE_WIDTH,
        SURFACE_HEIGHT,
        DecorationMode::ServerSide,
        false,
        false,
        state.decoration_theme.metrics(),
    )
    .expect("SSD layout");
    let origin = instance.origin();

    assert_eq!(render::surface_origins(&surfaces), vec![(72, 72)]);
    for button in layout.buttons {
        let x = origin.0 + button.visual.x + button.visual.width as i32 / 2;
        let y = origin.1 + button.visual.y + button.visual.height as i32 / 2;
        assert_eq!(
            state.decoration_hit_at(f64::from(x), f64::from(y)),
            Some((
                state.window_id_for_surface(44).expect("window id"),
                44,
                DecorationHit::Button(button.kind),
            ))
        );
    }
}

#[test]
fn ssd_layout_follows_resize_preview_without_client_commit() {
    let mut surface = test_surface(50);
    surface.width = 1800;
    surface.height = 1000;
    let mut state = xdg_state(
        surface,
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let surface_id = 50;
    let preview_width = 1400;
    let preview_height = 900;
    assert!(state.preview_resize_root_window_to(
        surface_id,
        preview_width,
        preview_height,
        SurfacePlacement::root(),
        ResizeEdges::new(false, false, true, false),
        ResizeInteractionId::new(1),
    ));

    let origins = render::surface_origins(&state.renderable_surfaces);
    let root_origin = origins[0];
    let instances =
        state.native_decoration_render_instances_for_scale(&state.renderable_surfaces, 1.0);
    let instance = instances.first().expect("SSD instance");
    let (_, _, rendered_width, rendered_height) = instance.scene_snapshot().bounds();
    assert_eq!(
        (rendered_width, rendered_height),
        (preview_width, preview_height + 26),
        "SSD outer bounds must use the active visual preview, not committed size"
    );

    let metrics = state.decoration_theme.metrics();
    let preview_layout = DecorationLayout::for_window(
        preview_width,
        preview_height,
        DecorationMode::ServerSide,
        false,
        false,
        metrics,
    )
    .expect("preview layout");
    let close = preview_layout.buttons.last().expect("close button");
    let preview_instance_origin = (
        root_origin.0 - preview_layout.client.x,
        root_origin.1 - preview_layout.client.y,
    );
    let close_x = preview_instance_origin.0 + close.visual.x + close.visual.width as i32 / 2;
    let close_y = preview_instance_origin.1 + close.visual.y + close.visual.height as i32 / 2;
    assert_eq!(
        instance.origin().0 + close.visual.right(),
        root_origin.0 + preview_width as i32 - metrics.right_padding as i32,
        "button cluster must follow the current visual right edge"
    );
    assert_eq!(
        state.decoration_hit_at(f64::from(close_x), f64::from(close_y)),
        Some((
            state.window_id_for_surface(surface_id).expect("window id"),
            surface_id,
            DecorationHit::Button(close.kind),
        )),
        "visible preview button must remain hit-testable at its preview position"
    );
}

#[test]
fn mode_transition_visual_geometry_stays_coherent_until_client_commit() {
    let surface_id = 51;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let floating = WindowGeometry::new(SurfacePlacement::absolute_root_at(120, 90), 900, 700);
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(
            floating.placement.local_x,
            floating.placement.local_y,
            floating.width as i32,
            floating.height as i32,
        ),
    );
    state.set_surface_placement(surface_id, floating.placement);

    let fullscreen_target = WindowGeometry::new(SurfacePlacement::root(), 1920, 1080);
    state.install_toplevel_visual_geometry(surface_id, fullscreen_target);

    assert_eq!(
        state.current_visual_root_window_geometry(surface_id),
        Some(fullscreen_target),
        "the unresolved configure must not expose the old floating visual box"
    );
    assert_eq!(
        state.current_root_window_geometry(surface_id),
        Some(floating),
        "the committed client geometry remains floating while the configure is delayed"
    );
    let instance = state
        .native_decoration_render_instances_for_scale(&state.renderable_surfaces, 1.0)
        .first()
        .cloned()
        .expect("fullscreen-target SSD instance");
    assert_eq!(instance.scene_snapshot().bounds().2, 1920);

    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(
            fullscreen_target.placement.local_x,
            fullscreen_target.placement.local_y,
            fullscreen_target.width as i32,
            fullscreen_target.height as i32,
        ),
    );
    state.set_surface_placement(surface_id, fullscreen_target.placement);
    state.update_toplevel_visual_render_assignment(surface_id);

    assert_eq!(
        state.current_visual_root_window_geometry(surface_id),
        Some(fullscreen_target),
        "the converged client geometry remains the same visual box"
    );
    assert!(
        !state.toplevel_visual_geometries.contains_key(&surface_id),
        "the temporary mode geometry retires after the client commits it"
    );
}

#[test]
fn zero_sized_xdg_mode_visual_survives_animation_endpoint_without_client_response() {
    let surface_id = 53;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let source = SurfacePlacement::absolute_root_at(160, 130);
    let target = SurfacePlacement::absolute_root_at(120, 90);
    state
        .set_test_effective_xdg_window_geometry(surface_id, XdgWindowGeometry::new(0, 0, 900, 700));
    state.set_surface_placement(surface_id, target);
    state.rebuild_active_scene_view();
    state.install_xdg_mode_transition_visual_geometry(
        surface_id,
        WindowGeometry::new(source, 0, 0),
        VisualGeometryTransition::Immediate,
        Some(77),
    );

    let animation_start =
        crate::presentation_animation::PresentationRect::new(160.0, 130.0, 900.0, 700.0)
            .expect("animation source");
    let animation_target =
        crate::presentation_animation::PresentationRect::new(120.0, 90.0, 900.0, 700.0)
            .expect("animation target");
    state.presentation_animator.set_enabled(true);
    state.start_test_presentation_transition(
        surface_id,
        animation_start,
        animation_target,
        crate::presentation_animation::AnimationTime::from_nanos(0),
    );
    let scene_node_id = state
        .presentation_scene_node_id_for_root(surface_id)
        .expect("mode visual scene node");
    let settled = state
        .presentation_animator
        .sample_for_scene_node(
            scene_node_id,
            crate::presentation_animation::AnimationTime::from_nanos(1_000_000),
        )
        .expect("presentation transition should reach its target");
    assert!(settled.mathematically_settled);
    assert_eq!(settled.rect, animation_target);

    state
        .rebase_interaction_to_presented_origin(
            surface_id,
            SurfacePlacement::absolute_root_at(settled.rect.x() as i32, settled.rect.y() as i32),
            RenderGenerationCause::WindowMode,
        )
        .expect("presentation target should update the visual placement");

    let visual = state
        .toplevel_visual_geometries
        .get(&surface_id)
        .expect("animation target must not retire the mode visual");
    assert!(visual.mode_transition);
    assert!(visual.active_resize.is_none());
    assert_eq!(visual.placement, target);
    assert!(visual.width == 0 || visual.height == 0);
    assert_eq!(
        visual
            .xdg_mode_transition_fence
            .and_then(|fence| fence.ack_commit_sequence_floor),
        None,
        "presentation convergence does not create an XDG ACK-to-commit fence"
    );
}

#[test]
fn xdg_mode_transition_fence_requires_ack_then_a_later_root_commit() {
    let surface_id = 54;
    let configure_serial = 101;
    let mut state = zero_sized_xdg_mode_transition_state(surface_id, configure_serial);
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    let pre_ack_commit = state.allocate_surface_commit_sequence();

    acknowledge_test_xdg_configure(&mut state, surface_id, configure_serial);
    let ack_floor = state
        .toplevel_visual_geometries
        .get(&surface_id)
        .and_then(|visual| visual.xdg_mode_transition_fence)
        .and_then(|fence| fence.ack_commit_sequence_floor)
        .expect("the exact transition ACK establishes its commit boundary");
    assert!(pre_ack_commit.get() <= ack_floor.get());

    state.update_toplevel_visual_render_assignment(surface_id);
    assert!(state.toplevel_visual_geometries.contains_key(&surface_id));

    state.update_toplevel_visual_render_assignment_after_root_commit(surface_id, pre_ack_commit);
    assert!(
        state.toplevel_visual_geometries.contains_key(&surface_id),
        "a root commit captured before ACK cannot satisfy the fence when published afterward"
    );

    let response_commit = state.allocate_surface_commit_sequence();
    state.update_toplevel_visual_render_assignment_after_root_commit(surface_id, response_commit);
    assert!(
        !state.toplevel_visual_geometries.contains_key(&surface_id),
        "a root commit received after the transition ACK can retire converged geometry"
    );
}

#[test]
fn xdg_mode_transition_without_an_emitted_configure_uses_generic_convergence() {
    let surface_id = 57;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let target = SurfacePlacement::absolute_root_at(120, 90);
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
    );
    state.set_surface_placement(surface_id, target);

    state.install_xdg_mode_transition_visual_geometry(
        surface_id,
        WindowGeometry::new(target, 0, 0),
        VisualGeometryTransition::Immediate,
        None,
    );

    assert!(
        !state.toplevel_visual_geometries.contains_key(&surface_id),
        "without an emitted configure, the temporary visual uses generic convergence"
    );
}

#[test]
fn newer_xdg_ack_consumes_transition_configure_for_mode_visual_fence() {
    let surface_id = 55;
    let transition_serial = 201;
    let newer_serial = 202;
    let mut state = zero_sized_xdg_mode_transition_state(surface_id, transition_serial);
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    state
        .xdg_surface_lifecycles
        .get_mut(&surface_id)
        .expect("XDG lifecycle")
        .record_configure(newer_serial);

    acknowledge_test_xdg_configure(&mut state, surface_id, newer_serial);
    let ack_floor = state
        .toplevel_visual_geometries
        .get(&surface_id)
        .and_then(|visual| visual.xdg_mode_transition_fence)
        .and_then(|fence| fence.ack_commit_sequence_floor);
    assert!(
        ack_floor.is_some(),
        "ACK of the newer outstanding configure consumes the transition configure"
    );
    state.update_toplevel_visual_render_assignment(surface_id);
    assert!(
        state.toplevel_visual_geometries.contains_key(&surface_id),
        "ACK alone does not retire the temporary visual"
    );

    let response_commit = state.allocate_surface_commit_sequence();
    state.update_toplevel_visual_render_assignment_after_root_commit(surface_id, response_commit);
    assert!(
        !state.toplevel_visual_geometries.contains_key(&surface_id),
        "the root commit after ACK of the newer configure retires the transition visual"
    );
}

#[test]
fn older_xdg_ack_cannot_satisfy_newer_mode_transition_visual() {
    let surface_id = 58;
    let older_serial = 401;
    let transition_serial = 402;
    let mut state = zero_sized_xdg_mode_transition_state(surface_id, transition_serial);
    let lifecycle = state
        .xdg_surface_lifecycles
        .get_mut(&surface_id)
        .expect("XDG lifecycle");
    lifecycle.configures.clear();
    lifecycle.record_configure(older_serial);
    lifecycle.record_configure(transition_serial);
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);

    acknowledge_test_xdg_configure(&mut state, surface_id, older_serial);
    let old_ack_commit = state.allocate_surface_commit_sequence();
    state.update_toplevel_visual_render_assignment_after_root_commit(surface_id, old_ack_commit);
    assert!(
        state.toplevel_visual_geometries.contains_key(&surface_id),
        "an ACK that consumes only an older configure cannot satisfy this transition"
    );
    assert_eq!(
        state
            .toplevel_visual_geometries
            .get(&surface_id)
            .and_then(|visual| visual.xdg_mode_transition_fence)
            .and_then(|fence| fence.ack_commit_sequence_floor),
        None
    );

    acknowledge_test_xdg_configure(&mut state, surface_id, transition_serial);
    let transition_response_commit = state.allocate_surface_commit_sequence();
    state.update_toplevel_visual_render_assignment_after_root_commit(
        surface_id,
        transition_response_commit,
    );
    assert!(
        !state.toplevel_visual_geometries.contains_key(&surface_id),
        "the transition configure's own later ACK and commit retire its visual"
    );
}

#[test]
fn later_xdg_ack_does_not_strand_transition_after_exact_ack() {
    let surface_id = 56;
    let transition_serial = 301;
    let newer_serial = 302;
    let mut state = zero_sized_xdg_mode_transition_state(surface_id, transition_serial);
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    acknowledge_test_xdg_configure(&mut state, surface_id, transition_serial);
    let first_ack_floor = state
        .toplevel_visual_geometries
        .get(&surface_id)
        .and_then(|visual| visual.xdg_mode_transition_fence)
        .and_then(|fence| fence.ack_commit_sequence_floor)
        .expect("exact transition ACK establishes the first boundary");

    state
        .xdg_surface_lifecycles
        .get_mut(&surface_id)
        .expect("XDG lifecycle")
        .record_configure(newer_serial);
    acknowledge_test_xdg_configure(&mut state, surface_id, newer_serial);
    let fence_floor_after_newer_ack = state
        .toplevel_visual_geometries
        .get(&surface_id)
        .and_then(|visual| visual.xdg_mode_transition_fence)
        .and_then(|fence| fence.ack_commit_sequence_floor);
    assert_eq!(fence_floor_after_newer_ack, Some(first_ack_floor));

    state.update_toplevel_visual_render_assignment(surface_id);
    assert!(state.toplevel_visual_geometries.contains_key(&surface_id));
    let response_commit = state.allocate_surface_commit_sequence();
    state.update_toplevel_visual_render_assignment_after_root_commit(surface_id, response_commit);
    assert!(
        !state.toplevel_visual_geometries.contains_key(&surface_id),
        "a newer accepted ACK cannot strand an already acknowledged transition configure"
    );
}

#[test]
fn repeated_mode_transition_visual_geometry_never_mixes_committed_sizes() {
    let surface_id = 52;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );

    for cycle in 0..100 {
        let floating = WindowGeometry::new(
            SurfacePlacement::absolute_root_at(120 + cycle % 3, 90 + cycle % 2),
            900,
            700,
        );
        let fullscreen = WindowGeometry::new(SurfacePlacement::root(), 1920, 1080);

        state.install_toplevel_visual_geometry(surface_id, fullscreen);
        assert_eq!(
            state.current_visual_root_window_geometry(surface_id),
            Some(fullscreen),
            "cycle {cycle}: entering fullscreen must use one target geometry"
        );
        state.set_test_effective_xdg_window_geometry(
            surface_id,
            XdgWindowGeometry::new(
                fullscreen.placement.local_x,
                fullscreen.placement.local_y,
                fullscreen.width as i32,
                fullscreen.height as i32,
            ),
        );
        state.set_surface_placement(surface_id, fullscreen.placement);
        state.update_toplevel_visual_render_assignment(surface_id);
        assert!(
            !state.toplevel_visual_geometries.contains_key(&surface_id),
            "cycle {cycle}: fullscreen override must retire after the matching commit"
        );

        state.install_toplevel_visual_geometry(surface_id, floating);
        assert_eq!(
            state.current_visual_root_window_geometry(surface_id),
            Some(floating),
            "cycle {cycle}: restoring must use the saved floating target"
        );
        state.set_test_effective_xdg_window_geometry(
            surface_id,
            XdgWindowGeometry::new(
                floating.placement.local_x,
                floating.placement.local_y,
                floating.width as i32,
                floating.height as i32,
            ),
        );
        state.set_surface_placement(surface_id, floating.placement);
        state.update_toplevel_visual_render_assignment(surface_id);
        assert!(
            !state.toplevel_visual_geometries.contains_key(&surface_id),
            "cycle {cycle}: floating override must retire after the matching commit"
        );
    }
}

#[test]
fn ssd_left_edge_preview_keeps_visual_geometry_for_render_and_hit_test() {
    let mut surface = test_surface(51);
    surface.width = 1800;
    surface.height = 1000;
    let mut state = xdg_state(
        surface,
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let preview_width = 1500;
    let preview_height = 920;
    let preview_placement = SurfacePlacement::root_at(120, 72);
    assert!(state.preview_resize_root_window_to(
        51,
        preview_width,
        preview_height,
        preview_placement,
        ResizeEdges::new(true, false, false, false),
        ResizeInteractionId::new(2),
    ));
    assert_eq!(state.renderable_surfaces[0].width, 1800);

    let instance = state
        .native_decoration_render_instances_for_scale(&state.renderable_surfaces, 1.0)
        .first()
        .cloned()
        .expect("SSD instance");
    assert_eq!(instance.scene_snapshot().bounds().2, preview_width);
    let layout = DecorationLayout::for_window(
        preview_width,
        preview_height,
        DecorationMode::ServerSide,
        false,
        false,
        state.decoration_theme.metrics(),
    )
    .expect("preview layout");
    let close = layout.buttons.last().expect("close button");
    let close_x = instance.origin().0 + close.visual.x + close.visual.width as i32 / 2;
    let close_y = instance.origin().1 + close.visual.y + close.visual.height as i32 / 2;
    assert_eq!(
        state.decoration_hit_at(f64::from(close_x), f64::from(close_y)),
        Some((
            state.window_id_for_surface(51).expect("window id"),
            51,
            DecorationHit::Button(close.kind),
        ))
    );
}

#[test]
fn csd_and_fullscreen_have_no_server_decoration_instance() {
    let csd = xdg_state(
        test_surface(45),
        DecorationPreference::ClientSide,
        ToplevelMode::Normal,
    );
    assert!(decoration_instances(&csd).is_empty());

    let fullscreen = xdg_state(
        test_surface(46),
        DecorationPreference::ServerSide,
        ToplevelMode::Fullscreen,
    );
    assert!(decoration_instances(&fullscreen).is_empty());
}

#[test]
fn uncommitted_preference_does_not_render_or_invalidate_ssd() {
    let surface_id = 52;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ClientSide,
        ToplevelMode::Normal,
    );
    let generation_before = state.scene_render_generation;

    state
        .xdg_decoration_states
        .get_mut(&surface_id)
        .expect("test decoration state")
        .set_preference(DecorationPreference::ServerSide);

    assert!(decoration_instances(&state).is_empty());
    assert_eq!(state.scene_render_generation, generation_before);
}

#[test]
fn v2_recreation_before_surface_commit_retains_previous_preference() {
    let mut state = WindowDecorationState::new();
    state.set_preference(DecorationPreference::ServerSide);
    let first_generation = state.current_generation().expect("first generation");
    assert!(state.destroy_object(first_generation));
    let second_generation = state.recreate_object();
    assert_ne!(first_generation, second_generation);
    assert_eq!(state.requested_mode(false), DecorationMode::ServerSide);
}

#[test]
fn v2_recreation_after_surface_commit_keeps_normal_unset_configure_policy() {
    let mut state = WindowDecorationState::new();
    state.set_preference(DecorationPreference::ServerSide);
    assert!(state.apply_configured_mode(DecorationMode::ServerSide));
    let first_generation = state.current_generation().expect("first generation");
    assert!(state.destroy_object(first_generation));
    let (captured, stale) = state.capture_surface_commit_decoration(None);
    assert_eq!(stale, None);
    assert_eq!(
        captured,
        Some(CapturedXdgDecorationCommitState::DecorationDestroyed {
            generation: first_generation,
        })
    );
    assert_eq!(state.applied_mode(), DecorationMode::ServerSide);
    state.recreate_object();
    assert!(!state.set_preference(DecorationPreference::Unset));
    assert_eq!(state.requested_mode(false), DecorationMode::ServerSide);
}

#[test]
fn v2_destroy_surface_commit_disables_rendering_and_hit_testing() {
    let surface_id = 53;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let generation_before = state.scene_render_generation;
    assert_eq!(
        state
            .xdg_decoration_states
            .get(&surface_id)
            .expect("test decoration state")
            .applied_mode(),
        DecorationMode::ServerSide
    );
    assert_eq!(decoration_instances(&state).len(), 1);
    assert!(matches!(
        state.decoration_hit_for_root_at(surface_id, (0, 0), 100.0, -10.0),
        Some(DecorationHit::Titlebar)
    ));

    let generation = state
        .xdg_decoration_states
        .get(&surface_id)
        .and_then(|decoration_state| decoration_state.current_generation())
        .expect("test decoration generation");
    assert!(
        state
            .xdg_decoration_states
            .get_mut(&surface_id)
            .expect("test decoration state")
            .destroy_object(generation)
    );
    assert_eq!(state.scene_render_generation, generation_before);
    let commit_sequence = SurfaceCommitSequence::initial();
    let captured = state.capture_xdg_decoration_commit_state(surface_id, commit_sequence);
    assert!(state.apply_captured_xdg_decoration(surface_id, commit_sequence, captured));
    assert_eq!(
        state
            .xdg_decoration_states
            .get(&surface_id)
            .expect("test decoration state")
            .applied_mode(),
        DecorationMode::ClientSide
    );
    assert!(decoration_instances(&state).is_empty());
    assert!(
        state
            .decoration_hit_for_root_at(surface_id, (0, 0), 100.0, -10.0)
            .is_none()
    );
    let captured = state.capture_xdg_decoration_commit_state(surface_id, commit_sequence);
    assert!(!state.apply_captured_xdg_decoration(surface_id, commit_sequence, captured));
    assert_eq!(
        state
            .xdg_decoration_states
            .get(&surface_id)
            .expect("test decoration state")
            .applied_mode(),
        DecorationMode::ClientSide
    );
}

#[test]
fn v2_mapped_creation_has_client_side_baseline_and_unset_preference() {
    let mut state = WindowDecorationState::new();

    assert_eq!(state.applied_mode(), DecorationMode::ClientSide);
    assert!(state.current_generation().is_some());
    assert!(!state.set_preference(DecorationPreference::Unset));
    assert_eq!(state.requested_mode(false), DecorationMode::ServerSide);
}

#[test]
fn maximized_ssd_outer_frame_matches_usable_output_across_repeated_cycles() {
    let mut state = xdg_state(
        test_surface(47),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    state.set_output_size(1280, 800);
    let surface_id = 47;
    let usable = state.usable_output_geometry();
    let metrics = state.decoration_theme.metrics();

    for _ in 0..100 {
        let geometry = state.window_geometry_for_surface_mode(surface_id, ToplevelMode::Maximized);
        assert_eq!(geometry.placement.local_x, usable.x as i32);
        assert_eq!(geometry.placement.local_y, usable.y as i32 + 26);
        assert_eq!(geometry.width, usable.width as u32);
        assert_eq!(
            geometry.height,
            usable.height as u32 - metrics.titlebar_height
        );

        let layout = DecorationLayout::for_window(
            geometry.width,
            geometry.height,
            DecorationMode::ServerSide,
            true,
            false,
            metrics,
        )
        .expect("maximized SSD layout");
        assert_eq!(layout.outer.width, usable.width as u32);
        assert_eq!(layout.outer.height, usable.height as u32);
        assert_eq!(layout.extents.top, metrics.titlebar_height);
    }
}

#[test]
fn titlebar_double_click_requires_spatial_proximity() {
    const LEFT_BUTTON: u32 = 0x110;
    let mut state = xdg_state(
        test_surface(48),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let window_id = state.window_id_for_surface(48).expect("window id");
    let instance = decoration_instances(&state).pop().expect("SSD instance");
    let x = f64::from(instance.origin().0 + 100);
    let y = f64::from(instance.origin().1 + 10);
    state.last_pointer_x = x;
    state.last_pointer_y = y;

    state.decoration_last_titlebar_click = Some((window_id, Instant::now(), x + 100.0, y));
    assert!(state.handle_decoration_button(LEFT_BUTTON, true));
    assert!(state.decoration_titlebar_click_capture.is_none());

    state.decoration_last_titlebar_click = Some((window_id, Instant::now(), x + 2.0, y + 2.0));
    assert!(state.handle_decoration_button(LEFT_BUTTON, true));
    assert_eq!(
        state.decoration_titlebar_click_capture,
        Some((window_id, LEFT_BUTTON))
    );
}

#[test]
fn ssd_controls_and_titles_use_fractional_scale_rasters() {
    let state = xdg_state(
        test_surface(49),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let instances =
            state.native_decoration_render_instances_for_scale(&state.renderable_surfaces, scale);
        let instance = instances.first().expect("SSD instance");
        let image = instance
            .primitives()
            .iter()
            .find_map(|primitive| match primitive {
                DecorationRenderPrimitive::Image { asset, .. } => Some(asset),
                _ => None,
            })
            .expect("rasterized control");
        let expected = (16.0 * scale).ceil() as u32;
        assert_eq!((image.width(), image.height()), (expected, expected));
        assert!(
            instance
                .primitives()
                .iter()
                .any(|primitive| matches!(primitive, DecorationRenderPrimitive::Text { .. }))
        );
    }
}

#[test]
fn window_scale_above_maximized_xdg_window_starts_at_canonical_opacity() {
    const MAXIMIZED_ROOT: u32 = 95;
    const FLOATING_ROOT: u32 = 96;

    let mut state = xdg_state(
        test_surface(MAXIMIZED_ROOT),
        DecorationPreference::ServerSide,
        ToplevelMode::Maximized,
    );
    set_window_open_preset(&mut state, AnimationPreset::Astrea);
    state.presentation_animator.set_enabled(true);

    let maximized_window_id = state
        .window_id_for_surface(MAXIMIZED_ROOT)
        .expect("maximized XDG window");
    let maximized_geometry =
        WindowGeometry::new(SurfacePlacement::absolute_root_at(0, 0), 1_920, 929);
    state.install_toplevel_visual_geometry(MAXIMIZED_ROOT, maximized_geometry);
    state
        .surface_presentation_generations
        .insert(MAXIMIZED_ROOT, 1);

    let floating_window_id = state.allocate_window_id().expect("floating window ID");
    let mut floating_window = DesktopWindow::new_xdg(floating_window_id, FLOATING_ROOT);
    floating_window.state.set_mode(ToplevelMode::Normal);
    state
        .insert_desktop_window(floating_window)
        .expect("floating XDG window");
    state.append_renderable_surface(test_surface(FLOATING_ROOT));
    let (_maximized_display, _maximized_client) =
        install_test_toplevel_role(&mut state, MAXIMIZED_ROOT);
    let (_floating_display, _floating_client) =
        install_test_toplevel_role(&mut state, FLOATING_ROOT);

    let floating_geometry =
        WindowGeometry::new(SurfacePlacement::absolute_root_at(320, 180), 944, 501);
    state.install_toplevel_visual_geometry(FLOATING_ROOT, floating_geometry);
    state
        .surface_presentation_generations
        .insert(FLOATING_ROOT, 1);
    state.rebuild_active_scene_view();
    state
        .ensure_native_output_id()
        .expect("test output identity");

    assert_eq!(
        state.animation_control.effective_effect(
            AnimationSlot::WindowOpen,
            AnimationRuntimeCapabilities::default(),
        ),
        AnimationEffect::WindowScale
    );

    let maximized_scene_node = state
        .scene_node_id_for_window_group(maximized_window_id)
        .expect("maximized scene node");
    let floating_scene_node = state
        .scene_node_id_for_window_group(floating_window_id)
        .expect("floating scene node");
    let maximized_geometry_before = state
        .current_visual_root_window_geometry(MAXIMIZED_ROOT)
        .expect("maximized geometry before WindowOpen");
    let floating_target = state
        .presentation_rect_for_geometry(FLOATING_ROOT, floating_geometry)
        .expect("floating presentation target");
    let floating_start = PresentationRect::new(
        floating_target.x() + floating_target.width() * 0.03,
        floating_target.y() + floating_target.height() * 0.03,
        floating_target.width() * 0.94,
        floating_target.height() * 0.94,
    )
    .expect("centered WindowScale start rectangle");
    let assert_rect_near = |actual: PresentationRect, expected: PresentationRect| {
        const EPSILON: f64 = 1.0e-9;
        assert!(
            (actual.x() - expected.x()).abs() <= EPSILON
                && (actual.y() - expected.y()).abs() <= EPSILON
                && (actual.width() - expected.width()).abs() <= EPSILON
                && (actual.height() - expected.height()).abs() <= EPSILON,
            "presentation rectangle {actual:?} differs from {expected:?}"
        );
    };

    assert!(state.maybe_begin_window_open_animation(FLOATING_ROOT));

    let window_open_started_at = state
        .presentation_animator
        .track_started_at_for_scene_node(floating_scene_node)
        .expect("WindowScale Geometry start time");
    let first_geometry = state
        .presentation_animator
        .sample_at_transition_start_for_scene_node(floating_scene_node)
        .expect("first WindowScale geometry sample");
    let first_presentation = state.presentation_scene_sample_at(window_open_started_at);
    assert_rect_near(first_geometry.rect, floating_start);
    assert_rect_near(
        first_presentation
            .transform_for_scene_node(floating_scene_node)
            .expect("first WindowScale presentation transform")
            .presented_rect,
        floating_start,
    );
    assert_eq!(
        first_presentation.opacity_for_scene_node(floating_scene_node),
        PresentationOpacity::OPAQUE
    );
    assert_eq!(
        first_presentation.opacity_for_scene_node(maximized_scene_node),
        PresentationOpacity::OPAQUE
    );
    assert!(
        !state
            .presentation_animator
            .has_opacity_track(floating_scene_node),
        "WindowScale must not install an opacity transition"
    );
    assert!(
        state
            .presentation_animator
            .sample_opacity_for_scene_node(floating_scene_node, AnimationTime::from_nanos(0),)
            .is_none()
    );
    assert_eq!(
        state
            .current_visual_root_window_geometry(FLOATING_ROOT)
            .expect("floating canonical geometry"),
        floating_geometry
    );
    assert_eq!(
        state
            .window(floating_window_id)
            .expect("floating window")
            .state
            .mode(),
        ToplevelMode::Normal
    );
    assert_eq!(
        state.current_visual_root_window_geometry(MAXIMIZED_ROOT),
        Some(maximized_geometry_before)
    );
    assert_eq!(
        state
            .window(maximized_window_id)
            .expect("maximized window")
            .state
            .mode(),
        ToplevelMode::Maximized
    );
    assert!(!state.presentation_animator.has_track(maximized_scene_node));
    let final_presentation =
        state.presentation_scene_sample_at(AnimationTime::from_nanos(u64::MAX));
    assert_rect_near(
        final_presentation
            .transform_for_scene_node(floating_scene_node)
            .expect("final WindowScale presentation transform")
            .presented_rect,
        floating_target,
    );
    assert_eq!(
        final_presentation.opacity_for_scene_node(floating_scene_node),
        PresentationOpacity::OPAQUE
    );
}
