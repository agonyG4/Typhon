use super::super::*;

#[derive(Debug, Clone)]
struct PrimarySourceData {
    client_id: ClientId,
    selection_key: SelectionSourceKey,
}

#[derive(Debug, Clone)]
struct PrimaryDeviceData {
    client_id: ClientId,
    seat_id: ObjectId,
}

#[derive(Debug, Clone)]
struct PrimaryOfferData {
    target_client_id: ClientId,
    target_id: u32,
    broker_offer_id: u64,
    source_generation: u64,
    source_key: SelectionSourceKey,
}

impl GlobalDispatch<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1, ()>
    for CompositorState
{
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1, ()>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1,
        request: zwp_primary_selection_device_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_primary_selection_device_manager_v1::Request::CreateSource { id } => {
                let selection_key = state.allocate_selection_source_key();
                let source = data_init.init(
                    id,
                    PrimarySourceData {
                        client_id: client.id(),
                        selection_key,
                    },
                );
                state.selection_state.register_source(
                    selection_key,
                    SelectionSourceKind::WaylandPrimary,
                    None,
                );
                state.selection_state.set_source_backend(
                    selection_key,
                    SelectionSourceBackend::WaylandPrimary {
                        source: source.clone(),
                        client_id: client.id(),
                    },
                );
                state.primary_sources.insert(
                    source.id(),
                    PrimarySourceBinding {
                        source,
                        selection_key,
                        client_id: client.id(),
                        mime_types: Vec::new(),
                    },
                );
            }
            zwp_primary_selection_device_manager_v1::Request::GetDevice { id, seat } => {
                if !seat.id().same_client_as(&resource.id()) {
                    return;
                }
                let device = data_init.init(
                    id,
                    PrimaryDeviceData {
                        client_id: client.id(),
                        seat_id: seat.id(),
                    },
                );
                state.primary_devices.push(PrimaryDeviceBinding {
                    device: device.clone(),
                    client_id: client.id(),
                    seat_id: seat.id(),
                });
                state.publish_primary_to_device(&device);
            }
            zwp_primary_selection_device_manager_v1::Request::Destroy => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zwp_primary_selection_device_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1, PrimaryDeviceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        request: zwp_primary_selection_device_v1::Request,
        data: &PrimaryDeviceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_primary_selection_device_v1::Request::SetSelection { source, serial } => {
                if data.seat_id.interface().name != "wl_seat"
                    || !state.client_has_focus(&data.client_id)
                {
                    return;
                }
                let Some(selection_epoch) = state.selection_input_epoch(&data.client_id, serial)
                else {
                    return;
                };
                if let Some(source) = source {
                    let Some(binding) = state.primary_sources.get(&source.id()).cloned() else {
                        return;
                    };
                    if binding.client_id != data.client_id
                        || !source.is_alive()
                        || binding.mime_types.is_empty()
                    {
                        return;
                    }
                    state.set_primary_selection_from_source(binding.selection_key, selection_epoch);
                } else {
                    state.clear_primary_selection(selection_epoch);
                }
            }
            zwp_primary_selection_device_v1::Request::Destroy => {
                state.remove_primary_device(resource);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zwp_primary_selection_device_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
        _data: &PrimaryDeviceData,
    ) {
        state.remove_primary_device(resource);
    }
}

impl Dispatch<zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1, PrimarySourceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        request: zwp_primary_selection_source_v1::Request,
        data: &PrimarySourceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_primary_selection_source_v1::Request::Offer { mime_type } => {
                let Some(binding) = state.primary_sources.get(&resource.id()) else {
                    return;
                };
                if binding.client_id != data.client_id {
                    return;
                }
                if mime_type.is_empty() || mime_type.len() > 4096 {
                    return;
                }
                if binding.mime_types.len() >= 128
                    || binding.mime_types.iter().any(|mime| mime == &mime_type)
                {
                    return;
                }
                state
                    .selection_state
                    .offer_source_mime_type_for_key(data.selection_key, mime_type.clone());
                if let Some(binding) = state.primary_sources.get_mut(&resource.id()) {
                    binding.mime_types.push(mime_type);
                }
            }
            zwp_primary_selection_source_v1::Request::Destroy => {
                state.remove_primary_source(resource);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zwp_primary_selection_source_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
        let _ = client;
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
        _data: &PrimarySourceData,
    ) {
        state.remove_primary_source(resource);
    }
}

impl Dispatch<zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1, PrimaryOfferData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
        request: zwp_primary_selection_offer_v1::Request,
        data: &PrimaryOfferData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            zwp_primary_selection_offer_v1::Request::Receive { mime_type, fd } => {
                state.receive_primary_offer(resource, data, mime_type, fd);
            }
            zwp_primary_selection_offer_v1::Request::Destroy => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "zwp_primary_selection_offer_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
        _data: &PrimaryOfferData,
    ) {
        state.primary_offers.remove(&resource.id());
    }
}

impl CompositorState {
    pub(in crate::compositor) fn set_primary_selection_from_source(
        &mut self,
        source_key: SelectionSourceKey,
        selection_epoch: SelectionMutationEpoch,
    ) {
        let Some(commit) = self.selection_state.commit_selection(
            SelectionKind::Primary,
            source_key,
            selection_epoch,
        ) else {
            return;
        };
        if let Some(replaced_source) = commit.replaced_source {
            self.cancel_selection_source(SelectionKind::Primary, replaced_source);
        }
        self.selection_state.mark_source_used(source_key);
        self.publish_primary_to_focused_client();
        self.publish_data_control_selection(SelectionKind::Primary);
    }

