use super::*;

pub(super) fn create_source_proxy(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    now_ns: u64,
) -> Result<(), XwmError> {
    xwm.data_bridge
        .dnd_outgoing
        .initialize_generation(xwm.generation);
    let window = xwm
        .connection
        .generate_id()
        .map_err(|error| XwmError::IdAllocation(error.to_string()))?;
    let cookie = xwm
        .connection
        .create_window(
            0,
            window,
            xwm.supporting_wm_check,
            0,
            0,
            1,
            1,
            0,
            xproto::WindowClass::INPUT_ONLY,
            0,
            &xproto::CreateWindowAux::new().event_mask(
                xproto::EventMask::PROPERTY_CHANGE | xproto::EventMask::STRUCTURE_NOTIFY,
            ),
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(cookie);
    xwm.data_bridge.dnd.internal_windows.insert(window);
    if !xwm.data_bridge.dnd.bind_source_proxy(id, window) {
        xwm.data_bridge.dnd.internal_windows.remove(&window);
        let _ = xwm.connection.destroy_window(window);
        return Ok(());
    }
    if let Some(session) = xwm.data_bridge.dnd.active.as_mut() {
        session.timestamp_deadline_ns = Some(now_ns.saturating_add(SOURCE_TIMESTAMP_TIMEOUT_NS));
    }
    let property_cookie = xwm
        .connection
        .change_property32(
            PropMode::REPLACE,
            window,
            xwm.atoms.get(XwmAtomName::XdndSourceTime),
            AtomEnum::INTEGER,
            &[1],
        )
        .map_err(XwmError::Connection)?;
    std::mem::forget(property_cookie);

    let (mime_types, deadline_ns) = {
        let session = xwm
            .data_bridge
            .dnd
            .active_session()
            .ok_or(XwmError::InvalidCommand("missing XDND session"))?;
        (
            session.mime_types.as_slice().to_vec(),
            now_ns.saturating_add(SOURCE_TIMESTAMP_TIMEOUT_NS),
        )
    };
    for (ordinal, mime_type) in mime_types.into_iter().enumerate() {
        let cookie = xwm
            .connection
            .intern_atom(false, mime_type.as_bytes())
            .map_err(XwmError::Connection)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        xwm.data_bridge.dnd.pending_replies.insert(
            sequence,
            DndPendingReply::MimeAtom {
                id,
                ordinal,
                deadline_ns,
            },
        );
    }
    Ok(())
}

pub(super) fn start_target_discovery(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    actual: X11WindowHandle,
    now_ns: u64,
) -> Result<(), XwmError> {
    if actual.generation() != xwm.generation || actual.xid() == 0 {
        return Ok(());
    }
    if xwm.data_bridge.dnd.active_session().is_some_and(|session| {
        session.id == id
            && matches!(
                session.progress,
                DndWireProgress::DropPending
                    | DndWireProgress::DropPendingAwaitingStatus
                    | DndWireProgress::AwaitingFinished
                    | DndWireProgress::TerminalConsumed
            )
    }) {
        return Ok(());
    }
    if let Some(session) = xwm.data_bridge.dnd.active.as_ref()
        && session.target == Some(actual)
        && session.progress == DndWireProgress::Positioned
    {
        return Ok(());
    }
    cancel_discovery_replies(xwm, id, None);
    if let Some(old_target) = xwm
        .data_bridge
        .dnd
        .active_session()
        .and_then(|session| session.target)
        && old_target != actual
    {
        send_leave_for_current_target(xwm, id, old_target)?;
        let _ = xwm.data_bridge.dnd.leave_target(id, old_target);
    }
    let serial = xwm.data_bridge.dnd.next_discovery_serial.saturating_add(1);
    if serial == 0 {
        reject_target(xwm, id, actual);
        return Ok(());
    }
    xwm.data_bridge.dnd.next_discovery_serial = serial;
    let deadline_ns = now_ns.saturating_add(TARGET_DISCOVERY_TIMEOUT_NS);
    let Some(session) = xwm
        .data_bridge
        .dnd
        .active
        .as_mut()
        .filter(|session| session.id == id)
    else {
        return Ok(());
    };
    session.discovery_serial = serial;
    session.discovery_target = Some(actual);
    session.discovery_recipient = None;
    session.discovery_deadline_ns = Some(deadline_ns);
    let cookie = xwm
        .connection
        .get_property(
            false,
            actual.xid(),
            xwm.atoms.get(XwmAtomName::XdndProxy),
            AtomEnum::WINDOW,
            0,
            1,
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    xwm.data_bridge.dnd.pending_replies.insert(
        sequence,
        DndPendingReply::TargetProxy {
            id,
            actual,
            serial,
            deadline_ns,
        },
    );
    Ok(())
}

pub(super) fn valid_single_window(reply: &xproto::GetPropertyReply) -> Option<u32> {
    if reply.format != 32 || reply.type_ != u32::from(AtomEnum::WINDOW) || reply.bytes_after != 0 {
        return None;
    }
    let values = reply.value32()?.collect::<Vec<_>>();
    (values.len() == 1 && values[0] != 0).then_some(values[0])
}

pub(super) fn valid_aware_version(reply: &xproto::GetPropertyReply) -> Option<XwaylandDndVersion> {
    if reply.format != 32 || reply.type_ != u32::from(AtomEnum::ATOM) || reply.bytes_after != 0 {
        return None;
    }
    let values = reply.value32()?.collect::<Vec<_>>();
    if values.len() != 1 {
        return None;
    }
    XwaylandDndVersion::negotiate_target(values[0])
}

pub(super) fn issue_aware_query(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    actual: X11WindowHandle,
    recipient: u32,
    serial: u64,
    deadline_ns: u64,
) -> Result<(), XwmError> {
    let cookie = xwm
        .connection
        .get_property(
            false,
            recipient,
            xwm.atoms.get(XwmAtomName::XdndAware),
            AtomEnum::ATOM,
            0,
            1,
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    xwm.data_bridge.dnd.pending_replies.insert(
        sequence,
        DndPendingReply::Aware {
            id,
            actual,
            recipient,
            serial,
            deadline_ns,
        },
    );
    Ok(())
}

pub(super) fn reply_is_current(
    xwm: &Xwm,
    id: XwaylandDndAdapterId,
    actual: X11WindowHandle,
    serial: u64,
    deadline_ns: u64,
    now_ns: u64,
) -> bool {
    xwm.generation == id.generation()
        && deadline_ns > now_ns
        && xwm.data_bridge.dnd.active_session().is_some_and(|session| {
            session.id == id
                && session.discovery_target == Some(actual)
                && session.discovery_serial == serial
                && session.discovery_deadline_ns == Some(deadline_ns)
        })
}

pub(super) fn issue_proxy_validation(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    actual: X11WindowHandle,
    proxy: u32,
    serial: u64,
    deadline_ns: u64,
) -> Result<(), XwmError> {
    let cookie = xwm
        .connection
        .get_property(
            false,
            proxy,
            xwm.atoms.get(XwmAtomName::XdndProxy),
            AtomEnum::WINDOW,
            0,
            1,
        )
        .map_err(XwmError::Connection)?;
    let sequence = cookie.sequence_number();
    std::mem::forget(cookie);
    xwm.data_bridge.dnd.pending_replies.insert(
        sequence,
        DndPendingReply::ProxySelf {
            id,
            actual,
            proxy,
            serial,
            deadline_ns,
        },
    );
    Ok(())
}

pub(super) fn publish_source_metadata_and_claim(
    xwm: &mut Xwm,
    id: XwaylandDndAdapterId,
    now_ns: u64,
) -> Result<(), XwmError> {
    let Some(session) = xwm
        .data_bridge
        .dnd
        .active_session()
        .filter(|session| session.id == id)
    else {
        return Ok(());
    };
    let Some(source_proxy) = session.source_proxy else {
        return Ok(());
    };
    if session.mime_atoms.iter().any(Option::is_none)
        || session.mime_atoms.len() != session.mime_types.as_slice().len()
    {
        return Ok(());
    }
    let Some(timestamp) = session.ownership_timestamp else {
        return Ok(());
    };
    let already_claimed = session.ownership_claim_issued;
    let mime_atoms = session
        .mime_atoms
        .iter()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    let source_actions = session.source_actions.clone();
    if mime_atoms.len() > 3 {
        let cookie = xwm
            .connection
            .change_property32(
                PropMode::REPLACE,
                source_proxy,
                xwm.atoms.get(XwmAtomName::XdndTypeList),
                AtomEnum::ATOM,
                &mime_atoms,
            )
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
    }
    if source_actions.contains(&XwaylandDndAction::Ask) {
        let concrete = super::super::dnd_wire::wayland_actions(&source_actions)
            .into_iter()
            .filter(|action| {
                matches!(
                    action,
                    crate::xwayland::WaylandDndAction::Copy
                        | crate::xwayland::WaylandDndAction::Move
                )
            })
            .collect::<Vec<_>>();
        let atoms = concrete
            .iter()
            .filter_map(|action| match action {
                crate::xwayland::WaylandDndAction::Copy => {
                    Some(xwm.atoms.get(XwmAtomName::XdndActionCopy))
                }
                crate::xwayland::WaylandDndAction::Move => {
                    Some(xwm.atoms.get(XwmAtomName::XdndActionMove))
                }
                crate::xwayland::WaylandDndAction::Ask => None,
            })
            .collect::<Vec<_>>();
        if !atoms.is_empty() {
            let cookie = xwm
                .connection
                .change_property32(
                    PropMode::REPLACE,
                    source_proxy,
                    xwm.atoms.get(XwmAtomName::XdndActionList),
                    AtomEnum::ATOM,
                    &atoms,
                )
                .map_err(XwmError::Connection)?;
            std::mem::forget(cookie);
            let mut descriptions = Vec::new();
            for action in concrete {
                descriptions.extend_from_slice(match action {
                    crate::xwayland::WaylandDndAction::Copy => b"Copy\0",
                    crate::xwayland::WaylandDndAction::Move => b"Move\0",
                    crate::xwayland::WaylandDndAction::Ask => b"Ask\0",
                });
            }
            let cookie = xwm
                .connection
                .change_property8(
                    PropMode::REPLACE,
                    source_proxy,
                    xwm.atoms.get(XwmAtomName::XdndActionDescription),
                    xwm.atoms.get(XwmAtomName::String),
                    &descriptions,
                )
                .map_err(XwmError::Connection)?;
            std::mem::forget(cookie);
        }
    }
    if !already_claimed {
        let cookie = xwm
            .connection
            .set_selection_owner(
                source_proxy,
                xwm.atoms.get(XwmAtomName::XdndSelection),
                timestamp,
            )
            .map_err(XwmError::Connection)?;
        std::mem::forget(cookie);
        let cookie = xwm
            .connection
            .get_selection_owner(xwm.atoms.get(XwmAtomName::XdndSelection))
            .map_err(XwmError::Connection)?;
        let sequence = cookie.sequence_number();
        std::mem::forget(cookie);
        let deadline_ns = now_ns.saturating_add(SOURCE_OWNERSHIP_TIMEOUT_NS);
        if let Some(session) = xwm
            .data_bridge
            .dnd
            .active
            .as_mut()
            .filter(|session| session.id == id && session.source_proxy == Some(source_proxy))
        {
            session.ownership_claim_issued = true;
            session.ownership_deadline_ns = Some(deadline_ns);
        }
        xwm.data_bridge.dnd.pending_replies.insert(
            sequence,
            DndPendingReply::Ownership {
                id,
                source_proxy,
                timestamp,
                deadline_ns,
            },
        );
    }
    Ok(())
}
