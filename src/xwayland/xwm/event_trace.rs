use super::{X11WindowHandle, Xwm};
use crate::compositor::DesktopWindowKind;
use crate::xwayland::trace::{self, TraceCategory, TraceFields};
use x11rb::protocol::Event;

pub(super) fn trace_raw_event(event: &Event) {
    match event {
        Event::CreateNotify(event) => {
            trace::emit_category(TraceCategory::Lifecycle, "CreateNotify", || {
                TraceFields::new()
                    .field("source", "x11")
                    .field("x_event_send_event", event.response_type & 0x80 != 0)
                    .field("xid", event.window)
                    .field("parent_xid", event.parent)
                    .field("override_redirect_event", event.override_redirect)
            })
        }
        Event::MapRequest(event) => {
            trace::emit_category(TraceCategory::Lifecycle, "MapRequest", || {
                TraceFields::new()
                    .field("source", "x11")
                    .field("x_event_send_event", event.response_type & 0x80 != 0)
                    .field("xid", event.window)
                    .field("parent_xid", event.parent)
            })
        }
        Event::MapNotify(event) => {
            trace::emit_category(TraceCategory::Lifecycle, "MapNotify", || {
                TraceFields::new()
                    .field("source", "x11")
                    .field("x_event_send_event", event.response_type & 0x80 != 0)
                    .field("xid", event.window)
                    .field("override_redirect_event", event.override_redirect)
            })
        }
        Event::ConfigureNotify(event) => {
            trace::emit_category(TraceCategory::Geometry, "ConfigureNotify", || {
                TraceFields::new()
                    .field("source", "x11")
                    .field("x_event_send_event", event.response_type & 0x80 != 0)
                    .field("xid", event.window)
                    .field("override_redirect_event", event.override_redirect)
                    .field("geometry_x", event.x)
                    .field("geometry_y", event.y)
                    .field("geometry_width", event.width)
                    .field("geometry_height", event.height)
            })
        }
        Event::PropertyNotify(event) => trace::emit("PropertyNotify", || {
            TraceFields::new()
                .field("source", "x11")
                .field("x_event_send_event", event.response_type & 0x80 != 0)
                .field("xid", event.window)
                .field("property_atom", event.atom)
                .field("property_state", format!("{:?}", event.state))
        }),
        Event::UnmapNotify(event) => {
            trace::emit_category(TraceCategory::Lifecycle, "UnmapNotify", || {
                TraceFields::new()
                    .field("source", "x11")
                    .field("x_event_send_event", event.response_type & 0x80 != 0)
                    .field("xid", event.window)
                    .field("from_configure", event.from_configure)
            })
        }
        Event::DestroyNotify(event) => {
            trace::emit_category(TraceCategory::Lifecycle, "DestroyNotify", || {
                TraceFields::new()
                    .field("source", "x11")
                    .field("x_event_send_event", event.response_type & 0x80 != 0)
                    .field("xid", event.window)
            })
        }
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

pub(super) fn trace_window_state(
    xwm: &Xwm,
    event: &'static str,
    handle: X11WindowHandle,
    fields: TraceFields,
) {
    let Some(record) = xwm.windows.get(handle) else {
        return;
    };
    trace::emit_category(TraceCategory::Lifecycle, event, || {
        let association = record.association;
        fields
            .field("source", "xwm")
            .field("xid", handle.xid())
            .field("generation", handle.generation().get())
            .field(
                "override_redirect_stored",
                record.kind == DesktopWindowKind::OverrideRedirect,
            )
            .field(
                "override_redirect",
                record.kind == DesktopWindowKind::OverrideRedirect,
            )
            .field("lifecycle", format!("{:?}", record.lifecycle))
            .field("property_epoch", record.property_epoch)
            .field("properties_ready", record.properties_ready)
            .field("buffer_ready", record.buffer_ready)
            .field("map_serial", record.map_serial)
            .field("geometry_x", record.geometry.x)
            .field("geometry_y", record.geometry.y)
            .field("geometry_width", record.geometry.width)
            .field("geometry_height", record.geometry.height)
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
            .optional(
                "client_leader",
                record.properties.client_leader.map(|leader| leader.xid()),
            )
            .optional(
                "root_tree_stack_epoch",
                xwm.override_redirect_stack_trace(handle).0,
            )
            .optional(
                "root_tree_stack_index",
                xwm.override_redirect_stack_trace(handle).1,
            )
    });
}
