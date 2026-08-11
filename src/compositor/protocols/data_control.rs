use super::super::*;

#[derive(Debug, Clone)]
struct DataControlSourceData {
    client_id: ClientId,
    selection_key: SelectionSourceKey,
}

#[derive(Debug, Clone)]
struct DataControlDeviceData {
    client_id: ClientId,
    seat_id: ObjectId,
}

#[derive(Debug, Clone)]
struct DataControlOfferData {
    target_client_id: ClientId,
    target_id: u32,
    broker_offer_id: u64,
    kind: SelectionKind,
    source_generation: u64,
    source_key: SelectionSourceKey,
}

impl GlobalDispatch<ext_data_control_manager_v1::ExtDataControlManagerV1, ()> for CompositorState {
    fn bind(
        _state: &mut Self,
        _handle: &DisplayHandle,
        _client: &Client,
        resource: New<ext_data_control_manager_v1::ExtDataControlManagerV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<ext_data_control_manager_v1::ExtDataControlManagerV1, ()> for CompositorState {
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ext_data_control_manager_v1::ExtDataControlManagerV1,
        request: ext_data_control_manager_v1::Request,
        _data: &(),
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_data_control_manager_v1::Request::CreateDataSource { id } => {
                let selection_key = state.allocate_selection_source_key();
                let source = data_init.init(
                    id,
                    DataControlSourceData {
                        client_id: client.id(),
                        selection_key,
                    },
                );
                state.selection_state.register_source(
                    selection_key,
                    SelectionSourceKind::DataControl,
                    None,
                );
                state.selection_state.set_source_backend(
                    selection_key,
                    SelectionSourceBackend::DataControl {
                        source: source.clone(),
                        client_id: client.id(),
                    },
                );
                state.data_control_sources.insert(
                    source.id(),
                    DataControlSourceBinding {
                        source,
                        selection_key,
                        client_id: client.id(),
                        mime_types: Vec::new(),
                        used: false,
                    },
                );
            }
            ext_data_control_manager_v1::Request::GetDataDevice { id, seat } => {
                if !seat.id().same_client_as(&resource.id()) {
                    return;
                }
                let device = data_init.init(
                    id,
                    DataControlDeviceData {
                        client_id: client.id(),
                        seat_id: seat.id(),
                    },
                );
                state.data_control_devices.push(DataControlDeviceBinding {
                    device: device.clone(),
                    client_id: client.id(),
                    seat_id: seat.id(),
                });
                state.publish_data_control_to_device(&device);
            }
            ext_data_control_manager_v1::Request::Destroy => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "ext_data_control_manager_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }
}

impl Dispatch<ext_data_control_device_v1::ExtDataControlDeviceV1, DataControlDeviceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ext_data_control_device_v1::ExtDataControlDeviceV1,
        request: ext_data_control_device_v1::Request,
        data: &DataControlDeviceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_data_control_device_v1::Request::SetSelection { source } => {
                state.set_data_control_selection(
                    resource,
                    client,
                    data,
                    source,
                    SelectionKind::Clipboard,
                );
            }
            ext_data_control_device_v1::Request::SetPrimarySelection { source } => {
                state.set_data_control_selection(
                    resource,
                    client,
                    data,
                    source,
                    SelectionKind::Primary,
                );
            }
            ext_data_control_device_v1::Request::Destroy => {
                state.remove_data_control_device(resource);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "ext_data_control_device_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ext_data_control_device_v1::ExtDataControlDeviceV1,
        _data: &DataControlDeviceData,
    ) {
        state.remove_data_control_device(resource);
    }
}

impl Dispatch<ext_data_control_source_v1::ExtDataControlSourceV1, DataControlSourceData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        client: &Client,
        resource: &ext_data_control_source_v1::ExtDataControlSourceV1,
        request: ext_data_control_source_v1::Request,
        data: &DataControlSourceData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_data_control_source_v1::Request::Offer { mime_type } => {
                let Some(binding) = state.data_control_sources.get(&resource.id()) else {
                    return;
                };
                if binding.client_id != data.client_id || binding.used {
                    state.post_protocol_error(
                        client,
                        resource,
                        ext_data_control_source_v1::Error::InvalidOffer,
                        "data-control source is no longer accepting MIME offers".to_string(),
                    );
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
                if let Some(binding) = state.data_control_sources.get_mut(&resource.id()) {
                    binding.mime_types.push(mime_type);
                }
            }
            ext_data_control_source_v1::Request::Destroy => {
                state.remove_data_control_source(resource);
            }
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "ext_data_control_source_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ext_data_control_source_v1::ExtDataControlSourceV1,
        _data: &DataControlSourceData,
    ) {
        state.remove_data_control_source(resource);
    }
}

