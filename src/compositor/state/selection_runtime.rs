use super::*;

impl CompositorState {
    pub(in crate::compositor) fn register_data_device(
        &mut self,
        device: wl_data_device::WlDataDevice,
        client_id: ClientId,
        seat_id: ObjectId,
    ) {
        self.data_devices
            .retain(|binding| binding.device.is_alive());
        self.data_devices.push(ClipboardDataDevice {
            device: device.clone(),
            client_id: client_id.clone(),
            seat_id,
        });
        if self.client_has_focus(&client_id) {
            self.publish_clipboard_to_data_device(&device);
        }
    }

    pub(in crate::compositor) fn remove_data_device(
        &mut self,
        device: &wl_data_device::WlDataDevice,
    ) {
        if let Some(client_id) = device.client().map(|client| client.id())
            && self
                .active_drag
                .as_ref()
                .is_some_and(|drag| drag.target_client.as_ref() == Some(&client_id))
        {
            self.cancel_drag_session("data_device_destroyed");
        }
        self.data_devices
            .retain(|binding| !same_wayland_resource(&binding.device, device));
        self.data_offers.retain(|_, offer| {
            offer.offer.is_alive() && !offer.offer.id().same_client_as(&device.id())
        });
    }

    pub(in crate::compositor) fn remove_data_source(
        &mut self,
        source: &wl_data_source::WlDataSource,
    ) {
        self.cancel_drag_for_source(source);
        let selection_key = self
            .data_sources
            .get(&source.id())
            .map(|binding| binding.selection_key);
        if let Some(binding) = self.data_sources.get_mut(&source.id()) {
            binding.use_state = DataSourceUse::Retired;
        }
        self.data_sources.remove(&source.id());
        if let Some(selection_key) = selection_key {
            let cleared = self.selection_state.remove_source_key(selection_key);
            for kind in cleared {
                if kind == SelectionKind::Clipboard {
                    self.active_clipboard = None;
                    self.next_clipboard_generation = self.selection_state.current_generation(kind);
                    if let Some(bridge) = self.clipboard_bridge.as_mut() {
                        let _ = bridge.clear_internal_selection();
                    }
                    self.data_offers.clear();
                }
                self.publish_data_control_selection(kind);
                match kind {
                    SelectionKind::Clipboard => self.publish_clipboard_to_focused_client(),
                    SelectionKind::Primary => self.publish_primary_to_focused_client(),
                }
            }
        }
    }

    pub(in crate::compositor) fn clear_dead_active_clipboard_source(&mut self) {
        let data_control_source = self.active_clipboard.as_ref().and_then(|selection| {
            if let ClipboardSourceBackend::DataControl { source, .. } = &selection.source {
                Some(source.clone())
            } else {
                None
            }
        });
        if let Some(source) = data_control_source {
            if (!source.is_alive() || source.client().is_none())
                && self.data_control_sources.contains_key(&source.id())
            {
                self.remove_data_control_source(&source);
            }
            return;
        }
        let active_source = self.active_clipboard.as_ref().and_then(|selection| {
            if let ClipboardSourceBackend::InternalWayland { source, .. } = &selection.source {
                Some(source.clone())
            } else {
                None
            }
        });
        let Some(source) = active_source else {
            return;
        };
        if source.is_alive() && source.client().is_some() {
            return;
        }
        self.remove_data_source(&source);
    }

    pub(in crate::compositor) fn cancel_selection_source(
        &self,
        kind: SelectionKind,
        source_key: SelectionSourceKey,
    ) {
        match kind {
            SelectionKind::Clipboard => {
                if let Some(source) = self
                    .data_sources
                    .values()
                    .find(|binding| binding.selection_key == source_key)
                    && source.source.is_alive()
                {
                    source.source.cancelled();
                }
            }
            SelectionKind::Primary => {
                if let Some(source) = self
                    .primary_sources
                    .values()
                    .find(|binding| binding.selection_key == source_key)
                    && source.source.is_alive()
                {
                    let _ = source
                        .source
                        .send_event(zwp_primary_selection_source_v1::Event::Cancelled);
                }
            }
        }
        if let Some(source) = self
            .data_control_sources
            .values()
            .find(|binding| binding.selection_key == source_key)
            && source.source.is_alive()
        {
            let _ = source
                .source
                .send_event(ext_data_control_source_v1::Event::Cancelled);
        }
    }
}
