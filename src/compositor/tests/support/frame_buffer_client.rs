#![allow(unused_imports)]
use super::super::*;
use super::{
    client_setup::*, clipboard_dmabuf::*, input_client::*, locked_relative::*, output_bindings::*,
    registry_state::*, server_runtime::*, subsurface_client::*, window_ops::*,
};

pub(in crate::compositor::tests) fn assign_test_toplevel(
    globals: &wayland_client::globals::GlobalList,
    qh: &QueueHandle<RegistryTestState>,
    surface: &client_wl_surface::WlSurface,
) -> Result<(), Box<dyn std::error::Error>> {
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(qh, 1..=6, ())?;
    let xdg_surface = wm_base.get_xdg_surface(surface, qh, ());
    let _toplevel = xdg_surface.get_toplevel(qh, ());
    Ok(())
}
pub(in crate::compositor::tests) fn create_surface_with_frame_callback(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let surface = compositor.create_surface(&qh, ());
    let _callback = surface.frame(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_surface_with_buffer_frame_callback(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_surface_with_delayed_buffer_frame_callback(socket_path, commands, Duration::ZERO)
}

pub(in crate::compositor::tests) fn create_surface_with_unpresented_buffer_frame_callback(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface)?;
    let _callback = surface.frame(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    Ok(())
}

pub(in crate::compositor::tests) fn exercise_uncommitted_frame_callback_ownership(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(bool, bool), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let callback_surface = compositor.create_surface(&qh, ());
    let _callback = callback_surface.frame(&qh, ());

    let (render_surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 32, 32)?;
    render_surface.commit();
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let before_commit = state.frame_done;

    callback_surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((before_commit, state.frame_done))
}

pub(in crate::compositor::tests) fn exercise_committed_frame_callback_order(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(Vec<u32>, Vec<u32>), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, _toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, 32, 32)?;

    let a = surface.frame(&qh, ());
    let b = surface.frame(&qh, ());
    let expected_a = a.id().protocol_id();
    let expected_b = b.id().protocol_id();
    surface.commit();
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    let c = surface.frame(&qh, ());
    let expected_c = c.id().protocol_id();
    surface.damage_buffer(0, 0, 1, 1);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    Ok((
        vec![expected_a, expected_b, expected_c],
        state.frame_done_callbacks,
    ))
}

pub(in crate::compositor::tests) fn create_visible_surface_frame_callback_commit_and_present(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface)?;
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;

    let _callback = surface.frame(&qh, ());
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    assert!(!state.frame_done);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_visible_surface_frame_callback_without_commit_and_capture_protocol_only(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface)?;
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let _callback = surface.frame(&qh, ());
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    assert!(!state.frame_done);

    Ok(capture_only_pending_surface_frame_callbacks(commands))
}

