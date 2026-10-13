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
use crate::wm::WindowDecorationPolicy;
use crate::xwayland::xwm::{
    X11DecorationHints, X11FrameExtents, X11Geometry, X11MotifDecorationHint, X11PublishedState,
    X11WindowSnapshot, X11WindowTypes,
};
use crate::xwayland::{X11WindowHandle, XwaylandGeneration};
use std::{borrow::Cow, num::NonZeroU64, time::Instant};

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

fn test_surface_with_size(surface_id: u32, width: u32, height: u32) -> RenderableSurface {
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
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            identity,
            BufferSize::new(width, height).expect("test size"),
            vec![SURFACE_PIXEL; (width * height) as usize],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wl_output::Transform::Normal,
        damage: RenderableSurfaceDamage::Full,
    }
}

fn test_x11_snapshot_with_size(surface_id: u32, width: u32, height: u32) -> X11WindowSnapshot {
    X11WindowSnapshot {
        handle: x11_test_handle(),
        surface_id,
        kind: DesktopWindowKind::Managed,
        window_types: X11WindowTypes::default(),
        decoration_hints: X11DecorationHints::default(),
        override_redirect: false,
        geometry: X11Geometry {
            x: 0,
            y: 0,
            width,
            height,
        },
        metadata: WindowMetadata::default(),
        constraints: WindowConstraints::default(),
        state: X11PublishedState::default(),
        transient_for: None,
        supports_delete: true,
        supports_take_focus: true,
        accepts_input: Some(true),
        window_role: None,
        startup_id: None,
        user_time: None,
        urgency: false,
        supports_sync_request: false,
        sync_counter: None,
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

struct PointerTestWaylandClient {
    _display: wayland_server::Display<CompositorState>,
    client: wayland_server::Client,
    display_handle: wayland_server::DisplayHandle,
}

fn pointer_test_wayland_client() -> PointerTestWaylandClient {
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = std::os::unix::net::UnixStream::pair().expect("test socket");
    let client = display_handle
        .insert_client(server_end, std::sync::Arc::new(()))
        .expect("test client");
    PointerTestWaylandClient {
        _display: display,
        client,
        display_handle,
    }
}

fn append_pointer_test_toplevel(
    state: &mut CompositorState,
    client: &PointerTestWaylandClient,
    placement: SurfacePlacement,
    visual_clip: Option<crate::compositor::render::SurfaceVisualAperture>,
) -> (u32, WindowId) {
    let resource = state.test_create_unmapped_surface_resource_at_version(
        &client.client,
        &client.display_handle,
        1,
    );
    let surface_id = compositor_surface_id(&resource);
    state
        .assign_surface_role(surface_id, SurfaceRole::XdgToplevel)
        .expect("client surface role");
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
        .expect("client window");
    state.xdg_decoration_states.insert(
        surface_id,
        WindowDecorationState::with_published_mode(DecorationMode::ClientSide),
    );
    let mut surface = test_surface(surface_id);
    surface.placement = placement;
    surface.visual_clip = visual_clip;
    state.append_renderable_surface(surface);
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
    );
    (surface_id, window_id)
}

fn prime_pointer_test_certificate(state: &mut CompositorState, surface_id: u32) -> (f64, f64) {
    state.rebuild_active_scene_view();
    state.pointer_hit_instrumentation_enabled = true;
    let surface_index = state
        .active_scene_surface_index(surface_id)
        .expect("active pointer test surface");
    let target = state.active_scene_surface_render_assignments()[surface_index].target;
    let point = (
        f64::from(target.x() + target.width() as i32 / 2),
        f64::from(target.y() + target.height() as i32 / 2),
    );
    assert!(matches!(
        state.pointer_scene_hit_at(point.0, point.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == surface_id
    ));
    assert_eq!(state.pointer_hit_metrics.pointer_hit_certificate_builds, 1);
    assert!(
        state
            .pointer_scene_hit_cache
            .as_ref()
            .and_then(PointerSceneHitCache::pointer_hit_certificate_safe_rect)
            .is_some()
    );
    point
}

fn assert_pointer_certificate_revalidated_after_mutation(
    state: &mut CompositorState,
    surface_id: u32,
    previous_generation: u64,
    expected_surface_id: Option<u32>,
) {
    assert_ne!(state.pointer_hit_generation, previous_generation);
    if let Some(cached_certificate_generation) = state
        .pointer_scene_hit_cache
        .as_ref()
        .and_then(PointerSceneHitCache::pointer_hit_certificate_generation)
    {
        assert!(
            cached_certificate_generation == previous_generation
                || cached_certificate_generation == state.pointer_hit_generation,
            "a cached certificate must be old or already rebuilt for the current generation"
        );
    }
    let surface_index = state
        .active_scene_surface_index(surface_id)
        .expect("mutated pointer test surface remains active");
    let target = state.active_scene_surface_render_assignments()[surface_index].target;
    let point = (
        f64::from(target.x() + target.width() as i32 / 3),
        f64::from(target.y() + target.height() as i32 / 3),
    );
    let reference = state.pointer_scene_hit_reference_at(point.0, point.1);
    let hit = state.pointer_scene_hit_at(point.0, point.1);
    assert_pointer_scene_hits_equal(&hit, &reference);
    match expected_surface_id {
        Some(expected_surface_id) => assert!(matches!(
            hit,
            PointerSceneHit::Client { ref target }
                if compositor_surface_id(&target.surface) == expected_surface_id
        )),
        None => assert!(matches!(hit, PointerSceneHit::None)),
    }
    let refreshed_cache = state
        .pointer_scene_hit_cache
        .as_ref()
        .expect("optimized hit test refreshes the exact cache");
    assert_eq!(
        refreshed_cache.pointer_hit_generation(),
        state.pointer_hit_generation
    );
    if let Some(certificate_generation) = refreshed_cache.pointer_hit_certificate_generation() {
        assert_eq!(certificate_generation, state.pointer_hit_generation);
    }
}

fn append_pointer_test_subsurface(
    state: &mut CompositorState,
    client: &PointerTestWaylandClient,
    parent_surface_id: u32,
    x: i32,
    y: i32,
) -> u32 {
    let resource = state.test_create_unmapped_surface_resource_at_version(
        &client.client,
        &client.display_handle,
        1,
    );
    let surface_id = compositor_surface_id(&resource);
    state
        .assign_surface_role(
            surface_id,
            SurfaceRole::Subsurface {
                parent_id: parent_surface_id,
            },
        )
        .expect("subsurface role");
    let mut surface = test_surface(surface_id);
    surface.placement = SurfacePlacement::subsurface(parent_surface_id, x, y);
    state.store_surface_placement(surface_id, surface.placement);
    state.append_renderable_surface(surface);
    surface_id
}

fn append_pointer_test_popup(
    state: &mut CompositorState,
    client: &PointerTestWaylandClient,
    parent_surface_id: u32,
    placement: SurfacePlacement,
) -> u32 {
    use wayland_protocols::xdg::shell::server::{xdg_popup, xdg_surface};

    let resource = state.test_create_unmapped_surface_resource_at_version(
        &client.client,
        &client.display_handle,
        1,
    );
    let surface_id = compositor_surface_id(&resource);
    state
        .assign_surface_role(surface_id, SurfaceRole::XdgPopup)
        .expect("popup surface role");
    let xdg_surface = client
        .client
        .create_resource::<xdg_surface::XdgSurface, XdgSurfaceData, CompositorState>(
            &client.display_handle,
            20,
            XdgSurfaceData {
                surface: resource.clone(),
                reservation: XdgAssociationReservation::Fresh,
            },
        )
        .expect("popup XDG surface");
    let popup = client
        .client
        .create_resource::<xdg_popup::XdgPopup, XdgPopupData, CompositorState>(
            &client.display_handle,
            21,
            XdgPopupData {
                surface: resource.clone(),
            },
        )
        .expect("XDG popup resource");
    let parent = state
        .surface_resource_by_id(parent_surface_id)
        .expect("popup parent surface");
    state.register_popup_surface(
        resource,
        Some(parent),
        xdg_surface,
        popup,
        XdgPositionerState::default(),
    );
    let mut surface = test_surface(surface_id);
    let parent_placement = state.surface_placement(parent_surface_id);
    surface.placement = SurfacePlacement::subsurface(
        parent_surface_id,
        placement.local_x.saturating_sub(parent_placement.local_x),
        placement.local_y.saturating_sub(parent_placement.local_y),
    );
    state.store_surface_placement(surface_id, surface.placement);
    state.append_renderable_surface(surface);
    let node = state
        .popup_nodes
        .get_mut(&surface_id)
        .expect("registered popup node");
    node.mapped = true;
    node.configured = true;
    state.refresh_active_scene_popup_view();
    surface_id
}

fn set_test_committed_input_region(
    state: &CompositorState,
    surface_id: u32,
    region: crate::compositor::state_data::SurfaceInputRegion,
) {
    let surface = state
        .surface_resource_by_id(surface_id)
        .expect("test surface resource");
    surface
        .data::<crate::compositor::state_data::SurfaceData>()
        .expect("test surface data")
        .apply_input_region_change(Some(region));
}

fn assert_pointer_scene_hits_equal(optimized: &PointerSceneHit, reference: &PointerSceneHit) {
    match (optimized, reference) {
        (PointerSceneHit::None, PointerSceneHit::None) => {}
        (
            PointerSceneHit::Client { target: optimized },
            PointerSceneHit::Client { target: reference },
        ) => {
            assert_eq!(
                compositor_surface_id(&optimized.surface),
                compositor_surface_id(&reference.surface)
            );
            assert!((optimized.surface_x - reference.surface_x).abs() <= 1.0e-6);
            assert!((optimized.surface_y - reference.surface_y).abs() <= 1.0e-6);
        }
        (
            PointerSceneHit::Decoration {
                window_id: optimized_window,
                root_surface_id: optimized_root,
                hit: optimized_hit,
            },
            PointerSceneHit::Decoration {
                window_id: reference_window,
                root_surface_id: reference_root,
                hit: reference_hit,
            },
        ) => {
            assert_eq!(optimized_window, reference_window);
            assert_eq!(optimized_root, reference_root);
            assert_eq!(optimized_hit, reference_hit);
        }
        _ => panic!("optimized pointer hit differs from exact reference"),
    }
}

struct XdgWindowOpenFixture {
    state: CompositorState,
    _display: wayland_server::Display<CompositorState>,
    _client: wayland_server::Client,
    surface_id: u32,
    scene_node_id: crate::core::SceneNodeId,
}

impl XdgWindowOpenFixture {
    fn new(
        surface_id: u32,
        preset: AnimationPreset,
        animate_maximized_window_open: bool,
        canonical_opacity: Option<PresentationOpacity>,
    ) -> Self {
        let mut state = xdg_state(
            test_surface(surface_id),
            DecorationPreference::ServerSide,
            ToplevelMode::Normal,
        );
        set_window_open_preset_with_maximized_policy(
            &mut state,
            preset,
            animate_maximized_window_open,
        );
        state.set_test_effective_xdg_window_geometry(
            surface_id,
            XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
        );
        state.xdg_surface_lifecycles.entry(surface_id).or_default();
        let (display, client) = install_test_toplevel_role(&mut state, surface_id);
        state.presentation_animator.set_enabled(true);
        let window_id = state
            .window_id_for_surface(surface_id)
            .expect("test window");
        if let Some(opacity) = canonical_opacity {
            state
                .window_mut(window_id)
                .expect("test window")
                .set_canonical_opacity(opacity);
        }
        let scene_node_id = state
            .scene_node_id_for_window_group(window_id)
            .expect("window group node");

        Self {
            state,
            _display: display,
            _client: client,
            surface_id,
            scene_node_id,
        }
    }

    fn start_and_physically_present_intermediate_geometry(
        &mut self,
    ) -> (
        crate::presentation_animation::PresentationTransactionMember,
        crate::presentation_animation::PresentationRect,
    ) {
        assert!(
            self.state
                .maybe_begin_window_open_animation(self.surface_id)
        );
        let member = self
            .state
            .presentation_animator
            .active_transaction_member(
                self.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            )
            .expect("WindowOpen Geometry member");
        let started_at = self
            .state
            .presentation_animator
            .track_started_at_for_scene_node(self.scene_node_id)
            .expect("WindowOpen start time");
        let sample = self
            .state
            .presentation_animator
            .sample_for_scene_node(
                self.scene_node_id,
                AnimationTime::from_nanos(started_at.as_nanos() + 80_000_000),
            )
            .expect("intermediate WindowOpen sample");
        self.state.publish_presented_window_geometry(
            1,
            PresentedWindowGeometry::new(self.surface_id, sample.rect),
        );
        assert!(
            self.state
                .presented_window_geometry(self.surface_id)
                .is_some()
        );
        assert_eq!(
            self.state.presentation_animator.active_transaction_member(
                self.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            ),
            Some(member),
            "intermediate physical presentation must not ACK the unsettled revision"
        );
        (member, sample.rect)
    }
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
    assert!(state.window_open_presentation_active(surface_id));
    assert!(state.presented_window_geometry(surface_id).is_none());

    let maximized_geometry =
        state.window_geometry_for_surface_mode(surface_id, ToplevelMode::Maximized);
    assert!(state.set_root_window_mode(surface_id, ToplevelMode::Maximized));

    assert_eq!(
        state.window(window_id).expect("test window").state.mode(),
        ToplevelMode::Maximized
    );
    assert!(!state.window_open_presentation_active(surface_id));
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
    assert_eq!(state.presentation_animator.transaction_count(), 0);
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
    assert_eq!(state.presentation_animator.transaction_count(), 0);
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
fn presented_xdg_window_scale_is_settled_by_late_maximize_when_policy_is_off() {
    let surface_id = 3_501;
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

    assert!(state.maybe_begin_window_open_animation(surface_id));
    let open_member = state
        .presentation_animator
        .active_transaction_member(
            scene_node_id,
            crate::presentation_animation::PresentationPropertyKind::Geometry,
        )
        .expect("WindowOpen Geometry member");
    let started_at = state
        .presentation_animator
        .track_started_at_for_scene_node(scene_node_id)
        .expect("WindowOpen start time");
    let intermediate = state
        .presentation_animator
        .sample_for_scene_node(
            scene_node_id,
            AnimationTime::from_nanos(started_at.as_nanos() + 90_000_000),
        )
        .expect("intermediate WindowOpen sample");
    let maximized_geometry =
        state.window_geometry_for_surface_mode(surface_id, ToplevelMode::Maximized);
    let maximized_rect = state
        .presentation_rect_for_geometry(surface_id, maximized_geometry)
        .expect("maximized target rect");
    assert_ne!(intermediate.rect, maximized_rect);
    state.publish_presented_window_geometry(
        1,
        PresentedWindowGeometry::new(surface_id, intermediate.rect),
    );
    assert!(state.presented_window_geometry(surface_id).is_some());
    assert_eq!(
        state.presentation_animator.active_transaction_member(
            scene_node_id,
            crate::presentation_animation::PresentationPropertyKind::Geometry,
        ),
        Some(open_member),
        "intermediate physical presentation must not ACK the unsettled WindowOpen revision"
    );
    assert!(state.window_open_presentation_active(surface_id));

    assert!(state.set_root_window_mode(surface_id, ToplevelMode::Maximized));

    assert_eq!(
        state.window(window_id).expect("test window").state.mode(),
        ToplevelMode::Maximized
    );
    assert_eq!(
        state.presentation_animator.active_transaction_member(
            scene_node_id,
            crate::presentation_animation::PresentationPropertyKind::Geometry,
        ),
        None,
        "late maximize should cancel WindowOpen Geometry without installing MaximizeEnter"
    );
    assert!(!state.presentation_animator.has_opacity_track(scene_node_id));
    assert_eq!(
        state.current_visual_root_window_geometry(surface_id),
        Some(maximized_geometry)
    );
}

#[test]
fn presented_xdg_window_glide_is_cancelled_on_late_maximize_and_restores_canonical_opacity() {
    let canonical_opacity = PresentationOpacity::new(0.42).expect("canonical opacity");
    let mut fixture = XdgWindowOpenFixture::new(
        3_502,
        AnimationPreset::Macos,
        false,
        Some(canonical_opacity),
    );
    let (open_geometry, _) = fixture.start_and_physically_present_intermediate_geometry();
    assert!(
        fixture
            .state
            .window_open_presentation_active(fixture.surface_id)
    );
    let open_opacity = fixture
        .state
        .presentation_animator
        .active_transaction_member(
            fixture.scene_node_id,
            crate::presentation_animation::PresentationPropertyKind::Opacity,
        )
        .expect("WindowOpen Opacity member");
    assert_eq!(
        open_geometry.transaction_id(),
        open_opacity.transaction_id()
    );
    let start_time = fixture
        .state
        .presentation_animator
        .track_started_at_for_scene_node(fixture.scene_node_id)
        .expect("WindowGlide start time");
    let intermediate_time = AnimationTime::from_nanos(start_time.as_nanos() + 80_000_000);
    let maximized_geometry = fixture
        .state
        .window_geometry_for_surface_mode(fixture.surface_id, ToplevelMode::Maximized);

    assert!(
        fixture
            .state
            .window_open_presentation_active(fixture.surface_id)
    );
    assert!(
        fixture
            .state
            .set_root_window_mode(fixture.surface_id, ToplevelMode::Maximized)
    );

    assert_eq!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            ),
        None
    );
    assert_eq!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Opacity,
            ),
        None,
        "both exact WindowOpen members must be retired"
    );
    assert_eq!(
        fixture
            .state
            .presentation_animator
            .track_curve(fixture.scene_node_id),
        None,
        "late maximize with policy Off must not install MaximizeEnter"
    );
    assert!(
        !fixture
            .state
            .window_open_presentation_active(fixture.surface_id)
    );
    assert_eq!(
        fixture
            .state
            .current_visual_root_window_geometry(fixture.surface_id),
        Some(maximized_geometry)
    );
    assert_eq!(
        fixture
            .state
            .presentation_scene_sample_at(intermediate_time)
            .opacity_for_scene_node(fixture.scene_node_id),
        canonical_opacity,
        "cancelling Glide Opacity must immediately expose canonical opacity"
    );
}

