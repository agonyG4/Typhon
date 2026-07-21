use crate::compositor::DesktopWindowKind;
use crate::xwayland::trace::{self, TraceFields};
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
    trace_raw_event(&event);
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
            if !xwm.observe_window_with_kind(handle, kind, geometry)? {
                xwm.reconcile_window_kind(handle, kind)?;
            }
            trace_window_state(xwm, "create_window_observed", handle, TraceFields::new());
        }
        Event::MapRequest(event) => {
            let handle = ensure_window(xwm, event.window)?;
            if xwm.windows.get(handle).is_some_and(|record| {
                record.map_requested
                    && !matches!(
                        record.lifecycle,
                        X11WindowLifecycle::Iconic | X11WindowLifecycle::Withdrawn
                    )
            }) {
                return Ok(());
            }
            xwm.cancel_window_properties(handle);
            xwm.windows
                .mark_map_requested(handle)
                .map_err(XwmError::InvalidCommand)?;
            xwm.refresh_window_properties(handle)?;
            xwm.emit_ready_if_complete(handle)?;
            trace_window_state(xwm, "map_request_processed", handle, TraceFields::new());
        }
        Event::MapNotify(event) => {
            let handle =
                ensure_window_with_kind(xwm, event.window, window_kind(event.override_redirect))?;
            xwm.reconcile_window_kind(handle, window_kind(event.override_redirect))?;
            if xwm
                .windows
                .get(handle)
                .is_some_and(|record| record.map_operation_pending)
            {
                xwm.windows
                    .confirm_map_notify(handle)
                    .map_err(XwmError::InvalidCommand)?;
                let ready_emitted = xwm.emit_ready_if_complete(handle)?;
                let lifecycle = xwm
                    .windows
                    .get(handle)
                    .map(|record| format!("{:?}", record.lifecycle))
                    .unwrap_or_else(|| "Unknown".to_owned());
                eprintln!(
                    "oblivion-one xwayland: event=xwm_map_notify window={} pending_map=true ready_emitted={} lifecycle={lifecycle}",
                    handle.xid(),
                    ready_emitted,
                );
                trace_window_state(
                    xwm,
                    "map_notify_processed",
                    handle,
                    TraceFields::new().field("map_path", "wm_requested"),
                );
                return Ok(());
            }
            xwm.windows
                .confirm_external_map_notify(handle)
                .map_err(XwmError::InvalidCommand)?;
            xwm.refresh_window_properties(handle)?;
            let ready_emitted = xwm.emit_ready_if_complete(handle)?;
            let lifecycle = xwm
                .windows
                .get(handle)
                .map(|record| format!("{:?}", record.lifecycle))
                .unwrap_or_else(|| "Unknown".to_owned());
            eprintln!(
                "oblivion-one xwayland: event=xwm_map_notify window={} pending_map=false ready_emitted={} lifecycle={lifecycle}",
                handle.xid(),
                ready_emitted,
            );
            trace_window_state(
                xwm,
                "map_notify_processed",
                handle,
                TraceFields::new().field("map_path", "external"),
            );
        }
        Event::UnmapNotify(event) => {
            let handle = X11WindowHandle::new(xwm.generation, event.window);
            let Some(record) = xwm.windows.get(handle) else {
                return Ok(());
            };
            let was_ready = record.snapshot.is_some()
                || matches!(record.lifecycle, X11WindowLifecycle::Renderable);
            let association = record.association;
            if xwm
                .windows
                .consume_wm_unmap(handle)
                .map_err(XwmError::InvalidCommand)?
            {
                trace_window_state(
                    xwm,
                    "wm_unmap_confirmation_consumed",
                    handle,
                    TraceFields::new().field("lifecycle", "Iconic"),
                );
                return Ok(());
            }
            if let Some(association) = association {
                xwm.clear_surface_buffer_ready(association.surface_id);
                xwm.association.remove_x11_window(handle);
                let _ = xwm
                    .windows
                    .clear_association(handle, association.surface_id, false)
                    .map_err(XwmError::InvalidCommand)?;
            }
            xwm.cancel_window_properties(handle);
            xwm.windows
                .mark_unmapped(handle)
                .map_err(XwmError::InvalidCommand)?;
            trace_window_state(xwm, "unmap_notify_processed", handle, TraceFields::new());
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
            let Some(_record) = xwm
                .windows
                .destroy(handle)
                .map_err(XwmError::InvalidCommand)?
            else {
                return Ok(());
            };
            trace::emit("destroy_window_processed", || {
                TraceFields::new()
                    .field("source", "xwm")
                    .field("xid", handle.xid())
                    .field("lifecycle", "Destroyed")
            });
            xwm.outgoing_events
                .push_back(XwmEvent::WindowDestroyed(handle));
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
            let geometry = X11Geometry {
                x: i32::from(event.x),
                y: i32::from(event.y),
                width: u32::from(event.width),
                height: u32::from(event.height),
            };
            if xwm.windows.contains(handle) {
                xwm.reconcile_window_kind(handle, window_kind(event.override_redirect))?;
            }
            if let Some(record) = xwm.windows.get_mut(handle) {
                record.geometry = geometry;
                if let Some(snapshot) = record.snapshot.as_mut() {
                    snapshot.geometry = record.geometry;
                }
            }
            trace_window_state(
                xwm,
                "configure_notify_processed",
                handle,
                TraceFields::new()
                    .field("geometry_x", geometry.x)
                    .field("geometry_y", geometry.y)
                    .field("geometry_width", geometry.width)
                    .field("geometry_height", geometry.height),
            );
            if !xwm.note_configure_notify(handle, geometry) && xwm.windows.get(handle).is_some() {
                xwm.outgoing_events.push_back(XwmEvent::ConfigureNotify {
                    window: handle,
                    geometry,
                });
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

fn trace_raw_event(event: &Event) {
    match event {
        Event::CreateNotify(event) => trace::emit("CreateNotify", || {
            TraceFields::new()
                .field("source", "x11")
                .field("x_event_send_event", event.response_type & 0x80 != 0)
                .field("xid", event.window)
                .field("parent_xid", event.parent)
                .field("override_redirect_event", event.override_redirect)
        }),
        Event::MapRequest(event) => trace::emit("MapRequest", || {
            TraceFields::new()
                .field("source", "x11")
                .field("x_event_send_event", event.response_type & 0x80 != 0)
                .field("xid", event.window)
                .field("parent_xid", event.parent)
        }),
        Event::MapNotify(event) => trace::emit("MapNotify", || {
            TraceFields::new()
                .field("source", "x11")
                .field("x_event_send_event", event.response_type & 0x80 != 0)
                .field("xid", event.window)
                .field("override_redirect_event", event.override_redirect)
        }),
        Event::ConfigureNotify(event) => trace::emit("ConfigureNotify", || {
            TraceFields::new()
                .field("source", "x11")
                .field("x_event_send_event", event.response_type & 0x80 != 0)
                .field("xid", event.window)
                .field("override_redirect_event", event.override_redirect)
                .field("geometry_x", event.x)
                .field("geometry_y", event.y)
                .field("geometry_width", event.width)
                .field("geometry_height", event.height)
        }),
        Event::PropertyNotify(event) => trace::emit("PropertyNotify", || {
            TraceFields::new()
                .field("source", "x11")
                .field("x_event_send_event", event.response_type & 0x80 != 0)
                .field("xid", event.window)
                .field("property_atom", event.atom)
                .field("property_state", format!("{:?}", event.state))
        }),
        Event::UnmapNotify(event) => trace::emit("UnmapNotify", || {
            TraceFields::new()
                .field("source", "x11")
                .field("x_event_send_event", event.response_type & 0x80 != 0)
                .field("xid", event.window)
                .field("from_configure", event.from_configure)
        }),
        Event::DestroyNotify(event) => trace::emit("DestroyNotify", || {
            TraceFields::new()
                .field("source", "x11")
                .field("x_event_send_event", event.response_type & 0x80 != 0)
                .field("xid", event.window)
        }),
        Event::ClientMessage(event) if event.format == 32 => trace::emit("ClientMessage", || {
            let data = event.data.as_data32();
            TraceFields::new()
                .field("source", "x11")
                .field("x_event_send_event", event.response_type & 0x80 != 0)
                .field("xid", event.window)
                .field("client_message_atom", event.type_)
                .field("client_message_data0", data[0])
                .field("client_message_data1", data[1])
                .field("client_message_data2", data[2])
                .field("client_message_data3", data[3])
                .field("client_message_data4", data[4])
        }),
        Event::SyncCounterNotify(event) => trace::emit("SyncCounterNotify", || {
            TraceFields::new()
                .field("source", "x11")
                .field("x_event_send_event", event.response_type & 0x80 != 0)
                .field("sync_counter", event.counter)
                .field("sync_counter_value", format!("{:?}", event.counter_value))
        }),
        _ => {}
    }
}

fn trace_window_state(
    xwm: &Xwm,
    event: &'static str,
    handle: X11WindowHandle,
    fields: TraceFields,
) {
    let Some(record) = xwm.windows.get(handle) else {
        return;
    };
    trace::emit(event, || {
        let association = record.association;
        fields
            .field("source", "xwm")
            .field("xid", handle.xid())
            .field(
                "override_redirect_stored",
                record.kind == DesktopWindowKind::OverrideRedirect,
            )
            .field("lifecycle", format!("{:?}", record.lifecycle))
            .field("property_epoch", record.property_epoch)
            .field("properties_ready", record.properties_ready)
            .field("buffer_ready", record.buffer_ready)
            .field("map_serial", record.map_serial)
            .field("inflight_wm_unmaps", record.inflight_wm_unmaps)
            .field(
                "window_types",
                format!("{:?}", record.properties.window_types),
            )
            .optional(
                "transient_for",
                record.properties.transient_for.map(|parent| parent.xid()),
            )
            .optional(
                "association_serial",
                association.map(|value| value.serial.get()),
            )
            .optional("surface_id", association.map(|value| value.surface_id))
    });
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
    } else if event.type_ == xwm.atoms.get(XwmAtomName::NetWmMoveresize) {
        if let Some(direction) = super::X11MoveResizeDirection::from_ewmh(data[2]) {
            xwm.outgoing_events
                .push_back(XwmEvent::MoveResizeRequested {
                    window: handle,
                    request: super::X11MoveResizeRequest {
                        root_x: data[0] as i32,
                        root_y: data[1] as i32,
                        direction,
                        button: data[3],
                        source: data[4],
                    },
                });
        }
    } else if event.type_ == xwm.atoms.get(XwmAtomName::NetMoveResizeWindow) {
        let flags = X11ConfigureFlags {
            x: data[0] & (1 << 8) != 0,
            y: data[0] & (1 << 9) != 0,
            width: data[0] & (1 << 10) != 0,
            height: data[0] & (1 << 11) != 0,
            ..X11ConfigureFlags::default()
        };
        xwm.outgoing_events.push_back(XwmEvent::ConfigureRequested {
            window: handle,
            request: X11ConfigureRequest {
                requested: X11Geometry {
                    x: data[1] as i32,
                    y: data[2] as i32,
                    width: data[3],
                    height: data[4],
                },
                fields: flags,
                border_width: 0,
                sibling: None,
                stack_mode: None,
            },
        });
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
    trace_window_state(
        xwm,
        "property_notify_before_refresh",
        handle,
        TraceFields::new().field("property_atom", event.atom),
    );
    if let Some(kind) = property_kind_for_atom(xwm, event.atom) {
        xwm.refresh_window_property(handle, kind)?;
    }
    trace_window_state(
        xwm,
        "property_notify_after_refresh_request",
        handle,
        TraceFields::new().field("property_atom", event.atom),
    );
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
    use std::{collections::HashMap, io::Write, num::NonZeroU64, os::unix::net::UnixStream};

    use crate::xwayland::XwaylandAssociationEvent;
    use x11rb::{
        protocol::xproto::{Screen, Setup},
        rust_connection::RustConnection,
    };

    use super::*;

    pub(crate) fn test_fixture(generation: super::super::XwaylandGeneration) -> (Xwm, UnixStream) {
        let (stream, peer) = UnixStream::pair().expect("XWM fixture socket pair");
        let reactor_stream = super::super::connection::ReactorStream::from_unix_stream(stream)
            .expect("XWM fixture reactor stream");
        let setup = Setup {
            roots: vec![Screen {
                root: 1,
                root_visual: 1,
                root_depth: 24,
                width_in_pixels: 1920,
                height_in_pixels: 1080,
                ..Screen::default()
            }],
            resource_id_base: 0x100000,
            resource_id_mask: 0x0fffff,
            maximum_request_length: u16::MAX,
            ..Setup::default()
        };
        let inner = RustConnection::for_connected_stream(reactor_stream, setup)
            .expect("XWM fixture X11 connection");
        let raw_fd = std::os::fd::AsRawFd::as_raw_fd(inner.stream());
        let connection = super::super::connection::X11Connection::new(inner, HashMap::new());
        let atoms = super::super::atoms::XwmAtoms::from_values(
            super::super::atoms::XwmAtomName::ALL
                .iter()
                .enumerate()
                .map(|(index, name)| (*name, (index as u32).saturating_add(1)))
                .collect(),
        );
        let xwm = Xwm {
            generation,
            connection,
            adoption: Default::default(),
            screen_number: 0,
            root: 1,
            atoms,
            capabilities: super::super::capabilities::XwmCapabilities {
                composite: true,
                xfixes: true,
                shape: false,
                randr: false,
                sync: false,
            },
            windows: super::super::window::X11WindowRegistry::default(),
            outgoing_events: Default::default(),
            association: super::super::association::SurfaceAssociationJoin::default(),
            resize_sync: super::super::resize_sync::ResizeSyncTracker::default(),
            focus: super::super::focus::FocusTracker::default(),
            sync_alarms: Default::default(),
            sync_handles_by_counter: Default::default(),
            timed_out_resize_counters: Default::default(),
            next_resize_counter_values: Default::default(),
            family_order: Default::default(),
            next_family_order: 0,
            expected_configures: Default::default(),
            immediate_resize_windows: Default::default(),
            fallback_resize_windows: Default::default(),
            last_resize_geometries: Default::default(),
            shapes: Default::default(),
            data_bridge: super::super::data_bridge::DataBridge::default(),
            randr: super::super::startup::default_randr_snapshot(),
            pending_properties: Default::default(),
            deferred_properties: Default::default(),
            property_metrics: Default::default(),
            root_event_mask_probe: None,
            root_event_mask: None,
            buffer_ready_surfaces: Default::default(),
            buffer_ready_commits: Default::default(),
            supporting_wm_check: 2,
            raw_fd,
        };
        (xwm, peer)
    }

    pub(crate) fn generation(value: u64) -> super::super::XwaylandGeneration {
        super::super::XwaylandGeneration::new(NonZeroU64::new(value).expect("nonzero"))
    }

    pub(crate) fn map_event(window: u32, override_redirect: bool) -> Event {
        Event::MapNotify(xproto::MapNotifyEvent {
            response_type: 19,
            sequence: 0,
            event: 1,
            window,
            override_redirect,
        })
    }

    fn configure_event(window: u32, override_redirect: bool) -> Event {
        Event::ConfigureNotify(xproto::ConfigureNotifyEvent {
            response_type: 22,
            sequence: 0,
            event: 1,
            window,
            above_sibling: 0,
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            border_width: 0,
            override_redirect,
        })
    }

    pub(crate) fn unmap_event(window: u32) -> Event {
        Event::UnmapNotify(xproto::UnmapNotifyEvent {
            response_type: 18,
            sequence: 0,
            event: 1,
            window,
            from_configure: false,
        })
    }

    fn destroy_event(window: u32) -> Event {
        Event::DestroyNotify(xproto::DestroyNotifyEvent {
            response_type: 17,
            sequence: 0,
            event: 1,
            window,
        })
    }

    pub(crate) fn prepare_managed_window(
        xwm: &mut Xwm,
        xid: u32,
        properties_ready: bool,
        associated: bool,
        buffer_ready: bool,
    ) -> X11WindowHandle {
        let handle = X11WindowHandle::new(xwm.generation, xid);
        assert!(xwm.windows.insert_observed(handle));
        xwm.windows.mark_map_requested(handle).expect("map request");
        if properties_ready {
            xwm.windows
                .get_mut(handle)
                .expect("window record")
                .properties_ready = true;
        }
        xwm.windows.mark_map_commanded(handle).expect("map command");
        if associated {
            xwm.windows
                .mark_associated(
                    handle,
                    super::super::AssociatedSurface {
                        generation: xwm.generation,
                        serial: NonZeroU64::new(0x1234).expect("serial"),
                        surface_id: 42,
                        map_serial: 0,
                    },
                )
                .expect("association");
        }
        if buffer_ready {
            xwm.windows
                .mark_buffer_ready(handle)
                .expect("buffer readiness");
        }
        handle
    }

    fn prepare_ready_managed_window(xwm: &mut Xwm, xid: u32) -> X11WindowHandle {
        let handle = prepare_managed_window(xwm, xid, true, true, true);
        xwm.windows
            .confirm_map_notify(handle)
            .expect("MapNotify confirmation");
        xwm.emit_ready_if_complete(handle)
            .expect("ready event emission");
        assert!(matches!(
            ready_events(xwm).as_slice(),
            [super::super::XwmEvent::WindowReady(snapshot)] if snapshot.handle == handle
        ));
        handle
    }

    pub(crate) fn ready_events(xwm: &mut Xwm) -> Vec<super::super::XwmEvent> {
        xwm.take_events().collect()
    }

    pub(crate) fn ready_surface_id(events: &[super::super::XwmEvent]) -> Option<u32> {
        events.iter().find_map(|event| match event {
            super::super::XwmEvent::WindowReady(snapshot) => Some(snapshot.surface_id),
            _ => None,
        })
    }

    pub(crate) fn complete_property_refresh(xwm: &mut Xwm, peer: &mut UnixStream) {
        xwm.flush().expect("flush property refresh");
        let mut sequences = xwm.pending_properties.keys().copied().collect::<Vec<_>>();
        sequences.sort_unstable();
        for sequence in sequences {
            let mut reply = [0u8; 32];
            reply[0] = 1;
            reply[2..4].copy_from_slice(&(sequence as u16).to_ne_bytes());
            peer.write_all(&reply).expect("write property reply");
        }
        xwm.drain_events(256).expect("drain property refresh");
    }

    #[test]
    fn buffered_property_replies_are_drained_without_raw_socket_input() {
        let generation = generation(10);
        let (mut xwm, mut peer) = test_fixture(generation);
        let handle = X11WindowHandle::new(generation, 107);
        xwm.observe_window(handle).expect("observe managed window");
        xwm.flush().expect("flush property requests");

        let mut sequences = xwm.pending_properties.keys().copied().collect::<Vec<_>>();
        sequences.sort_unstable();
        let property_count = sequences.len();
        assert_eq!(
            property_count,
            super::super::properties::PropertyKind::ALL.len()
        );
        for sequence in sequences {
            let mut reply = [0u8; 32];
            reply[0] = 1;
            reply[2..4].copy_from_slice(&(sequence as u16).to_ne_bytes());
            peer.write_all(&reply).expect("write property reply");
        }
        let mut expose = [0u8; 32];
        expose[0] = 12;
        expose[4..8].copy_from_slice(&handle.xid().to_ne_bytes());
        peer.write_all(&expose).expect("write trailing X event");

        let event_drain = super::drain(&mut xwm, 1).expect("buffer replies while draining event");
        assert_eq!(event_drain.processed, 1);
        assert!(!super::super::properties::socket_has_input(xwm.raw_fd));

        xwm.drain_events(256).expect("drain buffered replies");
        let record = xwm.windows.get(handle).expect("managed window record");
        assert_eq!(
            (xwm.property_metrics.completed, xwm.pending_properties.len()),
            (property_count as u64, 0)
        );
        assert!(record.properties_ready);
    }

    #[test]
    fn adoption_timeout_does_not_fabricate_unmap_before_late_readiness() {
        let generation = generation(11);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = prepare_managed_window(&mut xwm, 108, true, false, false);

        normalize(&mut xwm, map_event(handle.xid(), false)).expect("normalize MapNotify");
        assert!(ready_events(&mut xwm).is_empty());

        xwm.adoption.observe(
            handle,
            super::super::adoption::AdoptionWait::MapToAssociation,
            10,
        );
        xwm.collect_adoption_expirations(10);

        let record = xwm.windows.get(handle).expect("managed window record");
        assert!(record.map_requested);
        assert!(record.mapped_notified);
        assert!(record.properties_ready);

        xwm.note_x11_surface_serial(handle, 0x1234, 0)
            .expect("late X11 surface serial");
        xwm.ingest_wayland_association(XwaylandAssociationEvent::Committed {
            generation,
            serial: NonZeroU64::new(0x1234).expect("serial"),
            surface_id: 42,
        })
        .expect("late Wayland association");
        xwm.mark_window_buffer_ready(handle)
            .expect("late buffer readiness");

        let events = ready_events(&mut xwm);
        assert_eq!(events.len(), 1);
        assert_eq!(ready_surface_id(&events), Some(42));
    }

    #[test]
    fn managed_map_notify_emits_window_ready_when_it_is_the_final_gate() {
        let generation = generation(1);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = prepare_managed_window(&mut xwm, 100, true, true, true);

        normalize(&mut xwm, map_event(handle.xid(), false)).expect("normalize MapNotify");

        let events = ready_events(&mut xwm);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events.first(),
            Some(super::super::XwmEvent::WindowReady(snapshot))
                if snapshot.handle == handle && snapshot.surface_id == 42
        ));
        assert_eq!(ready_surface_id(&events), Some(42));
    }

    #[test]
    fn map_notify_then_buffer_ready_emits_once() {
        let generation = generation(2);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = prepare_managed_window(&mut xwm, 101, true, true, false);

        normalize(&mut xwm, map_event(handle.xid(), false)).expect("normalize MapNotify");
        assert!(ready_events(&mut xwm).is_empty());
        xwm.mark_window_buffer_ready(handle)
            .expect("buffer readiness");
        let events = ready_events(&mut xwm);
        assert_eq!(events.len(), 1);
        assert_eq!(ready_surface_id(&events), Some(42));
    }

    #[test]
    fn map_notify_then_association_emits_once() {
        let generation = generation(3);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = prepare_managed_window(&mut xwm, 102, true, false, true);

        normalize(&mut xwm, map_event(handle.xid(), false)).expect("normalize MapNotify");
        assert!(ready_events(&mut xwm).is_empty());
        xwm.note_x11_surface_serial(handle, 0x1234, 0)
            .expect("X11 surface serial");
        xwm.ingest_wayland_association(XwaylandAssociationEvent::Committed {
            generation,
            serial: NonZeroU64::new(0x1234).expect("serial"),
            surface_id: 42,
        })
        .expect("Wayland association");
        let events = ready_events(&mut xwm);
        assert_eq!(events.len(), 1);
        assert_eq!(ready_surface_id(&events), Some(42));
    }

    #[test]
    fn map_notify_then_final_property_reply_emits_once() {
        let generation = generation(4);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = prepare_managed_window(&mut xwm, 103, false, true, true);

        normalize(&mut xwm, map_event(handle.xid(), false)).expect("normalize MapNotify");
        assert!(ready_events(&mut xwm).is_empty());
        xwm.windows
            .get_mut(handle)
            .expect("window record")
            .properties_ready = true;
        xwm.emit_ready_if_complete(handle)
            .expect("final property readiness");
        let events = ready_events(&mut xwm);
        assert_eq!(events.len(), 1);
        assert_eq!(ready_surface_id(&events), Some(42));
    }

    #[test]
    fn duplicate_map_notify_does_not_duplicate_window_ready() {
        let generation = generation(5);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = prepare_managed_window(&mut xwm, 104, true, true, true);

        normalize(&mut xwm, map_event(handle.xid(), false)).expect("first MapNotify");
        normalize(&mut xwm, map_event(handle.xid(), false)).expect("duplicate MapNotify");
        let events = ready_events(&mut xwm);
        assert_eq!(events.len(), 1);
        assert_eq!(ready_surface_id(&events), Some(42));
    }

    #[test]
    fn destroy_after_auxiliary_reclassification_emits_terminal_cleanup() {
        let generation = generation(13);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = prepare_managed_window(&mut xwm, 110, true, true, true);

        normalize(&mut xwm, map_event(handle.xid(), false)).expect("ready MapNotify");
        assert_eq!(ready_surface_id(&ready_events(&mut xwm)), Some(42));
        let record = xwm
            .windows
            .get_mut(handle)
            .expect("published window record");
        record.snapshot = None;
        record.lifecycle = super::super::X11WindowLifecycle::Auxiliary;

        normalize(&mut xwm, destroy_event(handle.xid())).expect("terminal DestroyNotify");

        assert_eq!(
            ready_events(&mut xwm),
            vec![super::super::XwmEvent::WindowDestroyed(handle)]
        );
    }

    #[test]
    fn map_notify_before_map_command_is_classified_as_external_mapping() {
        let generation = generation(6);
        let (mut xwm, mut peer) = test_fixture(generation);
        let handle = X11WindowHandle::new(generation, 105);
        assert!(xwm.windows.insert_observed(handle));
        let record = xwm.windows.get_mut(handle).expect("window record");
        record.map_requested = true;
        record.properties_ready = true;
        record.association = Some(super::super::AssociatedSurface {
            generation,
            serial: NonZeroU64::new(0x1234).expect("serial"),
            surface_id: 42,
            map_serial: 0,
        });
        record.buffer_ready = true;

        normalize(&mut xwm, map_event(handle.xid(), false)).expect("external MapNotify");
        complete_property_refresh(&mut xwm, &mut peer);
        let events = ready_events(&mut xwm);
        assert_eq!(events.len(), 1);
        assert_eq!(ready_surface_id(&events), Some(42));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, super::super::XwmEvent::WindowMapRequested(_)))
        );
    }

    #[test]
    fn managed_map_notify_without_prior_request_is_adopted_as_mapped() {
        let generation = generation(12);
        let (mut xwm, mut peer) = test_fixture(generation);
        let handle = X11WindowHandle::new(generation, 109);
        assert!(xwm.windows.insert_observed(handle));
        xwm.windows
            .get_mut(handle)
            .expect("window record")
            .properties_ready = true;
        xwm.windows
            .mark_associated(
                handle,
                super::super::AssociatedSurface {
                    generation,
                    serial: NonZeroU64::new(0x1234).expect("serial"),
                    surface_id: 42,
                    map_serial: 0,
                },
            )
            .expect("association");
        xwm.windows
            .mark_buffer_ready(handle)
            .expect("buffer readiness");

        normalize(&mut xwm, map_event(handle.xid(), false)).expect("external MapNotify");
        complete_property_refresh(&mut xwm, &mut peer);

        let events = ready_events(&mut xwm);
        assert_eq!(events.len(), 1);
        assert_eq!(ready_surface_id(&events), Some(42));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, super::super::XwmEvent::WindowMapRequested(_)))
        );
        let record = xwm.windows.get(handle).expect("managed window record");
        assert!(record.map_requested);
        assert!(record.map_authorized);
        assert!(record.mapped_notified);
        assert!(!record.map_operation_pending);
    }

    #[test]
    fn external_map_notify_refreshes_properties_after_create_scan() {
        let generation = generation(24);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = X11WindowHandle::new(generation, 124);
        assert!(xwm.windows.insert_observed(handle));
        let record = xwm.windows.get_mut(handle).expect("window record");
        record.properties_ready = true;
        record.property_epoch = 7;

        normalize(&mut xwm, map_event(handle.xid(), false)).expect("external MapNotify");

        let record = xwm.windows.get(handle).expect("window record");
        assert_eq!(record.property_epoch, 8);
        assert!(record.refresh_all);
    }

    #[test]
    fn map_notify_reconciles_existing_override_redirect_kind() {
        let generation = generation(25);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = X11WindowHandle::new(generation, 125);
        assert!(xwm.windows.insert_observed(handle));

        normalize(&mut xwm, map_event(handle.xid(), true)).expect("override MapNotify");

        assert_eq!(
            xwm.windows.get(handle).expect("window record").kind,
            DesktopWindowKind::OverrideRedirect
        );
    }

    #[test]
    fn configure_notify_reconciles_override_redirect_kind() {
        let generation = generation(26);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = X11WindowHandle::new(generation, 126);
        assert!(xwm.windows.insert_observed(handle));

        normalize(&mut xwm, configure_event(handle.xid(), true)).expect("override ConfigureNotify");

        assert_eq!(
            xwm.windows.get(handle).expect("window record").kind,
            DesktopWindowKind::OverrideRedirect
        );
    }

    #[test]
    fn override_redirect_map_notify_keeps_external_lifecycle() {
        let generation = generation(7);
        let (mut xwm, mut peer) = test_fixture(generation);
        let handle = X11WindowHandle::new(generation, 106);
        assert!(xwm.windows.insert_observed_with_kind(
            handle,
            DesktopWindowKind::OverrideRedirect,
            X11Geometry::default()
        ));
        let record = xwm.windows.get_mut(handle).expect("window record");
        record.properties_ready = true;
        record.association = Some(super::super::AssociatedSurface {
            generation,
            serial: NonZeroU64::new(0x1234).expect("serial"),
            surface_id: 42,
            map_serial: 0,
        });
        record.buffer_ready = true;

        normalize(&mut xwm, map_event(handle.xid(), true)).expect("override MapNotify");
        complete_property_refresh(&mut xwm, &mut peer);
        let events = ready_events(&mut xwm);
        assert_eq!(events.len(), 1);
        assert_eq!(ready_surface_id(&events), Some(42));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, super::super::XwmEvent::WindowMapRequested(_)))
        );
    }

    #[test]
    fn override_redirect_unmap_can_remap_with_a_fresh_wayland_surface() {
        let generation = generation(14);
        let (mut xwm, mut peer) = test_fixture(generation);
        let handle = X11WindowHandle::new(generation, 111);
        assert!(xwm.windows.insert_observed_with_kind(
            handle,
            DesktopWindowKind::OverrideRedirect,
            X11Geometry::default()
        ));
        xwm.windows
            .get_mut(handle)
            .expect("window record")
            .properties_ready = true;

        xwm.note_x11_surface_serial(handle, 0x1234, 0)
            .expect("first X11 surface serial");
        xwm.ingest_wayland_association(XwaylandAssociationEvent::Committed {
            generation,
            serial: NonZeroU64::new(0x1234).expect("first serial"),
            surface_id: 42,
        })
        .expect("first Wayland association");
        xwm.mark_window_buffer_ready(handle)
            .expect("first buffer readiness");
        normalize(&mut xwm, map_event(handle.xid(), true)).expect("first MapNotify");
        complete_property_refresh(&mut xwm, &mut peer);
        assert_eq!(ready_surface_id(&ready_events(&mut xwm)), Some(42));

        normalize(&mut xwm, unmap_event(handle.xid())).expect("popup UnmapNotify");
        assert!(matches!(
            ready_events(&mut xwm).as_slice(),
            [super::super::XwmEvent::WindowWithdrawn(withdrawn)] if *withdrawn == handle
        ));
        let record = xwm.windows.get(handle).expect("withdrawn window record");
        assert!(record.association.is_none());
        assert!(!record.buffer_ready);

        xwm.windows
            .get_mut(handle)
            .expect("reused window record")
            .properties_ready = true;
        xwm.note_x11_surface_serial(handle, 0x5678, 0)
            .expect("replacement X11 surface serial");
        xwm.ingest_wayland_association(XwaylandAssociationEvent::Committed {
            generation,
            serial: NonZeroU64::new(0x5678).expect("replacement serial"),
            surface_id: 43,
        })
        .expect("replacement Wayland association");
        xwm.mark_window_buffer_ready(handle)
            .expect("replacement buffer readiness");
        normalize(&mut xwm, map_event(handle.xid(), true)).expect("replacement MapNotify");
        complete_property_refresh(&mut xwm, &mut peer);

        assert_eq!(ready_surface_id(&ready_events(&mut xwm)), Some(43));
    }

    #[test]
    fn wm_unmap_confirmation_is_iconic_and_restore_needs_a_new_buffer() {
        let generation = generation(27);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = prepare_ready_managed_window(&mut xwm, 127);
        let first_map_serial = xwm.windows.get(handle).expect("window record").map_serial;

        super::super::commands::execute(&mut xwm, super::super::XwmCommand::Unmap(handle))
            .expect("WM unmap command");
        assert_eq!(
            xwm.windows.get(handle).expect("window record").lifecycle,
            X11WindowLifecycle::Iconic
        );

        normalize(&mut xwm, unmap_event(handle.xid())).expect("WM UnmapNotify");
        assert!(ready_events(&mut xwm).is_empty());
        let record = xwm.windows.get(handle).expect("iconic window record");
        assert_eq!(record.inflight_wm_unmaps, 0);
        assert!(record.snapshot.is_some());
        assert!(record.association.is_some());
        assert!(!record.buffer_ready);

        super::super::commands::execute(&mut xwm, super::super::XwmCommand::Map(handle))
            .expect("restore map command");
        assert!(
            xwm.windows
                .get(handle)
                .expect("restoring window record")
                .map_serial
                > first_map_serial
        );
        normalize(&mut xwm, map_event(handle.xid(), false)).expect("restore MapNotify");
        assert!(ready_events(&mut xwm).is_empty());
        assert_eq!(
            xwm.windows
                .get(handle)
                .expect("restoring window record")
                .lifecycle,
            X11WindowLifecycle::AssociatedAwaitingBuffer
        );

        xwm.mark_window_buffer_ready(handle)
            .expect("new buffer readiness");
        assert!(ready_events(&mut xwm).is_empty());
        assert_eq!(
            xwm.windows
                .get(handle)
                .expect("restored window record")
                .lifecycle,
            X11WindowLifecycle::Renderable
        );
    }

    #[test]
    fn wm_unmap_confirmation_is_consumed_exactly_once() {
        let generation = generation(28);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = prepare_ready_managed_window(&mut xwm, 128);

        super::super::commands::execute(&mut xwm, super::super::XwmCommand::Unmap(handle))
            .expect("WM unmap command");
        normalize(&mut xwm, unmap_event(handle.xid())).expect("WM UnmapNotify");
        assert!(ready_events(&mut xwm).is_empty());

        normalize(&mut xwm, unmap_event(handle.xid())).expect("client UnmapNotify");
        assert!(matches!(
            ready_events(&mut xwm).as_slice(),
            [super::super::XwmEvent::WindowWithdrawn(withdrawn)] if *withdrawn == handle
        ));
        assert_eq!(
            xwm.windows
                .get(handle)
                .expect("withdrawn window record")
                .lifecycle,
            X11WindowLifecycle::Withdrawn
        );
    }

    #[test]
    fn wayland_surface_removal_clears_the_mapped_x11_record() {
        let generation = generation(17);
        let (mut xwm, mut peer) = test_fixture(generation);
        let handle = X11WindowHandle::new(generation, 114);
        assert!(xwm.windows.insert_observed_with_kind(
            handle,
            DesktopWindowKind::OverrideRedirect,
            X11Geometry::default()
        ));
        xwm.windows
            .get_mut(handle)
            .expect("window record")
            .properties_ready = true;
        normalize(&mut xwm, map_event(handle.xid(), true)).expect("first MapNotify");
        complete_property_refresh(&mut xwm, &mut peer);
        xwm.note_x11_surface_serial(handle, 0x1234, 0)
            .expect("first X11 serial");
        xwm.ingest_wayland_association(XwaylandAssociationEvent::Committed {
            generation,
            serial: NonZeroU64::new(0x1234).expect("first serial"),
            surface_id: 42,
        })
        .expect("first association");
        xwm.mark_window_buffer_ready(handle)
            .expect("first buffer readiness");
        assert_eq!(ready_surface_id(&ready_events(&mut xwm)), Some(42));

        xwm.ingest_wayland_association(XwaylandAssociationEvent::Removed {
            generation,
            serial: NonZeroU64::new(0x1234).expect("first serial"),
            surface_id: 42,
        })
        .expect("Wayland surface removal");
        assert!(matches!(
            ready_events(&mut xwm).as_slice(),
            [super::super::XwmEvent::WindowWithdrawn(withdrawn)] if *withdrawn == handle
        ));
        let record = xwm.windows.get(handle).expect("dissociated record");
        assert!(record.association.is_none());
        assert!(!record.buffer_ready);

        xwm.note_x11_surface_serial(handle, 0x5678, 0)
            .expect("replacement X11 serial");
        xwm.ingest_wayland_association(XwaylandAssociationEvent::Committed {
            generation,
            serial: NonZeroU64::new(0x5678).expect("replacement serial"),
            surface_id: 43,
        })
        .expect("replacement association");
        xwm.mark_window_buffer_ready(handle)
            .expect("replacement buffer readiness");

        assert_eq!(ready_surface_id(&ready_events(&mut xwm)), Some(43));
    }

    #[test]
    fn net_wm_moveresize_client_message_is_normalized() {
        let generation = generation(15);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = X11WindowHandle::new(generation, 112);
        assert!(xwm.windows.insert_observed(handle));
        let event = xproto::ClientMessageEvent::new(
            32,
            handle.xid(),
            xwm.atoms.get(XwmAtomName::NetWmMoveresize),
            xproto::ClientMessageData::from([320, 240, 4, 1, 1]),
        );

        normalize(&mut xwm, Event::ClientMessage(event)).expect("normalize moveresize request");

        assert_eq!(
            xwm.take_events().collect::<Vec<_>>(),
            vec![super::super::XwmEvent::MoveResizeRequested {
                window: handle,
                request: super::super::X11MoveResizeRequest {
                    root_x: 320,
                    root_y: 240,
                    direction: super::super::X11MoveResizeDirection::BottomRight,
                    button: 1,
                    source: 1,
                },
            }]
        );
    }

    #[test]
    fn net_moveresize_window_client_message_becomes_configure_request() {
        let generation = generation(16);
        let (mut xwm, _peer) = test_fixture(generation);
        let handle = X11WindowHandle::new(generation, 113);
        assert!(xwm.windows.insert_observed(handle));
        let event = xproto::ClientMessageEvent::new(
            32,
            handle.xid(),
            xwm.atoms.get(XwmAtomName::NetMoveResizeWindow),
            xproto::ClientMessageData::from([(1 << 8) | (1 << 10), 50, 60, 700, 500]),
        );

        normalize(&mut xwm, Event::ClientMessage(event))
            .expect("normalize one-shot moveresize request");

        assert_eq!(
            xwm.take_events().collect::<Vec<_>>(),
            vec![super::super::XwmEvent::ConfigureRequested {
                window: handle,
                request: X11ConfigureRequest {
                    requested: X11Geometry {
                        x: 50,
                        y: 60,
                        width: 700,
                        height: 500,
                    },
                    fields: X11ConfigureFlags {
                        x: true,
                        width: true,
                        ..X11ConfigureFlags::default()
                    },
                    border_width: 0,
                    sibling: None,
                    stack_mode: None,
                },
            }]
        );
    }

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

#[cfg(test)]
#[path = "events_regression_tests.rs"]
mod regression_tests;
