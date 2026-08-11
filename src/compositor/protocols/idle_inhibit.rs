use super::super::*;

#[derive(Debug, Clone)]
struct IdleInhibitorData {
    client_id: ClientId,
    target_surface: wl_surface::WlSurface,
}

impl GlobalDispatch<zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1,
        request: zwp_idle_inhibit_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_idle_inhibit_manager_v1::Request::CreateInhibitor { id, surface } => {
                if !surface.id().same_client_as(&resource.id()) {
                    return;
                }
                let inhibitor = data_init.init(
                    id,
                    IdleInhibitorData {
                        client_id: client.id(),
                        target_surface: surface.clone(),
                    },
                );
                state.add_idle_inhibitor(inhibitor, client.id(), surface);
            }
            zwp_idle_inhibit_manager_v1::Request::Destroy => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zwp_idle_inhibit_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1, IdleInhibitorData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
        request: zwp_idle_inhibitor_v1::Request,
        _data: &IdleInhibitorData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let zwp_idle_inhibitor_v1::Request::Destroy = request {
            state.remove_idle_inhibitor(resource);
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &zwp_idle_inhibitor_v1::ZwpIdleInhibitorV1,
        _data: &IdleInhibitorData,
    ) {
        state.remove_idle_inhibitor(resource);
    }
}
