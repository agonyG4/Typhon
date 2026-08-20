use super::*;
use crate::compositor::decoration::{
    layout::DecorationLayout,
    render_plan::DecorationRenderPrimitive,
    types::{DecorationHit, DecorationMode, DecorationPreference},
};
use crate::render_backend::buffer::{BufferIdAllocator, BufferSize, CommittedSurfaceBuffer};
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
    state
        .xdg_decoration_states
        .insert(surface.surface_id, decoration_state);
    state.renderable_surfaces.push(surface);
    state
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
    state.renderable_surfaces.push(surface);
    state
}

fn decoration_instances(state: &CompositorState) -> Vec<DecorationRenderInstance> {
    state.native_decoration_render_instances(&state.renderable_surfaces)
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
    state.renderable_surfaces = vec![first_surface, second_surface];
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
    state
        .xdg_decoration_states
        .insert(rear.surface_id, rear_decoration);
    let mut front_decoration = WindowDecorationState::new();
    front_decoration.set_preference(DecorationPreference::ServerSide);
    state
        .xdg_decoration_states
        .insert(front.surface_id, front_decoration);
    state.renderable_surfaces = vec![rear, front];
    state.window_stacking = vec![rear_id, front_id];

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
    state.renderable_surfaces.push(child);

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
    assert!(metrics.pointer_scene_hit_groups_inspected > 0);
    assert!(metrics.pointer_scene_hit_surfaces_inspected > 0);
    assert_eq!(metrics.pointer_scene_hit_origin_cache_clones, 0);
    assert_eq!(metrics.pointer_scene_hit_root_linear_searches, 0);
    assert!(metrics.pointer_scene_hit_cpu_nanos > 0);
    assert!(state.pointer_scene_hit_cache.is_some());
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
    state.renderable_surfaces.clear();
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
    state.surface_window_geometries.insert(
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

    state.surface_window_geometries.insert(
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
        state.surface_window_geometries.insert(
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
        state.surface_window_geometries.insert(
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
