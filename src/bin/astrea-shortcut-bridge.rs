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

#[path = "../astrea_shortcuts.rs"]
mod astrea_shortcuts;

use std::io::{self, Write};

use astrea_shortcuts::client::{astrea_shortcut_v1, astrea_shortcuts_manager_v1};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};

#[derive(Default)]
struct BridgeState;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(namespace) = args.next() else {
        return Err("usage: astrea-shortcut-bridge <namespace> <name> [description]".into());
    };
    let Some(name) = args.next() else {
        return Err("usage: astrea-shortcut-bridge <namespace> <name> [description]".into());
    };
    let description = args.next().unwrap_or_default();

    let connection = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<BridgeState>(&connection)?;
    let qh = queue.handle();
    let manager: astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1 =
        globals.bind(&qh, 1..=1, ())?;
    let _shortcut = manager.register_shortcut(namespace, name, description, &qh, ());
    connection.flush()?;

    let mut state = BridgeState;
    loop {
        queue.blocking_dispatch(&mut state)?;
        connection.flush()?;
    }
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

impl Dispatch<astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1, ()> for BridgeState {
    fn event(
        _state: &mut Self,
        _proxy: &astrea_shortcuts_manager_v1::AstreaShortcutsManagerV1,
        _event: astrea_shortcuts_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<astrea_shortcut_v1::AstreaShortcutV1, ()> for BridgeState {
    fn event(
        _state: &mut Self,
        _proxy: &astrea_shortcut_v1::AstreaShortcutV1,
        event: astrea_shortcut_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let mut stdout = io::stdout().lock();
        match event {
            astrea_shortcut_v1::Event::Pressed { serial, timestamp } => {
                let _ = writeln!(stdout, "pressed {serial} {timestamp}");
            }
            astrea_shortcut_v1::Event::Repeated { serial, timestamp } => {
                let _ = writeln!(stdout, "repeated {serial} {timestamp}");
            }
            astrea_shortcut_v1::Event::Released { serial, timestamp } => {
                let _ = writeln!(stdout, "released {serial} {timestamp}");
            }
            astrea_shortcut_v1::Event::Cancelled { serial } => {
                let _ = writeln!(stdout, "cancelled {serial}");
            }
        }
        let _ = stdout.flush();
    }
}
