use crate::compositor::DesktopWindowKind;
use x11rb::{
    connection::Connection,
    protocol::{Event, sync::Int64, xproto},
};

use super::{
    X11ConfigureFlags, X11ConfigureRequest, X11Geometry, X11StackMode, X11StateRequest,
    X11WindowHandle, Xwm, XwmDrain, XwmError, XwmEvent,
    atoms::XwmAtomName,
    ewmh::{decode_state_action, state_atom as decode_state_atom},
    properties::PropertyKind,
    window::X11WindowLifecycle,
};

pub(crate) fn drain(xwm: &mut Xwm, budget: usize) -> Result<XwmDrain, XwmError> {
    let mut processed = 0;
    while processed < budget {
        let Some(event) = xwm
            .connection
            .poll_for_event()
            .map_err(XwmError::Connection)?
        else {
            break;
        };
        processed += 1;
        normalize(xwm, event)?;
    }
    Ok(XwmDrain {
        processed,
        budget_exhausted: processed == budget && budget != 0,
    })
}

fn normalize(xwm: &mut Xwm, event: Event) -> Result<(), XwmError> {
    match event {
        Event::CreateNotify(event) => {
            if event.window == xwm.root {
                return Ok(());
            }
            let handle = X11WindowHandle::new(xwm.generation, event.window);
            let kind = window_kind(event.override_redirect);
            let geometry = X11Geometry {
                x: i32::from(event.x),
                y: i32::from(event.y),
                width: u32::from(event.width),
                height: u32::from(event.height),
            };
            xwm.observe_window_with_kind(handle, kind, geometry)?;
        }
        Event::MapRequest(event) => {
            let handle = ensure_window(xwm, event.window)?;
            if xwm
                .windows
                .get(handle)
                .is_some_and(|record| record.map_requested)
            {
                return Ok(());
            }
            xwm.cancel_window_properties(handle);
            xwm.windows
                .mark_map_requested(handle)
                .map_err(XwmError::InvalidCommand)?;
            xwm.refresh_window_properties(handle)?;
            xwm.emit_ready_if_complete(handle)?;
        }
        Event::MapNotify(event) => {
            let handle =
                ensure_window_with_kind(xwm, event.window, window_kind(event.override_redirect))?;
            if xwm
                .windows
                .get(handle)
                .is_some_and(|record| record.map_operation_pending)
            {
                xwm.windows
                    .confirm_map_notify(handle)
                    .map_err(XwmError::InvalidCommand)?;
                return Ok(());
            }
            let externally_mapped = xwm
                .windows
                .confirm_external_map_notify(handle)
                .map_err(XwmError::InvalidCommand)?;
            if externally_mapped {
                if !xwm
                    .windows
                    .get(handle)
                    .is_some_and(|record| record.properties_ready)
                {
                    xwm.refresh_window_properties(handle)?;
                }
                xwm.emit_ready_if_complete(handle)?;
                return Ok(());
            }
            xwm.cancel_window_properties(handle);
            xwm.windows
                .mark_map_requested(handle)
                .map_err(XwmError::InvalidCommand)?;
            xwm.refresh_window_properties(handle)?;
            xwm.emit_ready_if_complete(handle)?;
        }
        Event::UnmapNotify(event) => {
            let handle = X11WindowHandle::new(xwm.generation, event.window);
            let Some(record) = xwm.windows.get(handle) else {
                return Ok(());
            };
            let was_ready = record.snapshot.is_some()
                || matches!(record.lifecycle, X11WindowLifecycle::Renderable);
            if let Some(association) = record.association {
                xwm.clear_surface_buffer_ready(association.surface_id);
            }
            xwm.cancel_window_properties(handle);
            xwm.windows
                .mark_unmapped(handle)
                .map_err(XwmError::InvalidCommand)?;
            if was_ready {
                xwm.outgoing_events
                    .push_back(XwmEvent::WindowWithdrawn(handle));
            }
        }
        Event::DestroyNotify(event) => {
            let handle = X11WindowHandle::new(xwm.generation, event.window);
            xwm.note_focus_destroyed(event.window);
            xwm.clear_resize_sync(handle);
            xwm.association.remove_x11_window(handle);
            let Some(record) = xwm
                .windows
                .destroy(handle)
                .map_err(XwmError::InvalidCommand)?
            else {
                return Ok(());
            };
            if record.snapshot.is_some()
                || matches!(
                    record.lifecycle,
                    X11WindowLifecycle::Renderable | X11WindowLifecycle::Withdrawn
                )
            {
                xwm.outgoing_events
                    .push_back(XwmEvent::WindowDestroyed(handle));
            }
        }
        Event::ConfigureRequest(event) => {
            let handle = ensure_window(xwm, event.window)?;
            let sibling =
                (event.sibling != 0).then(|| X11WindowHandle::new(xwm.generation, event.sibling));
            let request = X11ConfigureRequest {
                requested: X11Geometry {
                    x: i32::from(event.x),
                    y: i32::from(event.y),
                    width: u32::from(event.width),
                    height: u32::from(event.height),
                },
                fields: configure_flags(event.value_mask),
                border_width: u32::from(event.border_width),
                sibling,
                stack_mode: stack_mode(event.stack_mode),
            };
            xwm.outgoing_events.push_back(XwmEvent::ConfigureRequested {
                window: handle,
                request,
            });
        }
        Event::ConfigureNotify(event) => {
            let handle = X11WindowHandle::new(xwm.generation, event.window);
            if let Some(record) = xwm.windows.get_mut(handle) {
                record.geometry = X11Geometry {
                    x: i32::from(event.x),
                    y: i32::from(event.y),
                    width: u32::from(event.width),
                    height: u32::from(event.height),
                };
                if let Some(snapshot) = record.snapshot.as_mut() {
                    snapshot.geometry = record.geometry;
                }
            }
        }
        Event::ClientMessage(event) if event.format == 32 => {
            normalize_client_message(xwm, event)?;
        }
        Event::PropertyNotify(event) => normalize_property_change(xwm, event)?,
        Event::SyncCounterNotify(event) => {
            if xwm.capabilities.sync {
                xwm.note_sync_counter_notify(event.counter, int64_to_u64(event.counter_value));
            }
        }
        Event::FocusIn(event) => {
            xwm.note_focus_in(event.event);
        }
        Event::FocusOut(event) => {
            xwm.note_focus_out(event.event);
        }
        Event::ShapeNotify(event) => {
            // Shape is negotiated only for version diagnostics.  Until the
            // compositor visual/input regions consume the region, rectangular
            // fallback is the sole advertised behavior.
            let _ = event;
        }
        _ => {}
    }
    Ok(())
}