#[test]
fn opacity_only_window_glide_ownership_suppresses_late_maximize_without_claiming_geometry() {
    let canonical_opacity = PresentationOpacity::new(0.42).expect("canonical opacity");
    let mut fixture = XdgWindowOpenFixture::new(
        3_503,
        AnimationPreset::Macos,
        false,
        Some(canonical_opacity),
    );
    let target = fixture
        .state
        .current_presentation_rect_for_root(fixture.surface_id)
        .expect("canonical presentation rect");
    let unrelated_start = crate::presentation_animation::PresentationRect::new(
        target.x() - 40.0,
        target.y(),
        target.width(),
        target.height(),
    )
    .expect("unrelated geometry start");
    let unrelated_transaction = fixture
        .state
        .presentation_animator
        .commit(
            crate::presentation_animation::PresentationTransactionRequest::geometry(
                AnimationTime::monotonic_now().expect("monotonic time"),
                vec![
                    crate::presentation_animation::PresentationGeometryMutation::new(
                        fixture.scene_node_id,
                        unrelated_start,
                        target,
                        crate::presentation_animation::AnimationCurve::easing(
                            std::time::Duration::from_millis(400),
                            crate::presentation_animation::EasingCurve::Linear,
                        ),
                    ),
                ],
            ),
        )
        .expect("unrelated Geometry transaction");
    let unrelated_member = unrelated_transaction.members()[0];

    assert!(
        fixture
            .state
            .maybe_begin_window_open_animation(fixture.surface_id)
    );

    let open_opacity = fixture
        .state
        .presentation_animator
        .active_transaction_member(
            fixture.scene_node_id,
            crate::presentation_animation::PresentationPropertyKind::Opacity,
        )
        .expect("opacity-only WindowOpen member");
    assert_ne!(
        open_opacity.transaction_id(),
        unrelated_member.transaction_id()
    );
    assert_eq!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            ),
        Some(unrelated_member),
        "WindowGlide fallback must preserve the unrelated Geometry owner"
    );
    assert!(
        fixture
            .state
            .window_open_presentation_active(fixture.surface_id)
    );
    let started_at = fixture
        .state
        .presentation_animator
        .track_started_at_for_scene_node(fixture.scene_node_id)
        .expect("unrelated Geometry start time");
    let intermediate_time = AnimationTime::from_nanos(started_at.as_nanos() + 80_000_000);
    let intermediate = fixture
        .state
        .presentation_animator
        .sample_for_scene_node(fixture.scene_node_id, intermediate_time)
        .expect("intermediate Geometry sample");
    fixture.state.publish_presented_window_geometry(
        1,
        PresentedWindowGeometry::new(fixture.surface_id, intermediate.rect),
    );

    let maximized_geometry = fixture
        .state
        .window_geometry_for_surface_mode(fixture.surface_id, ToplevelMode::Maximized);
    assert!(
        fixture
            .state
            .set_root_window_mode(fixture.surface_id, ToplevelMode::Maximized)
    );

    assert_eq!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Opacity,
            ),
        None,
        "exact WindowOpen Opacity ownership must be cancelled"
    );
    assert!(
        !fixture
            .state
            .window_open_presentation_active(fixture.surface_id)
    );
    assert_eq!(
        fixture
            .state
            .presentation_animator
            .track_curve(fixture.scene_node_id),
        None,
        "policy Off must use the canonical immediate maximized visual"
    );
    assert_eq!(
        fixture
            .state
            .current_visual_root_window_geometry(fixture.surface_id),
        Some(maximized_geometry)
    );
    assert_eq!(
        fixture
            .state
            .presentation_scene_sample_at(intermediate_time)
            .opacity_for_scene_node(fixture.scene_node_id),
        canonical_opacity
    );
}

#[test]
fn maximized_window_open_policy_on_keeps_presented_late_maximize_animation() {
    let mut fixture = XdgWindowOpenFixture::new(3_504, AnimationPreset::Astrea, true, None);
    let (open_geometry, _) = fixture.start_and_physically_present_intermediate_geometry();
    let maximize_curve = fixture
        .state
        .presentation_animation_policy
        .curve_for(crate::compositor::PresentationAnimationKind::MaximizeEnter);

    assert!(
        fixture
            .state
            .set_root_window_mode(fixture.surface_id, ToplevelMode::Maximized)
    );

    assert_ne!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            ),
        Some(open_geometry)
    );
    assert_eq!(
        fixture
            .state
            .presentation_animator
            .track_curve(fixture.scene_node_id),
        Some(maximize_curve),
        "policy On retains the existing MaximizeEnter behavior"
    );
}

#[test]
fn physically_acked_window_scale_is_settled_before_later_maximize() {
    let mut fixture = XdgWindowOpenFixture::new(3_505, AnimationPreset::Astrea, false, None);
    assert!(
        fixture
            .state
            .maybe_begin_window_open_animation(fixture.surface_id)
    );
    let open_geometry = fixture
        .state
        .presentation_animator
        .active_transaction_member(
            fixture.scene_node_id,
            crate::presentation_animation::PresentationPropertyKind::Geometry,
        )
        .expect("WindowOpen Geometry member");
    let started_at = fixture
        .state
        .presentation_animator
        .track_started_at_for_scene_node(fixture.scene_node_id)
        .expect("WindowOpen start time");
    let intermediate_rect = fixture
        .state
        .presentation_animator
        .sample_for_scene_node(
            fixture.scene_node_id,
            AnimationTime::from_nanos(started_at.as_nanos() + 80_000_000),
        )
        .expect("intermediate WindowOpen sample")
        .rect;
    fixture.state.publish_presented_window_geometry(
        1,
        PresentedWindowGeometry::new(fixture.surface_id, intermediate_rect),
    );
    let settled_rect = fixture
        .state
        .presentation_animator
        .sample_for_scene_node(fixture.scene_node_id, AnimationTime::from_nanos(u64::MAX))
        .expect("mathematically settled WindowOpen sample")
        .rect;
    fixture.state.publish_presented_window_geometry(
        2,
        PresentedWindowGeometry::new(fixture.surface_id, settled_rect),
    );
    assert!(
        fixture
            .state
            .presented_window_geometry(fixture.surface_id)
            .is_some()
    );
    let output_id = fixture
        .state
        .ensure_native_output_id()
        .expect("test output identity");
    assert!(
        fixture
            .state
            .presentation_animator
            .acknowledge_presented_geometry(
                output_id,
                crate::presentation_animation::PresentedGeometryAck {
                    output_id,
                    scene_node_id: fixture.scene_node_id,
                    property: crate::presentation_animation::PresentationPropertyKind::Geometry,
                    transaction_id: open_geometry.transaction_id(),
                    revision_id: open_geometry.revision_id(),
                    presented_rect: settled_rect,
                },
            )
    );
    assert_eq!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            ),
        None
    );
    assert!(
        !fixture
            .state
            .window_open_presentation_active(fixture.surface_id)
    );
    let maximize_curve = fixture
        .state
        .presentation_animation_policy
        .curve_for(crate::compositor::PresentationAnimationKind::MaximizeEnter);

    assert!(
        fixture
            .state
            .set_root_window_mode(fixture.surface_id, ToplevelMode::Maximized)
    );

    assert_eq!(
        fixture
            .state
            .presentation_animator
            .track_curve(fixture.scene_node_id),
        Some(maximize_curve),
        "fully ACKed WindowOpen must not suppress a later MaximizeEnter"
    );
}

#[test]
fn mathematically_settled_but_unacked_window_scale_still_suppresses_late_maximize() {
    let mut fixture = XdgWindowOpenFixture::new(3_506, AnimationPreset::Astrea, false, None);
    assert!(
        fixture
            .state
            .maybe_begin_window_open_animation(fixture.surface_id)
    );
    let open_geometry = fixture
        .state
        .presentation_animator
        .active_transaction_member(
            fixture.scene_node_id,
            crate::presentation_animation::PresentationPropertyKind::Geometry,
        )
        .expect("WindowOpen Geometry member");
    let started_at = fixture
        .state
        .presentation_animator
        .track_started_at_for_scene_node(fixture.scene_node_id)
        .expect("WindowOpen start time");
    let intermediate_rect = fixture
        .state
        .presentation_animator
        .sample_for_scene_node(
            fixture.scene_node_id,
            AnimationTime::from_nanos(started_at.as_nanos() + 80_000_000),
        )
        .expect("intermediate WindowOpen sample")
        .rect;
    fixture.state.publish_presented_window_geometry(
        1,
        PresentedWindowGeometry::new(fixture.surface_id, intermediate_rect),
    );
    let settled_sample = fixture
        .state
        .presentation_animator
        .sample_for_scene_node(fixture.scene_node_id, AnimationTime::from_nanos(u64::MAX))
        .expect("mathematically settled sample");
    assert!(settled_sample.mathematically_settled);
    assert_eq!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            ),
        Some(open_geometry),
        "mathematical settlement without the exact ACK remains active"
    );
    assert!(
        fixture
            .state
            .window_open_presentation_active(fixture.surface_id)
    );
    let maximized_geometry = fixture
        .state
        .window_geometry_for_surface_mode(fixture.surface_id, ToplevelMode::Maximized);

    assert!(
        fixture
            .state
            .set_root_window_mode(fixture.surface_id, ToplevelMode::Maximized)
    );

    assert_eq!(
        fixture
            .state
            .presentation_animator
            .track_curve(fixture.scene_node_id),
        None,
        "only the physical exact-revision ACK settles WindowOpen"
    );
    assert_eq!(
        fixture
            .state
            .current_visual_root_window_geometry(fixture.surface_id),
        Some(maximized_geometry)
    );
}

#[test]
fn unrelated_geometry_track_is_not_window_open_ownership() {
    let mut fixture = XdgWindowOpenFixture::new(3_507, AnimationPreset::Astrea, false, None);
    let target = fixture
        .state
        .current_presentation_rect_for_root(fixture.surface_id)
        .expect("canonical presentation rect");
    let start = crate::presentation_animation::PresentationRect::new(
        target.x() - 40.0,
        target.y(),
        target.width(),
        target.height(),
    )
    .expect("unrelated Geometry start");
    fixture
        .state
        .presentation_animator
        .commit(
            crate::presentation_animation::PresentationTransactionRequest::geometry(
                AnimationTime::monotonic_now().expect("monotonic time"),
                vec![
                    crate::presentation_animation::PresentationGeometryMutation::new(
                        fixture.scene_node_id,
                        start,
                        target,
                        crate::presentation_animation::AnimationCurve::easing(
                            std::time::Duration::from_millis(400),
                            crate::presentation_animation::EasingCurve::Linear,
                        ),
                    ),
                ],
            ),
        )
        .expect("unrelated Geometry transaction");

    assert!(
        fixture
            .state
            .presentation_animator
            .has_geometry_track(fixture.scene_node_id)
    );
    assert!(
        !fixture
            .state
            .window_open_presentation_active(fixture.surface_id),
        "a generic Geometry track must not be called WindowOpen"
    );
    let maximize_curve = fixture
        .state
        .presentation_animation_policy
        .curve_for(crate::compositor::PresentationAnimationKind::MaximizeEnter);
    fixture.state.publish_presented_window_geometry(
        1,
        PresentedWindowGeometry::new(fixture.surface_id, start),
    );

    assert!(
        fixture
            .state
            .set_root_window_mode(fixture.surface_id, ToplevelMode::Maximized)
    );

    assert_eq!(
        fixture
            .state
            .presentation_animator
            .track_curve(fixture.scene_node_id),
        Some(maximize_curve)
    );
}

