use super::super::*;

impl GlobalDispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<zwlr_layer_shell_v1::ZwlrLayerShellV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        request: zwlr_layer_shell_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwlr_layer_shell_v1::Request::GetLayerSurface {
                id,
                surface,
                output,
                layer,
                namespace,
            } => {
                let surface_id = compositor_surface_id(&surface);
                if state.toplevel_surfaces.contains_key(&surface_id)
                    || state.popup_surfaces.contains_key(&surface_id)
                    || state.layer_surfaces.contains_key(&surface_id)
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        zwlr_layer_shell_v1::Error::Role,
                        "wl_surface already has an incompatible role".to_string(),
                    );
                    return;
                }
                if let Err(error) = state.assign_surface_role(surface_id, SurfaceRole::LayerSurface)
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        zwlr_layer_shell_v1::Error::Role,
                        error.message(),
                    );
                    return;
                }
                if state.current_surface_buffers.contains_key(&surface_id)
                    || state
                        .renderable_surfaces
                        .iter()
                        .any(|renderable| renderable.surface_id == surface_id)
                {
                    state.rollback_surface_role_reservation(surface_id, SurfaceRole::LayerSurface);
                    state.post_protocol_error(
                        client,
                        resource,
                        zwlr_layer_shell_v1::Error::AlreadyConstructed,
                        "wl_surface already has committed content".to_string(),
                    );
                    return;
                }
                let Ok(layer) = crate::compositor::layer_shell::layer_from_protocol(layer) else {
                    state.post_protocol_error(
                        client,
                        resource,
                        zwlr_layer_shell_v1::Error::InvalidLayer,
                        "invalid layer".to_string(),
                    );
                    return;
                };
                let layer_surface = data_init.init(
                    id,
                    LayerSurfaceData {
                        surface: surface.clone(),
                    },
                );
                state.register_layer_surface(surface, layer_surface, output, namespace, layer);
                state.adopt_current_surface_content_for_role(surface_id);
            }
            zwlr_layer_shell_v1::Request::Destroy => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zwlr_layer_shell_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, LayerSurfaceData> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        request: zwlr_layer_surface_v1::Request,
        data: &LayerSurfaceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let surface_id = compositor_surface_id(&data.surface);
        match request {
            zwlr_layer_surface_v1::Request::SetSize { width, height } => {
                state.set_layer_surface_pending_size(surface_id, width, height);
            }
            zwlr_layer_surface_v1::Request::SetAnchor { anchor } => {
                let Some(anchors) = crate::compositor::layer_shell::anchors_from_protocol(anchor)
                else {
                    state.post_protocol_error(
                        client,
                        resource,
                        zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                        "invalid anchors".to_string(),
                    );
                    return;
                };
                state.set_layer_surface_pending_anchor(surface_id, anchors);
            }
            zwlr_layer_surface_v1::Request::SetExclusiveZone { zone } => {
                state.set_layer_surface_pending_exclusive_zone(surface_id, zone);
            }
            zwlr_layer_surface_v1::Request::SetMargin {
                top,
                right,
                bottom,
                left,
            } => {
                state.set_layer_surface_pending_margins(
                    surface_id,
                    crate::compositor::layer_shell::LayerMargins {
                        top,
                        right,
                        bottom,
                        left,
                    },
                );
            }
            zwlr_layer_surface_v1::Request::SetKeyboardInteractivity {
                keyboard_interactivity,
            } => {
                match crate::compositor::layer_shell::keyboard_interactivity_from_protocol(
                    keyboard_interactivity,
                ) {
                    Ok(mode) => {
                        state.set_layer_surface_pending_keyboard_interactivity(surface_id, mode)
                    }
                    Err(error) => {
                        state.post_protocol_error(
                            client,
                            resource,
                            error,
                            "invalid keyboard interactivity".to_string(),
                        );
                    }
                }
            }
            zwlr_layer_surface_v1::Request::GetPopup { popup } => {
                if let Some(popup_data) = popup.data::<XdgPopupData>()
                    && let Err(message) = state.associate_layer_surface_popup(
                        surface_id,
                        compositor_surface_id(&popup_data.surface),
                    )
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        zwlr_layer_surface_v1::Error::InvalidSurfaceState,
                        message.to_string(),
                    );
                }
            }
            zwlr_layer_surface_v1::Request::AckConfigure { serial } => {
                state.ack_layer_surface_configure(surface_id, serial);
            }
            zwlr_layer_surface_v1::Request::Destroy => {
                state.destroy_layer_surface_role(surface_id);
            }
            zwlr_layer_surface_v1::Request::SetLayer { layer } => {
                let Ok(layer) = crate::compositor::layer_shell::layer_from_protocol(layer) else {
                    state.post_protocol_error(
                        client,
                        resource,
                        zwlr_layer_shell_v1::Error::InvalidLayer,
                        "invalid layer".to_string(),
                    );
                    return;
                };
                state.set_layer_surface_pending_layer(surface_id, layer);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zwlr_layer_surface_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        _resource: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        data: &LayerSurfaceData,
    ) {
        state.destroy_layer_surface_role(compositor_surface_id(&data.surface));
    }
}
