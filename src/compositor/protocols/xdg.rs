use super::super::*;

fn xdg_surface_role_error(error: SurfaceRoleError) -> xdg_surface::Error {
    match error {
        SurfaceRoleError::AlreadyAssigned { .. } | SurfaceRoleError::XdgAssociationExists => {
            xdg_surface::Error::AlreadyConstructed
        }
        SurfaceRoleError::MissingXdgAssociation | SurfaceRoleError::MissingSurface => {
            xdg_surface::Error::NotConstructed
        }
        SurfaceRoleError::MissingParent | SurfaceRoleError::Cycle => {
            xdg_surface::Error::NotConstructed
        }
    }
}

fn log_xdg_reassociation(
    surface_id: u32,
    permanent: PermanentSurfaceRole,
    requested: SurfaceRole,
    result: &'static str,
) {
    if surface_tree_debug_enabled() {
        eprintln!(
            "oblivion-one compositor: xdg_reassociate surface={surface_id} permanent={} requested={} result={result}",
            permanent.label(),
            requested.label(),
        );
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &xdg_wm_base::XdgWmBase,
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
                let surface_id = compositor_surface_id(&surface);
                if state
                    .surface_client_ids
                    .get(&surface_id)
                    .is_none_or(|owner| *owner != client.id())
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_wm_base::Error::Role,
                        "wl_surface is not owned by the requesting client".to_string(),
                    );
                    return;
                }
                if surface
                    .data::<SurfaceData>()
                    .is_none_or(|data| data.has_pending_buffer())
                    || state.current_surface_buffers.contains_key(&surface_id)
                    || state
                        .renderable_surfaces
                        .iter()
                        .any(|renderable| renderable.surface_id == surface_id)
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_wm_base::Error::InvalidSurfaceState,
                        "wl_surface already has pending or committed content".to_string(),
                    );
                    return;
                }
                if state.has_unpublished_surface_work(surface_id) {
                    state
                        .compliance_metrics
                        .note_xdg_reassociation_blocked_stale_unpublished_work();
                    if surface_tree_debug_enabled() {
                        eprintln!(
                            "oblivion-one compositor: xdg_reassociation_blocked_stale_unpublished_work surface={surface_id}"
                        );
                    }
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_wm_base::Error::InvalidSurfaceState,
                        "wl_surface has unpublished work from a destroyed XDG role".to_string(),
                    );
                    return;
                }
                let reservation = match state.reserve_xdg_association(surface_id) {
                    Ok(reservation) => reservation,
                    Err(error) => {
                        state.post_protocol_error(
                            client,
                            resource,
                            xdg_wm_base::Error::Role,
                            error.message(),
                        );
                        return;
                    }
                };
                let xdg_surface = data_init.init(
                    id,
                    XdgSurfaceData {
                        surface,
                        reservation,
                    },
                );
                let previous_resource = state.xdg_surface_resources.insert(surface_id, xdg_surface);
                debug_assert!(
                    previous_resource.is_none(),
                    "xdg surface resource map contained a stale association"
                );
                if previous_resource.is_some() {
                    eprintln!(
                        "oblivion-one compositor: invariant violation: stale xdg surface resource surface={surface_id}"
                    );
                }
                let previous_wm_base = state
                    .xdg_surface_wm_bases
                    .insert(surface_id, resource.clone());
                debug_assert!(
                    previous_wm_base.is_none(),
                    "xdg surface wm-base map contained a stale association"
                );
                if previous_wm_base.is_some() {
                    eprintln!(
                        "oblivion-one compositor: invariant violation: stale xdg wm-base ownership surface={surface_id}"
                    );
                }
                let previous_lifecycle = state
                    .xdg_surface_lifecycles
                    .insert(surface_id, XdgSurfaceLifecycle::default());
                debug_assert!(
                    previous_lifecycle.is_none(),
                    "xdg lifecycle map contained a stale association"
                );
                if previous_lifecycle.is_some() {
                    eprintln!(
                        "oblivion-one compositor: invariant violation: stale xdg lifecycle surface={surface_id}"
                    );
                }
            }
            xdg_wm_base::Request::Destroy => {
                if state
                    .xdg_surface_wm_bases
                    .values()
                    .any(|wm_base| same_wayland_resource(wm_base, resource))
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_wm_base::Error::DefunctSurfaces,
                        "xdg_wm_base destroyed while xdg_surfaces remain".to_string(),
                    );
                }
            }
            xdg_wm_base::Request::Pong { .. } => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "xdg_wm_base",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<zxdg_decoration_manager_v1::ZxdgDecorationManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zxdg_decoration_manager_v1::ZxdgDecorationManagerV1,
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
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zxdg_decoration_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1, XdgToplevelData>
    for CompositorState
{
    fn request(
        state: &mut Self,
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
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zxdg_toplevel_decoration_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
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
        state: &mut Self,
        client: &Client,
        resource: &xdg_positioner::XdgPositioner,
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
                if width <= 0 || height <= 0 {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner size must be positive".to_string(),
                    );
                    return;
                }
                positioner.width = width;
                positioner.height = height;
            }
            xdg_positioner::Request::SetAnchorRect {
                x,
                y,
                width,
                height,
            } => {
                if width < 0 || height < 0 {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner anchor rectangle size cannot be negative".to_string(),
                    );
                    return;
                }
                positioner.anchor_rect = PopupAnchorRect {
                    x,
                    y,
                    width,
                    height,
                };
            }
            xdg_positioner::Request::SetOffset { x, y } => {
                positioner.offset_x = x;
                positioner.offset_y = y;
            }
            xdg_positioner::Request::SetAnchor { anchor } => match anchor {
                WEnum::Value(anchor) => positioner.anchor = PopupEdges::from_anchor(anchor),
                WEnum::Unknown(_) => {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner anchor is invalid".to_string(),
                    );
                }
            },
            xdg_positioner::Request::SetGravity { gravity } => match gravity {
                WEnum::Value(gravity) => positioner.gravity = PopupEdges::from_gravity(gravity),
                WEnum::Unknown(_) => {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner gravity is invalid".to_string(),
                    );
                }
            },
            xdg_positioner::Request::SetConstraintAdjustment {
                constraint_adjustment,
            } => match constraint_adjustment {
                WEnum::Value(constraint_adjustment) => {
                    positioner.constraint_adjustment =
                        PopupConstraintAdjustment::from_xdg(constraint_adjustment);
                }
                WEnum::Unknown(_) => {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner constraint adjustment is invalid".to_string(),
                    );
                }
            },
            xdg_positioner::Request::SetParentSize {
                parent_width,
                parent_height,
            } => {
                if parent_width < 0 || parent_height < 0 {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_positioner::Error::InvalidInput,
                        "xdg_positioner parent size cannot be negative".to_string(),
                    );
                    return;
                }
                positioner.parent_size = Some((parent_width, parent_height));
            }
            xdg_positioner::Request::SetReactive => {
                positioner.reactive = true;
            }
            xdg_positioner::Request::SetParentConfigure { serial } => {
                positioner.parent_configure = Some(serial);
            }
            xdg_positioner::Request::Destroy => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "xdg_positioner",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, XdgSurfaceData> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &xdg_surface::XdgSurface,
        request: xdg_surface::Request,
        data: &XdgSurfaceData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        let surface_id = compositor_surface_id(&data.surface);
        if !matches!(
            &request,
            xdg_surface::Request::GetToplevel { .. }
                | xdg_surface::Request::GetPopup { .. }
                | xdg_surface::Request::Destroy
        ) && !state.xdg_surface_is_constructed(surface_id)
        {
            state.post_protocol_error(
                client,
                resource,
                xdg_surface::Error::NotConstructed,
                "xdg_surface request requires a constructed role".to_string(),
            );
            return;
        }
        match request {
            xdg_surface::Request::GetToplevel { id } => {
                if let Err(error) = state.construct_xdg_role(surface_id, SurfaceRole::XdgToplevel) {
                    if state.is_dormant_xdg_cross_role_request(surface_id, SurfaceRole::XdgToplevel)
                    {
                        state
                            .compliance_metrics
                            .note_xdg_cross_role_reassociation_rejection();
                        if let Some(permanent) = state.permanent_surface_role(surface_id) {
                            log_xdg_reassociation(
                                surface_id,
                                permanent,
                                SurfaceRole::XdgToplevel,
                                "rejected",
                            );
                        }
                    }
                    let code = xdg_surface_role_error(error);
                    state.post_protocol_error(client, resource, code, error.message());
                    return;
                }
                if let Some(lifecycle) = state.xdg_surface_lifecycle_mut(surface_id) {
                    let _ = lifecycle.construct_toplevel();
                }
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
                state.adopt_current_surface_content_for_role(surface_id);
                if let XdgAssociationReservation::Reassociation { permanent_role } =
                    data.reservation
                {
                    state.compliance_metrics.note_xdg_same_role_reassociation();
                    log_xdg_reassociation(
                        surface_id,
                        permanent_role,
                        SurfaceRole::XdgToplevel,
                        "accepted",
                    );
                }
            }
            xdg_surface::Request::GetPopup {
                id,
                parent,
                positioner,
            } => {
                let Some(positioner_data) = positioner.data::<Arc<Mutex<XdgPositionerState>>>()
                else {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_wm_base::Error::InvalidPositioner,
                        "xdg_popup positioner object is invalid".to_string(),
                    );
                    return;
                };
                let Some(positioner_state) = positioner_data.lock().ok().map(|state| *state) else {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_wm_base::Error::InvalidPositioner,
                        "xdg_popup positioner state is unavailable".to_string(),
                    );
                    return;
                };
                if positioner_state.width <= 0
                    || positioner_state.height <= 0
                    || positioner_state.anchor_rect.width <= 0
                    || positioner_state.anchor_rect.height <= 0
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_wm_base::Error::InvalidPositioner,
                        "xdg_popup positioner is incomplete".to_string(),
                    );
                    return;
                }
                let parent_surface = match parent {
                    Some(parent) => {
                        let Some(parent_data) = parent.data::<XdgSurfaceData>() else {
                            state.post_protocol_error(
                                client,
                                resource,
                                xdg_wm_base::Error::InvalidPopupParent,
                                "xdg_popup parent is not an xdg_surface".to_string(),
                            );
                            return;
                        };
                        let parent_surface_id = compositor_surface_id(&parent_data.surface);
                        let same_client = state.surface_client_ids.get(&surface_id)
                            == state.surface_client_ids.get(&parent_surface_id);
                        if !same_client
                            || parent_surface_id == surface_id
                            || (!state.toplevel_surfaces.contains_key(&parent_surface_id)
                                && !state.popup_surfaces.contains_key(&parent_surface_id))
                        {
                            state.post_protocol_error(
                                client,
                                resource,
                                xdg_wm_base::Error::InvalidPopupParent,
                                "xdg_popup parent is invalid".to_string(),
                            );
                            return;
                        }
                        Some(parent_data.surface.clone())
                    }
                    None => None,
                };
                if let Err(error) = state.construct_xdg_role(surface_id, SurfaceRole::XdgPopup) {
                    if state.is_dormant_xdg_cross_role_request(surface_id, SurfaceRole::XdgPopup) {
                        state
                            .compliance_metrics
                            .note_xdg_cross_role_reassociation_rejection();
                        if let Some(permanent) = state.permanent_surface_role(surface_id) {
                            log_xdg_reassociation(
                                surface_id,
                                permanent,
                                SurfaceRole::XdgPopup,
                                "rejected",
                            );
                        }
                    }
                    let code = xdg_surface_role_error(error);
                    state.post_protocol_error(client, resource, code, error.message());
                    return;
                }
                if let Some(lifecycle) = state.xdg_surface_lifecycle_mut(surface_id) {
                    let _ = lifecycle.construct_popup();
                }
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
                state.adopt_current_surface_content_for_role(surface_id);
                if let XdgAssociationReservation::Reassociation { permanent_role } =
                    data.reservation
                {
                    state.compliance_metrics.note_xdg_same_role_reassociation();
                    log_xdg_reassociation(
                        surface_id,
                        permanent_role,
                        SurfaceRole::XdgPopup,
                        "accepted",
                    );
                }
            }
            xdg_surface::Request::SetWindowGeometry {
                x,
                y,
                width,
                height,
            } => {
                let surface_id = compositor_surface_id(&data.surface);
                if width <= 0 || height <= 0 {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_surface::Error::InvalidSize,
                        "xdg_surface window geometry must be positive".to_string(),
                    );
                    return;
                }
                state
                    .pending_surface_window_geometries
                    .insert(surface_id, XdgWindowGeometry::new(x, y, width, height));
            }
            xdg_surface::Request::AckConfigure { serial } => {
                let surface_id = compositor_surface_id(&data.surface);
                if !state.acknowledge_xdg_configure(surface_id, serial) {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_surface::Error::InvalidSerial,
                        "xdg_surface acknowledged an unknown or already consumed configure"
                            .to_string(),
                    );
                    return;
                }
                state.ack_xdg_surface_configure(surface_id, serial);
            }
            xdg_surface::Request::Destroy => {
                let surface_id = compositor_surface_id(&data.surface);
                if !state.validate_surface_destroy(surface_id) {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_surface::Error::DefunctRoleObject,
                        "xdg_surface destroyed before its role object".to_string(),
                    );
                    return;
                }
                state.unregister_xdg_surface_role(compositor_surface_id(&data.surface));
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "xdg_surface",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, XdgToplevelData> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &xdg_toplevel::XdgToplevel,
        request: xdg_toplevel::Request,
        data: &XdgToplevelData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_toplevel::Request::SetTitle { title } => {
                let surface_id = compositor_surface_id(&data.surface);
                if let Some(toplevel) = state.toplevel_surfaces.get(&surface_id)
                    && let Some(window) = state.window_mut(toplevel.window_id)
                {
                    window.metadata.title = Some(title);
                }
            }
            xdg_toplevel::Request::SetAppId { app_id } => {
                let surface_id = compositor_surface_id(&data.surface);
                if let Some(toplevel) = state.toplevel_surfaces.get(&surface_id)
                    && let Some(window) = state.window_mut(toplevel.window_id)
                {
                    window.metadata.app_id = Some(app_id.clone());
                }
                state.last_app_id = Some(app_id);
            }
            xdg_toplevel::Request::ShowWindowMenu { seat, serial, x, y } => {
                // The XML explicitly gives no guarantee that a window menu
                // will be drawn. Typhon has no compositor-owned menu surface,
                // so this is a validated policy no-op rather than a wildcard
                // that hides a supported request.
                let _ = (seat, serial, x, y);
            }
            xdg_toplevel::Request::Destroy => {
                state.unregister_toplevel_surface(compositor_surface_id(&data.surface));
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
                let Some(edges) = resize_edges_from_xdg(edges) else {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_toplevel::Error::InvalidResizeEdge,
                        "xdg_toplevel resize edge is invalid".to_string(),
                    );
                    return;
                };
                if resource_belongs_to_surface_client(&seat, &data.surface) {
                    state.begin_client_window_resize(&data.surface, serial, edges);
                }
            }
            xdg_toplevel::Request::SetParent { parent } => {
                let surface_id = compositor_surface_id(&data.surface);
                let parent_surface_id = parent
                    .as_ref()
                    .and_then(|parent| parent.data::<XdgToplevelData>())
                    .map(|parent| compositor_surface_id(&parent.surface));
                let valid_parent_client = parent
                    .as_ref()
                    .and_then(|parent| parent.data::<XdgToplevelData>())
                    .is_none_or(|parent| {
                        state.surface_client_ids.get(&surface_id)
                            == state
                                .surface_client_ids
                                .get(&compositor_surface_id(&parent.surface))
                    });
                if !valid_parent_client
                    || (parent.is_some() && parent_surface_id.is_none())
                    || state
                        .set_toplevel_parent(surface_id, parent_surface_id)
                        .is_err()
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_toplevel::Error::InvalidParent,
                        "xdg_toplevel parent is invalid".to_string(),
                    );
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
            xdg_toplevel::Request::SetMinSize { width, height } => {
                let surface_id = compositor_surface_id(&data.surface);
                if width < 0 || height < 0 {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_toplevel::Error::InvalidSize,
                        "xdg_toplevel minimum size cannot be negative".to_string(),
                    );
                    return;
                }
                let Some(window_id) = state
                    .toplevel_surfaces
                    .get(&surface_id)
                    .map(|toplevel| toplevel.window_id)
                else {
                    return;
                };
                let pending = state
                    .toplevel_surfaces
                    .get(&surface_id)
                    .and_then(|toplevel| toplevel.pending_constraints)
                    .or_else(|| state.window(window_id).map(|window| window.constraints))
                    .unwrap_or_default();
                if let Some(toplevel) = state.toplevel_surfaces.get_mut(&surface_id) {
                    toplevel.pending_constraints = Some(ToplevelSizeConstraints {
                        min_width: (width > 0).then_some(width as u32),
                        min_height: (height > 0).then_some(height as u32),
                        ..pending
                    });
                }
            }
            xdg_toplevel::Request::SetMaxSize { width, height } => {
                let surface_id = compositor_surface_id(&data.surface);
                if width < 0 || height < 0 {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_toplevel::Error::InvalidSize,
                        "xdg_toplevel maximum size cannot be negative".to_string(),
                    );
                    return;
                }
                let Some(window_id) = state
                    .toplevel_surfaces
                    .get(&surface_id)
                    .map(|toplevel| toplevel.window_id)
                else {
                    return;
                };
                let pending = state
                    .toplevel_surfaces
                    .get(&surface_id)
                    .and_then(|toplevel| toplevel.pending_constraints)
                    .or_else(|| state.window(window_id).map(|window| window.constraints))
                    .unwrap_or_default();
                let max_width = (width > 0).then_some(width as u32);
                let max_height = (height > 0).then_some(height as u32);
                if pending
                    .min_width
                    .is_some_and(|min| max_width.is_some_and(|max| max < min))
                    || pending
                        .min_height
                        .is_some_and(|min| max_height.is_some_and(|max| max < min))
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_toplevel::Error::InvalidSize,
                        "xdg_toplevel maximum size is smaller than minimum size".to_string(),
                    );
                    return;
                }
                if let Some(toplevel) = state.toplevel_surfaces.get_mut(&surface_id) {
                    toplevel.pending_constraints = Some(ToplevelSizeConstraints {
                        max_width,
                        max_height,
                        ..pending
                    });
                }
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "xdg_toplevel",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<xdg_popup::XdgPopup, XdgPopupData> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &xdg_popup::XdgPopup,
        request: xdg_popup::Request,
        data: &XdgPopupData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            xdg_popup::Request::Destroy => {
                let surface_id = compositor_surface_id(&data.surface);
                if !state.popup_destroy_is_topmost(surface_id) {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_wm_base::Error::NotTheTopmostPopup,
                        "xdg_popup must be destroyed from the topmost popup downward".to_string(),
                    );
                    return;
                }
                state.unregister_popup_surface(surface_id);
            }
            xdg_popup::Request::Reposition { positioner, token } => {
                let Some(positioner_state) = positioner
                    .data::<Arc<Mutex<XdgPositionerState>>>()
                    .and_then(|state| state.lock().ok().map(|state| *state))
                else {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_wm_base::Error::InvalidPositioner,
                        "xdg_popup reposition referenced a destroyed positioner".to_string(),
                    );
                    return;
                };
                if !positioner_state.is_complete() {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_wm_base::Error::InvalidPositioner,
                        "xdg_popup reposition referenced an incomplete positioner".to_string(),
                    );
                    return;
                }
                state.configure_popup_surface(
                    compositor_surface_id(&data.surface),
                    positioner_state,
                    Some(token),
                );
            }
            xdg_popup::Request::Grab { seat, serial } => {
                if !state.grab_popup_surface(&data.surface, &seat, serial) {
                    state.post_protocol_error(
                        client,
                        resource,
                        xdg_popup::Error::InvalidGrab,
                        "xdg_popup grab is not valid for this parent, seat, or serial".to_string(),
                    );
                }
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "xdg_popup",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}