pub(in crate::compositor::tests) fn create_surface_with_delayed_buffer_frame_callback(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    before_present_delay: Duration,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    assign_test_toplevel(&globals, &qh, &surface)?;
    let _callback = surface.frame(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    assert!(!state.frame_done);

    thread::sleep(before_present_delay);
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_surface_with_buffer_release(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    assert_eq!(state.buffer_release_count, 0);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn create_dmabuf_surface_then_replace_buffer(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_dmabuf_surface_then_replace_buffer_inner(socket_path, commands, false)
}

pub(in crate::compositor::tests) fn create_dmabuf_surface_then_replace_buffer_and_present_twice(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    create_dmabuf_surface_then_replace_buffer_inner(socket_path, commands, true)
}

pub(in crate::compositor::tests) fn create_dmabuf_surface_then_replace_buffer_inner(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    extra_present: bool,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff11_1111)?;
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff22_2222)?;

    let surface = compositor.create_surface(&qh, ());
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    assert_eq!(state.buffer_release_count, 0);

    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    assert_eq!(state.buffer_release_count, 0);
    if extra_present {
        commands.send(ServerCommand::PresentFrame)?;
        queue.roundtrip(&mut state)?;
    }
    Ok(state)
}

pub(in crate::compositor::tests) fn create_syncobj_dmabuf_surface_and_present(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    acquire_timeline: &DrmSyncobjTimeline,
    release_timeline: &DrmSyncobjTimeline,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd()?;
    let release_timeline_fd = release_timeline.export_timeline_fd()?;
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let first_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff44_4444)?;
    let second_buffer = create_test_dmabuf_buffer(&dmabuf, &qh, 0xff55_5555)?;

    acquire_timeline.signal_point(1)?;
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
    sync_surface.set_release_point(&sync_release_timeline, 0, 2);
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    assert!(!release_timeline.point_signaled(2)?);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;

    acquire_timeline.signal_point(3)?;
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 3);
    sync_surface.set_release_point(&sync_release_timeline, 0, 4);
    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    assert!(!release_timeline.point_signaled(2)?);

    commands.send(ServerCommand::PresentFrame)?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn capture_syncobj_resize_window_geometry_snapshots(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
    acquire_timeline: &DrmSyncobjTimeline,
    release_timeline: &DrmSyncobjTimeline,
) -> Result<ExplicitSyncWindowGeometrySnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let acquire_timeline_fd = acquire_timeline.export_timeline_fd()?;
    let release_timeline_fd = release_timeline.export_timeline_fd()?;
    let sync_acquire_timeline = syncobj.import_timeline(acquire_timeline_fd.as_fd(), &qh, ());
    let sync_release_timeline = syncobj.import_timeline(release_timeline_fd.as_fd(), &qh, ());
    let first_buffer = create_test_dmabuf_buffer_with_size(&dmabuf, &qh, 0xff44_4444, 332, 242)?;
    let second_buffer = create_test_dmabuf_buffer_with_size(&dmabuf, &qh, 0xff55_5555, 372, 272)?;

    acquire_timeline.signal_point(1)?;
    xdg_surface.set_window_geometry(16, 10, 300, 200);
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 1);
    sync_surface.set_release_point(&sync_release_timeline, 0, 2);
    surface.attach(Some(&first_buffer), 0, 0);
    surface.damage_buffer(0, 0, 332, 242);
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::BeginResize {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 200.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 120.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 240.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 150.0,
    })?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let before_blocked_commit = capture_renderable_surface_snapshot(commands);
    let before_blocked_geometry = capture_committed_window_geometry(commands);

    xdg_surface.set_window_geometry(16, 30, state.toplevel_width, state.toplevel_height);
    sync_surface.set_acquire_point(&sync_acquire_timeline, 0, 3);
    sync_surface.set_release_point(&sync_release_timeline, 0, 4);
    surface.attach(Some(&second_buffer), 0, 0);
    surface.damage_buffer(0, 0, 372, 272);
    surface.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 260.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 165.0,
    })?;
    wait_for_server_commands(commands);
    let while_acquire_blocked = capture_renderable_surface_snapshot(commands);
    let blocked_geometry = capture_committed_window_geometry(commands);

    acquire_timeline.signal_point(3)?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    let after_acquire_ready = capture_renderable_surface_snapshot(commands);
    let after_acquire_geometry = capture_committed_window_geometry(commands);

    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok(ExplicitSyncWindowGeometrySnapshots {
        before_blocked_commit,
        before_blocked_geometry,
        while_acquire_blocked,
        blocked_geometry,
        after_acquire_ready,
        after_acquire_geometry,
    })
}

pub(in crate::compositor::tests) fn create_test_dmabuf_buffer(
    dmabuf: &client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    qh: &QueueHandle<RegistryTestState>,
    pixel: u32,
) -> Result<client_wl_buffer::WlBuffer, Box<dyn std::error::Error>> {
    create_test_dmabuf_buffer_with_size(dmabuf, qh, pixel, 2, 2)
}

pub(in crate::compositor::tests) fn create_test_dmabuf_buffer_with_size(
    dmabuf: &client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    qh: &QueueHandle<RegistryTestState>,
    pixel: u32,
    width: i32,
    height: i32,
) -> Result<client_wl_buffer::WlBuffer, Box<dyn std::error::Error>> {
    let pixels = vec![pixel; usize::try_from(width.saturating_mul(height))?];
    let file = create_test_shm_file(&pixels)?;
    let stride = u32::try_from(width.saturating_mul(4))?;
    let params = dmabuf.create_params(qh, ());
    params.add(file.as_fd(), 0, 0, stride, 0, 0);
    Ok(params.create_immed(
        width,
        height,
        DRM_FORMAT_ARGB8888,
        client_zwp_linux_buffer_params_v1::Flags::empty(),
        qh,
        (),
    ))
}

