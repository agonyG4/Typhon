use super::*;

type QualifiedConnection = (
    Connection,
    wayland_client::globals::GlobalList,
    EventQueue<RegistryTestState>,
    client_wl_compositor::WlCompositor,
    client_wp_fifo_manager_v1::WpFifoManagerV1,
    client_wp_commit_timing_manager_v1::WpCommitTimingManagerV1,
);

fn qualified_connection(
    socket_path: &PathBuf,
) -> Result<QualifiedConnection, Box<dyn std::error::Error>> {
    let connection = Connection::from_socket(UnixStream::connect(socket_path)?)?;
    let (globals, queue) = registry_queue_init::<RegistryTestState>(&connection)?;
    let qh = queue.handle();
    let compositor = globals.bind(&qh, 1..=6, ())?;
    let fifo = globals.bind(&qh, 1..=1, ())?;
    let timing = globals.bind(&qh, 1..=1, ())?;
    Ok((connection, globals, queue, compositor, fifo, timing))
}

#[test]
fn qualified_frame_pacing_managers_create_one_object_per_surface() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);

    let (connection, _globals, queue, compositor, fifo_manager, timing_manager) =
        qualified_connection(&socket_path).unwrap();
    let qh = queue.handle();
    let surface = compositor.create_surface(&qh, ());
    let _fifo: client_wp_fifo_v1::WpFifoV1 = fifo_manager.get_fifo(&surface, &qh, ());
    let _timer = timing_manager.get_timer(&surface, &qh, ());
    connection.roundtrip().unwrap();

    drop(surface);
    drop(compositor);
    drop(timing_manager);
    drop(fifo_manager);
    drop(queue);
    drop(_globals);
    drop(connection);
    let _ = stop_test_server(running, server_thread);
}

#[test]
fn duplicate_fifo_object_is_the_exact_manager_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let (connection, _globals, queue, compositor, fifo_manager, _timing_manager) =
        qualified_connection(&socket_path).unwrap();
    let qh = queue.handle();
    let surface = compositor.create_surface(&qh, ());
    let _fifo = fifo_manager.get_fifo(&surface, &qh, ());
    connection.roundtrip().unwrap();
    let _duplicate = fifo_manager.get_fifo(&surface, &qh, ());
    let result = connection.roundtrip();
    assert!(result.is_err());
    let error = connection
        .protocol_error()
        .expect("FIFO duplicate must be a wire error");
    assert_eq!(error.object_interface, "wp_fifo_manager_v1");
    assert_eq!(
        error.code,
        client_wp_fifo_manager_v1::Error::AlreadyExists as u32
    );
    let _ = stop_test_server(running, server_thread);
}

#[test]
fn duplicate_commit_timer_is_the_exact_manager_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let (connection, _globals, queue, compositor, _fifo_manager, timing_manager) =
        qualified_connection(&socket_path).unwrap();
    let qh = queue.handle();
    let surface = compositor.create_surface(&qh, ());
    let _timer = timing_manager.get_timer(&surface, &qh, ());
    connection.roundtrip().unwrap();
    let _duplicate = timing_manager.get_timer(&surface, &qh, ());
    connection.flush().unwrap();

    let result = connection.roundtrip();
    assert!(result.is_err());
    let error = connection
        .protocol_error()
        .expect("timer duplicate must be a wire error");
    assert_eq!(error.object_interface, "wp_commit_timing_manager_v1");
    assert_eq!(
        error.code,
        client_wp_commit_timing_manager_v1::Error::CommitTimerExists as u32
    );
    let _ = stop_test_server(running, server_thread);
}

#[test]
fn commit_timer_rejects_invalid_and_duplicate_timestamps() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let (connection, _globals, queue, compositor, _fifo_manager, timing_manager) =
        qualified_connection(&socket_path).unwrap();
    let qh = queue.handle();
    let surface = compositor.create_surface(&qh, ());
    let timer = timing_manager.get_timer(&surface, &qh, ());
    connection.roundtrip().unwrap();
    timer.set_timestamp(0, 0, 1_000_000_000);
    connection.flush().unwrap();
    let result = connection.roundtrip();
    assert!(result.is_err());
    let error = connection
        .protocol_error()
        .expect("invalid timestamp must be a wire error");
    assert_eq!(error.object_interface, "wp_commit_timer_v1");
    assert_eq!(
        error.code,
        client_wp_commit_timer_v1::Error::InvalidTimestamp as u32
    );

    // The invalid request terminates the connection, so the duplicate-state
    // case is covered independently by a fresh qualified resource sequence.
    let _ = stop_test_server(running, server_thread);
}

