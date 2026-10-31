use super::*;

use std::os::unix::net::UnixStream;
use wayland_client::{
    Proxy, WEnum,
    protocol::{
        wl_buffer as client_wl_buffer, wl_shm as client_wl_shm, wl_shm_pool as client_wl_shm_pool,
    },
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1 as client_screencopy_frame_v1,
    zwlr_screencopy_manager_v1 as client_screencopy_manager_v1,
};

#[derive(Default)]
struct ScreencopyTestState {
    buffers: Vec<(bool, u32, u32, u32)>,
    buffer_done: bool,
    failed: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ScreencopyTestState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_screencopy_manager_v1::ZwlrScreencopyManagerV1, ()> for ScreencopyTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_screencopy_manager_v1::ZwlrScreencopyManagerV1,
        _event: client_screencopy_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_screencopy_frame_v1::ZwlrScreencopyFrameV1, ()> for ScreencopyTestState {
    fn event(
        state: &mut Self,
        _proxy: &client_screencopy_frame_v1::ZwlrScreencopyFrameV1,
        event: client_screencopy_frame_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            client_screencopy_frame_v1::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => state.buffers.push((
                format == WEnum::Value(client_wl_shm::Format::Xrgb8888),
                width,
                height,
                stride,
            )),
            client_screencopy_frame_v1::Event::BufferDone => state.buffer_done = true,
            client_screencopy_frame_v1::Event::Failed => state.failed = true,
            _ => {}
        }
    }
}

impl Dispatch<client_wl_output::WlOutput, ()> for ScreencopyTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_output::WlOutput,
        _event: client_wl_output::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_shm::WlShm, ()> for ScreencopyTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_shm::WlShm,
        _event: client_wl_shm::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_shm_pool::WlShmPool, ()> for ScreencopyTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_shm_pool::WlShmPool,
        _event: client_wl_shm_pool::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_wl_buffer::WlBuffer, ()> for ScreencopyTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_wl_buffer::WlBuffer,
        _event: client_wl_buffer::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

#[test]
fn screencopy_output_frame_advertises_xrgb_buffer_then_buffer_done() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ScreencopyTestState>(&connection).unwrap();
    let qh = queue.handle();
    let manager = globals
        .bind::<client_screencopy_manager_v1::ZwlrScreencopyManagerV1, _, _>(&qh, 3..=3, ())
        .unwrap();
    let output = globals
        .bind::<client_wl_output::WlOutput, _, _>(&qh, 1..=4, ())
        .unwrap();
    let mut state = ScreencopyTestState::default();
    let _frame = manager.capture_output(0, &output, &qh, ());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();

    assert_eq!(state.buffers.len(), 1);
    let (is_xrgb, width, height, stride) = state.buffers[0];
    assert!(is_xrgb);
    assert!(width > 0 && height > 0);
    assert_eq!(stride, width * 4);
    assert!(state.buffer_done);
    assert!(!state.failed);

    drop(connection);
    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn screencopy_copy_with_wrong_sized_shm_buffer_is_invalid_buffer() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (commands, server_thread) = spawn_controllable_test_server(server);

    let connection = Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
    let (globals, mut queue) = registry_queue_init::<ScreencopyTestState>(&connection).unwrap();
    let qh = queue.handle();
    let manager = globals
        .bind::<client_screencopy_manager_v1::ZwlrScreencopyManagerV1, _, _>(&qh, 3..=3, ())
        .unwrap();
    let output = globals
        .bind::<client_wl_output::WlOutput, _, _>(&qh, 1..=4, ())
        .unwrap();
    let shm = globals
        .bind::<client_wl_shm::WlShm, _, _>(&qh, 1..=1, ())
        .unwrap();
    let mut state = ScreencopyTestState::default();
    let frame = manager.capture_output(0, &output, &qh, ());
    connection.flush().unwrap();
    wait_for_server_commands(&commands);
    queue.roundtrip(&mut state).unwrap();
    let (_, width, height, _) = state.buffers[0];

    let pixels = vec![0xff20_3040_u32; 4];
    let file = create_test_shm_file(&pixels).unwrap();
    let pool = shm.create_pool(file.as_fd(), 16, &qh, ());
    let wrong = pool.create_buffer(0, 2, 2, 8, client_wl_shm::Format::Xrgb8888, &qh, ());
    assert_ne!((2, 2), (width, height));
    frame.copy(&wrong);
    connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let error = expect_protocol_error(
        &connection,
        "zwlr_screencopy_frame_v1",
        client_screencopy_frame_v1::Error::InvalidBuffer as u32,
    );
    assert_eq!(error.object_id, frame.id().protocol_id());

    drop(connection);
    drop(queue);
    let _ = stop_controllable_test_server(commands, server_thread);
}
