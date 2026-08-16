use super::*;
use crate::compositor::decoration::{
    layout::DecorationLayout,
    render_plan::DecorationRenderPrimitive,
    types::{DecorationHit, DecorationMode, DecorationPreference},
};
use crate::render_backend::buffer::{BufferIdAllocator, BufferSize, CommittedSurfaceBuffer};
use crate::xwayland::{X11WindowHandle, XwaylandGeneration};
use std::num::NonZeroU64;

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
        ToplevelMode::Floating,
    );
    let surfaces = state.renderable_surfaces.clone();
    let instances = decoration_instances(&state);
    let origins = render::surface_origins(&surfaces);
    let instance = instances.first().expect("SSD instance");

    assert_eq!(origins, vec![render::FIRST_SURFACE_OFFSET]);
    assert_eq!(instance.origin(), (71, 39));

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
        ToplevelMode::Floating,
    );

    assert_eq!(decoration_instances(&state)[0].origin(), (199, 107));

    state.renderable_surfaces[0].placement = SurfacePlacement::root_at(40, 50);
    state.renderable_surfaces[0].render_placement = None;
    assert_eq!(decoration_instances(&state)[0].origin(), (111, 89));

    state.renderable_surfaces[0].render_placement =
        Some(SurfacePlacement::absolute_root_at(300, 220));
    assert_eq!(decoration_instances(&state)[0].origin(), (299, 187));
}

#[test]
fn xwayland_ssd_render_follows_actual_frame_content_placement() {
    let mut surface = test_surface(43);
    surface.render_backend = SurfaceRenderBackend::Xwayland;
    surface.placement = SurfacePlacement::absolute_root_at(320, 180);
    let state = x11_state(surface);

    assert_eq!(decoration_instances(&state)[0].origin(), (319, 147));
}

#[test]
fn rendered_ssd_button_centers_hit_the_same_buttons() {
    let mut state = xdg_state(
        test_surface(44),
        DecorationPreference::ServerSide,
        ToplevelMode::Floating,
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
fn csd_and_fullscreen_have_no_server_decoration_instance() {
    let csd = xdg_state(
        test_surface(45),
        DecorationPreference::ClientSide,
        ToplevelMode::Floating,
    );
    assert!(decoration_instances(&csd).is_empty());

    let fullscreen = xdg_state(
        test_surface(46),
        DecorationPreference::ServerSide,
        ToplevelMode::Fullscreen,
    );
    assert!(decoration_instances(&fullscreen).is_empty());
}
