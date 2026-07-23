use std::collections::HashSet;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XwmCommandOutcome {
    Applied,
    DroppedTargetGone {
        window: super::X11WindowHandle,
    },
    DroppedStaleGeneration {
        window: Option<super::X11WindowHandle>,
    },
    AppliedAfterPruning {
        dropped_handles: usize,
    },
}

impl XwmCommand {
    pub(crate) fn kind_name(&self) -> &'static str {
        match self {
            Self::Map(_) => "Map",
            Self::Unmap(_) => "Unmap",
            Self::Configure { .. } => "Configure",
            Self::ConfigureFrame { .. } => "ConfigureFrame",
            Self::ConfigureNotify { .. } => "ConfigureNotify",
            Self::Stack { .. } => "Stack",
            Self::Focus { .. } => "Focus",
            Self::Raise(_) => "Raise",
            Self::RaiseAndSync { .. } => "RaiseAndSync",
            Self::RestackExact { .. } => "RestackExact",
            Self::RaiseFamily { .. } => "RaiseFamily",
            Self::StackFamily { .. } => "StackFamily",
            Self::Close(_) => "Close",
            Self::SetState { .. } => "SetState",
            Self::SyncClientLists { .. } => "SyncClientLists",
            Self::BeginResizeSync { .. } => "BeginResizeSync",
            Self::SetAllowCommits { .. } => "SetAllowCommits",
            Self::ReleaseResizeCommits { .. } => "ReleaseResizeCommits",
            Self::CompleteResizeSync(_) => "CompleteResizeSync",
        }
    }

    pub(crate) fn primary_handle(&self) -> Option<super::X11WindowHandle> {
        match self {
            Self::Map(handle)
            | Self::Unmap(handle)
            | Self::Raise(handle)
            | Self::Close(handle)
            | Self::CompleteResizeSync(handle) => Some(*handle),
            Self::RaiseAndSync { window, .. }
            | Self::Configure { window, .. }
            | Self::ConfigureFrame { window, .. }
            | Self::ConfigureNotify { window, .. }
            | Self::Stack { window, .. }
            | Self::SetState { window, .. }
            | Self::BeginResizeSync { window, .. }
            | Self::SetAllowCommits { window, .. }
            | Self::ReleaseResizeCommits { window, .. } => Some(*window),
            Self::RaiseFamily { family } | Self::StackFamily { family, .. } => {
                family.first().copied()
            }
            Self::RestackExact { order, .. } => order.first().copied(),
            Self::Focus { window, .. } => *window,
            Self::SyncClientLists { .. } => None,
        }
    }
}

