use super::*;
use crate::animation_control::AnimationEffect;
use crate::compositor::decoration::types::{DecorationMode, DecorationPreference};
use crate::render_backend::buffer::{BufferIdAllocator, BufferSize, CommittedSurfaceBuffer};
use crate::window_lifecycle_animation::{LifecycleDirection, LifecycleVisualGroup};

fn rect(x: f64, y: f64, width: f64, height: f64) -> PresentationRect {
    PresentationRect::new(x, y, width, height).expect("test presentation rect")
}

fn visual_group(visual_rect: PresentationRect) -> LifecycleVisualGroup {
    LifecycleVisualGroup::from_bounds(
        rect(400.0, 100.0, 800.0, 600.0),
        visual_rect,
        rect(200.0, 160.0, 960.0, 720.0),
        rect(1500.0, 500.0, 64.0, 64.0),
        1920,
        1080,
    )
    .expect("valid test visual group")
}

fn ssd_test_surface(surface_id: u32) -> RenderableSurface {
    let buffer_id = BufferIdAllocator::default()
        .allocate()
        .expect("test buffer identity");
    RenderableSurface {
        surface_id,
        x: 0,
        y: 0,
        width: 300,
        height: 200,
        placement: SurfacePlacement::root(),
        render_backend: SurfaceRenderBackend::NativeWayland,
        render_placement: None,
        visual_clip: None,
        render_target_size: None,
        generation: 1,
        commit_sequence: SurfaceCommitSequence::initial(),
        buffer: CommittedSurfaceBuffer::shm_snapshot(
            buffer_id,
            BufferSize::new(300, 200).expect("test buffer size"),
            vec![0xff12_3456; 300 * 200],
        ),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
        damage: RenderableSurfaceDamage::Full,
    }
}

fn ssd_test_state(surface_id: u32) -> (CompositorState, WindowId) {
    let mut state = CompositorState::new(None);
    let window_id = state.allocate_window_id().expect("test window id");
    state
        .insert_desktop_window(DesktopWindow::new_xdg(window_id, surface_id))
        .expect("insert test XDG window");
    let mut decoration_state = WindowDecorationState::new();
    decoration_state.set_preference(DecorationPreference::ServerSide);
    decoration_state.apply_configured_mode(DecorationMode::ServerSide);
    state
        .xdg_decoration_states
        .insert(surface_id, decoration_state);
    state.append_renderable_surface(ssd_test_surface(surface_id));
    state.surface_presentation_generations.insert(surface_id, 1);
    state.rebuild_active_scene_view();
    (state, window_id)
}

#[test]
fn active_lamp_geometry_stays_frozen_when_live_subsurface_moves() {
    let root_surface_id = 392;
    let (mut state, window_id) = ssd_test_state(root_surface_id);
    state.lifecycle_animation_renderer_available = Some(true);

    let mut child = ssd_test_surface(393);
    child.width = 64;
    child.height = 64;
    child.placement = SurfacePlacement::subsurface(root_surface_id, 16, 24);
    state.append_renderable_surface(child);
    state.surface_placements.insert(
        root_surface_id + 1,
        SurfacePlacement::subsurface(root_surface_id, 16, 24),
    );
    state
        .surface_presentation_generations
        .insert(root_surface_id + 1, 1);
    state.rebuild_active_scene_view();

    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    assert_eq!(
        state.lifecycle_effect(LifecycleDirection::Minimize),
        AnimationEffect::MinimizeLamp
    );
    state.begin_lifecycle_minimize(
        window_id,
        root_surface_id,
        Some(rect(384.0, 60.0, 832.0, 640.0)),
        Some(rect(384.0, 60.0, 832.0, 640.0)),
        Some(group),
        ResolvedEffectScene::default(),
        Vec::new(),
    );
    let initial = state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("sample"));
    assert_eq!(initial.lamps.len(), 1);
    assert_eq!(
        initial.visual_sources[0].kind,
        crate::window_lifecycle_animation::LifecycleVisualSourceKind::NoOwnedEffects
    );
    let captured_surfaces = state.lifecycle_renderable_surfaces(&initial);
    let captured_targets =
        crate::compositor::surface_render_space_assignments(&captured_surfaces, 1.0)
            .into_iter()
            .map(|assignment| assignment.target)
            .collect::<Vec<_>>();

    let live_placement = SurfacePlacement::subsurface(root_surface_id, -48, 72);
    state
        .renderable_surfaces
        .iter_mut()
        .find(|surface| surface.surface_id == root_surface_id + 1)
        .expect("live child surface")
        .placement = live_placement;

    let sample = state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("sample"));
    let projected = state.lifecycle_renderable_surfaces(&sample);
    let projected_targets = crate::compositor::surface_render_space_assignments(&projected, 1.0)
        .into_iter()
        .map(|assignment| assignment.target)
        .collect::<Vec<_>>();
    assert_eq!(projected_targets, captured_targets);
    let projected_child = projected
        .iter()
        .find(|surface| surface.surface_id == root_surface_id + 1)
        .expect("captured child remains projected");
    assert_eq!(
        projected_child.placement,
        SurfacePlacement::subsurface(root_surface_id, 16, 24)
    );
    assert_eq!(
        state
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == root_surface_id + 1)
            .expect("canonical child remains live")
            .placement,
        live_placement
    );
}

