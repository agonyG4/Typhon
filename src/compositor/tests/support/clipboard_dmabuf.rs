#![allow(unused_imports)]
use super::super::*;
use super::{
    client_setup::*, frame_buffer_client::*, input_client::*, locked_relative::*,
    output_bindings::*, registry_state::*, server_runtime::*, subsurface_client::*, window_ops::*,
};

pub(in crate::compositor::tests) struct ClipboardDisconnectResult {
    pub(in crate::compositor::tests) target_state: RegistryTestState,
    pub(in crate::compositor::tests) clipboard_state: ClipboardStateSnapshot,
}

pub(in crate::compositor::tests) fn forward_clipboard_between_two_clients(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, RegistryTestState, String), Box<dyn std::error::Error>> {
    let source_stream = UnixStream::connect(socket_path)?;
    let source_connection = Connection::from_socket(source_stream)?;
    let (source_globals, mut source_queue) =
        registry_queue_init::<RegistryTestState>(&source_connection)?;
    let source_qh = source_queue.handle();

    let source_compositor: client_wl_compositor::WlCompositor =
        source_globals.bind(&source_qh, 1..=6, ())?;
    let source_wm_base: client_xdg_wm_base::XdgWmBase =
        source_globals.bind(&source_qh, 1..=6, ())?;
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ())?;
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ())?;
    let _source_keyboard = source_seat.get_keyboard(&source_qh, ());
    let source_data_source = source_manager.create_data_source(&source_qh, ());
    source_data_source.offer("text/plain".to_string());
    source_data_source.offer("text/html".to_string());
    let source_data_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let source_surface = source_compositor.create_surface(&source_qh, ());
    let source_xdg_surface = source_wm_base.get_xdg_surface(&source_surface, &source_qh, ());
    let _source_toplevel = source_xdg_surface.get_toplevel(&source_qh, ());
    source_surface.commit();
    source_connection.flush()?;

    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    source_queue.roundtrip(&mut source_state)?;
    let serial = source_state
        .keyboard_key_serial
        .ok_or_else(|| io::Error::other("keyboard serial was not delivered"))?;
    source_data_device.set_selection(Some(&source_data_source), serial);
    source_connection.flush()?;
    source_connection.roundtrip()?;

    let target_stream = UnixStream::connect(socket_path)?;
    let target_connection = Connection::from_socket(target_stream)?;
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection)?;
    let target_qh = target_queue.handle();

    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ())?;
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ())?;
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ())?;
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ())?;
    let _target_keyboard = target_seat.get_keyboard(&target_qh, ());
    let target_surface = target_compositor.create_surface(&target_qh, ());
    let target_xdg_surface = target_wm_base.get_xdg_surface(&target_surface, &target_qh, ());
    let _target_toplevel = target_xdg_surface.get_toplevel(&target_qh, ());
    target_surface.commit();
    target_connection.flush()?;

    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state)?;
    let _target_data_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    target_connection.flush()?;
    target_queue.roundtrip(&mut target_state)?;

    let offer = target_state
        .data_device_selection_offer
        .clone()
        .ok_or_else(|| io::Error::other("target did not receive a clipboard selection offer"))?;
    let (read_fd, write_fd) = owned_pipe()?;
    offer.receive("text/plain".to_string(), write_fd.as_fd());
    target_connection.flush()?;
    drop(write_fd);
    target_connection.roundtrip()?;
    source_queue.roundtrip(&mut source_state)?;

    let mut received = String::new();
    File::from(read_fd).read_to_string(&mut received)?;

    Ok((source_state, target_state, received))
}