pub(in crate::compositor::tests) fn test_syncobj_device() -> Option<DrmSyncobjDevice> {
    DrmSyncobjDevice::open_available()
}

pub(in crate::compositor::tests) fn create_client_toplevel_with_shm_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.buffer-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn create_client_toplevel_with_shm_damage_only_update(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let mut file = create_test_shm_file(&[0xff11_1111, 0xff22_2222, 0xff33_3333, 0xff44_4444])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.damage-only-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;

    file.seek(SeekFrom::Start(0))?;
    for pixel in [0xffaa_0000_u32, 0xff00_aa00, 0xff00_00aa, 0xffaa_aa00] {
        file.write_all(&pixel.to_ne_bytes())?;
    }
    file.flush()?;
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn create_client_toplevel_with_dmabuf_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let params = dmabuf.create_params(&qh, ());
    params.add(file.as_fd(), 0, 0, 8, 0, 0);
    let buffer = params.create_immed(
        2,
        2,
        DRM_FORMAT_ARGB8888,
        client_zwp_linux_buffer_params_v1::Flags::empty(),
        &qh,
        (),
    );
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.dmabuf-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn create_toplevel_with_resized_shm_pool_buffer(
    socket_path: &PathBuf,
    resized_pool_size: i32,
    buffer_offset: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[
        0xffff_0000,
        0xff00_ff00,
        0xff00_00ff,
        0xffff_ffff,
        0xff55_0000,
        0xff00_5500,
        0xff00_0055,
        0xff55_5555,
    ])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    pool.resize(resized_pool_size);
    let buffer = pool.create_buffer(
        buffer_offset,
        2,
        2,
        8,
        client_wl_shm::Format::Argb8888,
        &qh,
        (),
    );
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());

    toplevel.set_app_id("oblivion.shm-resize-test".to_string());
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn resize_shm_pool_to_invalid_size(
    socket_path: &PathBuf,
    resized_pool_size: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    pool.resize(resized_pool_size);
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn create_shm_pool_with_invalid_size(
    socket_path: &PathBuf,
    pool_size: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let file = create_test_shm_file(&[0xffff_0000])?;
    let _pool = shm.create_pool(file.as_fd(), pool_size, &qh, ());
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn create_client_toplevel_with_shm_then_dmabuf_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;

    let shm_file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let pool = shm.create_pool(shm_file.as_fd(), 16, &qh, ());
    let shm_buffer = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());

    let dmabuf_file = create_test_shm_file(&[0xff11_1111, 0xff22_2222, 0xff33_3333, 0xff44_4444])?;
    let params = dmabuf.create_params(&qh, ());
    params.add(dmabuf_file.as_fd(), 0, 0, 8, 0, 0);
    let dmabuf_buffer = params.create_immed(
        2,
        2,
        DRM_FORMAT_ARGB8888,
        client_zwp_linux_buffer_params_v1::Flags::empty(),
        &qh,
        (),
    );

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_app_id("oblivion.dmabuf-switch-test".to_string());

    surface.attach(Some(&shm_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;

    surface.attach(Some(&dmabuf_buffer), 0, 0);
    surface.damage_buffer(0, 0, 2, 2);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn create_client_toplevel_with_sized_shm_buffer(
    socket_path: &PathBuf,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    create_client_toplevel_with_app_id_and_sized_shm_buffer(
        socket_path,
        "oblivion.buffer-test",
        width,
        height,
    )
}

pub(in crate::compositor::tests) fn create_client_toplevel_with_app_id_and_sized_shm_buffer(
    socket_path: &PathBuf,
    app_id: &str,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
    let (surface, _xdg_surface, toplevel) =
        create_test_buffered_toplevel(&compositor, &wm_base, &shm, &qh, width, height)?;

    toplevel.set_app_id(app_id.to_string());
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    retain_live_test_connection(connection);
    Ok(())
}

pub(in crate::compositor::tests) fn create_two_live_client_toplevels_and_capture_surface_count(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<usize, Box<dyn std::error::Error>> {
    let first = LiveTestClient::connect(socket_path)?;
    first.create_toplevel("oblivion.client-one", 2, 2)?;
    let second = LiveTestClient::connect(socket_path)?;
    second.create_toplevel("oblivion.client-two", 3, 2)?;
    wait_for_server_commands(commands);
    let count = capture_renderable_surface_count(commands);
    drop((first, second));
    Ok(count)
}

pub(in crate::compositor::tests) struct LiveTestClient {
    connection: Connection,
    queue: wayland_client::EventQueue<RegistryTestState>,
    compositor: client_wl_compositor::WlCompositor,
    wm_base: client_xdg_wm_base::XdgWmBase,
    shm: client_wl_shm::WlShm,
}

impl LiveTestClient {
    pub(in crate::compositor::tests) fn connect(
        socket_path: &PathBuf,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let stream = UnixStream::connect(socket_path)?;
        let connection = Connection::from_socket(stream)?;
        let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
        let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;
        Ok(Self {
            connection,
            queue,
            compositor,
            wm_base,
            shm,
        })
    }

    fn create_toplevel(
        &self,
        app_id: &str,
        width: usize,
        height: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let _surface = self.create_toplevel_surface(app_id, width, height)?;
        Ok(())
    }

    pub(in crate::compositor::tests) fn create_toplevel_surface(
        &self,
        app_id: &str,
        width: usize,
        height: usize,
    ) -> Result<client_wl_surface::WlSurface, Box<dyn std::error::Error>> {
        let qh = self.queue.handle();
        let (surface, _xdg_surface, toplevel) = create_test_buffered_toplevel(
            &self.compositor,
            &self.wm_base,
            &self.shm,
            &qh,
            width,
            height,
        )?;
        toplevel.set_app_id(app_id.to_string());
        surface.commit();
        self.connection.flush()?;
        self.connection.roundtrip()?;
        Ok(surface)
    }

    pub(in crate::compositor::tests) fn commit_surface(
        &self,
        surface: &client_wl_surface::WlSurface,
        width: usize,
        height: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let qh = self.queue.handle();
        commit_test_buffered_surface(surface, &self.shm, &qh, width, height)?;
        self.connection.flush()?;
        self.connection.roundtrip()?;
        Ok(())
    }
}

pub(in crate::compositor::tests) fn create_test_buffered_toplevel(
    compositor: &client_wl_compositor::WlCompositor,
    wm_base: &client_xdg_wm_base::XdgWmBase,
    shm: &client_wl_shm::WlShm,
    qh: &QueueHandle<RegistryTestState>,
    width: usize,
    height: usize,
) -> Result<
    (
        client_wl_surface::WlSurface,
        client_xdg_surface::XdgSurface,
        client_xdg_toplevel::XdgToplevel,
    ),
    Box<dyn std::error::Error>,
> {
    let surface = compositor.create_surface(qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, qh, ());
    let toplevel = xdg_surface.get_toplevel(qh, ());
    attach_test_buffered_surface(&surface, shm, qh, width, height)?;
    Ok((surface, xdg_surface, toplevel))
}

pub(in crate::compositor::tests) fn commit_test_buffered_surface(
    surface: &client_wl_surface::WlSurface,
    shm: &client_wl_shm::WlShm,
    qh: &QueueHandle<RegistryTestState>,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    attach_test_buffered_surface(surface, shm, qh, width, height)?;
    surface.commit();
    Ok(())
}

pub(in crate::compositor::tests) fn attach_test_buffered_surface(
    surface: &client_wl_surface::WlSurface,
    shm: &client_wl_shm::WlShm,
    qh: &QueueHandle<RegistryTestState>,
    width: usize,
    height: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let pixels = vec![0xff20_3040; width * height];
    let file = create_test_shm_file(&pixels)?;
    let pool = shm.create_pool(file.as_fd(), (pixels.len() * 4) as i32, qh, ());
    let buffer = pool.create_buffer(
        0,
        width as i32,
        height as i32,
        (width * 4) as i32,
        client_wl_shm::Format::Argb8888,
        qh,
        (),
    );
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, width as i32, height as i32);
    Ok(())
}

pub(in crate::compositor::tests) fn create_client_toplevel_with_positioned_subsurface_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent_file = create_test_shm_file(&[0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff])?;
    let parent_pool = shm.create_pool(parent_file.as_fd(), 16, &qh, ());
    let parent_buffer =
        parent_pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Argb8888, &qh, ());

    let child_file = create_test_shm_file(&[0xffff_ffff])?;
    let child_pool = shm.create_pool(child_file.as_fd(), 4, &qh, ());
    let child_buffer =
        child_pool.create_buffer(0, 1, 1, 4, client_wl_shm::Format::Argb8888, &qh, ());

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());

    subsurface.set_position(10, 12);
    parent.attach(Some(&parent_buffer), 0, 0);
    parent.damage_buffer(0, 0, 2, 2);
    child.attach(Some(&child_buffer), 0, 0);
    child.damage_buffer(0, 0, 1, 1);
    child.commit();
    parent.commit();
    connection.flush()?;
    connection.roundtrip()?;
    retain_live_test_connection(connection);
    Ok(())
}

