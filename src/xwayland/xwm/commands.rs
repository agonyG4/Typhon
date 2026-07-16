use x11rb::{
    connection::Connection,
    protocol::xproto::{
        self, AtomEnum, ConfigureWindowAux, ConnectionExt as XprotoConnectionExt, InputFocus,
        PropMode,
    },
    wrapper::ConnectionExt,
};

use super::{X11Geometry, X11PublishedState, Xwm, XwmCommand, XwmError, atoms::XwmAtomName};

pub(crate) fn execute(xwm: &mut Xwm, command: XwmCommand) -> Result<(), XwmError> {
    if let XwmCommand::SyncClientLists {
        client_list,
        stacking,
    } = &command
    {
        for handle in client_list.iter().chain(stacking.iter()) {
            validate_handle(xwm, *handle)?;
        }
    }
    let handle = command_handle(&command);
    if let Some(handle) = handle {
        validate_handle(xwm, handle)?;
    }

    match command {
        XwmCommand::Map(handle) => {
            xwm.connection
                .map_window(handle.xid())
                .map_err(XwmError::Connection)?;
            set_lifecycle_mapped(xwm, handle);
        }
        XwmCommand::Unmap(handle) => {
            xwm.connection
                .unmap_window(handle.xid())
                .map_err(XwmError::Connection)?;
            set_lifecycle_withdrawn(xwm, handle);
        }
        XwmCommand::Configure { window, geometry } => {
            xwm.connection
                .configure_window(window.xid(), &configure_aux(geometry))
                .map_err(XwmError::Connection)?;
        }
        XwmCommand::Focus { window, timestamp } => {
            let focus = window.map_or(x11rb::NONE, |handle| handle.xid());
            xwm.connection
                .set_input_focus(InputFocus::NONE, focus, timestamp)
                .map_err(XwmError::Connection)?;
            if let Some(handle) = window
                && xwm
                    .windows
                    .get(handle)
                    .and_then(|record| record.snapshot.as_ref())
                    .is_some_and(|snapshot| snapshot.supports_take_focus)
            {
                let event = xproto::ClientMessageEvent {
                    response_type: xproto::CLIENT_MESSAGE_EVENT,
                    format: 32,
                    sequence: 0,
                    window: handle.xid(),
                    type_: xwm.atoms.get(XwmAtomName::WmProtocols),
                    data: xproto::ClientMessageData::from([
                        xwm.atoms.get(XwmAtomName::WmTakeFocus),
                        timestamp,
                        0,
                        0,
                        0,
                    ]),
                };
                xwm.connection
                    .send_event(false, handle.xid(), xproto::EventMask::NO_EVENT, event)
                    .map_err(XwmError::Connection)?;
            }
            let value = window.map_or_else(Vec::new, |handle| vec![handle.xid()]);
            if value.is_empty() {
                xwm.connection
                    .delete_property(xwm.root, xwm.atoms.get(XwmAtomName::NetActiveWindow))
                    .map_err(XwmError::Connection)?;
            } else {
                xwm.connection
                    .change_property32(
                        PropMode::REPLACE,
                        xwm.root,
                        xwm.atoms.get(XwmAtomName::NetActiveWindow),
                        AtomEnum::WINDOW,
                        &value,
                    )
                    .map_err(XwmError::Connection)?;
            }
        }
        XwmCommand::Raise(handle) => {
            xwm.connection
                .configure_window(
                    handle.xid(),
                    &ConfigureWindowAux::new().stack_mode(xproto::StackMode::ABOVE),
                )
                .map_err(XwmError::Connection)?;
        }
        XwmCommand::Close(handle) => {
            let supports_delete = xwm
                .windows
                .get(handle)
                .and_then(|record| record.snapshot.as_ref())
                .is_some_and(|snapshot| snapshot.supports_delete);
            if supports_delete {
                let event = xproto::ClientMessageEvent {
                    response_type: xproto::CLIENT_MESSAGE_EVENT,
                    format: 32,
                    sequence: 0,
                    window: handle.xid(),
                    type_: xwm.atoms.get(XwmAtomName::WmProtocols),
                    data: xproto::ClientMessageData::from([
                        xwm.atoms.get(XwmAtomName::WmDeleteWindow),
                        0,
                        0,
                        0,
                        0,
                    ]),
                };
                xwm.connection
                    .send_event(false, handle.xid(), xproto::EventMask::NO_EVENT, event)
                    .map_err(XwmError::Connection)?;
            } else {
                xwm.connection
                    .kill_client(handle.xid())
                    .map_err(XwmError::Connection)?;
            }
        }
        XwmCommand::SetState { window, state } => {
            publish_state(xwm, window, state)?;
        }
        XwmCommand::SyncClientLists {
            client_list,
            stacking,
        } => {
            publish_client_list(xwm, XwmAtomName::NetClientList, &client_list)?;
            publish_client_list(xwm, XwmAtomName::NetClientListStacking, &stacking)?;
        }
    }
    Ok(())
}