pub(in crate::compositor::tests) fn disconnect_clipboard_source_after_target_offer(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<ClipboardDisconnectResult, Box<dyn std::error::Error>> {
    let source_stream = UnixStream::connect(socket_path)?;
    let source_connection = Connection::from_socket(source_stream)?;
    let (source_globals, mut source_queue) =
        registry_queue_init::<RegistryTestState>(&source_connection)?;
    let source_qh = source_queue.handle();

    let source_compositor: client_wl_compositor::WlCompositor =
        source_globals.bind(&source_qh, 1..=6, ())?;
    let source_wm_base: client_xdg_wm_base::XdgWmBase =
        source_globals.bind(&source_qh, 1..=6, ())?;
    let source_seat: client_wl_seat::WlSeat = source_globals.bind(&source_qh, 1..=7, ())?;
    let source_manager: client_wl_data_device_manager::WlDataDeviceManager =
        source_globals.bind(&source_qh, 1..=3, ())?;
    let source_keyboard = source_seat.get_keyboard(&source_qh, ());
    let source_data_source = source_manager.create_data_source(&source_qh, ());
    source_data_source.offer("text/plain".to_string());
    let source_data_device = source_manager.get_data_device(&source_seat, &source_qh, ());
    let source_surface = source_compositor.create_surface(&source_qh, ());
    let source_xdg_surface = source_wm_base.get_xdg_surface(&source_surface, &source_qh, ());
    let source_toplevel = source_xdg_surface.get_toplevel(&source_qh, ());
    source_surface.commit();
    source_connection.flush()?;

    let mut source_state = RegistryTestState::default();
    source_queue.roundtrip(&mut source_state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    source_queue.roundtrip(&mut source_state)?;
    let serial = source_state
        .keyboard_key_serial
        .ok_or_else(|| io::Error::other("keyboard serial was not delivered"))?;
    source_data_device.set_selection(Some(&source_data_source), serial);
    source_connection.flush()?;
    source_connection.roundtrip()?;

    let target_stream = UnixStream::connect(socket_path)?;
    let target_connection = Connection::from_socket(target_stream)?;
    let (target_globals, mut target_queue) =
        registry_queue_init::<RegistryTestState>(&target_connection)?;
    let target_qh = target_queue.handle();

    let target_compositor: client_wl_compositor::WlCompositor =
        target_globals.bind(&target_qh, 1..=6, ())?;
    let target_wm_base: client_xdg_wm_base::XdgWmBase =
        target_globals.bind(&target_qh, 1..=6, ())?;
    let target_seat: client_wl_seat::WlSeat = target_globals.bind(&target_qh, 1..=7, ())?;
    let target_manager: client_wl_data_device_manager::WlDataDeviceManager =
        target_globals.bind(&target_qh, 1..=3, ())?;
    let _target_keyboard = target_seat.get_keyboard(&target_qh, ());
    let target_surface = target_compositor.create_surface(&target_qh, ());
    let target_xdg_surface = target_wm_base.get_xdg_surface(&target_surface, &target_qh, ());
    let _target_toplevel = target_xdg_surface.get_toplevel(&target_qh, ());
    target_surface.commit();
    target_connection.flush()?;

    let mut target_state = RegistryTestState::default();
    target_queue.roundtrip(&mut target_state)?;
    let _target_data_device = target_manager.get_data_device(&target_seat, &target_qh, ());
    target_connection.flush()?;
    target_queue.roundtrip(&mut target_state)?;
    assert!(target_state.data_device_selection_offer.is_some());

    source_data_source.destroy();
    source_connection.flush()?;
    wait_for_server_commands(commands);
    drop(source_toplevel);
    drop(source_xdg_surface);
    drop(source_surface);
    drop(source_data_device);
    drop(source_data_source);
    drop(source_keyboard);
    drop(source_manager);
    drop(source_seat);
    drop(source_wm_base);
    drop(source_compositor);
    drop(source_globals);
    drop(source_qh);
    drop(source_queue);
    drop(source_connection);
    wait_for_server_commands(commands);
    wait_for_server_commands(commands);
    target_queue.roundtrip(&mut target_state)?;
    target_queue.roundtrip(&mut target_state)?;
    let clipboard_state = capture_clipboard_state(commands);
    Ok(ClipboardDisconnectResult {
        target_state,
        clipboard_state,
    })
}

pub(in crate::compositor::tests) fn receive_host_clipboard_from_bridge(
    socket_path: &PathBuf,
    commands: &Sender<ServerCommand>,
) -> Result<(RegistryTestState, String), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    let seat: client_wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let manager: client_wl_data_device_manager::WlDataDeviceManager =
        globals.bind(&qh, 1..=3, ())?;
    let _keyboard = seat.get_keyboard(&qh, ());
    let _device = manager.get_data_device(&seat, &qh, ());
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
    let _toplevel = xdg_surface.get_toplevel(&qh, ());
    surface.commit();
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    commands.send(ServerCommand::KeyboardKey {
        key: 30,
        pressed: true,
    })?;
    wait_for_server_commands(commands);
    queue.roundtrip(&mut state)?;

    let offer = state
        .data_device_selection_offer
        .clone()
        .ok_or_else(|| io::Error::other("target did not receive host clipboard offer"))?;
    let (read_fd, write_fd) = owned_pipe()?;
    offer.receive("text/plain".to_string(), write_fd.as_fd());
    connection.flush()?;
    drop(write_fd);
    connection.roundtrip()?;

    let mut received = String::new();
    File::from(read_fd).read_to_string(&mut received)?;
    Ok((state, received))
}

#[derive(Debug)]
pub(in crate::compositor::tests) struct ScriptedClipboardBridge {
    events: VecDeque<ClipboardBridgeEvent>,
    host_payload: &'static [u8],
    requests: Arc<Mutex<Vec<(HostClipboardOfferId, String)>>>,
}

impl ScriptedClipboardBridge {
    pub(in crate::compositor::tests) fn with_host_selection(
        offer_id: HostClipboardOfferId,
        mime_types: Vec<String>,
        host_payload: &'static [u8],
        requests: Arc<Mutex<Vec<(HostClipboardOfferId, String)>>>,
    ) -> Self {
        Self {
            events: VecDeque::from([ClipboardBridgeEvent::HostSelectionChanged {
                offer_id,
                mime_types,
            }]),
            host_payload,
            requests,
        }
    }
}

impl ClipboardBridge for ScriptedClipboardBridge {
    fn poll_events(&mut self) -> Vec<ClipboardBridgeEvent> {
        self.events.drain(..).collect()
    }

    fn request_host_data(
        &mut self,
        offer_id: HostClipboardOfferId,
        mime_type: String,
        fd: OwnedFd,
    ) -> Result<(), ClipboardBridgeError> {
        self.requests.lock().unwrap().push((offer_id, mime_type));
        File::from(fd)
            .write_all(self.host_payload)
            .map_err(|_| ClipboardBridgeError::Unavailable)
    }

    fn publish_internal_selection(
        &mut self,
        _generation: u64,
        _mime_types: Vec<String>,
    ) -> Result<(), ClipboardBridgeError> {
        Ok(())
    }

    fn clear_internal_selection(&mut self) -> Result<(), ClipboardBridgeError> {
        Ok(())
    }
}

pub(in crate::compositor::tests) fn owned_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }

    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

