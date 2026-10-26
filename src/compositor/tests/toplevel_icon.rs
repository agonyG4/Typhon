use super::*;
use wayland_protocols::xdg::toplevel_icon::v1::client::xdg_toplevel_icon_manager_v1 as icon_manager;
use wayland_protocols::xdg::toplevel_icon::v1::client::xdg_toplevel_icon_v1 as icon;

#[test]
fn exported_icon_pixels_are_exact_and_immutable() {
    use crate::compositor::toplevel_icon::ToplevelIcon;
    use crate::render_backend::buffer::{BufferSize, ShmBufferSnapshot};
    use std::os::{fd::AsRawFd, unix::fs::FileExt};
    let pixels = vec![0xff123456, 0x80402010, 0, 0xffffffff];
    let snapshot = ShmBufferSnapshot::new(BufferSize::new(2, 2).unwrap(), pixels.clone()).unwrap();
    let icon = ToplevelIcon::new(String::new(), Some(snapshot)).unwrap();
    let file = icon.file.unwrap();
    let mut bytes = [0; 16];
    file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, bytemuck::cast_slice::<u32, u8>(&pixels));
    assert_eq!(file.metadata().unwrap().len(), 16);
    assert_eq!(
        file.write_at(&[0], 0).unwrap_err().raw_os_error(),
        Some(libc::EPERM)
    );
    let seals = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GET_SEALS) };
    assert_eq!(seals & libc::F_SEAL_WRITE, libc::F_SEAL_WRITE);
    assert_eq!(seals & libc::F_SEAL_GROW, libc::F_SEAL_GROW);
    assert_eq!(seals & libc::F_SEAL_SHRINK, libc::F_SEAL_SHRINK);
}

impl Dispatch<icon_manager::XdgToplevelIconManagerV1, ()> for RegistryTestState {
    fn event(
        _: &mut Self,
        _: &icon_manager::XdgToplevelIconManagerV1,
        _: icon_manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<icon::XdgToplevelIconV1, ()> for RegistryTestState {
    fn event(
        _: &mut Self,
        _: &icon::XdgToplevelIconV1,
        _: icon::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

#[test]
fn assigned_icon_is_double_buffered_and_survives_icon_object_destruction() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, thread) = spawn_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shell: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let manager: icon_manager::XdgToplevelIconManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg_surface = shell.get_xdg_surface(&surface, &qh, ());
    let top = xdg_surface.get_toplevel(&qh, ());
    let icon = manager.create_icon(&qh, ());
    icon.set_name("portable-game".into());
    manager.set_icon(&top, Some(&icon));
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let server = stop_test_server(running, thread);
    assert!(server.state.toplevel_icons.current.is_empty());
    assert_eq!(server.state.toplevel_icons.pending.len(), 1);

    let (running, thread) = spawn_test_server(server);
    icon.destroy();
    surface.commit();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let server = stop_test_server(running, thread);
    assert_eq!(
        server
            .state
            .toplevel_icons
            .current
            .values()
            .next()
            .unwrap()
            .name,
        "portable-game"
    );

    let (running, thread) = spawn_test_server(server);
    manager.set_icon(&top, None);
    surface.commit();
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    let server = stop_test_server(running, thread);
    assert!(server.state.toplevel_icons.current.is_empty());
}

#[test]
fn assigning_an_icon_makes_it_immutable_before_surface_commit() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, thread) = spawn_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let qh = queue.handle();
    let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap();
    let shell: client_xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap();
    let manager: icon_manager::XdgToplevelIconManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap();
    let surface = compositor.create_surface(&qh, ());
    let xdg = shell.get_xdg_surface(&surface, &qh, ());
    let top = xdg.get_toplevel(&qh, ());
    let icon = manager.create_icon(&qh, ());
    icon.set_name("portable-game".into());
    manager.set_icon(&top, Some(&icon));
    icon.set_name("changed-too-late".into());
    let result = queue.roundtrip(&mut RegistryTestState::default());
    stop_test_server(running, thread);
    assert!(result.is_err());
}

#[test]
fn toplevel_icon_manager_is_advertised() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let connection = Connection::from_socket(UnixStream::connect(socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection).unwrap();
    let manager =
        globals.bind::<icon_manager::XdgToplevelIconManagerV1, _, _>(&queue.handle(), 1..=1, ());
    queue.roundtrip(&mut RegistryTestState::default()).unwrap();
    stop_test_server(running, server_thread);
    assert!(manager.is_ok(), "missing xdg_toplevel_icon_manager_v1");
}
