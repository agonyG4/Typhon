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
    ready: bool,
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
            client_screencopy_frame_v1::Event::Ready { .. } => state.ready = true,
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

impl Dispatch<client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for ScreencopyTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        _event: client_zwp_linux_dmabuf_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()>
    for ScreencopyTestState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        _event: client_zwp_linux_buffer_params_v1::Event,
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

struct DmabufScreencopyClient {
    connection: Connection,
    queue: wayland_client::EventQueue<ScreencopyTestState>,
    state: ScreencopyTestState,
    frame: client_screencopy_frame_v1::ZwlrScreencopyFrameV1,
    dmabuf: client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
    size: (u32, u32),
}

impl DmabufScreencopyClient {
    fn connect(socket_name: &str, commands: &Sender<ServerCommand>) -> Self {
        let socket_path = runtime_socket_path(socket_name);
        let connection =
            Connection::from_socket(UnixStream::connect(&socket_path).unwrap()).unwrap();
        let (globals, mut queue) = registry_queue_init::<ScreencopyTestState>(&connection).unwrap();
        let qh = queue.handle();
        let manager = globals
            .bind::<client_screencopy_manager_v1::ZwlrScreencopyManagerV1, _, _>(&qh, 3..=3, ())
            .unwrap();
        let output = globals
            .bind::<client_wl_output::WlOutput, _, _>(&qh, 1..=4, ())
            .unwrap();
        let dmabuf = globals
            .bind::<client_zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, _, _>(&qh, 3..=4, ())
            .unwrap();
        let mut state = ScreencopyTestState::default();
        let frame = manager.capture_output(0, &output, &qh, ());
        connection.flush().unwrap();
        wait_for_server_commands(commands);
        queue.roundtrip(&mut state).unwrap();
        let (_, width, height, _) = state.buffers[0];
        Self {
            connection,
            queue,
            state,
            frame,
            dmabuf,
            size: (width, height),
        }
    }

    fn dmabuf_buffer(&self, width: u32, height: u32, format: u32) -> client_wl_buffer::WlBuffer {
        let qh = self.queue.handle();
        let pixels = vec![0_u32; (width * height) as usize];
        let file = create_test_shm_file(&pixels).unwrap();
        let params = self.dmabuf.create_params(&qh, ());
        params.add(file.as_fd(), 0, 0, width * 4, 0, 0);
        params.create_immed(
            width as i32,
            height as i32,
            format,
            client_zwp_linux_buffer_params_v1::Flags::empty(),
            &qh,
            (),
        )
    }
}

fn gpu_screencopy_state(commands: &Sender<ServerCommand>) -> (bool, bool) {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::CapturePendingGpuScreencopy(reply))
        .unwrap();
    wait_for_server_commands(commands);
    receiver.recv_timeout(Duration::from_secs(1)).unwrap()
}

fn settle_gpu_screencopies(commands: &Sender<ServerCommand>, deliver: bool) -> usize {
    let (reply, receiver) = mpsc::channel();
    commands
        .send(ServerCommand::SettleGpuScreencopies { deliver, reply })
        .unwrap();
    wait_for_server_commands(commands);
    receiver.recv_timeout(Duration::from_secs(1)).unwrap()
}

#[test]
fn screencopy_dmabuf_copy_is_a_gpu_request_and_reports_ready() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut client = DmabufScreencopyClient::connect(&socket_name, &commands);
    let (width, height) = client.size;

    let buffer = client.dmabuf_buffer(width, height, DrmFormat::XRGB8888_FOURCC);
    client.frame.copy(&buffer);
    client.connection.flush().unwrap();
    wait_for_server_commands(&commands);

    // The DMA-BUF request must not start the private re-render capture path.
    assert_eq!(gpu_screencopy_state(&commands), (false, true));
    assert_eq!(settle_gpu_screencopies(&commands, true), 1);
    assert_eq!(gpu_screencopy_state(&commands), (false, false));
    client.queue.roundtrip(&mut client.state).unwrap();
    assert!(client.state.ready);
    assert!(!client.state.failed);

    drop(client);
    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn screencopy_dmabuf_gpu_request_failure_reports_failed() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let mut client = DmabufScreencopyClient::connect(&socket_name, &commands);
    let (width, height) = client.size;

    let buffer = client.dmabuf_buffer(width, height, DrmFormat::ARGB8888_FOURCC);
    client.frame.copy(&buffer);
    client.connection.flush().unwrap();
    wait_for_server_commands(&commands);

    assert_eq!(settle_gpu_screencopies(&commands, false), 1);
    client.queue.roundtrip(&mut client.state).unwrap();
    assert!(client.state.failed);
    assert!(!client.state.ready);

    drop(client);
    let _ = stop_controllable_test_server(commands, server_thread);
}

#[test]
fn screencopy_copy_with_wrong_sized_dmabuf_is_invalid_buffer() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind(&socket_name).unwrap();
    let (commands, server_thread) = spawn_controllable_test_server(server);
    let client = DmabufScreencopyClient::connect(&socket_name, &commands);
    let (width, height) = client.size;
    assert_ne!((2, 2), (width, height));

    let wrong = client.dmabuf_buffer(2, 2, DrmFormat::XRGB8888_FOURCC);
    client.frame.copy(&wrong);
    client.connection.flush().unwrap();
    wait_for_server_commands(&commands);

    let error = expect_protocol_error(
        &client.connection,
        "zwlr_screencopy_frame_v1",
        client_screencopy_frame_v1::Error::InvalidBuffer as u32,
    );
    assert_eq!(error.object_id, client.frame.id().protocol_id());
    assert_eq!(gpu_screencopy_state(&commands), (false, false));

    drop(client);
    let _ = stop_controllable_test_server(commands, server_thread);
}
