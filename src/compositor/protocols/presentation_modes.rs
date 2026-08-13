use super::super::*;

#[derive(Debug, Clone)]
pub(super) struct TearingControlResourceData {
    pub(super) surface_id: u32,
    pub(super) surface: wl_surface::WlSurface,
}

#[derive(Debug, Clone)]
pub(super) struct ContentTypeResourceData {
    pub(super) surface_id: u32,
    pub(super) surface: wl_surface::WlSurface,
}

impl GlobalDispatch<wp_tearing_control_manager_v1::WpTearingControlManagerV1, ()>
    for CompositorState
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<wp_tearing_control_manager_v1::WpTearingControlManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<wp_tearing_control_manager_v1::WpTearingControlManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wp_tearing_control_manager_v1::WpTearingControlManagerV1,
        request: wp_tearing_control_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_tearing_control_manager_v1::Request::Destroy => {}
            wp_tearing_control_manager_v1::Request::GetTearingControl { id, surface } => {
                let surface_id = compositor_surface_id(&surface);
                if state.tearing_control_resources.contains_key(&surface_id) {
                    resource.post_error(
                        wp_tearing_control_manager_v1::Error::TearingControlExists,
                        "a tearing control object already exists for this surface",
                    );
                    return;
                }
                let tearing = data_init.init(
                    id,
                    TearingControlResourceData {
                        surface_id,
                        surface: surface.clone(),
                    },
                );
                state.tearing_control_resources.insert(surface_id, tearing);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wp_tearing_control_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<wp_tearing_control_v1::WpTearingControlV1, TearingControlResourceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wp_tearing_control_v1::WpTearingControlV1,
        request: wp_tearing_control_v1::Request,
        data: &TearingControlResourceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_tearing_control_v1::Request::Destroy => {
                state.remove_tearing_control_resource(resource, data.surface_id)
            }
            wp_tearing_control_v1::Request::SetPresentationHint { hint } => {
                if !data.surface.is_alive() {
                    return;
                }
                let hint = match hint {
                    0 => SurfacePresentationHint::Vsync,
                    1 => SurfacePresentationHint::Async,
                    _ => return,
                };
                if let Some(surface) = state.surface_resource_by_id(data.surface_id)
                    && let Some(surface_data) = surface.data::<SurfaceData>()
                {
                    surface_data.set_pending_presentation_hint(hint);
                }
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wp_tearing_control_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &wp_tearing_control_v1::WpTearingControlV1,
        data: &TearingControlResourceData,
    ) {
        state.remove_tearing_control_resource(resource, data.surface_id);
        if let Some(surface) = state.surface_resource_by_id(data.surface_id)
            && let Some(surface_data) = surface.data::<SurfaceData>()
        {
            surface_data.revert_pending_presentation_hint();
        }
    }
}

impl GlobalDispatch<wp_content_type_manager_v1::WpContentTypeManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<wp_content_type_manager_v1::WpContentTypeManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<wp_content_type_manager_v1::WpContentTypeManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wp_content_type_manager_v1::WpContentTypeManagerV1,
        request: wp_content_type_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_content_type_manager_v1::Request::Destroy => {}
            wp_content_type_manager_v1::Request::GetSurfaceContentType { id, surface } => {
                let surface_id = compositor_surface_id(&surface);
                if state.content_type_resources.contains_key(&surface_id) {
                    resource.post_error(
                        wp_content_type_manager_v1::Error::AlreadyConstructed,
                        "a content type object already exists for this surface",
                    );
                    return;
                }
                let content_type = data_init.init(
                    id,
                    ContentTypeResourceData {
                        surface_id,
                        surface: surface.clone(),
                    },
                );
                state
                    .content_type_resources
                    .insert(surface_id, content_type);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wp_content_type_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<wp_content_type_v1::WpContentTypeV1, ContentTypeResourceData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &wp_content_type_v1::WpContentTypeV1,
        request: wp_content_type_v1::Request,
        data: &ContentTypeResourceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wp_content_type_v1::Request::Destroy => {
                state.remove_content_type_resource(resource, data.surface_id)
            }
            wp_content_type_v1::Request::SetContentType { content_type } => {
                if !data.surface.is_alive() {
                    return;
                }
                let content_type = match content_type {
                    0 => SurfaceContentType::None,
                    1 => SurfaceContentType::Photo,
                    2 => SurfaceContentType::Video,
                    3 => SurfaceContentType::Game,
                    _ => return,
                };
                if let Some(surface) = state.surface_resource_by_id(data.surface_id)
                    && let Some(surface_data) = surface.data::<SurfaceData>()
                {
                    surface_data.set_pending_content_type(content_type);
                }
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wp_content_type_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &wp_content_type_v1::WpContentTypeV1,
        data: &ContentTypeResourceData,
    ) {
        state.remove_content_type_resource(resource, data.surface_id);
        if let Some(surface) = state.surface_resource_by_id(data.surface_id)
            && let Some(surface_data) = surface.data::<SurfaceData>()
        {
            surface_data.revert_pending_content_type();
        }
    }
}

impl CompositorState {
    pub(in crate::compositor) fn remove_tearing_control_resource(
        &mut self,
        resource: &wp_tearing_control_v1::WpTearingControlV1,
        surface_id: u32,
    ) {
        if self
            .tearing_control_resources
            .get(&surface_id)
            .is_some_and(|existing| existing.id() == resource.id())
        {
            self.tearing_control_resources.remove(&surface_id);
        }
    }

    pub(in crate::compositor) fn remove_content_type_resource(
        &mut self,
        resource: &wp_content_type_v1::WpContentTypeV1,
        surface_id: u32,
    ) {
        if self
            .content_type_resources
            .get(&surface_id)
            .is_some_and(|existing| existing.id() == resource.id())
        {
            self.content_type_resources.remove(&surface_id);
        }
    }
}
