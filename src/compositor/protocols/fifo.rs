use super::super::*;

#[derive(Debug, Clone)]
pub(super) struct FifoResourceData {
    pub(super) surface_id: u32,
    pub(super) surface: wl_surface::WlSurface,
}

impl GlobalDispatch<wp_fifo_manager_v1::WpFifoManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<wp_fifo_manager_v1::WpFifoManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<wp_fifo_manager_v1::WpFifoManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wp_fifo_manager_v1::WpFifoManagerV1,
        request: wp_fifo_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_fifo_manager_v1::Request::Destroy => {}
            wp_fifo_manager_v1::Request::GetFifo { id, surface } => {
                let surface_id = compositor_surface_id(&surface);
                if state.fifo_resources.contains_key(&surface_id) {
                    resource.post_error(
                        wp_fifo_manager_v1::Error::AlreadyExists,
                        "a FIFO object already exists for this surface",
                    );
                    return;
                }
                let fifo = data_init.init(
                    id,
                    FifoResourceData {
                        surface_id,
                        surface: surface.clone(),
                    },
                );
                state.fifo_resources.insert(surface_id, fifo);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wp_fifo_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<wp_fifo_v1::WpFifoV1, FifoResourceData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wp_fifo_v1::WpFifoV1,
        request: wp_fifo_v1::Request,
        data: &FifoResourceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_fifo_v1::Request::Destroy => state.remove_fifo_resource(resource, data.surface_id),
            wp_fifo_v1::Request::SetBarrier => {
                if !data.surface.is_alive() {
                    state.note_protocol_error_metric();
                    resource.post_error(
                        wp_fifo_v1::Error::SurfaceDestroyed,
                        "associated wl_surface was destroyed",
                    );
                } else {
                    state.set_pending_fifo_barrier(data.surface_id);
                }
            }
            wp_fifo_v1::Request::WaitBarrier => {
                if !data.surface.is_alive() {
                    state.note_protocol_error_metric();
                    resource.post_error(
                        wp_fifo_v1::Error::SurfaceDestroyed,
                        "associated wl_surface was destroyed",
                    );
                } else {
                    state.set_pending_fifo_wait(data.surface_id);
                }
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wp_fifo_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &wp_fifo_v1::WpFifoV1,
        data: &FifoResourceData,
    ) {
        state.remove_fifo_resource(resource, data.surface_id);
    }
}