#[test]
fn stale_superseded_window_scale_member_does_not_suppress_late_maximize() {
    let mut fixture = XdgWindowOpenFixture::new(3_508, AnimationPreset::Astrea, false, None);
    let (open_geometry, _) = fixture.start_and_physically_present_intermediate_geometry();
    let target = fixture
        .state
        .current_presentation_rect_for_root(fixture.surface_id)
        .expect("canonical presentation rect");
    let replacement_start = crate::presentation_animation::PresentationRect::new(
        target.x() - 60.0,
        target.y(),
        target.width(),
        target.height(),
    )
    .expect("replacement Geometry start");
    let replacement = fixture
        .state
        .presentation_animator
        .commit(
            crate::presentation_animation::PresentationTransactionRequest::geometry(
                AnimationTime::monotonic_now().expect("monotonic time"),
                vec![
                    crate::presentation_animation::PresentationGeometryMutation::new(
                        fixture.scene_node_id,
                        replacement_start,
                        target,
                        crate::presentation_animation::AnimationCurve::easing(
                            std::time::Duration::from_millis(400),
                            crate::presentation_animation::EasingCurve::Linear,
                        ),
                    ),
                ],
            ),
        )
        .expect("replacement Geometry transaction");
    let replacement_member = replacement.members()[0];
    assert_ne!(Some(replacement_member), Some(open_geometry));
    assert_eq!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            ),
        Some(replacement_member)
    );
    assert!(
        !fixture
            .state
            .window_open_presentation_active(fixture.surface_id),
        "the stored WindowOpen revision is stale even though Geometry remains active"
    );
    let maximize_curve = fixture
        .state
        .presentation_animation_policy
        .curve_for(crate::compositor::PresentationAnimationKind::MaximizeEnter);

    assert!(
        fixture
            .state
            .set_root_window_mode(fixture.surface_id, ToplevelMode::Maximized)
    );

    assert_eq!(
        fixture
            .state
            .presentation_animator
            .track_curve(fixture.scene_node_id),
        Some(maximize_curve),
        "a stale WindowOpen identity must not cancel or replace a newer owner"
    );
}

#[test]
fn superseded_glide_geometry_keeps_window_open_active_through_exact_opacity() {
    let mut fixture = XdgWindowOpenFixture::new(3_509, AnimationPreset::Macos, false, None);
    let (open_geometry, _) = fixture.start_and_physically_present_intermediate_geometry();
    let open_opacity = fixture
        .state
        .presentation_animator
        .active_transaction_member(
            fixture.scene_node_id,
            crate::presentation_animation::PresentationPropertyKind::Opacity,
        )
        .expect("WindowOpen Opacity member");
    let target = fixture
        .state
        .current_presentation_rect_for_root(fixture.surface_id)
        .expect("canonical presentation rect");
    let replacement = fixture
        .state
        .presentation_animator
        .commit(
            crate::presentation_animation::PresentationTransactionRequest::geometry(
                AnimationTime::monotonic_now().expect("monotonic time"),
                vec![
                    crate::presentation_animation::PresentationGeometryMutation::new(
                        fixture.scene_node_id,
                        crate::presentation_animation::PresentationRect::new(
                            target.x() - 60.0,
                            target.y(),
                            target.width(),
                            target.height(),
                        )
                        .expect("replacement Geometry start"),
                        target,
                        crate::presentation_animation::AnimationCurve::easing(
                            std::time::Duration::from_millis(400),
                            crate::presentation_animation::EasingCurve::Linear,
                        ),
                    ),
                ],
            ),
        )
        .expect("replacement Geometry transaction");
    assert_eq!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Opacity,
            ),
        Some(open_opacity)
    );
    assert_ne!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            ),
        Some(open_geometry)
    );
    assert!(
        fixture
            .state
            .window_open_presentation_active(fixture.surface_id)
    );
    let maximized_geometry = fixture
        .state
        .window_geometry_for_surface_mode(fixture.surface_id, ToplevelMode::Maximized);

    assert!(
        fixture
            .state
            .set_root_window_mode(fixture.surface_id, ToplevelMode::Maximized)
    );

    assert_eq!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Opacity,
            ),
        None,
        "the still-exact WindowOpen Opacity member must be cancelled"
    );
    assert_eq!(
        fixture
            .state
            .presentation_animator
            .track_curve(fixture.scene_node_id),
        None
    );
    assert_eq!(
        fixture
            .state
            .current_visual_root_window_geometry(fixture.surface_id),
        Some(maximized_geometry)
    );
    assert_ne!(replacement.members()[0], open_geometry);
}

#[test]
fn presented_window_open_does_not_suppress_xdg_fullscreen_enter() {
    let mut fixture = XdgWindowOpenFixture::new(3_510, AnimationPreset::Astrea, false, None);
    let _ = fixture.start_and_physically_present_intermediate_geometry();
    let fullscreen_curve = fixture
        .state
        .presentation_animation_policy
        .curve_for(crate::compositor::PresentationAnimationKind::FullscreenEnter);

    assert!(
        fixture
            .state
            .set_root_window_mode(fixture.surface_id, ToplevelMode::Fullscreen)
    );

    assert_eq!(
        fixture
            .state
            .presentation_animator
            .track_curve(fixture.scene_node_id),
        Some(fullscreen_curve),
        "the maximized-open policy does not apply to FullscreenEnter"
    );
}

#[test]
fn xdg_unmap_retires_window_open_presentation_members() {
    let mut fixture = XdgWindowOpenFixture::new(3_512, AnimationPreset::Astrea, false, None);
    let _ = fixture.start_and_physically_present_intermediate_geometry();

    assert!(fixture.state.unmap_xdg_role_surfaces(fixture.surface_id));

    assert!(
        !fixture
            .state
            .window_open_presentation_ownership
            .contains_key(&fixture.surface_id)
    );
    assert_eq!(
        fixture
            .state
            .presentation_animator
            .active_transaction_member(
                fixture.scene_node_id,
                crate::presentation_animation::PresentationPropertyKind::Geometry,
            ),
        None,
        "unmapped XDG WindowOpen identity must not survive for a later map"
    );
    assert_eq!(fixture.state.presentation_animator.transaction_count(), 0);
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

#[test]
fn xdg_maximize_exit_first_frame_maps_the_committed_root_to_the_physical_source() {
    let surface_id = 97;
    let mut state = xdg_state(
        test_surface_with_size(surface_id, 500, 500),
        DecorationPreference::ClientSide,
        ToplevelMode::Normal,
    );
    let restore_geometry =
        WindowGeometry::new(SurfacePlacement::absolute_root_at(360, 250), 500, 500);
    state.set_surface_placement(surface_id, restore_geometry.placement);
    state
        .set_test_effective_xdg_window_geometry(surface_id, XdgWindowGeometry::new(0, 0, 500, 500));
    state.xdg_surface_lifecycles.entry(surface_id).or_default();
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    state.presentation_animator.set_enabled(true);

    let window_id = state
        .window_id_for_surface(surface_id)
        .expect("test window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    let normal_rect = state
        .presentation_rect_for_geometry(surface_id, restore_geometry)
        .expect("normal presentation rect");
    state.publish_presented_window_geometry(
        1,
        PresentedWindowGeometry::with_scene_node(scene_node_id, surface_id, normal_rect),
    );

    let maximized_geometry =
        state.window_geometry_for_surface_mode(surface_id, ToplevelMode::Maximized);
    let max_rect = state
        .presentation_rect_for_geometry(surface_id, maximized_geometry)
        .expect("maximized presentation rect");
    state.presentation_animator.set_enabled(false);
    assert!(state.set_root_window_mode(surface_id, ToplevelMode::Maximized));
    assert!(
        !state
            .presentation_animator
            .has_geometry_track(scene_node_id),
        "disabled maximize entry is classified as Immediate"
    );
    let maximize_configure_serial = state
        .toplevel_visual_geometries
        .get(&surface_id)
        .and_then(|visual| visual.xdg_mode_transition_fence)
        .expect("maximize response fence")
        .configure_serial;
    acknowledge_test_xdg_configure(&mut state, surface_id, maximize_configure_serial);
    let maximize_commit = state.allocate_surface_commit_sequence();
    let mut committed_maximized_surface = test_surface_with_size(
        surface_id,
        maximized_geometry.width,
        maximized_geometry.height,
    );
    committed_maximized_surface.generation = 2;
    committed_maximized_surface.commit_sequence = maximize_commit;
    committed_maximized_surface.placement = maximized_geometry.placement;
    state.renderable_surfaces[0] = committed_maximized_surface;
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(
            0,
            0,
            maximized_geometry.width as i32,
            maximized_geometry.height as i32,
        ),
    );
    state.update_toplevel_visual_render_assignment_after_root_commit(surface_id, maximize_commit);
    state.rebuild_active_scene_view();
    state.advance_render_generation(RenderGenerationCause::SurfaceCommit);
    state.presentation_animator.set_enabled(true);
    let maximize_scene = state.canonical_presentation_scene();
    let maximize_targets = state.native_frame_presentation_targets_for_scene(&maximize_scene);
    let settled_maximize_sample = state.presentation_scene_sample_for_targets_at_with_source(
        AnimationTime::monotonic_now().expect("monotonic maximize sample time"),
        crate::presentation_animation::PresentationSampleTimeSource::ScheduledTarget,
        &maximize_targets,
    );
    let settled_maximize_snapshot = crate::presentation_animation::PresentationFrameSnapshot::
        from_sample_with_presented_windows(
            &settled_maximize_sample,
            state.presented_window_geometries_for_targets(
                &settled_maximize_sample,
                &maximize_targets,
            ),
        );
    let settled_maximize_rect = settled_maximize_snapshot
        .presented_window_geometry(surface_id)
        .expect("settled maximize frame geometry")
        .presented_rect();
    assert_eq!(
        settled_maximize_snapshot.sample_time_source,
        crate::presentation_animation::PresentationSampleTimeSource::ScheduledTarget
    );
    assert_eq!(settled_maximize_rect, max_rect,);
    state.publish_presented_window_geometry(
        2,
        PresentedWindowGeometry::with_scene_node(scene_node_id, surface_id, settled_maximize_rect),
    );

    let last_physical = state
        .presented_window_geometry(surface_id)
        .expect("maximized geometry was physically published")
        .presented_rect();

    assert!(state.restore_normal_root_window(surface_id));
    let current_visual_geometry = state
        .current_visual_root_window_geometry(surface_id)
        .expect("restore destination geometry");
    assert_eq!(current_visual_geometry, restore_geometry);
    let restore_configure_serial = state
        .toplevel_visual_geometries
        .get(&surface_id)
        .and_then(|visual| visual.xdg_mode_transition_fence)
        .expect("restore response fence remains pending for the slow client")
        .configure_serial;
    assert_ne!(restore_configure_serial, maximize_configure_serial);

    let started_at = state
        .presentation_animator
        .track_started_at_for_scene_node(scene_node_id)
        .expect("MaximizeExit track start");
    let first_restore_frame_id = 3;
    let first_restore_render_generation = state.render_generation;
    let first_restore_scene_generation = state.scene_render_generation;
    let canonical_scene = state.canonical_presentation_scene();
    let targets = state.native_frame_presentation_targets_for_scene(&canonical_scene);
    let presentation = state.presentation_scene_sample_for_targets_at_with_source(
        started_at,
        crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback,
        &targets,
    );
    let frame_presentation = crate::presentation_animation::PresentationFrameSnapshot::
        from_sample_with_presented_windows(
            &presentation,
            state.presented_window_geometries_for_targets(&presentation, &targets),
        );
    let transform = frame_presentation
        .transform_for_root(surface_id)
        .expect("first resolved restore frame carries MaximizeExit");
    let projected_surfaces = state.apply_presentation_to_native_frame_surfaces(
        Cow::Borrowed(canonical_scene.surfaces.as_ref()),
        canonical_scene.presentation_owner_root_surface_ids.as_ref(),
        &presentation,
    );
    let projected_root = projected_surfaces
        .iter()
        .find(|surface| surface.surface_id == surface_id)
        .expect("projected root surface");
    let committed_root_extent = (projected_root.width, projected_root.height);
    let render_target_size = projected_root.render_target_size;
    let render_placement = projected_root.render_placement;

    assert_eq!(frame_presentation.sampled_at, started_at);
    assert_eq!(
        frame_presentation.sample_time_source,
        crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback
    );
    assert!(frame_presentation.sampled_at >= settled_maximize_snapshot.sampled_at);
    assert_eq!(frame_presentation.output_id, presentation.output_id);
    assert!(first_restore_render_generation > 0);
    assert!(first_restore_scene_generation > 0);
    assert_eq!(
        transform.scene_node_id, scene_node_id,
        "frame identity: render_generation={} scene_generation={} root_surface_id={} scene_node_id={:?}",
        state.render_generation, state.scene_render_generation, surface_id, transform.scene_node_id,
    );
    assert_eq!(
        transform.canonical_rect,
        state
            .presentation_rect_for_geometry(surface_id, restore_geometry)
            .unwrap()
    );
    assert_eq!(transform.presented_rect, last_physical);
    assert!(!transform.mathematically_settled);
    assert_eq!(
        committed_root_extent,
        (maximized_geometry.width, maximized_geometry.height)
    );
    assert_eq!(
        render_target_size,
        BufferSize::new(maximized_geometry.width, maximized_geometry.height),
        "the first rendered root extent must match the physically presented maximize source; frame_id={} render_generation={} scene_generation={} sampled_at={:?} sample_time_source={:?} transaction={:?} revision={:?} presented_rect={:?} render_placement={:?} current_visual_geometry={:?}",
        first_restore_frame_id,
        first_restore_render_generation,
        first_restore_scene_generation,
        frame_presentation.sampled_at,
        frame_presentation.sample_time_source,
        transform.transaction_id,
        transform.revision_id,
        transform.presented_rect,
        render_placement,
        current_visual_geometry,
    );
    assert_eq!(render_placement, Some(maximized_geometry.placement));

    state.publish_presented_window_geometry(
        first_restore_frame_id,
        PresentedWindowGeometry::with_scene_node(
            scene_node_id,
            surface_id,
            transform.presented_rect,
        ),
    );
    assert_eq!(
        state
            .presented_window_geometry(surface_id)
            .unwrap()
            .presented_rect(),
        last_physical,
        "the first physically publishable restore frame must freeze the source geometry"
    );

    acknowledge_test_xdg_configure(&mut state, surface_id, restore_configure_serial);
    let restore_commit = state.allocate_surface_commit_sequence();
    let mut committed_restore_surface = test_surface_with_size(surface_id, 500, 500);
    committed_restore_surface.generation = 3;
    committed_restore_surface.commit_sequence = restore_commit;
    committed_restore_surface.placement = restore_geometry.placement;
    state.renderable_surfaces[0] = committed_restore_surface;
    state
        .set_test_effective_xdg_window_geometry(surface_id, XdgWindowGeometry::new(0, 0, 500, 500));
    state.update_toplevel_visual_render_assignment_after_root_commit(surface_id, restore_commit);
    state.rebuild_active_scene_view();
    state.advance_render_generation(RenderGenerationCause::SurfaceCommit);

    let fast_response_at = AnimationTime::from_nanos(started_at.as_nanos() + 10_000_000);
    let fast_response_scene = state.canonical_presentation_scene();
    let fast_response_targets =
        state.native_frame_presentation_targets_for_scene(&fast_response_scene);
    let fast_response_sample = state.presentation_scene_sample_for_targets_at_with_source(
        fast_response_at,
        crate::presentation_animation::PresentationSampleTimeSource::ScheduledTarget,
        &fast_response_targets,
    );
    let fast_response_snapshot = crate::presentation_animation::PresentationFrameSnapshot::
        from_sample_with_presented_windows(
            &fast_response_sample,
            state.presented_window_geometries_for_targets(
                &fast_response_sample,
                &fast_response_targets,
            ),
        );
    let fast_response_transform = fast_response_sample
        .transform_for_root(surface_id)
        .expect("post-ACK restore frame keeps the active transition");
    let fast_response_surfaces = state.apply_presentation_to_native_frame_surfaces(
        Cow::Borrowed(fast_response_scene.surfaces.as_ref()),
        fast_response_scene
            .presentation_owner_root_surface_ids
            .as_ref(),
        &fast_response_sample,
    );
    let fast_response_root = fast_response_surfaces
        .iter()
        .find(|surface| surface.surface_id == surface_id)
        .expect("post-ACK root surface");
    assert_eq!(
        fast_response_root.render_target_size,
        BufferSize::new(
            fast_response_transform.presented_rect.width().round() as u32,
            fast_response_transform.presented_rect.height().round() as u32,
        ),
        "after the matching client commit, the physical surface follows the same frame-local window transform"
    );
    assert_eq!(
        fast_response_sample
            .transform_for_root(surface_id)
            .expect("post-ACK transition transform")
            .revision_id,
        transform.revision_id,
    );
    assert_eq!(
        fast_response_transform.transaction_id,
        transform.transaction_id
    );
    assert_eq!(
        fast_response_snapshot.sample_time_source,
        crate::presentation_animation::PresentationSampleTimeSource::ScheduledTarget
    );
    assert!(fast_response_snapshot.sampled_at >= frame_presentation.sampled_at);
    assert_ne!(
        fast_response_transform.presented_rect,
        transform.presented_rect
    );
    let fast_response_presented = fast_response_snapshot
        .presented_window_geometry(surface_id)
        .expect("post-ACK physical candidate geometry")
        .presented_rect();
    state.publish_presented_window_geometry(
        4,
        PresentedWindowGeometry::with_scene_node(
            scene_node_id,
            surface_id,
            fast_response_presented,
        ),
    );
    assert_eq!(
        state
            .presented_window_geometry(surface_id)
            .unwrap()
            .presented_rect(),
        fast_response_presented,
        "the later physical candidate freezes the later sample for the same presentation revision"
    );
}

#[test]
fn unchanged_presented_geometry_keeps_hit_validity_and_changed_geometry_remaps_input() {
    let surface_id = 3_911;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ClientSide,
        ToplevelMode::Normal,
    );
    let canonical = state
        .current_presentation_rect_for_root(surface_id)
        .expect("canonical presentation rectangle");
    let first_physical = PresentedWindowGeometry::new(surface_id, canonical);
    state.publish_presented_window_geometry(1, first_physical);
    let stable_generation = state.pointer_hit_generation;
    let topology_generation = state.interaction_topology_generation;
    let screen_point = (canonical.x() + 130.0, canonical.y() + 80.0);
    let first_mapping = state
        .presentation_input_point_for_root(surface_id, screen_point.0, screen_point.1)
        .expect("initial physical mapping");

    state.publish_presented_window_geometry(2, first_physical);
    assert_eq!(state.pointer_hit_generation, stable_generation);
    assert_eq!(state.interaction_topology_generation, topology_generation);

    let moved_physical_rect = crate::presentation_animation::PresentationRect::new(
        canonical.x() + 100.0,
        canonical.y() + 40.0,
        canonical.width(),
        canonical.height(),
    )
    .expect("moved physical rectangle");
    state.publish_presented_window_geometry(
        3,
        PresentedWindowGeometry::new(surface_id, moved_physical_rect),
    );
    let moved_mapping = state
        .presentation_input_point_for_root(surface_id, screen_point.0, screen_point.1)
        .expect("updated physical mapping");

    assert_ne!(state.pointer_hit_generation, stable_generation);
    assert_eq!(state.interaction_topology_generation, topology_generation);
    assert_eq!(first_mapping.0 - moved_mapping.0, 100.0);
    assert_eq!(first_mapping.1 - moved_mapping.1, 40.0);
}

