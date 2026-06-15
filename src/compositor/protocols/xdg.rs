use super::super::*;

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &xdg_wm_base::XdgWmBase,
        request: xdg_wm_base::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_wm_base::Request::CreatePositioner { id } => {
                data_init.init(id, Arc::new(Mutex::new(XdgPositionerState::default())));
            }
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
                data_init.init(id, XdgSurfaceData { surface });
            }
            xdg_wm_base::Request::Destroy | xdg_wm_base::Request::Pong { .. } => {}
            _ => {}
        }
    }
}

impl Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
        request: zxdg_decoration_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zxdg_decoration_manager_v1::Request::GetToplevelDecoration { id, toplevel } => {
                let Some(data) = toplevel.data::<XdgToplevelData>() else {
                    return;
                };
                let decoration = data_init.init(
                    id,
                    XdgToplevelData {
                        surface: data.surface.clone(),
                    },
                );
                send_client_side_decoration_configure(&decoration);
            }
            zxdg_decoration_manager_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, XdgToplevelData>
    for CompositorState
{
    fn request(
        _state: &mut Self,
        _client: &Client,
        resource: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
        request: zxdg_toplevel_decoration_v1::Request,
        _data: &XdgToplevelData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zxdg_toplevel_decoration_v1::Request::SetMode { .. }
            | zxdg_toplevel_decoration_v1::Request::UnsetMode => {
                send_client_side_decoration_configure(resource);
            }
            zxdg_toplevel_decoration_v1::Request::Destroy => {}
            _ => {}
        }
    }
}

fn send_client_side_decoration_configure(
    decoration: &zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1,
) {
    let _ = decoration.send_event(zxdg_toplevel_decoration_v1::Event::Configure {
        mode: WEnum::Value(zxdg_toplevel_decoration_v1::Mode::ClientSide),
    });
}

