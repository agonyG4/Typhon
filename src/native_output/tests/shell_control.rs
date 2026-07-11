use super::*;

use std::{
    collections::VecDeque,
    os::unix::net::UnixStream,
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
    time::Duration,
};

use oblivion_one::astrea_shell_control::client::{
    astrea_launch_request_v1 as client_astrea_launch_request_v1,
    astrea_shell_control_manager_v1 as client_astrea_shell_control_manager_v1,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum LaunchEvent {
    Accepted(u32),
    Failed(u32, String),
    Finished(i32),
}

#[derive(Default)]
struct LaunchClientState {
    events: Vec<LaunchEvent>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for LaunchClientState {
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

impl Dispatch<client_astrea_shell_control_manager_v1::AstreaShellControlManagerV1, ()>
    for LaunchClientState
{
    fn event(
        _state: &mut Self,
        _proxy: &client_astrea_shell_control_manager_v1::AstreaShellControlManagerV1,
        _event: client_astrea_shell_control_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<client_astrea_launch_request_v1::AstreaLaunchRequestV1, ()> for LaunchClientState {
    fn event(
        state: &mut Self,
        _proxy: &client_astrea_launch_request_v1::AstreaLaunchRequestV1,
        event: client_astrea_launch_request_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        state.events.push(match event {
            client_astrea_launch_request_v1::Event::Accepted { pid } => LaunchEvent::Accepted(pid),
            client_astrea_launch_request_v1::Event::Failed { code, message } => {
                LaunchEvent::Failed(code, message)
            }
            client_astrea_launch_request_v1::Event::Finished { status } => {
                LaunchEvent::Finished(status)
            }
            _ => return,
        });
    }
}

enum RuntimeCommand {
    Stop,
    TrackerCount(Sender<usize>),
    CompleteAgain {
        pid: u32,
        status: std::process::ExitStatus,
        reply: Sender<bool>,
    },
    HoldDrain(Sender<()>),
    ReleaseDrain,
}

fn spawn_shell_control_runtime(
    mut server: OwnCompositorServer,
) -> (
    Sender<RuntimeCommand>,
    JoinHandle<(OwnCompositorServer, usize)>,
) {
    let (commands, receiver) = mpsc::channel();
    let thread = thread::spawn(move || {
        let mut supervisor = ChildSupervisor::new();
        let mut tracker = AstreaLaunchLifecycleTracker::default();
        let mut pending_launches = VecDeque::new();
        let mut drain_enabled = true;
        let mut running = true;
        while running {
            while let Ok(command) = receiver.try_recv() {
                match command {
                    RuntimeCommand::Stop => running = false,
                    RuntimeCommand::TrackerCount(reply) => {
                        let _ = reply.send(tracker.len());
                    }
                    RuntimeCommand::CompleteAgain { pid, status, reply } => {
                        let _ = reply.send(tracker.complete(pid, status));
                    }
                    RuntimeCommand::HoldDrain(reply) => {
                        drain_enabled = false;
                        let _ = reply.send(());
                    }
                    RuntimeCommand::ReleaseDrain => drain_enabled = true,
                }
            }
            let _ = server.tick();
            if drain_enabled {
                drain_pending_process_launches(
                    &mut server,
                    &mut supervisor,
                    &mut tracker,
                    EffectiveCompositorAppGpuPolicy::CpuOnly,
                    NativePerfLogger::from_env(),
                    &mut pending_launches,
                );
            }
            if let Ok(exits) = supervisor.reap_exited() {
                for exit in exits {
                    tracker.complete(exit.pid, exit.status);
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        (server, tracker.len())
    });
    (commands, thread)
}

fn connect_launch_client(
    socket_name: &str,
) -> (
    Connection,
    EventQueue<LaunchClientState>,
    client_astrea_shell_control_manager_v1::AstreaShellControlManagerV1,
) {
    let stream = UnixStream::connect(runtime_socket_path(socket_name)).unwrap();
    let connection = Connection::from_socket(stream).unwrap();
    let (globals, queue) = registry_queue_init::<LaunchClientState>(&connection).unwrap();
    let qh = queue.handle();
    let manager = globals
        .bind(&qh, 1..=1, ())
        .expect("shell-control manager global");
    (connection, queue, manager)
}

fn runtime_socket_path(socket_name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR")).join(socket_name)
}

fn wait_until_finished(queue: &mut EventQueue<LaunchClientState>, state: &mut LaunchClientState) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !state
        .events
        .iter()
        .any(|event| matches!(event, LaunchEvent::Finished(_) | LaunchEvent::Failed(_, _)))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "launch request timed out"
        );
        queue.blocking_dispatch(state).unwrap();
    }
}

fn wait_until_accepted(queue: &mut EventQueue<LaunchClientState>, state: &mut LaunchClientState) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !state
        .events
        .iter()
        .any(|event| matches!(event, LaunchEvent::Accepted(_)))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "launch acceptance timed out"
        );
        queue.blocking_dispatch(state).unwrap();
    }
}

fn launch_argv(
    connection: &Connection,
    queue: &EventQueue<LaunchClientState>,
    manager: &client_astrea_shell_control_manager_v1::AstreaShellControlManagerV1,
    argv: &[&str],
) -> client_astrea_launch_request_v1::AstreaLaunchRequestV1 {
    let qh = queue.handle();
    let argv = argv
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    let request = manager.launch_argv_json(serde_json::to_string(&argv).unwrap(), &qh, ());
    connection.flush().unwrap();
    request
}

fn stop_runtime(
    commands: Sender<RuntimeCommand>,
    thread: JoinHandle<(OwnCompositorServer, usize)>,
) -> usize {
    commands.send(RuntimeCommand::Stop).unwrap();
    thread.join().unwrap().1
}

#[test]
fn shell_control_successful_exit_emits_accepted_then_finished() {
    let socket_name = format!("typhon-shell-control-success-{}", std::process::id());
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, runtime) = spawn_shell_control_runtime(server);
    let (connection, mut queue, manager) = connect_launch_client(&socket_name);
    queue.roundtrip(&mut LaunchClientState::default()).unwrap();
    let _request = launch_argv(&connection, &queue, &manager, &["/bin/sh", "-c", "exit 7"]);

    let mut state = LaunchClientState::default();
    wait_until_finished(&mut queue, &mut state);
    let pid = match state.events[0] {
        LaunchEvent::Accepted(pid) => pid,
        _ => panic!("accepted must precede finished"),
    };
    let (reply, receiver) = mpsc::channel();
    commands
        .send(RuntimeCommand::CompleteAgain {
            pid,
            status: std::process::ExitStatus::from_raw(7 << 8),
            reply,
        })
        .unwrap();
    assert!(!receiver.recv_timeout(Duration::from_secs(1)).unwrap());
    let tracker_count = stop_runtime(commands, runtime);

    assert!(matches!(state.events[0], LaunchEvent::Accepted(_)));
    assert_eq!(
        state.events,
        [state.events[0].clone(), LaunchEvent::Finished(7)]
    );
    assert_eq!(tracker_count, 0);
}

#[test]
fn shell_control_spawn_failure_emits_failed_without_acceptance() {
    let socket_name = format!("typhon-shell-control-failure-{}", std::process::id());
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, runtime) = spawn_shell_control_runtime(server);
    let (connection, mut queue, manager) = connect_launch_client(&socket_name);
    queue.roundtrip(&mut LaunchClientState::default()).unwrap();
    let _request = launch_argv(
        &connection,
        &queue,
        &manager,
        &["/definitely/not/a/typhon-shell-control-test"],
    );

    let mut state = LaunchClientState::default();
    wait_until_finished(&mut queue, &mut state);
    let tracker_count = stop_runtime(commands, runtime);

    assert!(matches!(
        state.events.as_slice(),
        [LaunchEvent::Failed(5, _)]
    ));
    assert_eq!(tracker_count, 0);
}