pub(in crate::compositor::tests) fn create_subsurface_buffer_before_parent_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());

    subsurface.set_position(0, 0);
    commit_test_buffered_surface(&child, &shm, &qh, 1, 1)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 2, 2)?;
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn capture_default_synchronized_child_before_and_after_parent_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<SynchronizedCommitSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());

    let before_child_generation = capture_render_generation(commands);
    commit_test_buffered_surface(&child, &shm, &qh, 11, 7)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let before_parent = capture_renderable_surface_snapshot(commands);
    let after_child_generation = capture_render_generation(commands);

    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent = capture_renderable_surface_snapshot(commands);
    let after_parent_generation = capture_render_generation(commands);

    Ok(SynchronizedCommitSnapshots {
        before_parent,
        after_parent,
        before_child_generation,
        after_child_generation,
        after_parent_generation,
    })
}

pub(in crate::compositor::tests) fn capture_subsurface_position_before_and_after_parent_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<
    (
        Vec<RenderableSurfaceSnapshot>,
        Vec<RenderableSurfaceSnapshot>,
    ),
    Box<dyn std::error::Error>,
> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 11, 7)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    subsurface.set_position(30, 40);
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let before_parent = capture_renderable_surface_snapshot(commands);

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_parent = capture_renderable_surface_snapshot(commands);
    Ok((before_parent, after_parent))
}

