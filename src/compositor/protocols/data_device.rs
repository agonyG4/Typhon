use super::super::*;

impl Dispatch<wl_data_device_manager::WlDataDeviceManager, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &wl_data_device_manager::WlDataDeviceManager,
        request: wl_data_device_manager::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_device_manager::Request::CreateDataSource { id } => {
                let source = data_init.init(
                    id,
                    DataSourceData {
                        client_id: client.id(),
                    },
                );
                state.register_data_source(source, client.id());
            }
            wl_data_device_manager::Request::GetDataDevice { id, seat } => {
                if !seat.id().same_client_as(&resource.id()) {
                    return;
                }
                let device = data_init.init(
                    id,
                    DataDeviceData {
                        client_id: client.id(),
                        seat_id: seat.id(),
                    },
                );
                state.register_data_device(device, client.id(), seat.id());
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wl_data_device_manager",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, DataDeviceData> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &wl_data_device::WlDataDevice,
        request: wl_data_device::Request,
        data: &DataDeviceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_device::Request::SetSelection { source, serial } => {
                if data.seat_id.interface().name != "wl_seat" {
                    return;
                }
                if let Some(source) = source.as_ref()
                    && state.data_sources.get(&source.id()).is_some_and(|source| {
                        source.use_state != DataSourceUse::Unused || source.actions_set
                    })
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_device::Error::UsedSource,
                        "data source has already been used".to_string(),
                    );
                    return;
                }
                state.set_clipboard_selection(&data.client_id, source, serial);
            }
            wl_data_device::Request::StartDrag {
                source,
                origin,
                icon,
                serial,
            } => {
                if origin
                    .client()
                    .is_none_or(|client| client.id() != data.client_id)
                    || !state.validate_start_drag_serial(serial, &origin)
                {
                    return;
                }
                if let Some(source) = source.as_ref() {
                    let Some(binding) = state.data_sources.get_mut(&source.id()) else {
                        return;
                    };
                    if binding.client_id != data.client_id || !source.is_alive() {
                        return;
                    }
                    if source.version() >= 3 && !binding.actions_set {
                        state.post_protocol_error(
                            client,
                            source,
                            wl_data_source::Error::InvalidSource,
                            "version 3 drag source must set actions before start_drag".to_string(),
                        );
                        return;
                    }
                    if binding.use_state != DataSourceUse::Unused {
                        state.post_protocol_error(
                            client,
                            resource,
                            wl_data_device::Error::UsedSource,
                            "data source has already been used".to_string(),
                        );
                        return;
                    }
                }
                if let Some(icon) = icon.as_ref() {
                    if icon
                        .client()
                        .is_none_or(|icon_client| icon_client.id() != data.client_id)
                    {
                        state.post_protocol_error(
                            client,
                            resource,
                            wl_data_device::Error::Role,
                            "drag icon surface belongs to another client".to_string(),
                        );
                        return;
                    }
                    let icon_id = compositor_surface_id(icon);
                    if state
                        .assign_surface_role(icon_id, SurfaceRole::DragIcon)
                        .is_err()
                    {
                        state.post_protocol_error(
                            client,
                            resource,
                            wl_data_device::Error::Role,
                            "drag icon surface already has a role".to_string(),
                        );
                        return;
                    }
                }
                if let Some(source) = source.as_ref()
                    && let Some(binding) = state.data_sources.get_mut(&source.id())
                {
                    binding.use_state = DataSourceUse::DragSource;
                }
                state.begin_drag_session(source, origin, icon, serial);
            }
            wl_data_device::Request::Release => {
                state.remove_data_device(resource);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wl_data_device",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &wl_data_device::WlDataDevice,
        _data: &DataDeviceData,
    ) {
        state.remove_data_device(resource);
    }
}

impl Dispatch<wl_data_source::WlDataSource, DataSourceData> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &wl_data_source::WlDataSource,
        request: wl_data_source::Request,
        data: &DataSourceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_source::Request::Offer { mime_type } => {
                if state
                    .data_sources
                    .get(&resource.id())
                    .is_some_and(|source| source.use_state != DataSourceUse::Unused)
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_source::Error::InvalidSource,
                        "data source is no longer accepting offers".to_string(),
                    );
                    return;
                }
                if state
                    .data_sources
                    .get(&resource.id())
                    .is_some_and(|source| source.client_id != data.client_id)
                {
                    return;
                }
                state.offer_data_source_mime_type(resource, mime_type);
            }
            wl_data_source::Request::Destroy => {
                state.remove_data_source(resource);
            }
            wl_data_source::Request::SetActions { dnd_actions } => {
                let actions = match dnd_actions {
                    WEnum::Value(actions) => actions.bits(),
                    WEnum::Unknown(actions) => actions,
                };
                const VALID_ACTIONS: u32 = 1 | 2 | 4;
                if actions & !VALID_ACTIONS != 0 {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_source::Error::InvalidActionMask,
                        "data source action mask contains invalid values".to_string(),
                    );
                    return;
                }
                let Some(source) = state.data_sources.get_mut(&resource.id()) else {
                    return;
                };
                if source.actions_set || source.use_state != DataSourceUse::Unused {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_source::Error::InvalidSource,
                        "data source actions were already set or source was used".to_string(),
                    );
                    return;
                }
                source.actions = actions;
                source.actions_set = true;
                state.source_drag_actions_changed(resource, actions);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wl_data_source",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &wl_data_source::WlDataSource,
        _data: &DataSourceData,
    ) {
        state.remove_data_source(resource);
    }
}