#[test]
fn shell_control_zero_exit_emits_finished_zero() {
    let socket_name = format!("typhon-shell-control-zero-{}", std::process::id());
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, runtime) = spawn_shell_control_runtime(server);
    let (connection, mut queue, manager) = connect_launch_client(&socket_name);
    queue.roundtrip(&mut LaunchClientState::default()).unwrap();
    let _request = launch_argv(&connection, &queue, &manager, &["/bin/sh", "-c", "exit 0"]);

    let mut state = LaunchClientState::default();
    wait_until_finished(&mut queue, &mut state);
    let tracker_count = stop_runtime(commands, runtime);

    assert!(matches!(
        state.events.as_slice(),
        [LaunchEvent::Accepted(_), LaunchEvent::Finished(0)]
    ));
    assert_eq!(tracker_count, 0);
}

#[test]
fn shell_control_signal_exit_emits_negative_signal_number() {
    let socket_name = format!("typhon-shell-control-signal-{}", std::process::id());
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, runtime) = spawn_shell_control_runtime(server);
    let (connection, mut queue, manager) = connect_launch_client(&socket_name);
    queue.roundtrip(&mut LaunchClientState::default()).unwrap();
    let _request = launch_argv(
        &connection,
        &queue,
        &manager,
        &["/bin/sh", "-c", "kill -TERM $$"],
    );

    let mut state = LaunchClientState::default();
    wait_until_finished(&mut queue, &mut state);
    let tracker_count = stop_runtime(commands, runtime);

    assert!(matches!(
        state.events.as_slice(),
        [LaunchEvent::Accepted(_), LaunchEvent::Finished(-15)]
    ));
    assert_eq!(tracker_count, 0);
}