pub(in crate::compositor::tests) fn capture_multiple_synchronized_child_commits(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<MultipleSynchronizedCommitSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commit_test_buffered_surface(&child, &shm, &qh, 11, 7)?;
    commit_test_buffered_surface(&child, &shm, &qh, 13, 9)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let before_parent = capture_renderable_surface_snapshot(commands);
    let superseded_buffer_releases = state.buffer_release_count;
    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let after_parent = capture_renderable_surface_snapshot(commands);
    Ok(MultipleSynchronizedCommitSnapshots {
        before_parent,
        after_parent,
        superseded_buffer_releases,
    })
}

pub(in crate::compositor::tests) fn capture_cached_child_before_and_after_set_desync(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<
    (
        Vec<RenderableSurfaceSnapshot>,
        Vec<RenderableSurfaceSnapshot>,
    ),
    Box<dyn std::error::Error>,
> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;

    commit_test_buffered_surface(&child, &shm, &qh, 9, 7)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let before_desync = capture_renderable_surface_snapshot(commands);
    subsurface.set_desync();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_desync = capture_renderable_surface_snapshot(commands);
    Ok((before_desync, after_desync))
}

pub(in crate::compositor::tests) fn capture_effectively_synchronized_grandchild_update(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<
    (
        Vec<RenderableSurfaceSnapshot>,
        Vec<RenderableSurfaceSnapshot>,
    ),
    Box<dyn std::error::Error>,
> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let root = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&root, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _child_role = subcompositor.get_subsurface(&child, &root, &qh, ());
    let grandchild = compositor.create_surface(&qh, ());
    let grandchild_role = subcompositor.get_subsurface(&grandchild, &child, &qh, ());
    grandchild_role.set_desync();

    commit_test_buffered_surface(&grandchild, &shm, &qh, 3, 3)?;
    commit_test_buffered_surface(&child, &shm, &qh, 7, 7)?;
    commit_test_buffered_surface(&root, &shm, &qh, 20, 15)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    commit_test_buffered_surface(&grandchild, &shm, &qh, 9, 5)?;
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let before_root = capture_renderable_surface_snapshot(commands);
    root.commit();
    connection.flush()?;
    queue.roundtrip(&mut RegistryTestState::default())?;
    let after_root = capture_renderable_surface_snapshot(commands);
    Ok((before_root, after_root))
}