#[test]
fn nonauthoritative_presented_geometry_does_not_remap_input() {
    let surface_id = 3_914;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ClientSide,
        ToplevelMode::Normal,
    );
    let canonical = state
        .current_presentation_rect_for_root(surface_id)
        .expect("canonical presentation rectangle");
    let screen_point = (canonical.x() + 130.0, canonical.y() + 80.0);
    let initial_mapping = state
        .presentation_input_point_for_root(surface_id, screen_point.0, screen_point.1)
        .expect("initial input mapping");
    let moved_physical_rect = crate::presentation_animation::PresentationRect::new(
        canonical.x() + 100.0,
        canonical.y() + 40.0,
        canonical.width(),
        canonical.height(),
    )
    .expect("moved physical rectangle");
    let pointer_generation = state.pointer_hit_generation;
    let topology_generation = state.interaction_topology_generation;

    state.publish_presented_window_geometry(
        0,
        PresentedWindowGeometry::new(surface_id, moved_physical_rect),
    );

    assert_eq!(state.pointer_hit_generation, pointer_generation);
    assert_eq!(state.interaction_topology_generation, topology_generation);
    assert_eq!(
        state
            .presentation_input_point_for_root(surface_id, screen_point.0, screen_point.1)
            .expect("non-authoritative geometry falls back to canonical input mapping"),
        initial_mapping
    );
}

#[test]
fn cancel_presented_root_invalidates_exact_pointer_hit_without_changing_topology() {
    let surface_id = 1;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ClientSide,
        ToplevelMode::Normal,
    );
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
    );
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    let expected_target_surface = state
        .surface_resource_by_id(surface_id)
        .expect("test pointer target has a registered surface resource");
    let output_id = state.native_output_id().expect("test output id");
    let window_id = state
        .window_id_for_surface(surface_id)
        .expect("test window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("test scene node");
    let canonical_rect = state
        .current_presentation_rect_for_root(surface_id)
        .expect("canonical presentation rectangle");
    let shift_x = 500.0;
    let shift_y = 400.0;
    let presented_rect = crate::presentation_animation::PresentationRect::new(
        canonical_rect.x() + shift_x,
        canonical_rect.y() + shift_y,
        canonical_rect.width(),
        canonical_rect.height(),
    )
    .expect("shifted physical rectangle");
    let sample = crate::presentation_animation::PresentationSceneSample::empty_for_output(
        output_id,
        AnimationTime::from_nanos(1),
        PresentationSampleTimeSource::MonotonicFallback,
    );
    let presentation =
        crate::presentation_animation::PresentationFrameSnapshot::from_sample_with_presented_windows(
            &sample,
            vec![PresentedWindowGeometry::with_scene_node(
                scene_node_id,
                surface_id,
                presented_rect,
            )],
        );
    let topology_generation = state.interaction_topology_generation;
    state.publish_presented_presentation(1, &presentation);
    let pointer_generation = state.pointer_hit_generation;
    assert_eq!(state.interaction_topology_generation, topology_generation);

    let origin = state.active_scene_surface_origins()[0];
    let point = (
        f64::from(origin.0) + shift_x + 50.0,
        f64::from(origin.1) + shift_y + 50.0,
    );
    let hit = state.pointer_scene_hit_at(point.0, point.1);
    assert!(
        matches!(
            hit,
            PointerSceneHit::Client { ref target }
                if target.surface.id() == expected_target_surface.id()
        ),
        "expected a client hit at the physically mapped point, got {hit:?}"
    );
    assert_eq!(
        state
            .pointer_scene_hit_cache
            .as_ref()
            .expect("the exact point hit is cached")
            .pointer_hit_generation(),
        pointer_generation
    );

    state.cancel_presentation_for_root(surface_id);

    assert_eq!(state.pointer_hit_generation, pointer_generation + 1);
    assert_eq!(state.interaction_topology_generation, topology_generation);
    assert!(state.presented_window_geometry(surface_id).is_none());
    assert!(
        state
            .presented_presentation
            .as_ref()
            .expect("presentation snapshot remains authoritative")
            .presented_window_geometry(surface_id)
            .is_none(),
        "root removal updates the mirrored snapshot projection too"
    );
    assert!(matches!(
        state.pointer_scene_hit_at(point.0, point.1),
        PointerSceneHit::None
    ));

    let pointer_generation_after_cancel = state.pointer_hit_generation;
    state.cancel_presentation_for_root(surface_id + 1);
    assert_eq!(
        state.pointer_hit_generation,
        pointer_generation_after_cancel
    );
    assert_eq!(state.interaction_topology_generation, topology_generation);
}

#[test]
fn stacking_change_advances_topology_and_rebuilds_visual_stack_groups() {
    let first_surface_id = 3_912;
    let second_surface_id = 3_913;
    let mut state = xdg_state(
        test_surface(first_surface_id),
        DecorationPreference::ClientSide,
        ToplevelMode::Normal,
    );
    let second_window_id = state.allocate_window_id().expect("second window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(second_window_id, second_surface_id))
        .expect("insert second XDG window");
    state.append_renderable_surface(test_surface(second_surface_id));
    state.rebuild_active_scene_view();
    assert_eq!(state.active_scene_surfaces().len(), 2);

    let _ = state.pointer_scene_hit_at(10.0, 10.0);
    let surfaces_before = state
        .active_scene_surfaces()
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    let pointer_generation_before = state.pointer_hit_generation;
    let topology_generation_before = state.interaction_topology_generation;
    let metrics_before = state.pointer_input_metrics_snapshot();

    assert!(state.raise_root_window(first_surface_id));
    let surfaces_after = state
        .active_scene_surfaces()
        .iter()
        .map(|surface| surface.surface_id)
        .collect::<Vec<_>>();
    assert_ne!(surfaces_before, surfaces_after);
    assert_ne!(state.pointer_hit_generation, pointer_generation_before);
    assert_ne!(
        state.interaction_topology_generation,
        topology_generation_before
    );

    let _ = state.pointer_scene_hit_at(10.0, 10.0);
    let metrics_after = state.pointer_input_metrics_snapshot();
    assert_eq!(
        metrics_after.visual_stack_groups_cache_rebuilds,
        metrics_before.visual_stack_groups_cache_rebuilds + 1
    );
    assert!(
        metrics_after.interaction_topology_invalidations
            > metrics_before.interaction_topology_invalidations
    );
}

#[test]
fn x11_maximize_exit_first_frame_maps_the_committed_root_to_the_physical_source() {
    let surface_id = 98;
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_x11(
            window_id,
            test_x11_snapshot_with_size(surface_id, 1280, 800),
        ))
        .expect("insert X11 window");
    state.append_renderable_surface(test_surface_with_size(surface_id, 1280, 800));
    let restore_geometry =
        WindowGeometry::new(SurfacePlacement::absolute_root_at(360, 250), 500, 500);
    let window = state.window_mut(window_id).expect("X11 window");
    window.state.capture_restore_geometry(restore_geometry);
    window.state.set_mode(ToplevelMode::Maximized);
    state.rebuild_active_scene_view();
    state.presentation_animator.set_enabled(true);

    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("X11 window group node");
    let physical_source = state
        .presentation_rect_for_geometry(
            surface_id,
            WindowGeometry::new(SurfacePlacement::absolute_root_at(0, 0), 1280, 800),
        )
        .expect("physical maximize geometry");
    state.publish_presented_window_geometry(
        1,
        PresentedWindowGeometry::with_scene_node(scene_node_id, surface_id, physical_source),
    );

    assert!(state.restore_normal_root_window(surface_id));
    let started_at = state
        .presentation_animator
        .track_started_at_for_scene_node(scene_node_id)
        .expect("X11 MaximizeExit track start");
    let canonical_scene = state.canonical_presentation_scene();
    let targets = state.native_frame_presentation_targets_for_scene(&canonical_scene);
    let sample = state.presentation_scene_sample_for_targets_at_with_source(
        started_at,
        crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback,
        &targets,
    );
    let snapshot = crate::presentation_animation::PresentationFrameSnapshot::
        from_sample_with_presented_windows(
            &sample,
            state.presented_window_geometries_for_targets(&sample, &targets),
        );
    let transform = snapshot
        .transform_for_root(surface_id)
        .expect("first X11 restore frame carries MaximizeExit");
    let projected_surfaces = state.apply_presentation_to_native_frame_surfaces(
        Cow::Borrowed(canonical_scene.surfaces.as_ref()),
        canonical_scene.presentation_owner_root_surface_ids.as_ref(),
        &sample,
    );
    let projected_root = projected_surfaces
        .iter()
        .find(|surface| surface.surface_id == surface_id)
        .expect("projected X11 root surface");

    assert_eq!(snapshot.sampled_at, started_at);
    assert_eq!(
        snapshot.sample_time_source,
        crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback
    );
    assert_eq!(transform.scene_node_id, scene_node_id);
    assert_eq!(transform.presented_rect, physical_source);
    assert_eq!(
        transform.canonical_rect,
        state
            .presentation_rect_for_geometry(surface_id, restore_geometry)
            .expect("X11 restore target")
    );
    assert!(!transform.mathematically_settled);
    assert_eq!((projected_root.width, projected_root.height), (1280, 800));
    assert_eq!(
        projected_root.render_target_size,
        BufferSize::new(1280, 800),
        "X11 committed content must begin at the physically presented maximize extent"
    );
}

