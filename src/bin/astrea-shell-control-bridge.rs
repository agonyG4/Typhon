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

use std::{env, process::ExitCode};

use astrea_shell_control::client::{astrea_launch_request_v1, astrea_shell_control_manager_v1};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};

const MAX_JSON_BYTES: usize = 64 * 1024;
const MAX_DESKTOP_ID_BYTES: usize = 1024;

#[derive(Default)]
struct BridgeState {
    done: bool,
    accepted_pid: Option<u32>,
    error: Option<String>,
}

fn main() -> ExitCode {
    match run() {
        Ok(pid) => {
            println!("accepted {pid}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("astrea-shell-control-bridge: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<u32, String> {
    let mut args = env::args().skip(1);
    let op = args.next().ok_or_else(usage)?;
    let payload = args.next().ok_or_else(usage)?;
    if args.next().is_some() {
        return Err(usage());
    }

    match op.as_str() {
        "launch-desktop" | "--desktop" => validate_desktop_id(&payload)?,
        "launch-argv-json" | "--argv-json" => validate_argv_json(&payload)?,
        _ => return Err(usage()),
    }

    let connection =
        Connection::connect_to_env().map_err(|err| format!("connect failed: {err}"))?;
    let (globals, mut queue) = registry_queue_init::<BridgeState>(&connection)
        .map_err(|err| format!("registry failed: {err}"))?;
    let qh = queue.handle();
    let manager: astrea_shell_control_manager_v1::AstreaShellControlManagerV1 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|err| format!("shell-control global unavailable: {err}"))?;
    match op.as_str() {
        "launch-desktop" | "--desktop" => {
            let _request = manager.launch_desktop(payload, &qh, ());
        }
        "launch-argv-json" | "--argv-json" => {
            let _request = manager.launch_argv_json(payload, &qh, ());
        }
        _ => unreachable!(),
    }
    connection
        .flush()
        .map_err(|err| format!("flush failed: {err}"))?;

    let mut state = BridgeState::default();
    while !state.done {
        queue
            .blocking_dispatch(&mut state)
            .map_err(|err| format!("dispatch failed: {err}"))?;
        connection
            .flush()
            .map_err(|err| format!("flush failed: {err}"))?;
    }
    state
        .accepted_pid
        .ok_or_else(|| state.error.unwrap_or_else(|| "request failed".to_string()))
}

fn usage() -> String {
    "usage: astrea-shell-control-bridge launch-desktop <desktop-id> | launch-argv-json <json-array>"
        .to_string()
}

fn validate_desktop_id(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("desktop id is empty".to_string());
    }
    if value.len() > MAX_DESKTOP_ID_BYTES {
        return Err("desktop id is too large".to_string());
    }
    if value.bytes().any(|byte| byte == 0) {
        return Err("desktop id contains NUL".to_string());
    }
    if value.contains('/') || value.contains('\\') || value.contains("..") {
        return Err("desktop id must not be a path".to_string());
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
    {
        return Err("desktop id contains unsupported characters".to_string());
    }
    Ok(())
}

fn validate_argv_json(value: &str) -> Result<(), String> {
    if value.len() > MAX_JSON_BYTES {
        return Err("argv JSON is too large".to_string());
    }
    let argv: Vec<String> =
        serde_json::from_str(value).map_err(|err| format!("malformed argv JSON: {err}"))?;
    let Some(program) = argv.first() else {
        return Err("argv is empty".to_string());
    };
    if program.is_empty() {
        return Err("argv program is empty".to_string());
    }
    if argv.iter().any(|arg| arg.bytes().any(|byte| byte == 0)) {
        return Err("argv contains NUL".to_string());
    }
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for BridgeState {
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

impl Dispatch<astrea_shell_control_manager_v1::AstreaShellControlManagerV1, ()> for BridgeState {
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

impl Dispatch<astrea_launch_request_v1::AstreaLaunchRequestV1, ()> for BridgeState {
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
                state.accepted_pid = Some(pid);
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