#[test]
fn resolved_effect_lifecycle_source_uses_the_captured_surface_topology() {
    let root_surface_id = 396;
    let child_surface_id = 397;
    let (mut state, window_id) = ssd_test_state(root_surface_id);
    state.lifecycle_animation_renderer_available = Some(true);
    let mut child = ssd_test_surface(child_surface_id);
    child.width = 64;
    child.height = 64;
    child.placement = SurfacePlacement::subsurface(root_surface_id, 16, 24);
    state.append_renderable_surface(child);
    state.surface_placements.insert(
        child_surface_id,
        SurfacePlacement::subsurface(root_surface_id, 16, 24),
    );
    state
        .surface_presentation_generations
        .insert(child_surface_id, 1);
    state.rebuild_active_scene_view();

    let anchor = crate::compositor::EffectAnchor::ReplaceSurface(root_surface_id);
    let target_bounds = crate::effects::EffectRect::new(0, 0, 16, 16).expect("effect bounds");
    let region = crate::effects::EffectRegion::from_rect(target_bounds);
    let effect = crate::compositor::ResolvedEffectInstance {
        id: crate::effects::EffectInstanceId::new(1).expect("effect instance ID"),
        program: crate::effects::EffectProgramId::new(1).expect("effect program ID"),
        anchor,
        region,
        target_bounds,
        parameter_block: crate::effects::EffectParameterBlock::default(),
        signature: 1,
        frame_demand: crate::effects::EffectFrameDemand::OnDamage,
        visual_group: None,
        anchor_scope: crate::compositor::EffectAnchorScope::Surface,
        scene_order: crate::compositor::EffectSceneOrder::for_anchor(anchor),
    };
    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    state.begin_lifecycle_minimize(
        window_id,
        root_surface_id,
        Some(rect(384.0, 60.0, 832.0, 640.0)),
        Some(rect(384.0, 60.0, 832.0, 640.0)),
        Some(group),
        ResolvedEffectScene::new(1, vec![effect]),
        Vec::new(),
    );

    let new_surface_id = child_surface_id + 1;
    let mut new_surface = ssd_test_surface(new_surface_id);
    new_surface.placement = SurfacePlacement::subsurface(root_surface_id, 500, 700);
    state.append_renderable_surface(new_surface);
    state
        .surface_presentation_generations
        .insert(new_surface_id, 2);
    let live_placement = SurfacePlacement::subsurface(root_surface_id, -48, 72);
    state
        .renderable_surfaces
        .iter_mut()
        .find(|surface| surface.surface_id == child_surface_id)
        .expect("live child surface")
        .placement = live_placement;
    let sample = state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("sample"));
    assert_eq!(sample.lamps.len(), 1);
    assert_eq!(
        sample.visual_sources[0].kind,
        crate::window_lifecycle_animation::LifecycleVisualSourceKind::ResolvedOwnedEffects
    );
    let projected = state.lifecycle_renderable_surfaces(&sample);
    assert_eq!(projected.len(), 2);
    assert!(
        state
            .renderable_surfaces
            .iter()
            .any(|surface| surface.surface_id == new_surface_id)
    );
    assert!(
        !projected
            .iter()
            .any(|surface| surface.surface_id == new_surface_id)
    );
    assert_eq!(
        projected
            .iter()
            .find(|surface| surface.surface_id == child_surface_id)
            .expect("captured source child")
            .placement,
        SurfacePlacement::subsurface(root_surface_id, 16, 24)
    );
    assert_eq!(
        state
            .renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == child_surface_id)
            .expect("live child remains canonical")
            .placement,
        live_placement
    );
}