#[test]
fn fullscreen_exit_first_frame_maps_the_committed_root_to_the_physical_source() {
    let surface_id = 99;
    let mut state = xdg_state(
        test_surface_with_size(surface_id, 500, 500),
        DecorationPreference::ClientSide,
        ToplevelMode::Normal,
    );
    let restore_geometry =
        WindowGeometry::new(SurfacePlacement::absolute_root_at(360, 250), 500, 500);
    state.set_surface_placement(surface_id, restore_geometry.placement);
    state
        .set_test_effective_xdg_window_geometry(surface_id, XdgWindowGeometry::new(0, 0, 500, 500));
    state.xdg_surface_lifecycles.entry(surface_id).or_default();
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    let window_id = state
        .window_id_for_surface(surface_id)
        .expect("test window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");

    state.presentation_animator.set_enabled(false);
    assert!(state.set_root_window_mode(surface_id, ToplevelMode::Fullscreen));
    let fullscreen_geometry =
        state.window_geometry_for_surface_mode(surface_id, ToplevelMode::Fullscreen);
    let fullscreen_configure_serial = state
        .toplevel_visual_geometries
        .get(&surface_id)
        .and_then(|visual| visual.xdg_mode_transition_fence)
        .expect("fullscreen response fence")
        .configure_serial;
    acknowledge_test_xdg_configure(&mut state, surface_id, fullscreen_configure_serial);
    let fullscreen_commit = state.allocate_surface_commit_sequence();
    let mut committed_fullscreen_surface = test_surface_with_size(
        surface_id,
        fullscreen_geometry.width,
        fullscreen_geometry.height,
    );
    committed_fullscreen_surface.generation = 2;
    committed_fullscreen_surface.commit_sequence = fullscreen_commit;
    committed_fullscreen_surface.placement = fullscreen_geometry.placement;
    state.renderable_surfaces[0] = committed_fullscreen_surface;
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(
            0,
            0,
            fullscreen_geometry.width as i32,
            fullscreen_geometry.height as i32,
        ),
    );
    state.update_toplevel_visual_render_assignment_after_root_commit(surface_id, fullscreen_commit);
    state.rebuild_active_scene_view();
    state.advance_render_generation(RenderGenerationCause::SurfaceCommit);
    state.presentation_animator.set_enabled(true);

    let physical_source = state
        .presentation_rect_for_geometry(surface_id, fullscreen_geometry)
        .expect("physical fullscreen geometry");
    state.publish_presented_window_geometry(
        1,
        PresentedWindowGeometry::with_scene_node(scene_node_id, surface_id, physical_source),
    );
    assert!(state.restore_after_fullscreen_root_window_from(
        surface_id,
        WindowModeRequestSource::OtherInternal,
    ));
    let started_at = state
        .presentation_animator
        .track_started_at_for_scene_node(scene_node_id)
        .expect("FullscreenExit track start");
    let canonical_scene = state.canonical_presentation_scene();
    let targets = state.native_frame_presentation_targets_for_scene(&canonical_scene);
    let sample = state.presentation_scene_sample_for_targets_at_with_source(
        started_at,
        crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback,
        &targets,
    );
    let snapshot = crate::presentation_animation::PresentationFrameSnapshot::
        from_sample_with_presented_windows(
            &sample,
            state.presented_window_geometries_for_targets(&sample, &targets),
        );
    let transform = snapshot
        .transform_for_root(surface_id)
        .expect("first fullscreen restore frame carries FullscreenExit");
    let projected_surfaces = state.apply_presentation_to_native_frame_surfaces(
        Cow::Borrowed(canonical_scene.surfaces.as_ref()),
        canonical_scene.presentation_owner_root_surface_ids.as_ref(),
        &sample,
    );
    let projected_root = projected_surfaces
        .iter()
        .find(|surface| surface.surface_id == surface_id)
        .expect("projected fullscreen root surface");

    assert_eq!(snapshot.sampled_at, started_at);
    assert_eq!(
        snapshot.sample_time_source,
        crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback
    );
    assert_eq!(transform.scene_node_id, scene_node_id);
    assert_eq!(transform.presented_rect, physical_source);
    assert_eq!(
        transform.canonical_rect,
        state
            .presentation_rect_for_geometry(surface_id, restore_geometry)
            .expect("fullscreen return target")
    );
    assert!(!transform.mathematically_settled);
    assert_eq!(
        (projected_root.width, projected_root.height),
        (fullscreen_geometry.width, fullscreen_geometry.height)
    );
    assert_eq!(
        projected_root.render_target_size,
        BufferSize::new(fullscreen_geometry.width, fullscreen_geometry.height),
        "fullscreen committed content must begin at the physically presented fullscreen extent"
    );
}

#[test]
fn xdg_mode_animation_reversals_start_at_the_last_physical_sample() {
    let surface_id = 100;
    let mut state = xdg_state(
        test_surface_with_size(surface_id, 500, 500),
        DecorationPreference::ClientSide,
        ToplevelMode::Normal,
    );
    let restore_geometry =
        WindowGeometry::new(SurfacePlacement::absolute_root_at(360, 250), 500, 500);
    state.set_surface_placement(surface_id, restore_geometry.placement);
    state
        .set_test_effective_xdg_window_geometry(surface_id, XdgWindowGeometry::new(0, 0, 500, 500));
    state.xdg_surface_lifecycles.entry(surface_id).or_default();
    let (_display, _client) = install_test_toplevel_role(&mut state, surface_id);
    state.presentation_animator.set_enabled(true);
    let window_id = state
        .window_id_for_surface(surface_id)
        .expect("test window");
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group node");
    let normal_rect = state
        .presentation_rect_for_geometry(surface_id, restore_geometry)
        .expect("normal presentation rect");
    state.publish_presented_window_geometry(
        1,
        PresentedWindowGeometry::with_scene_node(scene_node_id, surface_id, normal_rect),
    );

    let enter_started_at = AnimationTime::from_nanos(1_000_000_000);
    state.layout_animation_epoch = Some(enter_started_at);
    assert!(state.set_root_window_mode(surface_id, ToplevelMode::Maximized));
    let enter_sample_at = AnimationTime::from_nanos(enter_started_at.as_nanos() + 20_000_000);
    let enter_scene = state.canonical_presentation_scene();
    let enter_targets = state.native_frame_presentation_targets_for_scene(&enter_scene);
    let enter_sample = state.presentation_scene_sample_for_targets_at_with_source(
        enter_sample_at,
        crate::presentation_animation::PresentationSampleTimeSource::ScheduledTarget,
        &enter_targets,
    );
    let enter_snapshot = crate::presentation_animation::PresentationFrameSnapshot::
        from_sample_with_presented_windows(
            &enter_sample,
            state.presented_window_geometries_for_targets(&enter_sample, &enter_targets),
        );
    let enter_transform = enter_snapshot
        .transform_for_root(surface_id)
        .expect("in-flight MaximizeEnter sample");
    assert_ne!(
        enter_transform.presented_rect,
        enter_transform.canonical_rect
    );
    state.publish_presented_window_geometry(
        2,
        PresentedWindowGeometry::with_scene_node(
            scene_node_id,
            surface_id,
            enter_transform.presented_rect,
        ),
    );

    state.layout_animation_epoch = Some(enter_sample_at);
    assert!(state.restore_normal_root_window(surface_id));
    let exit_started_at = state
        .presentation_animator
        .track_started_at_for_scene_node(scene_node_id)
        .expect("reversed MaximizeExit start");
    assert_eq!(exit_started_at, enter_sample_at);
    let exit_start_scene = state.canonical_presentation_scene();
    let exit_start_targets = state.native_frame_presentation_targets_for_scene(&exit_start_scene);
    let exit_start_sample = state.presentation_scene_sample_for_targets_at_with_source(
        exit_started_at,
        crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback,
        &exit_start_targets,
    );
    let exit_start_snapshot = crate::presentation_animation::PresentationFrameSnapshot::
        from_sample_with_presented_windows(
            &exit_start_sample,
            state.presented_window_geometries_for_targets(
                &exit_start_sample,
                &exit_start_targets,
            ),
        );
    assert_eq!(
        exit_start_snapshot
            .transform_for_root(surface_id)
            .expect("reversed exit frame")
            .presented_rect,
        state
            .presented_window_geometry(surface_id)
            .expect("last physically presented maximize-enter frame")
            .presented_rect(),
        "ScheduledTarget to MonotonicFallback keeps the physical animation state"
    );
    assert!(exit_start_snapshot.sampled_at >= enter_snapshot.sampled_at);

    let exit_sample_at = AnimationTime::from_nanos(exit_started_at.as_nanos() + 20_000_000);
    let exit_scene = state.canonical_presentation_scene();
    let exit_targets = state.native_frame_presentation_targets_for_scene(&exit_scene);
    let exit_sample = state.presentation_scene_sample_for_targets_at_with_source(
        exit_sample_at,
        crate::presentation_animation::PresentationSampleTimeSource::ScheduledTarget,
        &exit_targets,
    );
    let exit_snapshot = crate::presentation_animation::PresentationFrameSnapshot::
        from_sample_with_presented_windows(
            &exit_sample,
            state.presented_window_geometries_for_targets(&exit_sample, &exit_targets),
        );
    let exit_transform = exit_snapshot
        .transform_for_root(surface_id)
        .expect("in-flight MaximizeExit sample");
    assert_ne!(exit_transform.presented_rect, exit_transform.canonical_rect);
    state.publish_presented_window_geometry(
        3,
        PresentedWindowGeometry::with_scene_node(
            scene_node_id,
            surface_id,
            exit_transform.presented_rect,
        ),
    );

    state.layout_animation_epoch = Some(exit_sample_at);
    assert!(state.set_root_window_mode(surface_id, ToplevelMode::Maximized));
    let reentered_at = state
        .presentation_animator
        .track_started_at_for_scene_node(scene_node_id)
        .expect("reversed MaximizeEnter start");
    assert_eq!(reentered_at, exit_sample_at);
    let reentered_scene = state.canonical_presentation_scene();
    let reentered_targets = state.native_frame_presentation_targets_for_scene(&reentered_scene);
    let reentered_sample = state.presentation_scene_sample_for_targets_at_with_source(
        reentered_at,
        crate::presentation_animation::PresentationSampleTimeSource::MonotonicFallback,
        &reentered_targets,
    );
    let reentered_snapshot = crate::presentation_animation::PresentationFrameSnapshot::
        from_sample_with_presented_windows(
            &reentered_sample,
            state.presented_window_geometries_for_targets(
                &reentered_sample,
                &reentered_targets,
            ),
        );
    assert_eq!(
        reentered_snapshot
            .transform_for_root(surface_id)
            .expect("reversed enter frame")
            .presented_rect,
        state
            .presented_window_geometry(surface_id)
            .expect("last physically presented maximize-exit frame")
            .presented_rect(),
        "restore reversal starts at the last physically presented exit geometry"
    );
    assert!(reentered_snapshot.sampled_at >= exit_snapshot.sampled_at);
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
fn pointer_hit_certificate_reuses_stable_client_mapping_without_locality_scan() {
    let mut state = CompositorState::new(None);
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = std::os::unix::net::UnixStream::pair().expect("test socket");
    let client = display_handle
        .insert_client(server_end, std::sync::Arc::new(()))
        .expect("test client");
    let resource =
        state.test_create_unmapped_surface_resource_at_version(&client, &display_handle, 1);
    let surface_id = compositor_surface_id(&resource);
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
        .expect("client window");
    state.append_renderable_surface(test_surface(surface_id));
    state.rebuild_active_scene_view();
    state.set_test_effective_xdg_window_geometry(
        surface_id,
        XdgWindowGeometry::new(0, 0, SURFACE_WIDTH as i32, SURFACE_HEIGHT as i32),
    );
    state
        .assign_surface_role(surface_id, SurfaceRole::XdgToplevel)
        .expect("eligible client role");
    state.pointer_hit_instrumentation_enabled = true;

    let assignment = &state.active_scene_surface_render_assignments()[0];
    let target = assignment.target;
    let target_origin = (f64::from(target.x()), f64::from(target.y()));
    let first_point = (f64::from(target.x() + 40), f64::from(target.y() + 30));
    let first = state.pointer_scene_hit_at(first_point.0, first_point.1);
    assert!(matches!(
        first,
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == surface_id
    ));
    let after_first = state.pointer_input_metrics_snapshot();

    for step in 0..128 {
        let x = f64::from(target.x() + 20 + step % 240);
        let y = f64::from(target.y() + 15 + step % 170);
        let hit = state.pointer_scene_hit_at(x, y);
        let PointerSceneHit::Client { target } = hit else {
            panic!("stable client area should remain a client hit");
        };
        assert_eq!(compositor_surface_id(&target.surface), surface_id);
        assert!((target.surface_x - (x - target_origin.0)).abs() < 1.0e-9);
        assert!((target.surface_y - (y - target_origin.1)).abs() < 1.0e-9);
    }

    let after_motion = state.pointer_input_metrics_snapshot();
    assert_eq!(after_first.pointer_hit_certificate_builds, 1);
    assert_eq!(after_motion.pointer_hit_certificate_hits, 128);
    assert_eq!(
        after_motion.full_scene_hit_scans,
        after_first.full_scene_hit_scans
    );
    assert_eq!(
        after_motion.owner_locality_fast_hits, after_first.owner_locality_fast_hits,
        "stable motion should be served by the certificate rather than locality traversal"
    );
    assert_eq!(
        after_motion.visual_stack_groups_cache_rebuilds,
        after_first.visual_stack_groups_cache_rebuilds
    );
}

#[test]
fn pointer_hit_certificate_is_not_reused_after_interaction_generation_changes() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (surface_id, _) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    state.rebuild_active_scene_view();
    state.pointer_hit_instrumentation_enabled = true;
    let surface_index = state
        .active_scene_surface_index(surface_id)
        .expect("active client surface");
    let target = state.active_scene_surface_render_assignments()[surface_index].target;
    let first_point = (f64::from(target.x() + 80), f64::from(target.y() + 60));
    assert!(matches!(
        state.pointer_scene_hit_at(first_point.0, first_point.1),
        PointerSceneHit::Client { .. }
    ));
    let certificate_generation = state.pointer_hit_generation;
    let metrics_before_change = state.pointer_input_metrics_snapshot();
    assert_eq!(metrics_before_change.pointer_hit_certificate_builds, 1);

    state.advance_pointer_hit_generation();
    let next_point = (f64::from(target.x() + 120), f64::from(target.y() + 90));
    let hit = state.pointer_scene_hit_at(next_point.0, next_point.1);

    assert_ne!(state.pointer_hit_generation, certificate_generation);
    assert!(matches!(
        hit,
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == surface_id
    ));
    let metrics_after_change = state.pointer_input_metrics_snapshot();
    assert_eq!(metrics_after_change.pointer_hit_certificate_hits, 0);
    assert_eq!(metrics_after_change.pointer_hit_certificate_misses, 1);
    assert_eq!(metrics_after_change.pointer_hit_certificate_builds, 2);
    assert_eq!(
        metrics_after_change.full_scene_hit_scans,
        metrics_before_change.full_scene_hit_scans + 1
    );
}