pub(in crate::compositor::tests) fn create_dmabuf_candidate_and_expect_created(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;

    let file = create_test_shm_file(&[0xffff_ffff, 0xffff_ffff, 0xffff_ffff, 0xffff_ffff])?;
    let params = dmabuf.create_params(&qh, ());
    params.add(file.as_fd(), 0, 0, 8, 0, 0);
    params.create(
        2,
        2,
        DRM_FORMAT_ARGB8888,
        client_zwp_linux_buffer_params_v1::Flags::empty(),
    );
    connection.flush()?;
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn request_dmabuf_default_feedback(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 = globals.bind(&qh, 4..=4, ())?;
    let _feedback = dmabuf.get_default_feedback(&qh, ());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn request_wl_drm_capabilities(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    request_wl_drm_at_version(socket_path, 2)
}

pub(in crate::compositor::tests) fn request_wl_drm_at_version(
    socket_path: &PathBuf,
    version: u32,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let _drm: client_wl_drm::WlDrm = globals.bind(&qh, version..=version, ())?;
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn request_wl_drm_authentication(
    socket_path: &PathBuf,
) -> Result<RegistryTestState, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let drm: client_wl_drm::WlDrm = globals.bind(&qh, 2..=2, ())?;
    drm.authenticate(0);
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(state)
}

pub(in crate::compositor::tests) fn import_invalid_syncobj_timeline(
    socket_path: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let file = File::open("/dev/null")?;
    let _timeline = syncobj.import_timeline(file.as_fd(), &qh, ());
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(())
}

pub(in crate::compositor::tests) fn set_syncobj_acquire_after_surface_destroy(
    socket_path: &PathBuf,
    timeline: &DrmSyncobjTimeline,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path)?;
    let connection = Connection::from_socket(stream)?;
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();

    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let syncobj: client_wp_linux_drm_syncobj_manager_v1::WpLinuxDrmSyncobjManagerV1 =
        globals.bind(&qh, 1..=1, ())?;

    let surface = compositor.create_surface(&qh, ());
    let sync_surface = syncobj.get_surface(&surface, &qh, ());
    let timeline_fd = timeline.export_timeline_fd()?;
    let sync_timeline = syncobj.import_timeline(timeline_fd.as_fd(), &qh, ());
    surface.destroy();
    sync_surface.set_acquire_point(&sync_timeline, 0, 1);
    connection.flush()?;

    let mut state = RegistryTestState::default();
    queue.roundtrip(&mut state)?;
    Ok(())
}