#[test]
fn minimized_commit_keeps_live_buffer_with_captured_lifecycle_topology() {
    let root_surface_id = 394;
    let child_surface_id = root_surface_id + 1;
    let (mut state, window_id) = ssd_test_state(root_surface_id);
    state.lifecycle_animation_renderer_available = Some(true);

    let mut child = ssd_test_surface(child_surface_id);
    child.width = 64;
    child.height = 64;
    child.placement = SurfacePlacement::subsurface(root_surface_id, 16, 24);
    state.append_renderable_surface(child);
    state.surface_placements.insert(
        child_surface_id,
        SurfacePlacement::subsurface(root_surface_id, 16, 24),
    );
    state
        .surface_presentation_generations
        .insert(child_surface_id, 1);
    state.rebuild_active_scene_view();

    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));
    state.begin_lifecycle_minimize(
        window_id,
        root_surface_id,
        Some(rect(384.0, 60.0, 832.0, 640.0)),
        Some(rect(384.0, 60.0, 832.0, 640.0)),
        Some(group),
        ResolvedEffectScene::default(),
        Vec::new(),
    );
    let before = state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("sample"));
    let before_payload = std::sync::Arc::clone(
        state
            .retained_lifecycle_payloads
            .get_exact(before.lamps[0].presentation_identity)
            .expect("retained topology payload"),
    );

    assert!(state.minimize_desktop_window(window_id));
    assert!(state.renderable_surfaces.is_empty());

    let size = BufferSize::new(120, 96).expect("committed buffer size");
    let mut buffer_identity_allocator = BufferIdAllocator::default();
    let _ = buffer_identity_allocator
        .allocate()
        .expect("existing test buffer identity");
    let buffer_identity = buffer_identity_allocator
        .allocate()
        .expect("new minimized commit buffer identity");
    let display = wayland_server::Display::<CompositorState>::new().expect("test display");
    let mut display_handle = display.handle();
    let (server_end, _peer) = std::os::unix::net::UnixStream::pair().expect("test client socket");
    let client = display_handle
        .insert_client(server_end, std::sync::Arc::new(()))
        .expect("test client");
    let wl_surface =
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
                surface: wl_surface.clone(),
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
            XdgToplevelData {
                surface: wl_surface,
            },
        )
        .expect("test XDG toplevel");
    state.toplevel_surfaces.insert(
        root_surface_id,
        ToplevelSurface {
            window_id,
            xdg_surface,
            toplevel,
            pending_constraints: None,
            wm_capabilities_sent: false,
        },
    );
    let buffer_data = crate::compositor::ShmBufferData {
        identity: buffer_identity.clone(),
        pool: std::sync::Arc::new(crate::compositor::ShmPoolData::new(
            std::sync::Arc::new(std::fs::File::open("/dev/null").expect("test file")),
            i32::try_from(size.width * size.height * 4).expect("test pool size"),
        )),
        offset: 0,
        width: i32::try_from(size.width).expect("buffer width"),
        height: i32::try_from(size.height).expect("buffer height"),
        stride: i32::try_from(size.width * 4).expect("buffer stride"),
        format: wayland_server::WEnum::Value(wayland_server::protocol::wl_shm::Format::Argb8888),
    };
    let resource = client
        .create_resource::<
            wayland_server::protocol::wl_buffer::WlBuffer,
            crate::compositor::ShmBufferData,
            CompositorState,
        >(&display_handle, 2, buffer_data)
        .expect("test buffer resource");
    let materialized = crate::compositor::MaterializedSurfaceBuffer {
        resource,
        data: CommittedSurfaceBuffer::shm_snapshot(
            buffer_identity.clone(),
            size,
            vec![0xffab_cdef; usize::try_from(size.width * size.height).expect("pixel count")],
        ),
        x: 0,
        y: 0,
        surface_size: Some(size),
        viewport_source: None,
        viewport_destination: None,
        buffer_scale: 1,
        commit_sequence: SurfaceCommitSequence(5),
        buffer_transform: wayland_server::protocol::wl_output::Transform::Normal,
    };

    state
        .commit_minimized_surface_buffer(
            root_surface_id,
            child_surface_id,
            &materialized,
            size,
            size.width,
            size.height,
            SurfacePlacement::subsurface(root_surface_id, -48, 72),
            99,
            RenderableSurfaceDamage::full(),
        )
        .expect("real minimized surface buffer commit");

    let minimized_child = state
        .window(window_id)
        .expect("minimized window")
        .state
        .minimized_surface(child_surface_id)
        .expect("live minimized child");
    assert_eq!((minimized_child.width, minimized_child.height), (120, 96));
    assert_eq!(
        minimized_child.placement,
        SurfacePlacement::subsurface(root_surface_id, -48, 72)
    );
    assert_eq!(minimized_child.commit_sequence, SurfaceCommitSequence(5));
    assert_eq!(minimized_child.buffer_id(), buffer_identity.id());

    let current = state.lifecycle_scene_sample_at(AnimationTime::monotonic_now().expect("sample"));
    let projected = state.lifecycle_renderable_surfaces(&current);
    let projected_child = projected
        .iter()
        .find(|surface| surface.surface_id == child_surface_id)
        .expect("retained child uses current minimized buffer");
    assert_eq!((projected_child.width, projected_child.height), (64, 64));
    assert_eq!(
        projected_child.placement,
        SurfacePlacement::subsurface(root_surface_id, 16, 24)
    );
    assert_eq!(projected_child.buffer_id(), buffer_identity.id());
    assert_eq!(projected_child.commit_sequence, SurfaceCommitSequence(5));
    assert_eq!(projected_child.generation, 99);
    assert_eq!(
        projected_child
            .cpu_pixels()
            .and_then(|pixels| pixels.first()),
        Some(&0xffab_cdef)
    );
    assert!(std::sync::Arc::ptr_eq(
        state
            .retained_lifecycle_payloads
            .get_exact(current.lamps[0].presentation_identity)
            .expect("same retained payload remains active"),
        &before_payload
    ));
}

