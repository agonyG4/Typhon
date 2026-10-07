use super::*;
use crate::astrea_background_effect_coverage::client::{
    astrea_background_effect_coverage_manager_v1 as client_coverage_manager,
    astrea_background_effect_coverage_v1 as client_coverage,
};
use crate::astrea_shell_auth::client::astrea_shell_auth_manager_v1 as client_shell_auth_manager;

#[test]
fn coverage_global_is_hidden_without_the_background_effect_renderer_capability() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_cpu_composition(&socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let globals = read_registry_globals(&runtime_socket_path(&socket_name)).unwrap();
    assert!(
        !globals
            .iter()
            .any(|name| name == "astrea_background_effect_coverage_manager_v1")
    );
    stop_test_server(running, server_thread);
}

impl Dispatch<client_coverage_manager::AstreaBackgroundEffectCoverageManagerV1, ()>
    for RegistryTestState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_coverage_manager::AstreaBackgroundEffectCoverageManagerV1,
        _event: client_coverage_manager::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_coverage::AstreaBackgroundEffectCoverageV1, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_coverage::AstreaBackgroundEffectCoverageV1,
        _event: client_coverage::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_shell_auth_manager::AstreaShellAuthManagerV1, ()> for RegistryTestState {
    fn event(
        _state: &mut Self,
        _proxy: &client_shell_auth_manager::AstreaShellAuthManagerV1,
        _event: client_shell_auth_manager::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

#[test]
fn coverage_manager_rejects_unauthorized_client_with_typed_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let socket_path = runtime_socket_path(&socket_name);
        let connection = Connection::from_socket(UnixStream::connect(socket_path)?)?;
        let (globals, _queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = _queue.handle();
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let manager: client_coverage_manager::AstreaBackgroundEffectCoverageManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let surface = compositor.create_surface(&qh, ());
        let _coverage = manager.get_coverage(&surface, &qh, ());
        connection.flush()?;
        assert!(connection.roundtrip().is_err());
        let error = connection
            .protocol_error()
            .expect("unauthorized manager request must be a protocol error");
        assert_eq!(
            error.object_interface,
            "astrea_background_effect_coverage_manager_v1"
        );
        assert_eq!(
            error.code,
            client_coverage_manager::Error::Unauthorized as u32
        );
        Ok(())
    })();
    stop_test_server(running, server_thread);
    result.unwrap();
}

#[test]
fn authorized_coverage_is_commit_bound_and_duplicate_object_is_typed() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let capability = std::fs::read_to_string(
        crate::compositor::astrea_shell_capability::test_capability_path(&socket_name),
    )
    .unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection =
            Connection::from_socket(UnixStream::connect(runtime_socket_path(&socket_name))?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let auth: client_shell_auth_manager::AstreaShellAuthManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        auth.authenticate(capability.trim().to_owned());
        let mut state = RegistryTestState::default();
        queue.roundtrip(&mut state)?;

        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let manager: client_coverage_manager::AstreaBackgroundEffectCoverageManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let surface = compositor.create_surface(&qh, ());
        let coverage = manager.get_coverage(&surface, &qh, ());
        coverage.set_rounded_rect(0.25, 0.5, 72.0, 34.0, 17.0);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut state)?;
        assert!(connection.protocol_error().is_none());

        coverage.set_triangle(35.0, 40.5, 47.0, 40.5, 41.0, 48.0);
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut state)?;
        coverage.clear();
        surface.commit();
        connection.flush()?;
        queue.roundtrip(&mut state)?;

        let _duplicate = manager.get_coverage(&surface, &qh, ());
        connection.flush()?;
        assert!(connection.roundtrip().is_err());
        let error = connection
            .protocol_error()
            .expect("duplicate coverage object must be a wire error");
        assert_eq!(
            error.object_interface,
            "astrea_background_effect_coverage_manager_v1"
        );
        assert_eq!(
            error.code,
            client_coverage_manager::Error::CoverageExists as u32
        );
        Ok(())
    })();
    stop_test_server(running, server_thread);
    result.unwrap();
}

#[test]
fn coverage_geometry_error_is_typed() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let capability = std::fs::read_to_string(
        crate::compositor::astrea_shell_capability::test_capability_path(&socket_name),
    )
    .unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection =
            Connection::from_socket(UnixStream::connect(runtime_socket_path(&socket_name))?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let auth: client_shell_auth_manager::AstreaShellAuthManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        auth.authenticate(capability.trim().to_owned());
        let mut state = RegistryTestState::default();
        queue.roundtrip(&mut state)?;
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let manager: client_coverage_manager::AstreaBackgroundEffectCoverageManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let surface = compositor.create_surface(&qh, ());
        let coverage = manager.get_coverage(&surface, &qh, ());
        coverage.set_rounded_rect(0.0, 0.0, -4.0, 10.0, 0.0);
        connection.flush()?;
        assert!(connection.roundtrip().is_err());
        let error = connection
            .protocol_error()
            .expect("invalid coverage geometry must be a wire error");
        assert_eq!(
            error.object_interface,
            "astrea_background_effect_coverage_v1"
        );
        assert_eq!(error.code, client_coverage::Error::InvalidGeometry as u32);
        Ok(())
    })();
    stop_test_server(running, server_thread);
    result.unwrap();
}

#[test]
fn late_coverage_request_after_surface_destroy_reports_typed_error() {
    let socket_name = unique_socket_name();
    let server = OwnCompositorServer::bind_native_base(&socket_name).unwrap();
    let capability = std::fs::read_to_string(
        crate::compositor::astrea_shell_capability::test_capability_path(&socket_name),
    )
    .unwrap();
    let (running, server_thread) = spawn_test_server(server);
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let connection =
            Connection::from_socket(UnixStream::connect(runtime_socket_path(&socket_name))?)?;
        let (globals, mut queue) = registry_queue_init::<RegistryTestState>(&connection)?;
        let qh = queue.handle();
        let auth: client_shell_auth_manager::AstreaShellAuthManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        auth.authenticate(capability.trim().to_owned());
        let mut state = RegistryTestState::default();
        queue.roundtrip(&mut state)?;
        let compositor: client_wl_compositor::WlCompositor = globals.bind(&qh, 1..=6, ())?;
        let manager: client_coverage_manager::AstreaBackgroundEffectCoverageManagerV1 =
            globals.bind(&qh, 1..=1, ())?;
        let surface = compositor.create_surface(&qh, ());
        let coverage = manager.get_coverage(&surface, &qh, ());
        connection.flush()?;
        queue.roundtrip(&mut state)?;
        surface.destroy();
        coverage.set_triangle(0.0, 0.0, 8.0, 0.0, 4.0, 8.0);
        connection.flush()?;
        assert!(connection.roundtrip().is_err());
        let error = connection
            .protocol_error()
            .expect("late coverage request must be rejected");
        assert_eq!(
            error.object_interface,
            "astrea_background_effect_coverage_v1"
        );
        assert_eq!(error.code, client_coverage::Error::SurfaceDestroyed as u32);
        Ok(())
    })();
    stop_test_server(running, server_thread);
    result.unwrap();
}