impl Dispatch<xdg_positioner::XdgPositioner, Arc<Mutex<XdgPositionerState>>> for CompositorState {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _resource: &xdg_positioner::XdgPositioner,
        request: xdg_positioner::Request,
        data: &Arc<Mutex<XdgPositionerState>>,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        let Ok(mut positioner) = data.lock() else {
            return;
        };
        match request {
            xdg_positioner::Request::SetSize { width, height } => {
                positioner.width = width.max(1);
                positioner.height = height.max(1);
            }
            xdg_positioner::Request::SetAnchorRect {
                x,
                y,
                width,
                height,
            } => {
                positioner.anchor_rect = PopupAnchorRect {
                    x,
                    y,
                    width: width.max(0),
                    height: height.max(0),
                };
            }
            xdg_positioner::Request::SetOffset { x, y } => {
                positioner.offset_x = x;
                positioner.offset_y = y;
            }
            xdg_positioner::Request::SetAnchor {
                anchor: WEnum::Value(anchor),
            } => {
                positioner.anchor = PopupEdges::from_anchor(anchor);
            }
            xdg_positioner::Request::SetGravity {
                gravity: WEnum::Value(gravity),
            } => {
                positioner.gravity = PopupEdges::from_gravity(gravity);
            }
            xdg_positioner::Request::SetConstraintAdjustment {
                constraint_adjustment: WEnum::Value(constraint_adjustment),
            } => {
                positioner.constraint_adjustment =
                    PopupConstraintAdjustment::from_xdg(constraint_adjustment);
            }
            xdg_positioner::Request::SetParentSize {
                parent_width,
                parent_height,
            } => {
                positioner.parent_size = Some((parent_width.max(1), parent_height.max(1)));
            }
            xdg_positioner::Request::SetReactive => {
                positioner.reactive = true;
            }
            xdg_positioner::Request::Destroy
            | xdg_positioner::Request::SetParentConfigure { .. } => {}
            _ => {}
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, XdgSurfaceData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &xdg_surface::XdgSurface,
        request: xdg_surface::Request,
        data: &XdgSurfaceData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_surface::Request::GetToplevel { id } => {
                let toplevel = data_init.init(
                    id,
                    XdgToplevelData {
                        surface: data.surface.clone(),
                    },
                );
                state.note_xdg_toplevel_created("unknown");
                state.register_toplevel_surface(
                    data.surface.clone(),
                    resource.clone(),
                    toplevel.clone(),
                );
            }
            xdg_surface::Request::GetPopup {
                id,
                parent,
                positioner,
            } => {
                let parent_surface = parent
                    .as_ref()
                    .and_then(|parent| parent.data::<XdgSurfaceData>())
                    .map(|data| data.surface.clone());
                let positioner_state = positioner
                    .data::<Arc<Mutex<XdgPositionerState>>>()
                    .and_then(|state| state.lock().ok().map(|state| *state))
                    .unwrap_or_default();
                let popup = data_init.init(
                    id,
                    XdgPopupData {
                        surface: data.surface.clone(),
                    },
                );
                state.register_popup_surface(
                    data.surface.clone(),
                    parent_surface,
                    resource.clone(),
                    popup,
                    positioner_state,
                );
            }
            xdg_surface::Request::SetWindowGeometry {
                x,
                y,
                width,
                height,
            } => {
                let surface_id = compositor_surface_id(&data.surface);
                if width > 0 && height > 0 {
                    state
                        .surface_window_geometries
                        .insert(surface_id, XdgWindowGeometry::new(x, y, width, height));
                    if let Some(positioner) = state
                        .popup_surfaces
                        .get(&surface_id)
                        .map(|popup| popup.positioner)
                        && positioner.reactive
                        && state.configured_xdg_surfaces.contains(&surface_id)
                    {
                        state.configure_popup_surface(surface_id, positioner, None);
                    }
                    let child_popups = state
                        .popup_surfaces
                        .iter()
                        .filter_map(|(popup_surface_id, popup)| {
                            (popup.parent_surface_id == Some(surface_id)
                                && popup.positioner.reactive
                                && state.configured_xdg_surfaces.contains(popup_surface_id))
                            .then_some((*popup_surface_id, popup.positioner))
                        })
                        .collect::<Vec<_>>();
                    for (popup_surface_id, positioner) in child_popups {
                        state.configure_popup_surface(popup_surface_id, positioner, None);
                    }
                }
            }
            xdg_surface::Request::AckConfigure { serial } => {
                state.ack_xdg_surface_configure(compositor_surface_id(&data.surface), serial);
            }
            xdg_surface::Request::Destroy => {}
            _ => {}
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, XdgToplevelData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &xdg_toplevel::XdgToplevel,
        request: xdg_toplevel::Request,
        data: &XdgToplevelData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_toplevel::Request::SetAppId { app_id } => {
                let surface_id = compositor_surface_id(&data.surface);
                if let Some(toplevel) = state.toplevel_surfaces.get_mut(&surface_id) {
                    toplevel.app_id = Some(app_id.clone());
                }
                state.last_app_id = Some(app_id);
            }
            xdg_toplevel::Request::Destroy => {
                state
                    .toplevel_surfaces
                    .remove(&compositor_surface_id(&data.surface));
            }
            xdg_toplevel::Request::Move { seat, serial } => {
                if resource_belongs_to_surface_client(&seat, &data.surface) {
                    state.begin_client_window_move(&data.surface, serial);
                }
            }
            xdg_toplevel::Request::Resize {
                seat,
                serial,
                edges,
            } => {
                if resource_belongs_to_surface_client(&seat, &data.surface)
                    && let Some(edges) = resize_edges_from_xdg(edges)
                {
                    state.begin_client_window_resize(&data.surface, serial, edges);
                }
            }
            xdg_toplevel::Request::SetMaximized => {
                state.set_root_window_mode(
                    compositor_surface_id(&data.surface),
                    ToplevelMode::Maximized,
                );
            }
            xdg_toplevel::Request::UnsetMaximized => {
                state.restore_floating_root_window(compositor_surface_id(&data.surface));
            }
            xdg_toplevel::Request::SetFullscreen { .. } => {
                state.set_root_window_mode(
                    compositor_surface_id(&data.surface),
                    ToplevelMode::Fullscreen,
                );
            }
            xdg_toplevel::Request::UnsetFullscreen => {
                state.restore_floating_root_window(compositor_surface_id(&data.surface));
            }
            xdg_toplevel::Request::SetMinimized => {
                state.minimize_root_window(compositor_surface_id(&data.surface));
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, XdgPopupData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        _resource: &xdg_popup::XdgPopup,
        request: xdg_popup::Request,
        data: &XdgPopupData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_popup::Request::Destroy => {
                state.unregister_popup_surface(compositor_surface_id(&data.surface));
            }
            xdg_popup::Request::Reposition { positioner, token } => {
                let positioner_state = positioner
                    .data::<Arc<Mutex<XdgPositionerState>>>()
                    .and_then(|state| state.lock().ok().map(|state| *state))
                    .unwrap_or_default();
                state.configure_popup_surface(
                    compositor_surface_id(&data.surface),
                    positioner_state,
                    Some(token),
                );
            }
            xdg_popup::Request::Grab { seat, serial } => {
                state.grab_popup_surface(&data.surface, &seat, serial);
            }
            _ => {}
        }
    }
}
