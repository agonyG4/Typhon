use super::super::*;

impl Dispatch<wp_viewporter::WpViewporter, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wp_viewporter::WpViewporter,
        request: wp_viewporter::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_viewporter::Request::Destroy => {}
            wp_viewporter::Request::GetViewport { id, surface } => {
                data_init.init(id, ViewportData { surface });
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_viewport::WpViewport, ViewportData> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &wp_viewport::WpViewport,
        request: wp_viewport::Request,
        data: &ViewportData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let Some(surface_data) = data.surface.data::<SurfaceData>() else {
            return;
        };
        match request {
            wp_viewport::Request::Destroy => {
                surface_data.set_pending_viewport_destination(None);
            }
            wp_viewport::Request::SetDestination { width, height } => {
                let destination = if width == -1 && height == -1 {
                    None
                } else if width > 0 && height > 0 {
                    BufferSize::new(width as u32, height as u32)
                } else {
                    return;
                };
                surface_data.set_pending_viewport_destination(destination);
            }
            wp_viewport::Request::SetSource { .. } => {}
            _ => {}
        }
    }
}

impl Dispatch<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        request: wp_fractional_scale_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_fractional_scale_manager_v1::Request::Destroy => {}
            wp_fractional_scale_manager_v1::Request::GetFractionalScale { id, surface } => {
                let fractional_scale =
                    data_init.init(id, FractionalScaleData::new(surface.clone()));
                state.register_fractional_scale_resource(&surface, fractional_scale);
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, FractionalScaleData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wp_fractional_scale_v1::WpFractionalScaleV1,
        request: wp_fractional_scale_v1::Request,
        data: &FractionalScaleData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_fractional_scale_v1::Request::Destroy = request {
            state.unregister_fractional_scale_resource(
                data.surface_id(),
                resource.id().protocol_id(),
            );
        }
    }
}
