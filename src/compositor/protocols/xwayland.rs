use super::super::*;
use crate::compositor::server::XwaylandShellGlobalData;

use wayland_protocols::xwayland::shell::v1::server::{xwayland_shell_v1, xwayland_surface_v1};

#[derive(Debug, Clone)]
struct XwaylandShellData {
    identity: XwaylandClientIdentity,
}

#[derive(Debug, Clone, Copy)]
struct XwaylandSurfaceData {
    generation: XwaylandGeneration,
    surface_id: u32,
}

impl GlobalDispatch<xwayland_shell_v1::XwaylandShellV1, XwaylandShellGlobalData>
    for CompositorState
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        client: &Client,
        resource: New<xwayland_shell_v1::XwaylandShellV1>,
        global_data: &XwaylandShellGlobalData,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let Some(identity) = global_data
            .active
            .lock()
            .ok()
            .and_then(|active| active.clone())
        else {
            return;
        };
        if identity.client_id != client.id() {
            return;
        }
        if let Ok(mut events) = global_data.bind_events.lock() {
            events.push(identity.clone());
        }
        data_init.init(resource, XwaylandShellData { identity });
    }

    fn can_view(client: Client, global_data: &XwaylandShellGlobalData) -> bool {
        global_data.active.lock().ok().is_some_and(|active| {
            active
                .as_ref()
                .is_some_and(|identity| identity.client_id == client.id())
        })
    }
}

impl Dispatch<xwayland_shell_v1::XwaylandShellV1, XwaylandShellData> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &xwayland_shell_v1::XwaylandShellV1,
        request: xwayland_shell_v1::Request,
        data: &XwaylandShellData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if state.xwayland.client_identity.as_ref() != Some(&data.identity)
            || data.identity.client_id != client.id()
        {
            state.post_protocol_error(
                client,
                resource,
                xwayland_shell_v1::Error::Role,
                "xwayland-shell-v1 request came from an inactive XWayland generation",
            );
            return;
        }
        match request {
            xwayland_shell_v1::Request::GetXwaylandSurface { id, surface } => {
                let surface_id = compositor_surface_id(&surface);
                if let Err(error) = state.get_xwayland_surface(surface_id, data.identity.generation)
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        xwayland_shell_v1::Error::Role,
                        error.message(),
                    );
                    return;
                }
                let surface_resource = data_init.init(
                    id,
                    XwaylandSurfaceData {
                        generation: data.identity.generation,
                        surface_id,
                    },
                );
                state.register_xwayland_surface_resource(surface_id, surface_resource);
            }
            xwayland_shell_v1::Request::Destroy => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "xwayland_shell_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<xwayland_surface_v1::XwaylandSurfaceV1, XwaylandSurfaceData> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &xwayland_surface_v1::XwaylandSurfaceV1,
        request: xwayland_surface_v1::Request,
        data: &XwaylandSurfaceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if state
            .xwayland
            .client_identity
            .as_ref()
            .is_none_or(|identity| {
                identity.client_id != client.id() || identity.generation != data.generation
            })
        {
            state.post_protocol_error(
                client,
                resource,
                xwayland_surface_v1::Error::InvalidSerial,
                "xwayland surface belongs to an inactive generation",
            );
            return;
        }
        match request {
            xwayland_surface_v1::Request::SetSerial {
                serial_lo,
                serial_hi,
            } => {
                let Some(serial) = crate::xwayland::serial_from_parts(serial_lo, serial_hi) else {
                    state.post_protocol_error(
                        client,
                        resource,
                        xwayland_surface_v1::Error::InvalidSerial,
                        "xwayland surface serial must be nonzero",
                    );
                    return;
                };
                if state
                    .set_xwayland_pending_serial(data.surface_id, data.generation, serial)
                    .is_err()
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        xwayland_surface_v1::Error::AlreadyAssociated,
                        "wl_surface already has an XWayland association",
                    );
                }
            }
            xwayland_surface_v1::Request::Destroy => {
                state.destroy_xwayland_surface_object(data.surface_id);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "xwayland_surface_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        _resource: &xwayland_surface_v1::XwaylandSurfaceV1,
        data: &XwaylandSurfaceData,
    ) {
        state.destroy_xwayland_surface_object(data.surface_id);
    }
}