#[test]
fn pointer_hit_certificate_expires_after_interaction_mutations() {
    let mut geometry_state = CompositorState::new(None);
    let geometry_client = pointer_test_wayland_client();
    let (geometry_surface_id, _) = append_pointer_test_toplevel(
        &mut geometry_state,
        &geometry_client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    prime_pointer_test_certificate(&mut geometry_state, geometry_surface_id);
    let generation = geometry_state.pointer_hit_generation;
    geometry_state.install_toplevel_visual_geometry(
        geometry_surface_id,
        WindowGeometry::new(
            SurfacePlacement::absolute_root_at(130, 125),
            SURFACE_WIDTH,
            SURFACE_HEIGHT,
        ),
    );
    assert_pointer_certificate_revalidated_after_mutation(
        &mut geometry_state,
        geometry_surface_id,
        generation,
        Some(geometry_surface_id),
    );

    let mut placement_state = CompositorState::new(None);
    let placement_client = pointer_test_wayland_client();
    let (placement_surface_id, _) = append_pointer_test_toplevel(
        &mut placement_state,
        &placement_client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    prime_pointer_test_certificate(&mut placement_state, placement_surface_id);
    let generation = placement_state.pointer_hit_generation;
    assert!(placement_state.set_surface_placement(
        placement_surface_id,
        SurfacePlacement::absolute_root_at(145, 135),
    ));
    assert_pointer_certificate_revalidated_after_mutation(
        &mut placement_state,
        placement_surface_id,
        generation,
        Some(placement_surface_id),
    );

    let mut input_state = CompositorState::new(None);
    let input_client = pointer_test_wayland_client();
    let (input_surface_id, _) = append_pointer_test_toplevel(
        &mut input_state,
        &input_client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    prime_pointer_test_certificate(&mut input_state, input_surface_id);
    let generation = input_state.pointer_hit_generation;
    let mut commit = empty_cached_subsurface_commit();
    commit.commit_sequence = SurfaceCommitSequence(1);
    commit.input_region = Some(crate::compositor::state_data::SurfaceInputRegion::Custom(
        Vec::new(),
    ));
    input_state.publish_surface_tree(input_surface_id, vec![(input_surface_id, commit)]);
    assert_pointer_certificate_revalidated_after_mutation(
        &mut input_state,
        input_surface_id,
        generation,
        None,
    );

    let mut stacking_state = CompositorState::new(None);
    let stacking_client = pointer_test_wayland_client();
    let (winner_surface_id, winner_window_id) = append_pointer_test_toplevel(
        &mut stacking_state,
        &stacking_client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    let (raised_surface_id, raised_window_id) = append_pointer_test_toplevel(
        &mut stacking_state,
        &stacking_client,
        SurfacePlacement::absolute_root_at(1_000, 1_000),
        None,
    );
    stacking_state.window_stacking = vec![raised_window_id, winner_window_id];
    prime_pointer_test_certificate(&mut stacking_state, winner_surface_id);
    let generation = stacking_state.pointer_hit_generation;
    assert!(stacking_state.raise_root_window(raised_surface_id));
    assert_pointer_certificate_revalidated_after_mutation(
        &mut stacking_state,
        winner_surface_id,
        generation,
        Some(winner_surface_id),
    );

    let mut popup_state = CompositorState::new(None);
    let popup_client = pointer_test_wayland_client();
    let (parent_surface_id, _) = append_pointer_test_toplevel(
        &mut popup_state,
        &popup_client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    prime_pointer_test_certificate(&mut popup_state, parent_surface_id);
    let generation = popup_state.pointer_hit_generation;
    append_pointer_test_popup(
        &mut popup_state,
        &popup_client,
        parent_surface_id,
        SurfacePlacement::absolute_root_at(1_000, 1_000),
    );
    popup_state.rebuild_active_scene_view();
    assert_pointer_certificate_revalidated_after_mutation(
        &mut popup_state,
        parent_surface_id,
        generation,
        Some(parent_surface_id),
    );
}

#[test]
fn pointer_hit_certificate_falls_back_at_the_client_boundary() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (surface_id, _window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    state.rebuild_active_scene_view();
    state.pointer_hit_instrumentation_enabled = true;
    let surface_index = state
        .active_scene_surface_index(surface_id)
        .expect("active client surface");
    let target = state.active_scene_surface_render_assignments()[surface_index].target;
    let inside = (f64::from(target.x() + 80), f64::from(target.y() + 60));
    assert!(matches!(
        state.pointer_scene_hit_at(inside.0, inside.1),
        PointerSceneHit::Client { .. }
    ));
    assert!(
        state
            .pointer_scene_hit_cache
            .as_ref()
            .and_then(PointerSceneHitCache::pointer_hit_certificate_safe_rect)
            .is_some(),
        "the interior client point should establish a certificate"
    );

    let boundary = (
        f64::from(target.x()) + f64::from(target.width()),
        f64::from(target.y() + 60),
    );
    let optimized = state.pointer_scene_hit_at(boundary.0, boundary.1);
    let reference = state.pointer_scene_hit_reference_at(boundary.0, boundary.1);
    assert!(matches!(optimized, PointerSceneHit::None));
    assert!(matches!(reference, PointerSceneHit::None));
    assert_eq!(state.pointer_hit_metrics.pointer_hit_certificate_misses, 1);
    assert!(state.pointer_hit_metrics.full_scene_hit_scans > 1);
}

#[test]
fn pointer_hit_certificate_clips_before_a_higher_window_envelope() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (lower_id, lower_window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    let (upper_id, upper_window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(250, 100),
        None,
    );
    state.window_stacking = vec![lower_window_id, upper_window_id];
    state.rebuild_active_scene_view();
    state.pointer_hit_instrumentation_enabled = true;
    let lower_index = state
        .active_scene_surface_index(lower_id)
        .expect("lower surface active");
    let upper_index = state
        .active_scene_surface_index(upper_id)
        .expect("upper surface active");
    let lower_target = state.active_scene_surface_render_assignments()[lower_index].target;
    let upper_target = state.active_scene_surface_render_assignments()[upper_index].target;
    let point = (
        f64::from(lower_target.x() + 80),
        f64::from(lower_target.y() + 60),
    );
    assert!(point.0 < f64::from(upper_target.x()));
    assert!(matches!(
        state.pointer_scene_hit_at(point.0, point.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == lower_id
    ));
    let safe_rect = state
        .pointer_scene_hit_cache
        .as_ref()
        .and_then(PointerSceneHitCache::pointer_hit_certificate_safe_rect)
        .expect("exposed lower region gets a certificate");
    assert_eq!(safe_rect.2, f64::from(upper_target.x()));

    let inside_clipped_cell = (f64::from(upper_target.x() - 1), point.1);
    assert!(matches!(
        state.pointer_scene_hit_at(inside_clipped_cell.0, inside_clipped_cell.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == lower_id
    ));
    let before_crossing = state.pointer_hit_metrics;
    let crossing = (f64::from(upper_target.x()), point.1);
    let optimized = state.pointer_scene_hit_at(crossing.0, crossing.1);
    let reference = state.pointer_scene_hit_reference_at(crossing.0, crossing.1);
    assert!(matches!(
        optimized,
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == upper_id
    ));
    assert_pointer_scene_hits_equal(&optimized, &reference);
    assert_eq!(
        state.pointer_hit_metrics.pointer_hit_certificate_misses,
        before_crossing.pointer_hit_certificate_misses + 1
    );
}

#[test]
fn pointer_hit_certificate_clips_before_a_higher_subsurface_target() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (parent_id, _) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    let child_id = append_pointer_test_subsurface(&mut state, &client, parent_id, 180, 20);
    state.rebuild_active_scene_view();
    state.pointer_hit_instrumentation_enabled = true;
    let parent_index = state
        .active_scene_surface_index(parent_id)
        .expect("parent active");
    let child_index = state
        .active_scene_surface_index(child_id)
        .expect("subsurface active");
    let parent_target = state.active_scene_surface_render_assignments()[parent_index].target;
    let child_target = state.active_scene_surface_render_assignments()[child_index].target;
    let point = (
        f64::from(parent_target.x() + 50),
        f64::from(parent_target.y() + 50),
    );
    assert!(point.0 < f64::from(child_target.x()));
    assert!(matches!(
        state.pointer_scene_hit_at(point.0, point.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == parent_id
    ));
    let safe_rect = state
        .pointer_scene_hit_cache
        .as_ref()
        .and_then(PointerSceneHitCache::pointer_hit_certificate_safe_rect)
        .expect("exposed parent area gets a certificate");
    assert_eq!(safe_rect.2, f64::from(child_target.x()));

    let before_crossing = state.pointer_input_metrics_snapshot();
    let crossing = (f64::from(child_target.x()), point.1);
    let optimized = state.pointer_scene_hit_at(crossing.0, crossing.1);
    let reference = state.pointer_scene_hit_reference_at(crossing.0, crossing.1);
    assert!(matches!(
        optimized,
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == child_id
    ));
    assert_pointer_scene_hits_equal(&optimized, &reference);
    assert_eq!(
        state.pointer_hit_metrics.pointer_hit_certificate_misses,
        before_crossing.pointer_hit_certificate_misses + 1
    );
}

#[test]
fn pointer_hit_certificate_clips_before_a_higher_popup_target() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (parent_id, _) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    let popup_id = append_pointer_test_popup(
        &mut state,
        &client,
        parent_id,
        SurfacePlacement::absolute_root_at(230, 100),
    );
    state.rebuild_active_scene_view();
    state.pointer_hit_instrumentation_enabled = true;
    let parent_index = state
        .active_scene_surface_index(parent_id)
        .expect("parent active");
    let popup_index = state
        .active_scene_surface_index(popup_id)
        .expect("popup active");
    let parent_target = state.active_scene_surface_render_assignments()[parent_index].target;
    let popup_target = state.active_scene_surface_render_assignments()[popup_index].target;
    let point = (
        f64::from(parent_target.x() + 50),
        f64::from(parent_target.y() + 50),
    );
    assert!(point.0 < f64::from(popup_target.x()));
    assert!(matches!(
        state.pointer_scene_hit_at(point.0, point.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == parent_id
    ));
    let safe_rect = state
        .pointer_scene_hit_cache
        .as_ref()
        .and_then(PointerSceneHitCache::pointer_hit_certificate_safe_rect)
        .expect("exposed parent area gets a certificate");
    assert_eq!(safe_rect.2, f64::from(popup_target.x()));

    let inside_clipped_cell = (f64::from(popup_target.x() - 1), point.1);
    assert!(matches!(
        state.pointer_scene_hit_at(inside_clipped_cell.0, inside_clipped_cell.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == parent_id
    ));
    let before_crossing = state.pointer_input_metrics_snapshot();
    let crossing = (f64::from(popup_target.x()), point.1);
    let optimized = state.pointer_scene_hit_at(crossing.0, crossing.1);
    let reference = state.pointer_scene_hit_reference_at(crossing.0, crossing.1);
    assert!(matches!(
        optimized,
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == popup_id
    ));
    assert_pointer_scene_hits_equal(&optimized, &reference);
    assert_eq!(
        state.pointer_hit_metrics.pointer_hit_certificate_misses,
        before_crossing.pointer_hit_certificate_misses + 1
    );
}

#[test]
fn higher_custom_input_hole_rejects_a_lower_surface_certificate() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (lower_id, lower_window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    let (upper_id, upper_window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(150, 120),
        None,
    );
    state.window_stacking = vec![lower_window_id, upper_window_id];
    state.rebuild_active_scene_view();
    set_test_committed_input_region(
        &state,
        upper_id,
        crate::compositor::state_data::SurfaceInputRegion::Custom(Vec::new()),
    );
    state.pointer_hit_instrumentation_enabled = true;
    let lower_index = state
        .active_scene_surface_index(lower_id)
        .expect("lower surface active");
    let lower_target = state.active_scene_surface_render_assignments()[lower_index].target;
    let point = (
        f64::from(lower_target.x() + 100),
        f64::from(lower_target.y() + 80),
    );
    let hit = state.pointer_scene_hit_at(point.0, point.1);
    assert!(matches!(
        hit,
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == lower_id
    ));
    let reference = state.pointer_scene_hit_reference_at(point.0, point.1);
    assert_pointer_scene_hits_equal(&hit, &reference);
    assert_eq!(state.pointer_hit_metrics.pointer_hit_certificate_builds, 0);
    assert_eq!(
        state.pointer_hit_metrics.pointer_hit_certificate_rejections, 1,
        "the upper custom-input hole is inside its broad blocker envelope"
    );
}

#[test]
fn higher_subsurface_input_hole_rejects_a_parent_certificate() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (parent_id, _window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    let child_id = append_pointer_test_subsurface(&mut state, &client, parent_id, 20, 20);
    state.rebuild_active_scene_view();
    set_test_committed_input_region(
        &state,
        child_id,
        crate::compositor::state_data::SurfaceInputRegion::Custom(Vec::new()),
    );
    state.pointer_hit_instrumentation_enabled = true;
    let parent_index = state
        .active_scene_surface_index(parent_id)
        .expect("parent surface active");
    let parent_target = state.active_scene_surface_render_assignments()[parent_index].target;
    let point = (
        f64::from(parent_target.x() + 50),
        f64::from(parent_target.y() + 50),
    );
    let hit = state.pointer_scene_hit_at(point.0, point.1);
    assert!(matches!(
        hit,
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == parent_id
    ));
    let reference = state.pointer_scene_hit_reference_at(point.0, point.1);
    assert_pointer_scene_hits_equal(&hit, &reference);
    assert_eq!(state.pointer_hit_metrics.pointer_hit_certificate_builds, 0);
    assert_eq!(
        state.pointer_hit_metrics.pointer_hit_certificate_rejections,
        1
    );
}

#[test]
fn higher_popup_input_hole_rejects_a_parent_certificate() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (parent_id, _window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    let popup_id = append_pointer_test_popup(
        &mut state,
        &client,
        parent_id,
        SurfacePlacement::absolute_root_at(150, 120),
    );
    state.rebuild_active_scene_view();
    assert!(state.active_scene_popup_surface_ids().contains(&popup_id));
    set_test_committed_input_region(
        &state,
        popup_id,
        crate::compositor::state_data::SurfaceInputRegion::Custom(Vec::new()),
    );
    state.pointer_hit_instrumentation_enabled = true;
    let parent_index = state
        .active_scene_surface_index(parent_id)
        .expect("parent surface active");
    let parent_target = state.active_scene_surface_render_assignments()[parent_index].target;
    let point = (
        f64::from(parent_target.x() + 100),
        f64::from(parent_target.y() + 80),
    );
    let hit = state.pointer_scene_hit_at(point.0, point.1);
    assert!(matches!(
        hit,
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == parent_id
    ));
    let reference = state.pointer_scene_hit_reference_at(point.0, point.1);
    assert_pointer_scene_hits_equal(&hit, &reference);
    assert_eq!(state.pointer_hit_metrics.pointer_hit_certificate_builds, 0);
    assert_eq!(
        state.pointer_hit_metrics.pointer_hit_certificate_rejections,
        1
    );
}

#[test]
fn custom_input_and_visual_aperture_targets_do_not_get_certificates() {
    let mut custom_state = CompositorState::new(None);
    let custom_client = pointer_test_wayland_client();
    let (custom_id, _custom_window_id) = append_pointer_test_toplevel(
        &mut custom_state,
        &custom_client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    custom_state.rebuild_active_scene_view();
    set_test_committed_input_region(
        &custom_state,
        custom_id,
        crate::compositor::state_data::SurfaceInputRegion::Custom(vec![
            crate::compositor::state_data::InputRegionOp::Add(
                crate::compositor::state_data::InputRegionRect::new(0, 0, 100, 100)
                    .expect("custom input rectangle"),
            ),
        ]),
    );
    let custom_index = custom_state
        .active_scene_surface_index(custom_id)
        .expect("custom surface active");
    let custom_target = custom_state.active_scene_surface_render_assignments()[custom_index].target;
    let custom_point = (
        f64::from(custom_target.x() + 20),
        f64::from(custom_target.y() + 20),
    );
    assert!(matches!(
        custom_state.pointer_scene_hit_at(custom_point.0, custom_point.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == custom_id
    ));
    assert_eq!(
        custom_state
            .pointer_hit_metrics
            .pointer_hit_certificate_builds,
        0
    );
    assert_eq!(
        custom_state
            .pointer_hit_metrics
            .pointer_hit_certificate_rejections,
        1
    );

    let mut clipped_state = CompositorState::new(None);
    let clipped_client = pointer_test_wayland_client();
    let aperture = crate::compositor::render::SurfaceVisualAperture::logical_only(
        render::SurfaceTargetRect::new(0, 0, 150, 200),
    );
    let (clipped_id, _clipped_window_id) = append_pointer_test_toplevel(
        &mut clipped_state,
        &clipped_client,
        SurfacePlacement::root(),
        Some(aperture),
    );
    clipped_state.rebuild_active_scene_view();
    let clipped_index = clipped_state
        .active_scene_surface_index(clipped_id)
        .expect("clipped surface active");
    let clipped_target =
        clipped_state.active_scene_surface_render_assignments()[clipped_index].target;
    let clipped_point = (
        f64::from(clipped_target.x() + 30),
        f64::from(clipped_target.y() + 30),
    );
    assert!(matches!(
        clipped_state.pointer_scene_hit_at(clipped_point.0, clipped_point.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == clipped_id
    ));
    assert_eq!(
        clipped_state
            .pointer_hit_metrics
            .pointer_hit_certificate_builds,
        0
    );
    assert_eq!(
        clipped_state
            .pointer_hit_metrics
            .pointer_hit_certificate_rejections,
        1
    );
}

#[test]
fn server_decoration_complexity_rejects_client_certificates() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (surface_id, _) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    state.xdg_decoration_states.insert(
        surface_id,
        WindowDecorationState::with_published_mode(DecorationMode::ServerSide),
    );
    state.rebuild_active_scene_view();
    let surface_index = state
        .active_scene_surface_index(surface_id)
        .expect("active decorated surface");
    let target = state.active_scene_surface_render_assignments()[surface_index].target;
    let point = (f64::from(target.x() + 80), f64::from(target.y() + 60));
    assert!(matches!(
        state.pointer_scene_hit_at(point.0, point.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == surface_id
    ));
    assert_eq!(state.pointer_hit_metrics.pointer_hit_certificate_builds, 0);
    assert_eq!(
        state.pointer_hit_metrics.pointer_hit_certificate_rejections, 1,
        "the server-decoration resize envelope overlaps the client area"
    );
}

#[test]
fn pointer_hit_certificate_survives_identical_physical_frame_churn() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (surface_id, _window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    state.rebuild_active_scene_view();
    state.pointer_hit_instrumentation_enabled = true;
    let output_id = state.native_output_id().expect("test output id");
    let scene_node_id = state
        .scene_node_id_for_window_group(
            state
                .window_id_for_surface(surface_id)
                .expect("test window"),
        )
        .expect("test scene node");
    let canonical_rect = state
        .current_presentation_rect_for_root(surface_id)
        .expect("canonical presentation rectangle");
    let presented_rect = PresentationRect::new(
        canonical_rect.x() + 400.0,
        canonical_rect.y() + 300.0,
        canonical_rect.width(),
        canonical_rect.height(),
    )
    .expect("translated physical rectangle");
    let sample = PresentationSceneSample::empty_for_output(
        output_id,
        AnimationTime::from_nanos(1_000_000),
        PresentationSampleTimeSource::MonotonicFallback,
    );
    let initial_presentation = PresentationFrameSnapshot::from_sample_with_presented_windows(
        &sample,
        vec![PresentedWindowGeometry::with_scene_node(
            scene_node_id,
            surface_id,
            presented_rect,
        )],
    );
    state.publish_presented_presentation(1, &initial_presentation);
    let transform = PresentationGeometryTransform::new(canonical_rect, presented_rect);
    let initial_point = transform.map_point((canonical_rect.x() + 50.0, canonical_rect.y() + 50.0));
    assert!(matches!(
        state.pointer_scene_hit_at(initial_point.0, initial_point.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == surface_id
    ));
    let pointer_generation = state.pointer_hit_generation;
    let topology_generation = state.interaction_topology_generation;
    let before_churn = state.pointer_input_metrics_snapshot();
    assert_eq!(before_churn.pointer_hit_certificate_builds, 1);

    for frame_id in 2..=12 {
        let node = if frame_id % 2 == 0 {
            SceneNodeId::from_raw(u64::from(frame_id) + 20_000).expect("replacement node")
        } else {
            scene_node_id
        };
        let sample = PresentationSceneSample::empty_for_output(
            output_id,
            AnimationTime::from_nanos(frame_id * 1_000_000),
            PresentationSampleTimeSource::MonotonicFallback,
        );
        let mut presentation = PresentationFrameSnapshot::from_sample_with_presented_windows(
            &sample,
            vec![PresentedWindowGeometry::with_scene_node(
                node,
                surface_id,
                presented_rect,
            )],
        );
        presentation.signature = frame_id;
        state.publish_presented_presentation(frame_id, &presentation);

        let point = (
            presented_rect.x() + 25.0 + (frame_id * 7) as f64,
            presented_rect.y() + 35.0 + (frame_id * 3) as f64,
        );
        assert!(matches!(
            state.pointer_scene_hit_at(point.0, point.1),
            PointerSceneHit::Client { ref target }
                if compositor_surface_id(&target.surface) == surface_id
        ));
    }

    let after_churn = state.pointer_input_metrics_snapshot();
    assert_eq!(state.pointer_hit_generation, pointer_generation);
    assert_eq!(state.interaction_topology_generation, topology_generation);
    assert_eq!(
        after_churn.full_scene_hit_scans,
        before_churn.full_scene_hit_scans
    );
    assert_eq!(
        after_churn.owner_locality_fast_hits,
        before_churn.owner_locality_fast_hits
    );
    assert_eq!(
        after_churn.visual_stack_groups_cache_rebuilds,
        before_churn.visual_stack_groups_cache_rebuilds
    );
    assert_eq!(after_churn.pointer_hit_certificate_hits, 11);
}

#[test]
fn pointer_hit_certificate_affine_matches_physical_presentation_mapping() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (surface_id, _window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    state.rebuild_active_scene_view();
    state.pointer_hit_instrumentation_enabled = true;
    let output_id = state.native_output_id().expect("test output id");
    let scene_node_id = state
        .scene_node_id_for_window_group(
            state
                .window_id_for_surface(surface_id)
                .expect("test window"),
        )
        .expect("test scene node");
    let canonical_rect = state
        .current_presentation_rect_for_root(surface_id)
        .expect("canonical presentation rectangle");
    let presented_rect = PresentationRect::new(
        500.25,
        320.5,
        canonical_rect.width() * 1.5,
        canonical_rect.height() * 0.75,
    )
    .expect("scaled physical rectangle");
    let sample = PresentationSceneSample::empty_for_output(
        output_id,
        AnimationTime::from_nanos(1_000_000),
        PresentationSampleTimeSource::MonotonicFallback,
    );
    let presentation = PresentationFrameSnapshot::from_sample_with_presented_windows(
        &sample,
        vec![PresentedWindowGeometry::with_scene_node(
            scene_node_id,
            surface_id,
            presented_rect,
        )],
    );
    state.publish_presented_presentation(1, &presentation);

    let surface_index = state
        .active_scene_surface_index(surface_id)
        .expect("active surface");
    let renderable = state.active_scene_surfaces()[surface_index].clone();
    let assignment = state.active_scene_surface_render_assignments()[surface_index].clone();
    let target = assignment.target;
    let transform = PresentationGeometryTransform::new(canonical_rect, presented_rect);
    let first_point = transform.map_point((f64::from(target.x() + 50), f64::from(target.y() + 40)));
    assert!(matches!(
        state.pointer_scene_hit_at(first_point.0, first_point.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == surface_id
    ));

    for step in 0..32 {
        let canonical_point = (
            f64::from(target.x() + 10 + step * 7),
            f64::from(target.y() + 15 + step % 150),
        );
        let output_point = transform.map_point(canonical_point);
        let optimized = state.pointer_scene_hit_at(output_point.0, output_point.1);
        let PointerSceneHit::Client {
            target: optimized_target,
        } = optimized
        else {
            panic!("certified transformed client point must remain a client hit");
        };
        let (canonical_x, canonical_y, _) = state
            .presentation_input_point_for_root(surface_id, output_point.0, output_point.1)
            .expect("physical point maps through current interaction projection");
        let (expected_x, expected_y) = render::surface_local_point_for_assignment(
            &renderable,
            &assignment,
            canonical_x,
            canonical_y,
        )
        .expect("mapped point belongs to the render assignment");
        assert_eq!(compositor_surface_id(&optimized_target.surface), surface_id);
        assert!((optimized_target.surface_x - expected_x).abs() <= 1.0e-6);
        assert!((optimized_target.surface_y - expected_y).abs() <= 1.0e-6);
    }
}

#[test]
fn pointer_hit_certificate_keeps_fifty_window_motion_scene_independent() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (winner_id, winner_window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    let mut stacking = vec![winner_window_id];
    for index in 0..49 {
        let surface_id = state.allocate_surface_id();
        let window_id = state.allocate_window_id().expect("extra window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
            .expect("extra window");
        let mut surface = test_surface_with_size(surface_id, SURFACE_WIDTH, SURFACE_HEIGHT);
        surface.placement = SurfacePlacement::absolute_root_at(1_000 + index * 400, 1_000);
        state.append_renderable_surface(surface);
        stacking.push(window_id);
    }
    state.window_stacking = stacking;
    state.rebuild_active_scene_view();
    state.pointer_hit_instrumentation_enabled = true;
    let winner_index = state
        .active_scene_surface_index(winner_id)
        .expect("winner active");
    let target = state.active_scene_surface_render_assignments()[winner_index].target;
    let first = (f64::from(target.x() + 20), f64::from(target.y() + 20));
    assert!(matches!(
        state.pointer_scene_hit_at(first.0, first.1),
        PointerSceneHit::Client { ref target }
            if compositor_surface_id(&target.surface) == winner_id
    ));
    assert_eq!(state.visual_stack_groups_cache.len(), 50);
    let before_motion = state.pointer_input_metrics_snapshot();

    for step in 0..2_000 {
        let x = f64::from(target.x() + 1 + step % 250);
        let y = f64::from(target.y() + 1 + step / 250);
        let hit = state.pointer_scene_hit_at(x, y);
        assert!(matches!(
            hit,
            PointerSceneHit::Client { ref target }
                if compositor_surface_id(&target.surface) == winner_id
        ));
    }

    let after_motion = state.pointer_input_metrics_snapshot();
    assert_eq!(
        after_motion.full_scene_hit_scans,
        before_motion.full_scene_hit_scans
    );
    assert_eq!(
        after_motion.owner_locality_fast_hits,
        before_motion.owner_locality_fast_hits
    );
    assert_eq!(
        after_motion.visual_stack_groups_cache_rebuilds,
        before_motion.visual_stack_groups_cache_rebuilds
    );
    assert_eq!(
        after_motion.pointer_hit_certificate_hits - before_motion.pointer_hit_certificate_hits,
        2_000
    );
}

#[test]
fn pointer_hit_certificate_matches_exact_reference_over_overlapping_scene_grid() {
    let mut state = CompositorState::new(None);
    let client = pointer_test_wayland_client();
    let (lower_id, lower_window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    let (upper_id, upper_window_id) = append_pointer_test_toplevel(
        &mut state,
        &client,
        SurfacePlacement::absolute_root_at(200, 150),
        None,
    );
    state.window_stacking = vec![lower_window_id, upper_window_id];
    state.rebuild_active_scene_view();
    set_test_committed_input_region(
        &state,
        upper_id,
        crate::compositor::state_data::SurfaceInputRegion::Custom(vec![
            crate::compositor::state_data::InputRegionOp::Add(
                crate::compositor::state_data::InputRegionRect::new(0, 0, 75, 200)
                    .expect("partial upper input rectangle"),
            ),
        ]),
    );
    let lower_index = state
        .active_scene_surface_index(lower_id)
        .expect("lower surface active");
    let upper_index = state
        .active_scene_surface_index(upper_id)
        .expect("upper surface active");
    let lower_target = state.active_scene_surface_render_assignments()[lower_index].target;
    let upper_target = state.active_scene_surface_render_assignments()[upper_index].target;
    let left = lower_target.x() - 2;
    let top = lower_target.y() - 2;
    let right = upper_target.x() + upper_target.width() as i32 + 2;
    let bottom = upper_target.y() + upper_target.height() as i32 + 2;

    for y in (top..=bottom).step_by(17) {
        for x in (left..=right).step_by(19) {
            let point = (f64::from(x), f64::from(y));
            let optimized = state.pointer_scene_hit_at(point.0, point.1);
            let reference = state.pointer_scene_hit_reference_at(point.0, point.1);
            assert_pointer_scene_hits_equal(&optimized, &reference);
        }
    }

    let mut nested_state = CompositorState::new(None);
    let nested_client = pointer_test_wayland_client();
    let (parent_id, _) = append_pointer_test_toplevel(
        &mut nested_state,
        &nested_client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    append_pointer_test_subsurface(&mut nested_state, &nested_client, parent_id, 35, 25);
    let popup_id = append_pointer_test_popup(
        &mut nested_state,
        &nested_client,
        parent_id,
        SurfacePlacement::absolute_root_at(190, 150),
    );
    nested_state.rebuild_active_scene_view();
    let parent_index = nested_state
        .active_scene_surface_index(parent_id)
        .expect("nested parent active");
    let popup_index = nested_state
        .active_scene_surface_index(popup_id)
        .expect("nested popup active");
    let parent_target = nested_state.active_scene_surface_render_assignments()[parent_index].target;
    let popup_target = nested_state.active_scene_surface_render_assignments()[popup_index].target;
    let nested_left = parent_target.x().min(popup_target.x()) - 4;
    let nested_top = parent_target.y().min(popup_target.y()) - 4;
    let nested_right = (parent_target.x() + parent_target.width() as i32)
        .max(popup_target.x() + popup_target.width() as i32)
        + 4;
    let nested_bottom = (parent_target.y() + parent_target.height() as i32)
        .max(popup_target.y() + popup_target.height() as i32)
        + 4;
    for y in (nested_top..=nested_bottom).step_by(23) {
        for x in (nested_left..=nested_right).step_by(29) {
            let point = (f64::from(x), f64::from(y));
            let optimized = nested_state.pointer_scene_hit_at(point.0, point.1);
            let reference = nested_state.pointer_scene_hit_reference_at(point.0, point.1);
            assert_pointer_scene_hits_equal(&optimized, &reference);
        }
    }

    let mut decorated = xdg_state(
        test_surface(42),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let origin = decorated.active_scene_surface_origins()[0];
    for y in (origin.1 - 20..origin.1 + SURFACE_HEIGHT as i32 + 10).step_by(13) {
        for x in (origin.0 - 10..origin.0 + SURFACE_WIDTH as i32 + 10).step_by(17) {
            let optimized = decorated.pointer_scene_hit_at(f64::from(x), f64::from(y));
            let reference = decorated.pointer_scene_hit_reference_at(f64::from(x), f64::from(y));
            assert_pointer_scene_hits_equal(&optimized, &reference);
        }
    }

    let mut physical_state = CompositorState::new(None);
    let physical_client = pointer_test_wayland_client();
    let (physical_surface_id, _) = append_pointer_test_toplevel(
        &mut physical_state,
        &physical_client,
        SurfacePlacement::absolute_root_at(100, 100),
        None,
    );
    physical_state.rebuild_active_scene_view();
    let output_id = physical_state.native_output_id().expect("test output");
    let scene_node_id = physical_state
        .scene_node_id_for_window_group(
            physical_state
                .window_id_for_surface(physical_surface_id)
                .expect("physical test window"),
        )
        .expect("physical test scene node");
    let canonical_rect = physical_state
        .current_presentation_rect_for_root(physical_surface_id)
        .expect("physical canonical rectangle");
    let presented_rect = PresentationRect::new(
        470.5,
        290.25,
        canonical_rect.width() * 1.25,
        canonical_rect.height() * 0.8,
    )
    .expect("physical output rectangle");
    let sample = PresentationSceneSample::empty_for_output(
        output_id,
        AnimationTime::from_nanos(1),
        PresentationSampleTimeSource::MonotonicFallback,
    );
    let presentation = PresentationFrameSnapshot::from_sample_with_presented_windows(
        &sample,
        vec![PresentedWindowGeometry::with_scene_node(
            scene_node_id,
            physical_surface_id,
            presented_rect,
        )],
    );
    physical_state.publish_presented_presentation(1, &presentation);
    let physical_transform = PresentationGeometryTransform::new(canonical_rect, presented_rect);
    let physical_index = physical_state
        .active_scene_surface_index(physical_surface_id)
        .expect("physical surface active");
    let physical_target =
        physical_state.active_scene_surface_render_assignments()[physical_index].target;
    for y in (-3..=SURFACE_HEIGHT as i32 + 3).step_by(19) {
        for x in (-3..=SURFACE_WIDTH as i32 + 3).step_by(23) {
            let canonical_point = (
                f64::from(physical_target.x()) + f64::from(x),
                f64::from(physical_target.y()) + f64::from(y),
            );
            let output_point = physical_transform.map_point(canonical_point);
            let optimized = physical_state.pointer_scene_hit_at(output_point.0, output_point.1);
            let reference =
                physical_state.pointer_scene_hit_reference_at(output_point.0, output_point.1);
            assert_pointer_scene_hits_equal(&optimized, &reference);
        }
    }
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
fn forced_xdg_server_decoration_without_protocol_object_renders_and_hits_titlebar() {
    let surface = test_surface(450);
    let surface_id = surface.surface_id;
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
        .expect("insert XDG window");
    state.append_renderable_surface(surface);
    state.rebuild_active_scene_view();
    let client_geometry_before = state.current_root_window_geometry(surface_id);

    assert_eq!(
        state.effective_window_decoration_mode(window_id),
        DecorationMode::ClientSide
    );
    assert!(decoration_instances(&state).is_empty());
    assert!(!state.xdg_decoration_resources.contains_key(&surface_id));
    assert!(
        state
            .decoration_hit_for_root_at(surface_id, (0, 0), 100.0, -10.0)
            .is_none()
    );

    state.set_window_decoration_policy(window_id, WindowDecorationPolicy::Server);

    assert_eq!(
        state.effective_window_decoration_mode(window_id),
        DecorationMode::ServerSide
    );
    assert!(state.surface_uses_server_side_decorations(surface_id, ToplevelMode::Normal));
    let instances = decoration_instances(&state);
    assert_eq!(instances.len(), 1);
    assert_eq!(
        instances[0].plan.layout.titlebar.bottom(),
        instances[0].plan.layout.client.y
    );
    assert!(instances[0].plan.layout.titlebar.height > 0);
    assert_eq!(
        state.current_root_window_geometry(surface_id),
        client_geometry_before
    );
    assert!(matches!(
        state.decoration_hit_for_root_at(surface_id, (0, 0), 100.0, -10.0),
        Some(DecorationHit::Titlebar)
    ));
}

#[test]
fn forced_xdg_ssd_is_in_presentation_coverage_and_blocks_direct_scanout() {
    let surface_id = 454;
    let mut state = CompositorState::new(None);
    let surface = super::desktop_window_tests::x11_scanout_surface(
        surface_id,
        state.output_size.width,
        state.output_size.height,
        super::super::SurfacePlacement::absolute_root_at(0, 0),
        crate::render_backend::buffer::DrmFormat::Xrgb8888,
    );
    let mut surface = surface;
    surface.render_backend = super::super::SurfaceRenderBackend::NativeWayland;
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
        .expect("insert XDG window");
    assert!(state.set_window_decoration_policy(window_id, WindowDecorationPolicy::Server));
    state.append_renderable_surface(surface);
    state.surface_presentation_generations.insert(surface_id, 1);
    state.rebuild_active_scene_view();

    let analysis = state.direct_scanout_scene_analysis();
    assert!(analysis.coverage.visible_content_above.contains(
        &super::super::PresentationCoverageContent {
            root_surface_id: surface_id,
            kind: super::super::PresentationCoverageContentKind::ServerSideDecoration,
        }
    ));
    assert!(
        analysis
            .blockers
            .reasons()
            .contains(&super::super::DirectScanoutSceneRejection::ServerSideDecorationVisible)
    );
    assert!(analysis.candidate.is_none());
}

#[test]
fn forced_server_destroy_recreate_keeps_the_effective_mode_generation_bound() {
    let surface_id = 452;
    let mut state = xdg_state(
        test_surface(surface_id),
        DecorationPreference::ServerSide,
        ToplevelMode::Normal,
    );
    let window_id = state.window_id_for_surface(surface_id).expect("window id");
    assert!(state.set_window_decoration_policy(window_id, WindowDecorationPolicy::Server));
    assert_eq!(
        state.effective_window_decoration_mode(window_id),
        DecorationMode::ServerSide
    );

    let decoration_state = state
        .xdg_decoration_states
        .get_mut(&surface_id)
        .expect("decoration state");
    let old_generation = decoration_state
        .current_generation()
        .expect("live decoration generation");
    assert!(decoration_state.destroy_object(old_generation));
    let commit_sequence = super::super::SurfaceCommitSequence(700);
    let captured = state.capture_xdg_decoration_commit_state(surface_id, commit_sequence);
    assert!(captured.is_some());

    let baseline = state.effective_window_decoration_mode(window_id);
    let new_generation = state
        .xdg_decoration_states
        .get_mut(&surface_id)
        .expect("decoration state")
        .recreate_object_with_published_mode(baseline);
    assert_ne!(old_generation, new_generation);
    assert_eq!(
        state.effective_window_decoration_mode(window_id),
        DecorationMode::ServerSide
    );

    let generation_before_publish = state.scene_render_generation;
    assert!(!state.apply_captured_xdg_decoration(surface_id, commit_sequence, captured));
    assert_eq!(
        state.effective_window_decoration_mode(window_id),
        DecorationMode::ServerSide
    );
    assert_eq!(state.scene_render_generation, generation_before_publish);
    assert_eq!(
        state
            .xdg_decoration_mode_for_configure(surface_id)
            .expect("forced configure")
            .mode,
        DecorationMode::ServerSide
    );
}

#[test]
fn forced_xdg_decoration_disappears_in_fullscreen_and_returns_afterward() {
    let surface = test_surface(451);
    let surface_id = surface.surface_id;
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
        .expect("insert XDG window");
    state.append_renderable_surface(surface);
    state.rebuild_active_scene_view();
    state.set_window_decoration_policy(window_id, WindowDecorationPolicy::Server);

    state
        .window_mut(window_id)
        .expect("XDG window")
        .state
        .set_mode(ToplevelMode::Fullscreen);
    assert_eq!(
        state.effective_window_decoration_mode(window_id),
        DecorationMode::None
    );
    assert!(decoration_instances(&state).is_empty());
    assert!(
        state
            .decoration_hit_for_root_at(surface_id, (0, 0), 100.0, -10.0)
            .is_none()
    );

    state
        .window_mut(window_id)
        .expect("XDG window")
        .state
        .set_mode(ToplevelMode::Normal);
    assert_eq!(
        state.effective_window_decoration_mode(window_id),
        DecorationMode::ServerSide
    );
    assert_eq!(decoration_instances(&state).len(), 1);
}

#[test]
fn forced_server_policy_keeps_tiled_minimal_chrome() {
    let surface = test_surface(452);
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface.surface_id))
        .expect("insert XDG window");
    let management = state
        .window(window_id)
        .and_then(|window| window.management)
        .expect("inserted windows get management");
    state.window_mut(window_id).expect("XDG window").management =
        Some(management.with_layout(crate::wm::LayoutMembership::Tiled));
    state.append_renderable_surface(surface);
    state.rebuild_active_scene_view();

    state.set_window_decoration_policy(window_id, WindowDecorationPolicy::Server);

    assert_eq!(
        state
            .window(window_id)
            .and_then(|window| window.management)
            .expect("window management")
            .chrome_policy(),
        crate::wm::WindowChromePolicy::Minimal
    );
    assert_eq!(
        state.effective_window_decoration_mode(window_id),
        DecorationMode::ServerSide
    );
}

#[test]
fn x11_server_policy_overrides_client_hints_but_not_role_or_fullscreen() {
    let mut state = x11_state(test_surface(453));
    let handle = x11_test_handle();
    let window_id = state.window_id_for_x11_handle(handle).expect("X11 window");
    let window = state.window_mut(window_id).expect("X11 window");
    window.x11_decoration_hints = X11DecorationHints {
        motif: X11MotifDecorationHint::Undecorated,
        gtk_frame_extents: Some(X11FrameExtents {
            left: 4,
            right: 4,
            top: 24,
            bottom: 4,
        }),
    };

    assert_eq!(
        state.x11_effective_decoration_mode(handle),
        DecorationMode::ClientSide
    );
    state.set_window_decoration_policy(window_id, WindowDecorationPolicy::Server);
    assert_eq!(
        state.x11_effective_decoration_mode(handle),
        DecorationMode::ServerSide
    );
    assert_eq!(state.x11_decoration_frame_extents(handle), [0, 0, 26, 0]);
    assert_eq!(decoration_instances(&state).len(), 1);
    assert!(matches!(
        state.decoration_hit_for_root_at(453, (0, 0), 100.0, -10.0),
        Some(DecorationHit::Titlebar)
    ));

    state
        .window_mut(window_id)
        .expect("X11 window")
        .state
        .set_mode(ToplevelMode::Fullscreen);
    assert_eq!(
        state.x11_effective_decoration_mode(handle),
        DecorationMode::None
    );
    assert_eq!(state.x11_decoration_frame_extents(handle), [0; 4]);
    assert!(decoration_instances(&state).is_empty());

    state
        .window_mut(window_id)
        .expect("X11 window")
        .state
        .set_mode(ToplevelMode::Normal);
    state.window_mut(window_id).expect("X11 window").x11_role =
        Some(crate::compositor::desktop_window::X11DesktopRole::AuxiliaryPopup);
    assert_eq!(
        state.x11_effective_decoration_mode(handle),
        DecorationMode::None
    );
    assert_eq!(state.x11_decoration_frame_extents(handle), [0; 4]);
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
