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
            _ => {}
        }
    }
}

impl Dispatch<wl_data_device::WlDataDevice, DataDeviceData> for CompositorState {
    fn request(
        state: &mut Self,
        _client: &Client,
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
                state.set_clipboard_selection(&data.client_id, source, serial);
            }
            wl_data_device::Request::StartDrag { source, serial, .. } => {
                if !state.client_has_recent_input_serial(&data.client_id, serial) {
                    return;
                }
                if let Some(source) = source
                    && state
                        .data_sources
                        .get(&source.id())
                        .is_some_and(|binding| binding.client_id == data.client_id)
                    && source.is_alive()
                {
                    source.cancelled();
                }
            }
            wl_data_device::Request::Release => {
                state.remove_data_device(resource);
            }
            _ => {}
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
        _client: &Client,
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
                    .is_some_and(|source| source.client_id != data.client_id)
                {
                    return;
                }
                state.offer_data_source_mime_type(resource, mime_type);
            }
            wl_data_source::Request::Destroy => {
                state.remove_data_source(resource);
            }
            wl_data_source::Request::SetActions { .. } => {}
            _ => {}
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
        _client: &Client,
        resource: &wl_data_offer::WlDataOffer,
        request: wl_data_offer::Request,
        data: &DataOfferData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            wl_data_offer::Request::Receive { mime_type, fd } => {
                state.receive_clipboard_offer(
                    resource,
                    &data.target_client_id,
                    data.source_generation,
                    mime_type,
                    fd,
                );
            }
            wl_data_offer::Request::Destroy => {
                state.data_offers.remove(&resource.id());
            }
            wl_data_offer::Request::Accept { .. }
            | wl_data_offer::Request::Finish
            | wl_data_offer::Request::SetActions { .. } => {}
            _ => {}
        }
    }

    fn destroyed(
        state: &mut Self,
        _client: ClientId,
        resource: &wl_data_offer::WlDataOffer,
        _data: &DataOfferData,
    ) {
        state.data_offers.remove(&resource.id());
    }
}