pub(in crate::compositor::tests) fn capture_decorated_tree_during_root_resize_commit(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<
    (
        Vec<RenderableSurfaceSnapshot>,
        Vec<RenderableSurfaceSnapshot>,
    ),
    Box<dyn std::error::Error>,
> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let root = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&root, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let titlebar = compositor.create_surface(&qh, ());
    let titlebar_role = subcompositor.get_subsurface(&titlebar, &root, &qh, ());
    titlebar_role.set_position(0, 0);
    let border = compositor.create_surface(&qh, ());
    let border_role = subcompositor.get_subsurface(&border, &root, &qh, ());
    border_role.set_position(0, 20);

    commit_test_buffered_surface(&titlebar, &shm, &qh, 300, 20)?;
    commit_test_buffered_surface(&border, &shm, &qh, 10, 180)?;
    commit_test_buffered_surface(&root, &shm, &qh, 300, 200)?;
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commands.send(ServerCommand::BeginFrameAction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 304.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 204.0,
    })?;
    commands.send(ServerCommand::UpdateInteraction {
        x: f64::from(render::FIRST_SURFACE_OFFSET.0) + 344.0,
        y: f64::from(render::FIRST_SURFACE_OFFSET.1) + 234.0,
    })?;
    commands.send(ServerCommand::PrepareFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let resized_width = usize::try_from(state.toplevel_width)?;
    let resized_height = usize::try_from(state.toplevel_height)?;
    commit_test_buffered_surface(&titlebar, &shm, &qh, resized_width, 20)?;
    commit_test_buffered_surface(&border, &shm, &qh, 10, resized_height - 20)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let before_root = capture_renderable_surface_snapshot(commands);

    commit_test_buffered_surface(&root, &shm, &qh, resized_width, resized_height)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let after_root = capture_renderable_surface_snapshot(commands);
    commands.send(ServerCommand::EndInteraction)?;
    wait_for_server_commands(commands);
    Ok((before_root, after_root))
}

pub(in crate::compositor::tests) fn capture_synchronized_child_frame_callback_lifecycle(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(bool, bool), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 20, 15)?;
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    let _callback = child.frame(&qh, ());
    child.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    let before_parent = state.frame_done;

    parent.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::PresentFrame)?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;
    Ok((before_parent, state.frame_done))
}

pub(in crate::compositor::tests) fn capture_root_commit_before_synchronized_child_update(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<RootBeforeChildSnapshots, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let root = compositor.create_surface(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let _subsurface = subcompositor.get_subsurface(&child, &root, &qh, ());
    commit_test_buffered_surface(&child, &shm, &qh, 5, 5)?;
    commit_test_buffered_surface(&root, &shm, &qh, 20, 15)?;
    connection.flush()?;
    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    commit_test_buffered_surface(&root, &shm, &qh, 30, 25)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let after_root = capture_renderable_surface_snapshot(commands);
    commit_test_buffered_surface(&child, &shm, &qh, 9, 7)?;
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let after_child_without_parent = capture_renderable_surface_snapshot(commands);
    root.commit();
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    let after_next_parent = capture_renderable_surface_snapshot(commands);
    Ok(RootBeforeChildSnapshots {
        after_root,
        after_child_without_parent,
        after_next_parent,
    })
}

pub(in crate::compositor::tests) fn create_toplevel_then_attach_null_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    commit_test_buffered_surface(&surface, &shm, &qh, 2, 2)?;
    surface.attach(None, 0, 0);
    surface.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}

pub(in crate::compositor::tests) fn create_toplevel_with_nested_subsurfaces_then_attach_null_buffer(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let subcompositor: client_wl_subcompositor::WlSubcompositor = globals.bind(&qh, 1..=1, ())?;
    let shm: client_wl_shm::WlShm = globals.bind(&qh, 1..=1, ())?;

    let parent = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&parent, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    let child = compositor.create_surface(&qh, ());
    let grandchild = compositor.create_surface(&qh, ());
    let child_subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
    let grandchild_subsurface = subcompositor.get_subsurface(&grandchild, &child, &qh, ());

    child_subsurface.set_position(10, 12);
    grandchild_subsurface.set_position(3, 4);
    commit_test_buffered_surface(&grandchild, &shm, &qh, 1, 1)?;
    commit_test_buffered_surface(&child, &shm, &qh, 1, 1)?;
    commit_test_buffered_surface(&parent, &shm, &qh, 2, 2)?;
    parent.attach(None, 0, 0);
    parent.commit();
    connection.flush()?;
    connection.roundtrip()?;
    Ok(())
}