pub(crate) fn execute(xwm: &mut Xwm, command: XwmCommand) -> Result<XwmCommandOutcome, XwmError> {
    trace::emit("xwm_command", || {
        TraceFields::new()
            .field("source", "xwm")
            .field("command", format!("{command:?}"))
    });
    let (command, pruned_handles) = match normalize_command(xwm, command)? {
        NormalizedCommand::Drop(outcome) => return Ok(outcome),
        NormalizedCommand::Execute {
            command,
            pruned_handles,
        } => (command, pruned_handles),
    };

    match command {
        XwmCommand::Map(handle) => {
            if !xwm
                .windows
                .map_command_is_new(handle)
                .map_err(XwmError::InvalidCommand)?
            {
                return Ok(command_outcome(pruned_handles));
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
            let surface_id = xwm
                .windows
                .get(handle)
                .and_then(|record| record.association.map(|association| association.surface_id));
            xwm.connection
                .unmap_window(handle.xid())
                .map_err(XwmError::Connection)?;
            xwm.clear_resize_sync(handle);
            if let Some(surface_id) = surface_id {
                xwm.clear_surface_buffer_ready(surface_id);
            }
            xwm.windows
                .mark_wm_unmap_requested(handle)
                .map_err(XwmError::InvalidCommand)?;
        }
        XwmCommand::Configure {
            window,
            geometry,
            fields,
            border_width,
        } => {
            if (fields.width || fields.height || fields.border_width)
                && queue_resize_desired(xwm, window, geometry, true)?
            {
                return Ok(command_outcome(pruned_handles));
            }
            if !fields.width
                && !fields.height
                && !fields.border_width
                && xwm.resize_sync.is_pending(window)
            {
                let _ = xwm
                    .resize_sync
                    .merge_desired_position(window, geometry.x, geometry.y);
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
                    .push_back(XwmEvent::ResizeSyncImmediate { window, geometry });
            }
        }
        XwmCommand::ConfigureFrame { window, geometry } => {
            xwm.connection
                .configure_window(
                    window.xid(),
                    &configure_aux(geometry, X11ConfigureFlags::all(), 0),
                )
                .map_err(XwmError::Connection)?;
            xwm.note_expected_configure(window, geometry);
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
            let mut aux = ConfigureWindowAux::new().stack_mode(to_x11_stack_mode(mode));
            if let Some(sibling) = sibling {
                aux = aux.sibling(sibling.xid());
            }
            xwm.connection
                .configure_window(window.xid(), &aux)
                .map_err(XwmError::Connection)?;
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
            xwm.note_family_order(&[handle]);
            xwm.connection
                .configure_window(
                    handle.xid(),
                    &ConfigureWindowAux::new().stack_mode(xproto::StackMode::ABOVE),
                )
                .map_err(XwmError::Connection)?;
        }
        XwmCommand::RaiseAndSync {
            window,
            client_list,
            stacking,
        } => {
            xwm.note_family_order(&[window]);
            xwm.connection
                .configure_window(
                    window.xid(),
                    &ConfigureWindowAux::new().stack_mode(xproto::StackMode::ABOVE),
                )
                .map_err(XwmError::Connection)?;
            publish_client_list(xwm, XwmAtomName::NetClientList, &client_list)?;
            publish_client_list(xwm, XwmAtomName::NetClientListStacking, &stacking)?;
        }
        XwmCommand::RestackExact {
            order,
            client_list,
            stacking,
        } => {
            if order.len() >= 2 {
                xwm.note_family_order(&order);
                let mut previous: Option<super::X11WindowHandle> = None;
                for handle in order {
                    let mut aux = ConfigureWindowAux::new().stack_mode(xproto::StackMode::ABOVE);
                    if let Some(sibling) = previous {
                        aux = aux.sibling(sibling.xid());
                    }
                    xwm.connection
                        .configure_window(handle.xid(), &aux)
                        .map_err(XwmError::Connection)?;
                    previous = Some(handle);
                }
            }
            publish_client_list(xwm, XwmAtomName::NetClientList, &client_list)?;
            publish_client_list(xwm, XwmAtomName::NetClientListStacking, &stacking)?;
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
    Ok(command_outcome(pruned_handles))
}

enum NormalizedCommand {
    Drop(XwmCommandOutcome),
    Execute {
        command: XwmCommand,
        pruned_handles: usize,
    },
}

fn command_outcome(pruned_handles: usize) -> XwmCommandOutcome {
    if pruned_handles == 0 {
        XwmCommandOutcome::Applied
    } else {
        XwmCommandOutcome::AppliedAfterPruning {
            dropped_handles: pruned_handles,
        }
    }
}

fn normalize_command(xwm: &Xwm, command: XwmCommand) -> Result<NormalizedCommand, XwmError> {
    let primary = command.primary_handle();
    let single_target = !matches!(
        command,
        XwmCommand::RaiseAndSync { .. }
            | XwmCommand::RestackExact { .. }
            | XwmCommand::RaiseFamily { .. }
            | XwmCommand::StackFamily { .. }
            | XwmCommand::SyncClientLists { .. }
    );
    if single_target && let Some(handle) = primary {
        if handle.generation() != xwm.generation {
            return Ok(NormalizedCommand::Drop(
                XwmCommandOutcome::DroppedStaleGeneration {
                    window: Some(handle),
                },
            ));
        }
        if !xwm.windows.contains(handle) {
            return Ok(NormalizedCommand::Drop(
                XwmCommandOutcome::DroppedTargetGone { window: handle },
            ));
        }
    }

    match command {
        XwmCommand::Stack {
            window,
            sibling,
            mode,
        } => {
            let (sibling, pruned_handles) = match sibling {
                Some(sibling)
                    if sibling.generation() != xwm.generation || !xwm.windows.contains(sibling) =>
                {
                    (None, 1)
                }
                sibling => (sibling, 0),
            };
            Ok(NormalizedCommand::Execute {
                command: XwmCommand::Stack {
                    window,
                    sibling,
                    mode,
                },
                pruned_handles,
            })
        }
        XwmCommand::RaiseAndSync {
            window,
            client_list,
            stacking,
        } => {
            let (client_list, client_pruned) = prune_handles(xwm, client_list);
            let (stacking, stacking_pruned) = prune_handles(xwm, stacking);
            let pruned_handles = client_pruned.saturating_add(stacking_pruned);
            if window.generation() != xwm.generation || !xwm.windows.contains(window) {
                return Ok(NormalizedCommand::Execute {
                    command: XwmCommand::SyncClientLists {
                        client_list,
                        stacking,
                    },
                    pruned_handles: pruned_handles.saturating_add(1),
                });
            }
            Ok(NormalizedCommand::Execute {
                command: XwmCommand::RaiseAndSync {
                    window,
                    client_list,
                    stacking,
                },
                pruned_handles,
            })
        }
        XwmCommand::RestackExact {
            order,
            client_list,
            stacking,
        } => {
            let (order, order_pruned) = prune_handles(xwm, order);
            let (client_list, client_pruned) = prune_handles(xwm, client_list);
            let (stacking, stacking_pruned) = prune_handles(xwm, stacking);
            Ok(NormalizedCommand::Execute {
                command: XwmCommand::RestackExact {
                    order,
                    client_list,
                    stacking,
                },
                pruned_handles: order_pruned
                    .saturating_add(client_pruned)
                    .saturating_add(stacking_pruned),
            })
        }
        XwmCommand::RaiseFamily { family } => {
            let (family, pruned_handles) = prune_handles(xwm, family);
            Ok(NormalizedCommand::Execute {
                command: XwmCommand::RaiseFamily { family },
                pruned_handles,
            })
        }
        XwmCommand::StackFamily { family, mode } => {
            let (family, pruned_handles) = prune_handles(xwm, family);
            Ok(NormalizedCommand::Execute {
                command: XwmCommand::StackFamily { family, mode },
                pruned_handles,
            })
        }
        XwmCommand::SyncClientLists {
            client_list,
            stacking,
        } => {
            let (client_list, client_pruned) = prune_handles(xwm, client_list);
            let (stacking, stacking_pruned) = prune_handles(xwm, stacking);
            Ok(NormalizedCommand::Execute {
                command: XwmCommand::SyncClientLists {
                    client_list,
                    stacking,
                },
                pruned_handles: client_pruned.saturating_add(stacking_pruned),
            })
        }
        command => Ok(NormalizedCommand::Execute {
            command,
            pruned_handles: 0,
        }),
    }
}

fn prune_handles(
    xwm: &Xwm,
    handles: Vec<super::X11WindowHandle>,
) -> (Vec<super::X11WindowHandle>, usize) {
    let mut seen = HashSet::new();
    let mut dropped_handles: usize = 0;
    let live = handles
        .into_iter()
        .filter(|handle| {
            let valid = handle.generation() == xwm.generation && xwm.windows.contains(*handle);
            if !valid || !seen.insert(*handle) {
                dropped_handles = dropped_handles.saturating_add(1);
                false
            } else {
                true
            }
        })
        .collect();
    (live, dropped_handles)
}

pub(crate) fn flush(xwm: &Xwm) -> Result<(), XwmError> {
    trace::emit("x11_resize_command_order", || {
        TraceFields::new()
            .field("source", "xwm")
            .field("command_order", "flush")
    });
    xwm.connection.flush().map_err(XwmError::Connection)
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

pub(crate) fn configure_immediate(
    xwm: &mut Xwm,
    window: super::X11WindowHandle,
    geometry: X11Geometry,
    final_pending: bool,
) -> Result<(), XwmError> {
    if xwm.last_resize_geometries.get(&window).copied() == Some(geometry) {
        if final_pending {
            xwm.immediate_resize_windows.remove(&window);
            xwm.outgoing_events
                .push_back(XwmEvent::ResizeSyncImmediate { window, geometry });
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
            .push_back(XwmEvent::ResizeSyncImmediate { window, geometry });
    } else {
        xwm.immediate_resize_windows.insert(window);
    }
    xwm.last_resize_geometries.insert(window, geometry);
    xwm.note_expected_configure(window, geometry);
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
        .filter(|snapshot| snapshot.supports_sync_request)
        .and_then(|snapshot| snapshot.sync_counter)
    else {
        return configure_immediate(xwm, window, geometry, final_pending);
    };
    if !xwm.capabilities.sync {
        return configure_immediate(xwm, window, geometry, final_pending);
    }
    if xwm.resize_sync.sync_disabled(window) {
        return configure_immediate(xwm, window, geometry, final_pending);
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
    if xwm.last_resize_geometries.get(&window).copied() == Some(geometry) {
        if final_pending {
            xwm.outgoing_events
                .push_back(XwmEvent::ResizeSyncImmediate { window, geometry });
        }
        return Ok(());
    }
    let desired = geometry;
    let sync_counter = u32::try_from(sync_counter)
        .map_err(|_| XwmError::InvalidCommand("XSync counter ID exceeds X11 width"))?;
    initialize_sync_counter(xwm, window, sync_counter)?;
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

    let command_error = set_allow_commits(xwm, window, false)
        .map(|()| {
            trace::emit("x11_resize_command_order", || {
                TraceFields::new()
                    .field("source", "xwm")
                    .field("xid", window.xid())
                    .field(
                        "transaction_id",
                        xwm.resize_sync.transaction_id(window).unwrap_or_default(),
                    )
                    .field("command_order", "allow_off")
            });
        })
        .and_then(|()| {
            send_sync_request(xwm, window, counter_value).map(|()| {
                trace::emit("x11_resize_command_order", || {
                    TraceFields::new()
                        .field("source", "xwm")
                        .field("xid", window.xid())
                        .field(
                            "transaction_id",
                            xwm.resize_sync.transaction_id(window).unwrap_or_default(),
                        )
                        .field("command_order", "sync_message")
                });
            })
        })
        .and_then(|()| {
            xwm.connection
                .configure_window(
                    window.xid(),
                    &configure_aux(geometry, X11ConfigureFlags::all(), 0),
                )
                .map_err(XwmError::Connection)
                .map(|_| {
                    trace::emit("x11_resize_command_order", || {
                        TraceFields::new()
                            .field("source", "xwm")
                            .field("xid", window.xid())
                            .field(
                                "transaction_id",
                                xwm.resize_sync.transaction_id(window).unwrap_or_default(),
                            )
                            .field("command_order", "configure")
                    });
                })
        })
        .err();
    if let Some(error) = command_error {
        let _ = set_allow_commits(xwm, window, true);
        xwm.clear_resize_sync(window);
        return Err(error);
    }
    xwm.last_resize_geometries.insert(window, geometry);
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

pub(crate) fn initialize_sync_counter(
    xwm: &mut Xwm,
    window: super::X11WindowHandle,
    sync_counter: u32,
) -> Result<(), XwmError> {
    if xwm.sync_counter_initializations.get(&window) == Some(&sync_counter) {
        return Ok(());
    }
    // The client may have created and advanced this counter before the WM
    // admitted the window. Establish a known baseline before creating the
    // first alarm; generated request serials start strictly above it.
    xwm.connection
        .sync_set_counter(sync_counter, Int64 { hi: 0, lo: 0 })
        .map_err(XwmError::Connection)?;
    xwm.sync_counter_initializations
        .insert(window, sync_counter);
    xwm.next_resize_counter_values.insert(window, 0);
    trace::emit("sync_counter_initialized", || {
        TraceFields::new()
            .field("source", "xwm")
            .field("xid", window.xid())
            .field("sync_counter", sync_counter)
            .field("baseline", 0)
    });
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
            xwm.focus.current_server_time(),
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