impl Dispatch<ext_data_control_offer_v1::ExtDataControlOfferV1, DataControlOfferData>
    for CompositorState
{
    fn request(
        state: &mut Self,
        _client: &Client,
        resource: &ext_data_control_offer_v1::ExtDataControlOfferV1,
        request: ext_data_control_offer_v1::Request,
        data: &DataControlOfferData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            ext_data_control_offer_v1::Request::Receive { mime_type, fd } => {
                state.receive_data_control_offer(resource, data, mime_type, fd);
            }
            ext_data_control_offer_v1::Request::Destroy => {}
            other => {
                let _ = other;
                state.compliance_metrics.note_unhandled_request(
                    "ext_data_control_offer_v1",
                    resource.version(),
                    UnhandledRequestClass::FutureVersionOrGeneratedNonExhaustive,
                );
            }
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &ext_data_control_offer_v1::ExtDataControlOfferV1,
        _data: &DataControlOfferData,
    ) {
        state.data_control_offers.remove(&resource.id());
    }
}

impl CompositorState {
    fn set_data_control_selection(
        &mut self,
        resource: &ext_data_control_device_v1::ExtDataControlDeviceV1,
        client: &Client,
        data: &DataControlDeviceData,
        source: Option<ext_data_control_source_v1::ExtDataControlSourceV1>,
        kind: SelectionKind,
    ) {
        if data.seat_id.interface().name != "wl_seat" {
            return;
        }
        let Some(source) = source else {
            let mutation_epoch = self.selection_state.allocate_mutation_epoch();
            let Some(clear) = self.selection_state.clear_selection(kind, mutation_epoch) else {
                return;
            };
            if let Some(source_key) = clear.cleared_source {
                self.cancel_selection_source(kind, source_key);
            }
            self.publish_data_control_selection(kind);
            if kind == SelectionKind::Clipboard {
                if let Some(bridge) = self.clipboard_bridge.as_mut() {
                    let _ = bridge.clear_internal_selection();
                }
                self.publish_clipboard_to_focused_client();
            } else {
                self.publish_primary_to_focused_client();
            }
            return;
        };
        let Some(binding) = self.data_control_sources.get(&source.id()).cloned() else {
            return;
        };
        if binding.client_id != data.client_id || !source.is_alive() {
            return;
        }
        if binding.used {
            self.post_protocol_error(
                client,
                resource,
                ext_data_control_device_v1::Error::UsedSource,
                "data-control source has already been used".to_string(),
            );
            return;
        }
        if binding.mime_types.is_empty() {
            return;
        }
        let mutation_epoch = self.selection_state.allocate_mutation_epoch();
        let Some(commit) =
            self.selection_state
                .commit_selection(kind, binding.selection_key, mutation_epoch)
        else {
            return;
        };
        if let Some(previous_key) = commit.replaced_source {
            self.cancel_selection_source(kind, previous_key);
        }
        self.selection_state.mark_source_used(binding.selection_key);
        if let Some(binding) = self.data_control_sources.get_mut(&source.id()) {
            binding.used = true;
        }
        self.publish_data_control_selection(kind);
        match kind {
            SelectionKind::Clipboard => {
                if let Some(bridge) = self.clipboard_bridge.as_mut() {
                    let _ = bridge
                        .publish_internal_selection(commit.generation, binding.mime_types.clone());
                }
                self.publish_clipboard_to_focused_client();
            }
            SelectionKind::Primary => self.publish_primary_to_focused_client(),
        }
    }

    pub(in crate::compositor) fn remove_data_control_source(
        &mut self,
        source: &ext_data_control_source_v1::ExtDataControlSourceV1,
    ) {
        let Some(binding) = self.data_control_sources.remove(&source.id()) else {
            return;
        };
        let mutation_epoch = self.selection_state.allocate_mutation_epoch();
        let cleared = self
            .selection_state
            .remove_source_key(binding.selection_key, mutation_epoch);
        for kind in cleared {
            if kind == SelectionKind::Clipboard
                && let Some(bridge) = self.clipboard_bridge.as_mut()
            {
                let _ = bridge.clear_internal_selection();
            }
            self.publish_data_control_selection(kind);
            match kind {
                SelectionKind::Clipboard => self.publish_clipboard_to_focused_client(),
                SelectionKind::Primary => self.publish_primary_to_focused_client(),
            }
        }
    }