fn int64_to_u64(value: Int64) -> u64 {
    if value.hi < 0 {
        return 0;
    }
    (u64::try_from(value.hi).unwrap_or(0) << 32) | u64::from(value.lo)
}

fn normalize_client_message(
    xwm: &mut Xwm,
    event: xproto::ClientMessageEvent,
) -> Result<(), XwmError> {
    let handle = ensure_window(xwm, event.window)?;
    let data = event.data.as_data32();
    if event.type_ == xwm.atoms.get(XwmAtomName::WlSurfaceSerial) {
        xwm.note_x11_surface_serial(handle, data[0], data[1])?;
    } else if event.type_ == xwm.atoms.get(XwmAtomName::NetWmState) {
        if let Some(action) = decode_state_action(data[0]) {
            let request = X11StateRequest {
                action,
                first: decode_state_atom(
                    data[1],
                    xwm.atoms.get(XwmAtomName::NetWmStateFullscreen),
                    xwm.atoms.get(XwmAtomName::NetWmStateMaximizedHorz),
                    xwm.atoms.get(XwmAtomName::NetWmStateMaximizedVert),
                    xwm.atoms.get(XwmAtomName::NetWmStateHidden),
                ),
                second: decode_state_atom(
                    data[2],
                    xwm.atoms.get(XwmAtomName::NetWmStateFullscreen),
                    xwm.atoms.get(XwmAtomName::NetWmStateMaximizedHorz),
                    xwm.atoms.get(XwmAtomName::NetWmStateMaximizedVert),
                    xwm.atoms.get(XwmAtomName::NetWmStateHidden),
                ),
            };
            xwm.outgoing_events.push_back(XwmEvent::StateRequested {
                window: handle,
                request,
            });
        }
    } else if event.type_ == xwm.atoms.get(XwmAtomName::NetActiveWindow) {
        let (current_time, user_time) = xwm.note_active_window_request(handle, data[1]);
        xwm.outgoing_events.push_back(XwmEvent::FocusRequested {
            window: handle,
            source: data[0],
            timestamp: data[1],
            current_time,
            user_time,
        });
    } else if event.type_ == xwm.atoms.get(XwmAtomName::NetCloseWindow) {
        xwm.outgoing_events
            .push_back(XwmEvent::CloseRequestedByClient(handle));
    } else if event.type_ == xwm.atoms.get(XwmAtomName::WmChangeState) && data[0] == 3 {
        xwm.outgoing_events.push_back(XwmEvent::StateRequested {
            window: handle,
            request: X11StateRequest {
                action: super::X11StateAction::Add,
                first: Some(super::X11StateAtom::Hidden),
                second: None,
            },
        });
    }
    Ok(())
}

fn normalize_property_change(
    xwm: &mut Xwm,
    event: xproto::PropertyNotifyEvent,
) -> Result<(), XwmError> {
    let handle = X11WindowHandle::new(xwm.generation, event.window);
    if !xwm.windows.contains(handle) {
        return Ok(());
    }
    if let Some(kind) = property_kind_for_atom(xwm, event.atom) {
        xwm.refresh_window_property(handle, kind)?;
    }
    Ok(())
}

fn property_kind_for_atom(xwm: &Xwm, atom: u32) -> Option<PropertyKind> {
    PropertyKind::ALL
        .into_iter()
        .find(|kind| kind.atom(xwm) == atom)
}