#[test]
fn commit_timer_rejects_a_second_pending_timestamp() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let (connection, _globals, queue, compositor, _fifo_manager, timing_manager) =
        qualified_connection(&socket_path).unwrap();
    let qh = queue.handle();
    let surface = compositor.create_surface(&qh, ());
    let timer = timing_manager.get_timer(&surface, &qh, ());
    connection.roundtrip().unwrap();
    timer.set_timestamp(0, 0, 0);
    timer.set_timestamp(0, 0, 1);
    let result = connection.roundtrip();
    assert!(result.is_err());
    let error = connection
        .protocol_error()
        .expect("second timestamp must be a wire error");
    assert_eq!(error.object_interface, "wp_commit_timer_v1");
    assert_eq!(
        error.code,
        client_wp_commit_timer_v1::Error::TimestampExists as u32
    );
    let _ = stop_test_server(running, server_thread);
}

#[test]
fn fifo_request_after_surface_destroy_is_the_exact_surface_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let (connection, _globals, queue, compositor, fifo_manager, _timing_manager) =
        qualified_connection(&socket_path).unwrap();
    let qh = queue.handle();
    let surface = compositor.create_surface(&qh, ());
    let fifo: client_wp_fifo_v1::WpFifoV1 = fifo_manager.get_fifo(&surface, &qh, ());
    connection.roundtrip().unwrap();
    surface.destroy();
    connection.roundtrip().unwrap();
    fifo.set_barrier();
    let result = connection.roundtrip();
    assert!(result.is_err());
    let error = connection
        .protocol_error()
        .expect("FIFO request after surface destruction must fail");
    assert_eq!(error.object_interface, "wp_fifo_v1");
    assert_eq!(
        error.code,
        client_wp_fifo_v1::Error::SurfaceDestroyed as u32
    );
    let _ = stop_test_server(running, server_thread);
}

#[test]
fn fifo_wait_is_ordered_and_hidden_surface_forward_progress_is_finite() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let (connection, _globals, queue, compositor, fifo_manager, _timing_manager) =
        qualified_connection(&socket_path).unwrap();
    let qh = queue.handle();
    let surface = compositor.create_surface(&qh, ());
    let fifo = fifo_manager.get_fifo(&surface, &qh, ());
    connection.roundtrip().unwrap();

    fifo.set_barrier();
    surface.commit();
    connection.roundtrip().unwrap();
    fifo.wait_barrier();
    surface.commit();
    connection.roundtrip().unwrap();

    std::thread::sleep(Duration::from_millis(45));
    let server = stop_test_server(running, server_thread);
    assert!(server.state.active_fifo_barriers.is_empty());
    assert!(server.state.pending_surface_tree_transactions.is_empty());
    let metrics = server.surface_pacing_metrics();
    assert!(metrics.barriers_captured >= 1);
    assert!(metrics.waits_captured >= 1);
    assert!(metrics.barriers_cleared_by_fallback >= 1);
}

#[test]
fn timestamp_is_one_shot_surface_state() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let socket_path = runtime_socket_path(&socket_name);
    let (running, server_thread) = spawn_test_server(server);
    let (connection, _globals, queue, compositor, _fifo_manager, timing_manager) =
        qualified_connection(&socket_path).unwrap();
    let qh = queue.handle();
    let surface = compositor.create_surface(&qh, ());
    let timer = timing_manager.get_timer(&surface, &qh, ());
    timer.set_timestamp(0, 0, 0);
    surface.commit();
    connection.roundtrip().unwrap();
    surface.commit();
    connection.roundtrip().unwrap();

    let _ = stop_test_server(running, server_thread);
}