    pub(in crate::compositor) fn clear_primary_selection(
        &mut self,
        mutation_epoch: SelectionMutationEpoch,
    ) {
        let Some(clear) = self
            .selection_state
            .clear_selection(SelectionKind::Primary, mutation_epoch)
        else {
            return;
        };
        if let Some(source_key) = clear.cleared_source {
            self.cancel_selection_source(SelectionKind::Primary, source_key);
        }
        self.publish_primary_to_focused_client();
        self.publish_data_control_selection(SelectionKind::Primary);
    }

    pub(in crate::compositor) fn remove_primary_source(
        &mut self,
        source: &zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1,
    ) {
        let Some(binding) = self.primary_sources.remove(&source.id()) else {
            return;
        };
        let mutation_epoch = self.selection_state.allocate_mutation_epoch();
        let cleared = self
            .selection_state
            .remove_source_key(binding.selection_key, mutation_epoch);
        if cleared.contains(&SelectionKind::Primary) {
            self.publish_primary_to_focused_client();
            self.publish_data_control_selection(SelectionKind::Primary);
        }
    }

    pub(in crate::compositor) fn remove_primary_device(
        &mut self,
        device: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
    ) {
        self.primary_devices
            .retain(|binding| !same_wayland_resource(&binding.device, device));
        self.primary_offers.retain(|_, offer| {
            offer.offer.is_alive() && !offer.offer.id().same_client_as(&device.id())
        });
    }

    pub(in crate::compositor) fn publish_primary_to_focused_client(&mut self) {
        let Some(client_id) = self.focused_client_id() else {
            return;
        };
        let devices = self
            .primary_devices
            .iter()
            .filter(|binding| {
                binding.client_id == client_id
                    && binding.device.is_alive()
                    && binding.seat_id.interface().name == "wl_seat"
            })
            .map(|binding| binding.device.clone())
            .collect::<Vec<_>>();
        for device in devices {
            self.publish_primary_to_device(&device);
        }
    }

    pub(in crate::compositor) fn publish_primary_clear_to_client(&mut self, client_id: &ClientId) {
        let devices = self
            .primary_devices
            .iter()
            .filter(|binding| binding.client_id == *client_id && binding.device.is_alive())
            .map(|binding| binding.device.clone())
            .collect::<Vec<_>>();
        for device in devices {
            let _ =
                device.send_event(zwp_primary_selection_device_v1::Event::Selection { id: None });
        }
    }

    pub(in crate::compositor) fn publish_primary_to_device(
        &mut self,
        device: &zwp_primary_selection_device_v1::ZwpPrimarySelectionDeviceV1,
    ) {
        if !device.is_alive() {
            return;
        }
        let Some(selection) = self
            .selection_state
            .active_selection(SelectionKind::Primary)
            .cloned()
        else {
            let _ =
                device.send_event(zwp_primary_selection_device_v1::Event::Selection { id: None });
            return;
        };
        let Some(client) = device.client() else {
            return;
        };
        let Some(handle) = device.handle().upgrade() else {
            return;
        };
        let display = DisplayHandle::from(handle);
        let target_id = device.id().protocol_id();
        let Some(broker_offer_id) = self.selection_state.register_offer(
            SelectionKind::Primary,
            target_id,
            selection.generation,
        ) else {
            return;
        };
        let Ok(offer) = client.create_resource::<
            zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
            PrimaryOfferData,
            CompositorState,
        >(
            &display,
            device.version().min(1),
            PrimaryOfferData {
                target_client_id: client.id(),
                target_id,
                broker_offer_id,
                source_generation: selection.generation,
                source_key: selection.source_key,
            },
        ) else {
            return;
        };
        self.primary_offers.insert(
            offer.id(),
            PrimaryOfferBinding {
                offer: offer.clone(),
                target_client_id: client.id(),
                target_id,
                broker_offer_id,
                source_generation: selection.generation,
                source_key: selection.source_key,
                mime_types: selection.mime_types.clone(),
            },
        );
        let _ = device.send_event(zwp_primary_selection_device_v1::Event::DataOffer {
            offer: offer.clone(),
        });
        for mime_type in selection.mime_types {
            let _ = offer.send_event(zwp_primary_selection_offer_v1::Event::Offer { mime_type });
        }
        let _ = device
            .send_event(zwp_primary_selection_device_v1::Event::Selection { id: Some(offer) });
    }

    fn receive_primary_offer(
        &mut self,
        offer: &zwp_primary_selection_offer_v1::ZwpPrimarySelectionOfferV1,
        data: &PrimaryOfferData,
        mime_type: String,
        fd: OwnedFd,
    ) {
        let Some(binding) = self.primary_offers.get(&offer.id()) else {
            return;
        };
        if binding.target_client_id != data.target_client_id
            || binding.target_id != data.target_id
            || binding.broker_offer_id != data.broker_offer_id
            || binding.source_generation != data.source_generation
            || binding.source_key != data.source_key
            || !binding.mime_types.iter().any(|mime| mime == &mime_type)
            || !self.selection_state.offer_is_current(
                data.broker_offer_id,
                SelectionKind::Primary,
                data.source_generation,
                data.target_id,
                data.source_key,
                &mime_type,
            )
        {
            return;
        }
        self.request_selection_data(SelectionKind::Primary, data.source_key, mime_type, fd);
    }
}
