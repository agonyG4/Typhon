use x11rb::{
    connection::Connection,
    protocol::sync::{
        ConnectionExt as SyncConnectionExt, CreateAlarmAux, Int64, TESTTYPE, VALUETYPE,
    },
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
            xwm.clear_resize_sync(handle);
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
        XwmCommand::BeginResizeSync {
            window,
            geometry,
            counter_value,
            deadline_ns,
        } => begin_resize_sync(xwm, window, geometry, counter_value, deadline_ns)?,
        XwmCommand::SetAllowCommits { window, allowed } => {
            set_allow_commits(xwm, window, allowed)?;
        }
        XwmCommand::CompleteResizeSync(window) => {
            xwm.complete_resize_sync(window)?;
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
        XwmCommand::BeginResizeSync { window, .. }
        | XwmCommand::SetAllowCommits { window, .. }
        | XwmCommand::CompleteResizeSync(window) => Some(*window),
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

fn begin_resize_sync(
    xwm: &mut Xwm,
    window: super::X11WindowHandle,
    geometry: X11Geometry,
    counter_value: u64,
    deadline_ns: u64,
) -> Result<(), XwmError> {
    let Some(sync_counter) = xwm
        .windows
        .get(window)
        .and_then(|record| record.snapshot.as_ref())
        .and_then(|snapshot| snapshot.sync_counter)
    else {
        xwm.connection
            .configure_window(window.xid(), &configure_aux(geometry))
            .map_err(XwmError::Connection)?;
        return Ok(());
    };
    let sync_counter = u32::try_from(sync_counter)
        .map_err(|_| XwmError::InvalidCommand("XSync counter ID exceeds X11 width"))?;
    xwm.clear_resize_sync(window);
    let alarm = xwm
        .connection
        .generate_id()
        .map_err(|error| XwmError::IdAllocation(error.to_string()))?;
    let requested_counter_value = if counter_value == 0 {
        let next = xwm.next_resize_counter_values.entry(window).or_insert(0);
        *next = next
            .checked_add(1)
            .ok_or(XwmError::InvalidCommand("XSync counter value exhausted"))?;
        *next
    } else {
        counter_value
    };
    let counter_value = int64(requested_counter_value);
    xwm.connection
        .sync_create_alarm(
            alarm,
            &CreateAlarmAux::new()
                .counter(sync_counter)
                .value_type(VALUETYPE::ABSOLUTE)
                .value(counter_value)
                .test_type(TESTTYPE::POSITIVE_COMPARISON)
                .delta(Int64 { hi: 0, lo: 0 })
                .events(1u32),
        )
        .map_err(XwmError::Connection)?;
    if let Err(error) = xwm
        .resize_sync
        .begin(window, requested_counter_value, deadline_ns)
    {
        let _ = xwm.connection.sync_destroy_alarm(alarm);
        return Err(XwmError::ResizeSync(error));
    }
    xwm.sync_alarms.insert(window, alarm);
    xwm.sync_handles_by_counter.insert(sync_counter, window);

    if let Err(error) = set_allow_commits(xwm, window, false)
        .and_then(|_| {
            xwm.connection
                .configure_window(window.xid(), &configure_aux(geometry))
                .map_err(XwmError::Connection)
        })
        .and_then(|_| send_sync_request(xwm, window, counter_value))
    {
        xwm.clear_resize_sync(window);
        return Err(error);
    }
    Ok(())
}

fn set_allow_commits(
    xwm: &Xwm,
    window: super::X11WindowHandle,
    allowed: bool,
) -> Result<(), XwmError> {
    xwm.connection
        .change_property32(
            PropMode::REPLACE,
            window.xid(),
            xwm.atoms.get(XwmAtomName::XwaylandAllowCommits),
            AtomEnum::CARDINAL,
            &[u32::from(allowed)],
        )
        .map_err(XwmError::Connection)?;
    Ok(())
}

fn send_sync_request(
    xwm: &Xwm,
    window: super::X11WindowHandle,
    counter_value: Int64,
) -> Result<(), XwmError> {
    let event = xproto::ClientMessageEvent {
        response_type: xproto::CLIENT_MESSAGE_EVENT,
        format: 32,
        sequence: 0,
        window: window.xid(),
        type_: xwm.atoms.get(XwmAtomName::NetWmSyncRequest),
        data: xproto::ClientMessageData::from([0, counter_value.lo, counter_value.hi as u32, 0, 0]),
    };
    xwm.connection
        .send_event(false, window.xid(), xproto::EventMask::NO_EVENT, event)
        .map_err(XwmError::Connection)?;
    Ok(())
}

fn int64(value: u64) -> Int64 {
    Int64 {
        hi: (value >> 32) as i32,
        lo: value as u32,
    }
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