    fn remove_data_control_device(
        &mut self,
        device: &ext_data_control_device_v1::ExtDataControlDeviceV1,
    ) {
        self.data_control_devices
            .retain(|binding| !same_wayland_resource(&binding.device, device));
        self.data_control_offers.retain(|_, offer| {
            offer.offer.is_alive() && !offer.offer.id().same_client_as(&device.id())
        });
    }

    fn publish_data_control_to_device(
        &mut self,
        device: &ext_data_control_device_v1::ExtDataControlDeviceV1,
    ) {
        self.publish_data_control_selection_to_device(device, SelectionKind::Clipboard);
        self.publish_data_control_selection_to_device(device, SelectionKind::Primary);
    }

    pub(in crate::compositor) fn publish_data_control_selection(&mut self, kind: SelectionKind) {
        let devices = self
            .data_control_devices
            .iter()
            .filter(|binding| {
                binding.device.is_alive() && binding.seat_id.interface().name == "wl_seat"
            })
            .map(|binding| binding.device.clone())
            .collect::<Vec<_>>();
        for device in devices {
            self.publish_data_control_selection_to_device(&device, kind);
        }
    }

    fn publish_data_control_selection_to_device(
        &mut self,
        device: &ext_data_control_device_v1::ExtDataControlDeviceV1,
        kind: SelectionKind,
    ) {
        let Some(selection) = self.selection_state.active_selection(kind).cloned() else {
            self.send_data_control_selection_event(device, kind, None);
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
        let Some(broker_offer_id) =
            self.selection_state
                .register_offer(kind, target_id, selection.generation)
        else {
            return;
        };
        let Ok(offer) = client.create_resource::<
            ext_data_control_offer_v1::ExtDataControlOfferV1,
            DataControlOfferData,
            CompositorState,
        >(
            &display,
            device.version().min(1),
            DataControlOfferData {
                target_client_id: client.id(),
                target_id,
                broker_offer_id,
                kind,
                source_generation: selection.generation,
                source_key: selection.source_key,
            },
        ) else {
            return;
        };
        self.data_control_offers.insert(
            offer.id(),
            DataControlOfferBinding {
                offer: offer.clone(),
                target_client_id: client.id(),
                target_id,
                broker_offer_id,
                kind,
                source_generation: selection.generation,
                source_key: selection.source_key,
                mime_types: selection.mime_types.clone(),
            },
        );
        let _ =
            device.send_event(ext_data_control_device_v1::Event::DataOffer { id: offer.clone() });
        for mime_type in selection.mime_types {
            let _ = offer.send_event(ext_data_control_offer_v1::Event::Offer { mime_type });
        }
        self.send_data_control_selection_event(device, kind, Some(offer));
    }

    fn send_data_control_selection_event(
        &self,
        device: &ext_data_control_device_v1::ExtDataControlDeviceV1,
        kind: SelectionKind,
        offer: Option<ext_data_control_offer_v1::ExtDataControlOfferV1>,
    ) {
        let event = match kind {
            SelectionKind::Clipboard => ext_data_control_device_v1::Event::Selection { id: offer },
            SelectionKind::Primary => {
                ext_data_control_device_v1::Event::PrimarySelection { id: offer }
            }
        };
        let _ = device.send_event(event);
    }

    fn receive_data_control_offer(
        &mut self,
        offer: &ext_data_control_offer_v1::ExtDataControlOfferV1,
        data: &DataControlOfferData,
        mime_type: String,
        fd: OwnedFd,
    ) {
        let Some(binding) = self.data_control_offers.get(&offer.id()) else {
            return;
        };
        if binding.target_client_id != data.target_client_id
            || binding.target_id != data.target_id
            || binding.broker_offer_id != data.broker_offer_id
            || binding.kind != data.kind
            || binding.source_generation != data.source_generation
            || binding.source_key != data.source_key
            || !binding.mime_types.iter().any(|mime| mime == &mime_type)
            || !self.selection_state.offer_is_current(
                data.broker_offer_id,
                data.kind,
                data.source_generation,
                data.target_id,
                data.source_key,
                &mime_type,
            )
        {
            return;
        }
        self.request_selection_data(data.kind, data.source_key, mime_type, fd);
    }
}