#[allow(dead_code)]
const MAX_TEXT_PROPERTY_BYTES: usize = 64 * 1024;

#[allow(dead_code)]
pub(crate) fn normalized_title(
    net_wm_name: Option<&[u8]>,
    wm_name: Option<&[u8]>,
) -> Option<String> {
    net_wm_name
        .and_then(decode_text_property)
        .or_else(|| wm_name.and_then(decode_text_property))
}

#[allow(dead_code)]
pub(crate) fn normalized_app_id(wm_class: &[u8]) -> Option<String> {
    if wm_class.len() > MAX_TEXT_PROPERTY_BYTES {
        return None;
    }
    let fields = wm_class
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    fields
        .get(1)
        .or_else(|| fields.first())
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .filter(|value| !value.is_empty())
}

fn decode_text_property(value: &[u8]) -> Option<String> {
    if value.is_empty() || value.len() > MAX_TEXT_PROPERTY_BYTES {
        return None;
    }
    let value = value.split(|byte| *byte == 0).next().unwrap_or_default();
    (!value.is_empty()).then(|| String::from_utf8_lossy(value).into_owned())
}

fn ensure_window(xwm: &mut Xwm, xid: u32) -> Result<X11WindowHandle, XwmError> {
    ensure_window_with_kind(xwm, xid, DesktopWindowKind::Managed)
}

fn ensure_window_with_kind(
    xwm: &mut Xwm,
    xid: u32,
    kind: DesktopWindowKind,
) -> Result<X11WindowHandle, XwmError> {
    let handle = X11WindowHandle::new(xwm.generation, xid);
    if !xwm.windows.contains(handle) {
        xwm.observe_window_with_kind(handle, kind, X11Geometry::default())?;
    }
    Ok(handle)
}

fn window_kind(override_redirect: bool) -> DesktopWindowKind {
    if override_redirect {
        DesktopWindowKind::OverrideRedirect
    } else {
        DesktopWindowKind::Managed
    }
}

fn configure_flags(mask: xproto::ConfigWindow) -> X11ConfigureFlags {
    let mask = u16::from(mask);
    let has = |flag| mask & u16::from(flag) != 0;
    X11ConfigureFlags {
        x: has(xproto::ConfigWindow::X),
        y: has(xproto::ConfigWindow::Y),
        width: has(xproto::ConfigWindow::WIDTH),
        height: has(xproto::ConfigWindow::HEIGHT),
        border_width: has(xproto::ConfigWindow::BORDER_WIDTH),
        sibling: has(xproto::ConfigWindow::SIBLING),
        stack_mode: has(xproto::ConfigWindow::STACK_MODE),
    }
}

fn stack_mode(mode: xproto::StackMode) -> Option<X11StackMode> {
    match u32::from(mode) {
        0 => Some(X11StackMode::Above),
        1 => Some(X11StackMode::Below),
        2 => Some(X11StackMode::TopIf),
        3 => Some(X11StackMode::BottomIf),
        4 => Some(X11StackMode::Opposite),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_prefers_net_wm_name_over_wm_name() {
        assert_eq!(
            normalized_title(Some(b"modern"), Some(b"legacy")),
            Some("modern".to_owned())
        );
        assert_eq!(
            normalized_title(None, Some(b"legacy")),
            Some("legacy".to_owned())
        );
    }

    #[test]
    fn wm_class_maps_to_stable_app_id() {
        assert_eq!(
            normalized_app_id(b"instance\0StableClass\0"),
            Some("StableClass".to_owned())
        );
        assert_eq!(
            normalized_app_id(b"StableClass\0"),
            Some("StableClass".to_owned())
        );
    }

    #[test]
    fn malformed_property_is_bounded_and_nonfatal() {
        let oversized = vec![b'x'; MAX_TEXT_PROPERTY_BYTES + 1];
        assert_eq!(
            normalized_title(Some(&oversized), Some(b"fallback")),
            Some("fallback".to_owned())
        );
        assert_eq!(normalized_app_id(&oversized), None);
        assert_eq!(
            normalized_title(Some(b"\0garbage"), Some(b"fallback")),
            Some("fallback".to_owned())
        );
    }

    #[test]
    fn state_action_and_stack_mode_values_are_normalized() {
        assert_eq!(
            decode_state_action(0),
            Some(super::super::X11StateAction::Remove)
        );
        assert_eq!(
            decode_state_action(1),
            Some(super::super::X11StateAction::Add)
        );
        assert_eq!(
            decode_state_action(2),
            Some(super::super::X11StateAction::Toggle)
        );
        assert_eq!(decode_state_action(3), None);
        assert_eq!(
            stack_mode(xproto::StackMode::ABOVE),
            Some(X11StackMode::Above)
        );
        assert_eq!(
            stack_mode(xproto::StackMode::OPPOSITE),
            Some(X11StackMode::Opposite)
        );
        assert_eq!(stack_mode(xproto::StackMode::from(99_u32)), None);
    }
}