#[test]
fn invalid_fresh_surface_snapshot_does_not_reserve_a_lifecycle_owner() {
    let root_surface_id = 398;
    let child_surface_id = root_surface_id + 1;
    let (mut state, window_id) = ssd_test_state(root_surface_id);
    state.lifecycle_animation_renderer_available = Some(true);
    let mut child = ssd_test_surface(child_surface_id);
    child.placement = SurfacePlacement::subsurface(root_surface_id, 8, 12);
    state.append_renderable_surface(child);
    state.surface_placements.insert(
        child_surface_id,
        SurfacePlacement::subsurface(root_surface_id, 8, 12),
    );
    state.rebuild_active_scene_view();
    state
        .surface_presentation_generations
        .remove(&child_surface_id);
    let scene_node_id = state
        .scene_node_id_for_window_group(window_id)
        .expect("window group scene node");
    let group = visual_group(rect(384.0, 60.0, 832.0, 640.0));

    state.begin_lifecycle_minimize(
        window_id,
        root_surface_id,
        Some(rect(384.0, 60.0, 832.0, 640.0)),
        Some(rect(384.0, 60.0, 832.0, 640.0)),
        Some(group),
        ResolvedEffectScene::default(),
        Vec::new(),
    );

    assert!(
        state
            .presentation_animator
            .active_retained_visual(
                scene_node_id,
                crate::presentation_animation::PresentationRetainedVisualKind::WindowLifecycle,
            )
            .is_none()
    );
}