#[test]
fn shell_control_destroy_after_acceptance_prunes_observer_without_killing_child() {
    let socket_name = format!("typhon-shell-control-observer-{}", std::process::id());
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, runtime) = spawn_shell_control_runtime(server);
    let (connection, mut queue, manager) = connect_launch_client(&socket_name);
    queue.roundtrip(&mut LaunchClientState::default()).unwrap();
    let request = launch_argv(
        &connection,
        &queue,
        &manager,
        &["/bin/sh", "-c", "sleep 0.05"],
    );

    let mut state = LaunchClientState::default();
    wait_until_accepted(&mut queue, &mut state);
    request.destroy();
    connection.flush().unwrap();
    thread::sleep(Duration::from_millis(150));

    let (reply, receiver) = mpsc::channel();
    commands.send(RuntimeCommand::TrackerCount(reply)).unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 0);
    let tracker_count = stop_runtime(commands, runtime);

    assert!(
        state
            .events
            .iter()
            .all(|event| !matches!(event, LaunchEvent::Finished(_)))
    );
    assert_eq!(tracker_count, 0);
}

#[test]
fn shell_control_destroy_before_drain_does_not_spawn_or_track() {
    let socket_name = format!("typhon-shell-control-destroy-{}", std::process::id());
    let mut server = OwnCompositorServer::bind(&socket_name).unwrap();
    server.authorize_astrea_shell_pid(std::process::id());
    let (commands, runtime) = spawn_shell_control_runtime(server);
    let (held, held_receiver) = mpsc::channel();
    commands.send(RuntimeCommand::HoldDrain(held)).unwrap();
    held_receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    let (connection, queue, manager) = connect_launch_client(&socket_name);
    let request = launch_argv(&connection, &queue, &manager, &["/bin/sh", "-c", "sleep 1"]);
    request.destroy();
    connection.flush().unwrap();
    commands.send(RuntimeCommand::ReleaseDrain).unwrap();
    thread::sleep(Duration::from_millis(50));

    let (reply, receiver) = mpsc::channel();
    commands.send(RuntimeCommand::TrackerCount(reply)).unwrap();
    assert_eq!(receiver.recv_timeout(Duration::from_secs(1)).unwrap(), 0);
    let tracker_count = stop_runtime(commands, runtime);

    assert_eq!(tracker_count, 0);
}
