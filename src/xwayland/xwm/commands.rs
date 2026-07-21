use crate::xwayland::trace::{self, TraceFields};
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

use super::{
    X11ConfigureFlags, X11Geometry, X11PublishedState, X11StackMode, Xwm, XwmCommand, XwmError,
    XwmEvent,
    atoms::XwmAtomName,
    ewmh::publishable_state,
    focus::{FocusModel, focus_model, should_send_take_focus},
};

pub(crate) fn execute(xwm: &mut Xwm, command: XwmCommand) -> Result<(), XwmError> {
    trace::emit("xwm_command", || {
        TraceFields::new()
            .field("source", "xwm")
            .field("command", format!("{command:?}"))
    });
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
    if let XwmCommand::Stack {
        sibling: Some(sibling),
        ..
    } = &command
    {
        validate_handle(xwm, *sibling)?;
    }
    if let XwmCommand::RaiseFamily { family } | XwmCommand::StackFamily { family, .. } = &command {
        for handle in family {
            validate_handle(xwm, *handle)?;
        }
    }

    match command {
        XwmCommand::Map(handle) => {
            if !xwm
                .windows
                .map_command_is_new(handle)
                .map_err(XwmError::InvalidCommand)?
            {
                return Ok(());
            }
            xwm.connection
                .map_window(handle.xid())
                .map_err(XwmError::Connection)?;
            xwm.connection
                .change_property32(
                    PropMode::REPLACE,
                    handle.xid(),
                    xwm.atoms.get(XwmAtomName::WmState),
                    xwm.atoms.get(XwmAtomName::WmState),
                    &[1, 0],
                )
                .map_err(XwmError::Connection)?;
            xwm.connection
                .change_property32(
                    PropMode::REPLACE,
                    handle.xid(),
                    xwm.atoms.get(XwmAtomName::NetFrameExtents),
                    AtomEnum::CARDINAL,
                    &[0, 0, 0, 0],
                )
                .map_err(XwmError::Connection)?;
            xwm.note_family_order(&[handle]);
            set_lifecycle_map_commanded(xwm, handle);
        }
        XwmCommand::Unmap(handle) => {
            xwm.connection
                .unmap_window(handle.xid())
                .map_err(XwmError::Connection)?;
            xwm.clear_resize_sync(handle);
            set_lifecycle_withdrawn(xwm, handle);
        }
        XwmCommand::Configure {
            window,
            geometry,
            fields,
            border_width,
        } => {
            if (fields.x || fields.y || fields.width || fields.height || fields.border_width)
                && queue_resize_desired(xwm, window, geometry, true)?
            {
                return Ok(());
            }
            xwm.connection
                .configure_window(window.xid(), &configure_aux(geometry, fields, border_width))
                .map_err(XwmError::Connection)?;
            xwm.note_expected_configure(window, geometry);
            if xwm.immediate_resize_windows.remove(&window)
                || xwm.fallback_resize_windows.remove(&window)
            {
                xwm.last_resize_geometries.remove(&window);
                xwm.outgoing_events
                    .push_back(XwmEvent::ResizeSyncImmediate(window));
            }
        }
        XwmCommand::ConfigureNotify { window, geometry } => {
            let event = xproto::ConfigureNotifyEvent {
                response_type: xproto::CONFIGURE_NOTIFY_EVENT,
                sequence: 0,
                event: window.xid(),
                window: window.xid(),
                above_sibling: x11rb::NONE,
                x: geometry.x as i16,
                y: geometry.y as i16,
                width: geometry.width.min(u32::from(u16::MAX)) as u16,
                height: geometry.height.min(u32::from(u16::MAX)) as u16,
                border_width: 0,
                override_redirect: false,
            };
            xwm.connection
                .send_event(
                    false,
                    window.xid(),
                    xproto::EventMask::STRUCTURE_NOTIFY,
                    event,
                )
                .map_err(XwmError::Connection)?;
        }
        XwmCommand::Stack {
            window,
            sibling,
            mode,
        } => {
            xwm.note_family_order(&[window]);
            if sibling.is_none() && matches!(mode, X11StackMode::Above) {
                for family_handle in transient_family_handles(xwm, window) {
                    xwm.connection
                        .configure_window(
                            family_handle.xid(),
                            &ConfigureWindowAux::new().stack_mode(to_x11_stack_mode(mode)),
                        )
                        .map_err(XwmError::Connection)?;
                }
            } else {
                let mut aux = ConfigureWindowAux::new().stack_mode(to_x11_stack_mode(mode));
                if let Some(sibling) = sibling {
                    aux = aux.sibling(sibling.xid());
                }
                xwm.connection
                    .configure_window(window.xid(), &aux)
                    .map_err(XwmError::Connection)?;
            }
        }
        XwmCommand::StackFamily { family, mode } => {
            xwm.note_family_order(&family);
            for handle in family {
                xwm.connection
                    .configure_window(
                        handle.xid(),
                        &ConfigureWindowAux::new().stack_mode(to_x11_stack_mode(mode)),
                    )
                    .map_err(XwmError::Connection)?;
            }
        }
        XwmCommand::Focus { window, timestamp } => {
            xwm.note_focus_command(window, timestamp);
            let focus = window.map_or(x11rb::NONE, |handle| handle.xid());
            let model = window
                .and_then(|handle| xwm.windows.get(handle))
                .and_then(|record| record.snapshot.as_ref())
                .map(|snapshot| focus_model(snapshot.accepts_input, snapshot.supports_take_focus))
                .unwrap_or(FocusModel::Input);
            if matches!(model, FocusModel::Input) {
                xwm.connection
                    .set_input_focus(InputFocus::NONE, focus, timestamp)
                    .map_err(XwmError::Connection)?;
            }
            if let Some(handle) = window
                && xwm
                    .windows
                    .get(handle)
                    .and_then(|record| record.snapshot.as_ref())
                    .is_some_and(|snapshot| {
                        should_send_take_focus(snapshot.accepts_input, snapshot.supports_take_focus)
                    })
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
            let family = transient_family_handles(xwm, handle);
            xwm.note_family_order(&family);
            for family_handle in family {
                xwm.connection
                    .configure_window(
                        family_handle.xid(),
                        &ConfigureWindowAux::new().stack_mode(xproto::StackMode::ABOVE),
                    )
                    .map_err(XwmError::Connection)?;
            }
        }
        XwmCommand::RaiseFamily { family } => {
            xwm.note_family_order(&family);
            for handle in family {
                xwm.connection
                    .configure_window(
                        handle.xid(),
                        &ConfigureWindowAux::new().stack_mode(xproto::StackMode::ABOVE),
                    )
                    .map_err(XwmError::Connection)?;
            }
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
            final_pending,
        } => {
            trace::emit("resize_begin_commanded", || {
                TraceFields::new()
                    .field("source", "xwm")
                    .field("xid", window.xid())
                    .field("resize_counter", counter_value)
                    .field("deadline_ns", deadline_ns)
                    .field("allow_commits", false)
                    .field("final_pending", final_pending)
            });
            begin_resize_sync(
                xwm,
                window,
                geometry,
                counter_value,
                deadline_ns,
                final_pending,
            )?
        }
        XwmCommand::SetAllowCommits { window, allowed } => {
            trace::emit("resize_allow_commits_commanded", || {
                TraceFields::new()
                    .field("source", "xwm")
                    .field("xid", window.xid())
                    .field("allow_commits", allowed)
            });
            set_allow_commits(xwm, window, allowed)?;
        }
        XwmCommand::ReleaseResizeCommits {
            window,
            counter_value,
            association_serial,
            commit_floor,
        } => {
            trace::emit("resize_commits_released", || {
                TraceFields::new()
                    .field("source", "xwm")
                    .field("xid", window.xid())
                    .field("resize_counter", counter_value)
                    .field("association_serial", association_serial.get())
                    .field("commit_floor", commit_floor.get())
                    .field("allow_commits", true)
            });
            release_resize_commits(xwm, window, counter_value, association_serial, commit_floor)?;
        }
        XwmCommand::CompleteResizeSync(window) => {
            trace::emit("resize_complete_commanded", || {
                TraceFields::new()
                    .field("source", "xwm")
                    .field("xid", window.xid())
            });
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
        XwmCommand::RaiseFamily { family } | XwmCommand::StackFamily { family, .. } => {
            family.first().copied()
        }
        XwmCommand::Configure { window, .. }
        | XwmCommand::ConfigureNotify { window, .. }
        | XwmCommand::SetState { window, .. } => Some(*window),
        XwmCommand::Stack { window, .. } => Some(*window),
        XwmCommand::Focus { window, .. } => *window,
        XwmCommand::SyncClientLists { .. } => None,
        XwmCommand::BeginResizeSync { window, .. }
        | XwmCommand::SetAllowCommits { window, .. }
        | XwmCommand::ReleaseResizeCommits { window, .. }
        | XwmCommand::CompleteResizeSync(window) => Some(*window),
    }
}

fn transient_family_handles(
    xwm: &Xwm,
    requested: super::X11WindowHandle,
) -> Vec<super::X11WindowHandle> {
    let mut parent_by_handle = std::collections::HashMap::new();
    for (handle, snapshot) in xwm.windows.snapshots() {
        parent_by_handle.insert(handle, snapshot.transient_for);
    }
    let mut root = requested;
    let mut seen = std::collections::HashSet::new();
    while seen.insert(root) {
        let Some(Some(parent)) = parent_by_handle.get(&root) else {
            break;
        };
        root = *parent;
    }
    let mut family = parent_by_handle
        .keys()
        .copied()
        .filter(|handle| {
            let mut current = *handle;
            let mut seen = std::collections::HashSet::new();
            while seen.insert(current) {
                if current == root {
                    return true;
                }
                let Some(Some(parent)) = parent_by_handle.get(&current) else {
                    break;
                };
                current = *parent;
            }
            false
        })
        .collect::<Vec<_>>();
    family.sort_by_key(|handle| {
        let mut depth = 0usize;
        let mut current = *handle;
        let mut seen = std::collections::HashSet::new();
        while current != root && seen.insert(current) {
            let Some(Some(parent)) = parent_by_handle.get(&current) else {
                break;
            };
            current = *parent;
            depth += 1;
        }
        (
            depth,
            xwm.family_order.get(handle).copied().unwrap_or(u64::MAX),
        )
    });
    family
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

pub(crate) fn begin_resize_sync(
    xwm: &mut Xwm,
    window: super::X11WindowHandle,
    geometry: X11Geometry,
    counter_value: u64,
    deadline_ns: u64,
    final_pending: bool,
) -> Result<(), XwmError> {
    xwm.fallback_resize_windows.remove(&window);
    let Some(sync_counter) = xwm
        .windows
        .get(window)
        .and_then(|record| record.snapshot.as_ref())
        .and_then(|snapshot| snapshot.sync_counter)
    else {
        if xwm.last_resize_geometries.get(&window).copied() == Some(geometry) {
            if final_pending {
                xwm.immediate_resize_windows.remove(&window);
                xwm.outgoing_events
                    .push_back(XwmEvent::ResizeSyncImmediate(window));
            }
            return Ok(());
        }
        xwm.connection
            .configure_window(
                window.xid(),
                &configure_aux(geometry, X11ConfigureFlags::all(), 0),
            )
            .map_err(XwmError::Connection)?;
        if final_pending {
            xwm.immediate_resize_windows.remove(&window);
            xwm.outgoing_events
                .push_back(XwmEvent::ResizeSyncImmediate(window));
        } else {
            xwm.immediate_resize_windows.insert(window);
        }
        xwm.last_resize_geometries.insert(window, geometry);
        xwm.note_expected_configure(window, geometry);
        return Ok(());
    };
    if !xwm.capabilities.sync {
        if xwm.last_resize_geometries.get(&window).copied() == Some(geometry) {
            if final_pending {
                xwm.immediate_resize_windows.remove(&window);
                xwm.outgoing_events
                    .push_back(XwmEvent::ResizeSyncImmediate(window));
            }
            return Ok(());
        }
        xwm.connection
            .configure_window(
                window.xid(),
                &configure_aux(geometry, X11ConfigureFlags::all(), 0),
            )
            .map_err(XwmError::Connection)?;
        if final_pending {
            xwm.immediate_resize_windows.remove(&window);
            xwm.outgoing_events
                .push_back(XwmEvent::ResizeSyncImmediate(window));
        } else {
            xwm.immediate_resize_windows.insert(window);
        }
        xwm.last_resize_geometries.insert(window, geometry);
        xwm.note_expected_configure(window, geometry);
        return Ok(());
    }
    if xwm.resize_sync.is_pending(window) {
        if xwm
            .resize_sync
            .queue_desired(window, geometry, final_pending)
        {
            log_resize_event(
                if final_pending {
                    "x11_resize_final_pending"
                } else {
                    "x11_resize_coalesced"
                },
                xwm,
                window,
                Some(geometry),
                None,
            );
        }
        return Ok(());
    }
    let desired = geometry;
    let sync_counter = u32::try_from(sync_counter)
        .map_err(|_| XwmError::InvalidCommand("XSync counter ID exceeds X11 width"))?;
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
    if let Err(error) = xwm.resize_sync.begin_transaction(
        window,
        requested_counter_value,
        deadline_ns,
        desired,
        final_pending,
    ) {
        let _ = xwm.connection.sync_destroy_alarm(alarm);
        return Err(XwmError::ResizeSync(error));
    }
    xwm.sync_alarms.insert(window, alarm);
    xwm.sync_handles_by_counter.insert(sync_counter, window);

    if let Err(error) = set_allow_commits(xwm, window, false)
        .and_then(|_| {
            xwm.connection
                .configure_window(
                    window.xid(),
                    &configure_aux(geometry, X11ConfigureFlags::all(), 0),
                )
                .map_err(XwmError::Connection)
        })
        .and_then(|_| send_sync_request(xwm, window, counter_value))
    {
        let _ = set_allow_commits(xwm, window, true);
        xwm.clear_resize_sync(window);
        return Err(error);
    }
    xwm.note_expected_configure(window, geometry);
    log_resize_event(
        "x11_resize_sync_started",
        xwm,
        window,
        Some(geometry),
        Some(requested_counter_value),
    );
    Ok(())
}

fn queue_resize_desired(
    xwm: &mut Xwm,
    window: super::X11WindowHandle,
    geometry: X11Geometry,
    final_pending: bool,
) -> Result<bool, XwmError> {
    if !xwm.resize_sync.is_pending(window) {
        return Ok(false);
    }
    if xwm
        .resize_sync
        .queue_desired(window, geometry, final_pending)
    {
        log_resize_event(
            if final_pending {
                "x11_resize_final_pending"
            } else {
                "x11_resize_coalesced"
            },
            xwm,
            window,
            Some(geometry),
            None,
        );
    }
    Ok(true)
}

fn log_resize_event(
    event: &str,
    xwm: &Xwm,
    window: super::X11WindowHandle,
    geometry: Option<X11Geometry>,
    counter_value: Option<u64>,
) {
    if std::env::var_os("TYPHON_XWAYLAND_LOG").is_none() {
        return;
    }
    let current = geometry.or_else(|| {
        xwm.resize_sync
            .transaction(window)
            .map(|(_, geometry, _)| geometry)
    });
    let latest = xwm
        .resize_sync
        .desired(window)
        .map(|desired| desired.geometry);
    let transaction_id = xwm.resize_sync.transaction_id(window).unwrap_or_default();
    let counter = counter_value
        .or_else(|| match xwm.resize_sync.state(window) {
            super::ResizeSyncState::ConfigureSent { counter_value, .. }
            | super::ResizeSyncState::AckObserved { counter_value, .. }
            | super::ResizeSyncState::AckedWaitingCommit { counter_value, .. }
            | super::ResizeSyncState::Presented { counter_value } => Some(counter_value),
            _ => None,
        })
        .unwrap_or_default();
    eprintln!(
        "oblivion-one xwayland: event={event} xid={} transaction_id={} counter={} geometry={:?} latest_desired={:?}",
        window.xid(),
        transaction_id,
        counter,
        current,
        latest
    );
}

pub(crate) fn set_allow_commits(
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

fn release_resize_commits(
    xwm: &mut Xwm,
    window: super::X11WindowHandle,
    counter_value: u64,
    association_serial: std::num::NonZeroU64,
    commit_floor: crate::compositor::SurfaceCommitSequence,
) -> Result<(), XwmError> {
    let valid_ack = matches!(
        xwm.resize_sync.state(window),
        super::ResizeSyncState::AckObserved {
            counter_value: expected,
            ..
        } if expected == counter_value
    );
    if !valid_ack {
        return Err(XwmError::InvalidCommand(
            "resize commit release does not match observed ACK",
        ));
    }
    let valid_association = xwm
        .association
        .completed
        .get(&window)
        .is_some_and(|association| association.serial == association_serial);
    if !valid_association {
        return Err(XwmError::InvalidCommand(
            "resize commit release does not match current association",
        ));
    }
    set_allow_commits(xwm, window, true)?;
    xwm.connection.flush().map_err(XwmError::Connection)?;
    if !xwm
        .resize_sync
        .release_commits(window, counter_value, association_serial, commit_floor)
    {
        return Err(XwmError::InvalidCommand(
            "resize commit release was not accepted",
        ));
    }
    xwm.process_pending_resize_commits();
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
        type_: xwm.atoms.get(XwmAtomName::WmProtocols),
        data: xproto::ClientMessageData::from([
            xwm.atoms.get(XwmAtomName::NetWmSyncRequest),
            0,
            counter_value.lo,
            counter_value.hi as u32,
            0,
        ]),
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

fn configure_aux(
    geometry: X11Geometry,
    fields: X11ConfigureFlags,
    border_width: u32,
) -> ConfigureWindowAux {
    let mut aux = ConfigureWindowAux::new();
    if fields.x {
        aux = aux.x(geometry.x);
    }
    if fields.y {
        aux = aux.y(geometry.y);
    }
    if fields.width {
        aux = aux.width(geometry.width.max(1));
    }
    if fields.height {
        aux = aux.height(geometry.height.max(1));
    }
    if fields.border_width {
        aux = aux.border_width(border_width);
    }
    aux
}

fn to_x11_stack_mode(mode: X11StackMode) -> xproto::StackMode {
    match mode {
        X11StackMode::Above => xproto::StackMode::ABOVE,
        X11StackMode::Below => xproto::StackMode::BELOW,
        X11StackMode::TopIf => xproto::StackMode::TOP_IF,
        X11StackMode::BottomIf => xproto::StackMode::BOTTOM_IF,
        X11StackMode::Opposite => xproto::StackMode::OPPOSITE,
    }
}

fn publish_state(
    xwm: &mut Xwm,
    handle: super::X11WindowHandle,
    state: X11PublishedState,
) -> Result<(), XwmError> {
    let state = publishable_state(state);
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
    xwm.connection
        .change_property32(
            PropMode::REPLACE,
            handle.xid(),
            xwm.atoms.get(XwmAtomName::WmState),
            xwm.atoms.get(XwmAtomName::WmState),
            &[if state.hidden { 3 } else { 1 }, 0],
        )
        .map_err(XwmError::Connection)?;
    Ok(())
}

fn set_lifecycle_map_commanded(xwm: &mut Xwm, handle: super::X11WindowHandle) {
    let _ = xwm.windows.mark_map_commanded(handle);
}

fn set_lifecycle_withdrawn(xwm: &mut Xwm, handle: super::X11WindowHandle) {
    let _ = xwm.windows.mark_unmapped(handle);
}