pub(crate) fn flush(xwm: &Xwm) -> Result<(), XwmError> {
    xwm.connection.flush().map_err(XwmError::Connection)
}

fn command_handle(command: &XwmCommand) -> Option<super::X11WindowHandle> {
    match command {
        XwmCommand::Map(handle)
        | XwmCommand::Unmap(handle)
        | XwmCommand::Raise(handle)
        | XwmCommand::Close(handle) => Some(*handle),
        XwmCommand::Configure { window, .. } | XwmCommand::SetState { window, .. } => Some(*window),
        XwmCommand::Focus { window, .. } => *window,
        XwmCommand::SyncClientLists { .. } => None,
    }
}

fn publish_client_list(
    xwm: &Xwm,
    atom: XwmAtomName,
    handles: &[super::X11WindowHandle],
) -> Result<(), XwmError> {
    let values = handles
        .iter()
        .map(|handle| handle.xid())
        .collect::<Vec<_>>();
    xwm.connection
        .change_property32(
            PropMode::REPLACE,
            xwm.root,
            xwm.atoms.get(atom),
            AtomEnum::WINDOW,
            &values,
        )
        .map_err(XwmError::Connection)?;
    Ok(())
}

fn validate_handle(xwm: &Xwm, handle: super::X11WindowHandle) -> Result<(), XwmError> {
    if handle.generation() != xwm.generation {
        return Err(XwmError::StaleGeneration);
    }
    if !xwm.windows.contains(handle) {
        return Err(XwmError::InvalidCommand("unknown X11 window"));
    }
    Ok(())
}

fn configure_aux(geometry: X11Geometry) -> ConfigureWindowAux {
    ConfigureWindowAux::new()
        .x(geometry.x)
        .y(geometry.y)
        .width(geometry.width.max(1))
        .height(geometry.height.max(1))
}

fn publish_state(
    xwm: &mut Xwm,
    handle: super::X11WindowHandle,
    state: X11PublishedState,
) -> Result<(), XwmError> {
    let mut atoms = Vec::with_capacity(3);
    if state.fullscreen {
        atoms.push(xwm.atoms.get(XwmAtomName::NetWmStateFullscreen));
    }
    if state.maximized {
        atoms.push(xwm.atoms.get(XwmAtomName::NetWmStateMaximizedVert));
        atoms.push(xwm.atoms.get(XwmAtomName::NetWmStateMaximizedHorz));
    }
    if state.hidden {
        atoms.push(xwm.atoms.get(XwmAtomName::NetWmStateHidden));
    }
    xwm.connection
        .change_property32(
            PropMode::REPLACE,
            handle.xid(),
            xwm.atoms.get(XwmAtomName::NetWmState),
            AtomEnum::ATOM,
            &atoms,
        )
        .map_err(XwmError::Connection)?;
    Ok(())
}

fn set_lifecycle_mapped(xwm: &mut Xwm, handle: super::X11WindowHandle) {
    let _ = xwm.windows.mark_mapped(handle);
}

fn set_lifecycle_withdrawn(xwm: &mut Xwm, handle: super::X11WindowHandle) {
    let _ = xwm.windows.mark_unmapped(handle);
}
