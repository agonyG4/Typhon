#![allow(
    dead_code,
    missing_docs,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unused_unsafe,
    unused_variables,
    clippy::all
)]

#[path = "../astrea_shell_control.rs"]
mod astrea_shell_control;

use std::{env, os::unix::net::UnixStream, path::PathBuf, process::ExitCode};

use astrea_shell_control::client::{astrea_launch_request_v1, astrea_shell_control_manager_v1};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};

#[derive(Default)]
struct LaunchState {
    done: bool,
    pid: Option<u32>,
    error: Option<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("typhon-launch: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) == Some("doctor") {
        return doctor();
    }
    if args.first().map(String::as_str) == Some("--desktop") {
        if args.len() != 2 {
            return Err("usage: typhon-launch --desktop <desktop-id>".to_string());
        }
        let pid = launch_request(RequestKind::Desktop(args.remove(1)))?;
        println!("accepted {pid}");
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("--argv-json") {
        if args.len() != 2 {
            return Err("usage: typhon-launch --argv-json '<json-array>'".to_string());
        }
        let pid = launch_request(RequestKind::ArgvJson(args.remove(1)))?;
        println!("accepted {pid}");
        return Ok(());
    }
    if args.first().map(String::as_str) == Some("--") {
        args.remove(0);
        if args.is_empty() {
            return Err("usage: typhon-launch -- <program> [args...]".to_string());
        }
        let pid = launch_request(RequestKind::ArgvJson(
            serde_json::to_string(&args).map_err(|err| err.to_string())?,
        ))?;
        println!("accepted {pid}");
        return Ok(());
    }
    Err(
        "usage: typhon-launch doctor | --desktop <id> | --argv-json '<json>' | -- <argv...>"
            .to_string(),
    )
}

enum RequestKind {
    Desktop(String),
    ArgvJson(String),
}

fn launch_request(kind: RequestKind) -> Result<u32, String> {
    let connection =
        Connection::connect_to_env().map_err(|err| format!("connect failed: {err}"))?;
    let (globals, mut queue) = registry_queue_init::<LaunchState>(&connection)
        .map_err(|err| format!("registry failed: {err}"))?;
    let qh = queue.handle();
    let manager: astrea_shell_control_manager_v1::AstreaShellControlManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|err| format!("shell-control global unavailable: {err}"))?;
    match kind {
        RequestKind::Desktop(id) => {
            let _request = manager.launch_desktop(id, &qh, ());
        }
        RequestKind::ArgvJson(json) => {
            let _request = manager.launch_argv_json(json, &qh, ());
        }
    }
    connection
        .flush()
        .map_err(|err| format!("flush failed: {err}"))?;
    let mut state = LaunchState::default();
    while !state.done {
        queue
            .blocking_dispatch(&mut state)
            .map_err(|err| format!("dispatch failed: {err}"))?;
        connection
            .flush()
            .map_err(|err| format!("flush failed: {err}"))?;
    }
    state
        .pid
        .ok_or_else(|| state.error.unwrap_or_else(|| "request failed".to_string()))
}

fn doctor() -> Result<(), String> {
    println!(
        "WAYLAND_DISPLAY={}",
        env::var("WAYLAND_DISPLAY").unwrap_or_default()
    );
    println!("DISPLAY={}", env::var("DISPLAY").unwrap_or_default());
    println!(
        "XDG_RUNTIME_DIR={}",
        env::var("XDG_RUNTIME_DIR").unwrap_or_default()
    );
    println!(
        "DBUS_SESSION_BUS_ADDRESS={}",
        env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default()
    );
    println!(
        "XDG_SESSION_ID={}",
        env::var("XDG_SESSION_ID").unwrap_or_default()
    );
    println!("XDG_VTNR={}", env::var("XDG_VTNR").unwrap_or_default());
    println!(
        "connected compositor socket={}",
        compositor_socket_reachable()
    );
    println!(
        "shell-control global availability={}",
        shell_control_available()
    );
    println!("session bus reachable={}", session_bus_reachable());
    println!("org.freedesktop.portal.Desktop owner=not-queried");
    Ok(())
}

fn compositor_socket_reachable() -> bool {
    let Some(runtime) = env::var_os("XDG_RUNTIME_DIR") else {
        return false;
    };
    let Some(display) = env::var_os("WAYLAND_DISPLAY") else {
        return false;
    };
    UnixStream::connect(PathBuf::from(runtime).join(display)).is_ok()
}

fn shell_control_available() -> bool {
    let Ok(connection) = Connection::connect_to_env() else {
        return false;
    };
    let Ok((globals, _queue)) = registry_queue_init::<LaunchState>(&connection) else {
        return false;
    };
    globals.contents().with_list(|globals| {
        globals
            .iter()
            .any(|global| global.interface == "astrea_shell_control_manager_v1")
    })
}

fn session_bus_reachable() -> bool {
    let Some(address) = env::var_os("DBUS_SESSION_BUS_ADDRESS") else {
        return false;
    };
    !address.is_empty()
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for LaunchState {
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

impl Dispatch<astrea_shell_control_manager_v1::AstreaShellControlManagerV1, ()> for LaunchState {
    fn event(
        _state: &mut Self,
        _proxy: &astrea_shell_control_manager_v1::AstreaShellControlManagerV1,
        _event: astrea_shell_control_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<astrea_launch_request_v1::AstreaLaunchRequestV1, ()> for LaunchState {
    fn event(
        state: &mut Self,
        _proxy: &astrea_launch_request_v1::AstreaLaunchRequestV1,
        event: astrea_launch_request_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            astrea_launch_request_v1::Event::Accepted { pid } => {
                state.pid = Some(pid);
                state.done = true;
            }
            astrea_launch_request_v1::Event::Failed { code, message } => {
                state.error = Some(format!("request failed ({code}): {message}"));
                state.done = true;
            }
            astrea_launch_request_v1::Event::Finished { status } => {
                state.error = Some(format!("child exited before mapping a surface: {status}"));
                state.done = true;
            }
        }
    }
}