impl Dispatch<wl_data_offer::WlDataOffer, DataOfferData> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &wl_data_offer::WlDataOffer,
        request: wl_data_offer::Request,
        data: &DataOfferData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        debug_assert_eq!(
            data.kind,
            state
                .data_offers
                .get(&resource.id())
                .map(|offer| offer.kind)
                .unwrap_or(data.kind)
        );
        match request {
            wl_data_offer::Request::Receive { mime_type, fd } => {
                if state.data_offers.get(&resource.id()).is_some_and(|offer| {
                    matches!(
                        offer.drag_phase,
                        Some(DragOfferPhase::Finished | DragOfferPhase::Destroyed)
                    )
                }) {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_offer::Error::InvalidOffer,
                        "finished data offer cannot receive more data".to_string(),
                    );
                    return;
                }
                state.receive_clipboard_offer(
                    resource,
                    &data.target_client_id,
                    data.source_generation,
                    mime_type,
                    fd,
                );
            }
            wl_data_offer::Request::Destroy => {
                // The generated destructor invokes `destroyed`; keeping the
                // cleanup there preserves the active offer long enough to
                // terminate a post-drop session correctly.
            }
            wl_data_offer::Request::Accept {
                serial: _,
                mime_type,
            } => {
                let Some(offer) = state.data_offers.get_mut(&resource.id()) else {
                    return;
                };
                if offer.kind != DataOfferKind::DragAndDrop
                    || matches!(
                        offer.drag_phase,
                        Some(DragOfferPhase::Finished | DragOfferPhase::Destroyed)
                    )
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_offer::Error::InvalidOffer,
                        "selection offer does not accept drag-and-drop requests".to_string(),
                    );
                    return;
                }
                if let Some(mime_type) = mime_type.as_ref()
                    && !offer.mime_types.iter().any(|mime| mime == mime_type)
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_offer::Error::InvalidOffer,
                        "data offer MIME type was not offered".to_string(),
                    );
                    return;
                }
                offer.accepted_mime = mime_type.to_owned();
                state.update_drag_acceptance(resource, mime_type.to_owned());
            }
            wl_data_offer::Request::Finish => {
                let Some(offer) = state.data_offers.get(&resource.id()) else {
                    return;
                };
                let duplicate_terminal = matches!(
                    offer.drag_phase,
                    Some(DragOfferPhase::Finished | DragOfferPhase::Destroyed)
                ) || state.active_drag.as_ref().is_some_and(|drag| {
                    drag.offer
                        .as_ref()
                        .is_some_and(|current| same_wayland_resource(current, resource))
                        && matches!(
                            drag.phase,
                            DragSessionPhase::Finished | DragSessionPhase::Cancelled
                        )
                });
                if offer.kind != DataOfferKind::DragAndDrop
                    || offer.drag_phase != Some(DragOfferPhase::Dropped)
                    || offer.accepted_mime.is_none()
                    || offer.selected_action.is_none()
                {
                    if duplicate_terminal {
                        state.note_dnd_duplicate_terminal_attempt();
                    }
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_offer::Error::InvalidFinish,
                        "data offer finish was not preceded by a valid drop".to_string(),
                    );
                    return;
                }
                if !state.finish_drag_offer(resource) {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_offer::Error::InvalidFinish,
                        "data offer finish was received before the drop completed".to_string(),
                    );
                }
            }
            wl_data_offer::Request::SetActions {
                dnd_actions,
                preferred_action,
            } => {
                let actions = match dnd_actions {
                    WEnum::Value(actions) => actions.bits(),
                    WEnum::Unknown(actions) => actions,
                };
                let preferred = match preferred_action {
                    WEnum::Value(action) => action.bits(),
                    WEnum::Unknown(action) => action,
                };
                const VALID_ACTIONS: u32 = 1 | 2 | 4;
                let Some(existing_offer) = state.data_offers.get(&resource.id()) else {
                    return;
                };
                if existing_offer.kind != DataOfferKind::DragAndDrop
                    || matches!(
                        existing_offer.drag_phase,
                        Some(DragOfferPhase::Finished | DragOfferPhase::Destroyed)
                    )
                    || (existing_offer.drag_phase == Some(DragOfferPhase::Dropped)
                        && state.active_drag.as_ref().is_none_or(|drag| {
                            drag.phase != DragSessionPhase::DroppedAwaitingAskResolution
                        }))
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_offer::Error::InvalidOffer,
                        "selection offer cannot negotiate drag-and-drop actions".to_string(),
                    );
                    return;
                }
                if actions & !VALID_ACTIONS != 0 {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_offer::Error::InvalidActionMask,
                        "data offer action mask contains invalid values".to_string(),
                    );
                    return;
                }
                if preferred != 0
                    && (preferred & !actions != 0
                        || preferred & (preferred - 1) != 0
                        || preferred & !existing_offer.source_actions != 0)
                {
                    state.post_protocol_error(
                        client,
                        resource,
                        wl_data_offer::Error::InvalidAction,
                        "data offer preferred action is invalid".to_string(),
                    );
                    return;
                }
                let offer_id = resource.clone();
                if let Some(offer) = state.data_offers.get_mut(&resource.id()) {
                    offer.selected_action = (preferred != 0).then_some(preferred);
                }
                state.update_drag_actions(&offer_id, actions, preferred);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "wl_data_offer",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &wl_data_offer::WlDataOffer,
        _data: &DataOfferData,
    ) {
        state.destroy_data_offer(resource);
    }
}
