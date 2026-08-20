use super::*;

impl CompositorState {
    pub(in crate::compositor) fn register_data_source(
        &mut self,
        source: wl_data_source::WlDataSource,
        client_id: ClientId,
    ) {
        let selection_key = self.allocate_selection_source_key();
        self.selection_state.register_source(
            selection_key,
            SelectionSourceKind::WaylandClipboard,
            None,
        );
        self.selection_state.set_source_backend(
            selection_key,
            SelectionSourceBackend::WaylandClipboard {
                source: source.clone(),
                client_id: client_id.clone(),
            },
        );
        self.data_sources.insert(
            source.id(),
            ClipboardDataSource {
                source,
                selection_key,
                client_id,
                mime_types: Vec::new(),
                use_state: DataSourceUse::Unused,
                actions: 0,
                actions_set: false,
            },
        );
    }

    pub(in crate::compositor) fn offer_data_source_mime_type(
        &mut self,
        source: &wl_data_source::WlDataSource,
        mime_type: String,
    ) {
        let Some(binding) = self.data_sources.get(&source.id()) else {
            return;
        };
        if mime_type.is_empty()
            || mime_type.len() > 4096
            || binding.mime_types.len() >= 128
            || binding
                .mime_types
                .iter()
                .any(|existing| existing == &mime_type)
        {
            return;
        }
        let selection_key = binding.selection_key;
        self.selection_state
            .offer_source_mime_type_for_key(selection_key, mime_type.clone());
        if let Some(binding) = self.data_sources.get_mut(&source.id()) {
            binding.mime_types.push(mime_type);
        }
    }

    pub(in crate::compositor) fn set_clipboard_selection(
        &mut self,
        client_id: &ClientId,
        source: Option<wl_data_source::WlDataSource>,
        serial: u32,
    ) -> bool {
        if !self.client_has_keyboard_focus(client_id) {
            return false;
        }
        let Some(mutation_epoch) = self.selection_input_epoch(client_id, serial) else {
            return false;
        };

        let Some(source) = source else {
            let Some(clear) = self
                .selection_state
                .clear_selection(SelectionKind::Clipboard, mutation_epoch)
            else {
                return false;
            };
            if let Some(source_key) = clear.cleared_source {
                self.cancel_selection_source(SelectionKind::Clipboard, source_key);
            }
            if let Some(bridge) = self.clipboard_bridge.as_mut() {
                let _ = bridge.clear_internal_selection();
            }
            self.retire_clipboard_selection_offers();
            self.publish_clipboard_to_keyboard_focused_client();
            self.publish_data_control_selection(SelectionKind::Clipboard);
            return true;
        };

        let Some(binding) = self.data_sources.get(&source.id()).cloned() else {
            return false;
        };
        if binding.client_id != *client_id || !source.is_alive() || binding.mime_types.is_empty() {
            return false;
        }
        if binding.use_state != DataSourceUse::Unused {
            return false;
        }

        let Some(commit) = self.selection_state.commit_selection(
            SelectionKind::Clipboard,
            binding.selection_key,
            mutation_epoch,
        ) else {
            return false;
        };
        if let Some(previous_source) = commit.replaced_source {
            self.cancel_selection_source(SelectionKind::Clipboard, previous_source);
        }
        self.selection_state.mark_source_used(binding.selection_key);
        if let Some(binding) = self.data_sources.get_mut(&source.id()) {
            binding.use_state = DataSourceUse::Selection;
        }
        if let Some(bridge) = self.clipboard_bridge.as_mut() {
            let _ = bridge.publish_internal_selection(commit.generation, binding.mime_types);
        }
        self.retire_clipboard_selection_offers();
        self.publish_clipboard_to_keyboard_focused_client();
        self.publish_data_control_selection(SelectionKind::Clipboard);
        true
    }

    pub(in crate::compositor) fn install_host_clipboard_selection(
        &mut self,
        offer_id: HostClipboardOfferId,
        mime_types: Vec<String>,
    ) {
        let mime_types = normalize_selection_mime_types(mime_types);
        if mime_types.is_empty() {
            self.clear_host_clipboard_selection();
            return;
        }
        let mutation_epoch = self.selection_state.allocate_mutation_epoch();
        let source_key = self.allocate_selection_source_key();
        self.selection_state.register_source(
            source_key,
            SelectionSourceKind::HostClipboardBridge,
            None,
        );
        self.selection_state.set_source_backend(
            source_key,
            SelectionSourceBackend::HostClipboardBridge { offer_id },
        );
        for mime_type in &mime_types {
            self.selection_state
                .offer_source_mime_type_for_key(source_key, mime_type.clone());
        }
        let Some(commit) = self.selection_state.commit_selection(
            SelectionKind::Clipboard,
            source_key,
            mutation_epoch,
        ) else {
            return;
        };
        if let Some(previous_source) = commit.replaced_source {
            self.cancel_selection_source(SelectionKind::Clipboard, previous_source);
            if previous_source != source_key {
                self.selection_state
                    .remove_source_key(previous_source, mutation_epoch);
            }
        }
        self.selection_state.mark_source_used(source_key);
        self.retire_clipboard_selection_offers();
        self.publish_clipboard_to_keyboard_focused_client();
        self.publish_data_control_selection(SelectionKind::Clipboard);
    }

    pub(in crate::compositor) fn clear_host_clipboard_selection(&mut self) {
        let Some(active) = self
            .selection_state
            .active_selection(SelectionKind::Clipboard)
            .cloned()
        else {
            return;
        };
        if !matches!(
            self.selection_state.source_backend(active.source_key),
            Some(SelectionSourceBackend::HostClipboardBridge { .. })
        ) {
            return;
        }
        let mutation_epoch = self.selection_state.allocate_mutation_epoch();
        let Some(clear) = self
            .selection_state
            .clear_selection(SelectionKind::Clipboard, mutation_epoch)
        else {
            return;
        };
        if let Some(source_key) = clear.cleared_source {
            self.selection_state
                .remove_source_key(source_key, mutation_epoch);
        }
        if let Some(bridge) = self.clipboard_bridge.as_mut() {
            let _ = bridge.clear_internal_selection();
        }
        self.retire_clipboard_selection_offers();
        self.publish_clipboard_to_keyboard_focused_client();
        self.publish_data_control_selection(SelectionKind::Clipboard);
    }

    pub(in crate::compositor) fn poll_clipboard_bridge(&mut self) {
        let Some(bridge) = self.clipboard_bridge.as_mut() else {
            return;
        };
        let events = bridge.poll_events();
        for event in events {
            match event {
                ClipboardBridgeEvent::HostSelectionChanged {
                    offer_id,
                    mime_types,
                } => self.install_host_clipboard_selection(offer_id, mime_types),
                ClipboardBridgeEvent::HostSelectionCleared => self.clear_host_clipboard_selection(),
            }
        }
    }

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
        if self.client_has_keyboard_focus(&client_id) {
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
        let Some(selection_key) = selection_key else {
            return;
        };
        let mutation_epoch = self.selection_state.allocate_mutation_epoch();
        let cleared = self
            .selection_state
            .remove_source_key(selection_key, mutation_epoch);
        for kind in cleared {
            if kind == SelectionKind::Clipboard {
                if let Some(bridge) = self.clipboard_bridge.as_mut() {
                    let _ = bridge.clear_internal_selection();
                }
                self.retire_clipboard_selection_offers();
            }
            self.publish_data_control_selection(kind);
            match kind {
                SelectionKind::Clipboard => self.publish_clipboard_to_keyboard_focused_client(),
                SelectionKind::Primary => self.publish_primary_to_keyboard_focused_client(),
            }
        }
    }

    pub(in crate::compositor) fn clear_dead_active_clipboard_source(&mut self) {
        let Some(active) = self
            .selection_state
            .active_selection(SelectionKind::Clipboard)
            .cloned()
        else {
            return;
        };
        let Some(backend) = self
            .selection_state
            .source_backend(active.source_key)
            .cloned()
        else {
            return;
        };
        match backend {
            SelectionSourceBackend::WaylandClipboard { source, .. }
                if !source.is_alive() || source.client().is_none() =>
            {
                self.remove_data_source(&source);
            }
            SelectionSourceBackend::DataControl { source, .. }
                if !source.is_alive() || source.client().is_none() =>
            {
                self.remove_data_control_source(&source);
            }
            _ => {}
        }
    }

    pub(in crate::compositor) fn cancel_selection_source(
        &self,
        kind: SelectionKind,
        source_key: SelectionSourceKey,
    ) {
        let Some(backend) = self.selection_state.source_backend(source_key) else {
            return;
        };
        match (kind, backend) {
            (SelectionKind::Clipboard, SelectionSourceBackend::WaylandClipboard { source, .. })
                if source.is_alive() =>
            {
                source.cancelled()
            }
            (SelectionKind::Primary, SelectionSourceBackend::WaylandPrimary { source, .. })
                if source.is_alive() =>
            {
                let _ = source.send_event(zwp_primary_selection_source_v1::Event::Cancelled);
            }
            (_, SelectionSourceBackend::DataControl { source, .. }) if source.is_alive() => {
                let _ = source.send_event(ext_data_control_source_v1::Event::Cancelled);
            }
            _ => {}
        }
    }

    pub(in crate::compositor) fn request_selection_data(
        &mut self,
        kind: SelectionKind,
        source_key: SelectionSourceKey,
        mime_type: String,
        fd: OwnedFd,
    ) {
        let Some(active) = self.selection_state.active_selection(kind) else {
            return;
        };
        if active.source_key != source_key
            || !active.mime_types.iter().any(|mime| mime == &mime_type)
        {
            return;
        }
        let Some(backend) = self.selection_state.source_backend(source_key).cloned() else {
            return;
        };
        match backend {
            SelectionSourceBackend::WaylandClipboard { source, client_id } => {
                if !self.data_sources.get(&source.id()).is_some_and(|binding| {
                    binding.selection_key == source_key
                        && binding.client_id == client_id
                        && binding.source.is_alive()
                }) {
                    return;
                }
                let _ = source.send_event(wl_data_source::Event::Send {
                    mime_type,
                    fd: fd.as_fd(),
                });
            }
            SelectionSourceBackend::WaylandPrimary { source, client_id } => {
                if !self
                    .primary_sources
                    .get(&source.id())
                    .is_some_and(|binding| {
                        binding.selection_key == source_key
                            && binding.client_id == client_id
                            && binding.source.is_alive()
                    })
                {
                    return;
                }
                let _ = source.send_event(zwp_primary_selection_source_v1::Event::Send {
                    mime_type,
                    fd: fd.as_fd(),
                });
            }
            SelectionSourceBackend::DataControl { source, client_id } => {
                if !self
                    .data_control_sources
                    .get(&source.id())
                    .is_some_and(|binding| {
                        binding.selection_key == source_key
                            && binding.client_id == client_id
                            && binding.source.is_alive()
                    })
                {
                    return;
                }
                let _ = source.send_event(ext_data_control_source_v1::Event::Send {
                    mime_type,
                    fd: fd.as_fd(),
                });
            }
            SelectionSourceBackend::HostClipboardBridge { offer_id } => {
                if let Some(bridge) = self.clipboard_bridge.as_mut() {
                    let _ = bridge.request_host_data(offer_id, mime_type, fd);
                }
            }
        }
    }

    pub(in crate::compositor) fn publish_clipboard_to_keyboard_focused_client(&mut self) {
        let Some(client_id) = self.keyboard_focused_client_id() else {
            return;
        };
        self.publish_clipboard_to_client(&client_id);
    }

    pub(in crate::compositor) fn publish_clipboard_to_client(&mut self, client_id: &ClientId) {
        let devices = self
            .data_devices
            .iter()
            .filter(|binding| {
                binding.client_id == *client_id
                    && binding.device.is_alive()
                    && binding.seat_id.interface().name == "wl_seat"
            })
            .map(|binding| binding.device.clone())
            .collect::<Vec<_>>();
        for device in devices {
            self.publish_clipboard_to_data_device(&device);
        }
    }

    pub(in crate::compositor) fn publish_clipboard_to_data_device(
        &mut self,
        device: &wl_data_device::WlDataDevice,
    ) {
        if !device.is_alive() {
            return;
        }
        let Some(selection) = self
            .selection_state
            .active_selection(SelectionKind::Clipboard)
            .cloned()
        else {
            let _ = device.send_event(wl_data_device::Event::Selection { id: None });
            return;
        };
        let Some(client) = device.client() else {
            return;
        };
        let Some(handle) = device.handle().upgrade() else {
            return;
        };
        let display = DisplayHandle::from(handle);
        let Some(broker_offer_id) = self.selection_state.register_offer(
            SelectionKind::Clipboard,
            device.id().protocol_id(),
            selection.generation,
        ) else {
            return;
        };
        let Ok(offer) = client
            .create_resource::<wl_data_offer::WlDataOffer, DataOfferData, CompositorState>(
                &display,
                device.version().min(3),
                DataOfferData {
                    target_client_id: client.id(),
                    source_generation: selection.generation,
                    kind: DataOfferKind::Selection,
                },
            )
        else {
            return;
        };
        self.data_offers.insert(
            offer.id(),
            ClipboardDataOffer {
                offer: offer.clone(),
                target_client_id: client.id(),
                target_id: device.id().protocol_id(),
                source_generation: selection.generation,
                broker_offer_id: Some(broker_offer_id),
                source_key: Some(selection.source_key),
                mime_types: selection.mime_types.clone(),
                kind: DataOfferKind::Selection,
                accepted_mime: None,
                selected_action: None,
                drag_phase: None,
                source_actions: 0,
                destination_actions: None,
                preferred_action: 0,
            },
        );
        let _ = device.send_event(wl_data_device::Event::DataOffer { id: offer.clone() });
        for mime_type in selection.mime_types {
            let _ = offer.send_event(wl_data_offer::Event::Offer { mime_type });
        }
        let _ = device.send_event(wl_data_device::Event::Selection { id: Some(offer) });
    }

    pub(in crate::compositor) fn receive_clipboard_offer(
        &mut self,
        offer: &wl_data_offer::WlDataOffer,
        client_id: &ClientId,
        source_generation: u64,
        mime_type: String,
        fd: OwnedFd,
    ) {
        let Some(binding) = self.data_offers.get(&offer.id()).cloned() else {
            return;
        };
        if binding.kind == DataOfferKind::DragAndDrop {
            if binding.target_client_id != *client_id
                || !binding.mime_types.iter().any(|mime| mime == &mime_type)
            {
                return;
            }
            let Some(active) = self.active_drag.as_ref() else {
                return;
            };
            if active
                .offer
                .as_ref()
                .is_none_or(|current| !same_wayland_resource(current, offer))
            {
                return;
            }
            let Some(source) = active.source.as_ref() else {
                return;
            };
            let _ = source.send_event(wl_data_source::Event::Send {
                mime_type,
                fd: fd.as_fd(),
            });
            return;
        }
        let Some(source_key) = binding.source_key else {
            return;
        };
        let Some(broker_offer_id) = binding.broker_offer_id else {
            return;
        };
        if binding.target_client_id != *client_id
            || binding.source_generation != source_generation
            || !binding.mime_types.iter().any(|mime| mime == &mime_type)
            || !self.selection_state.offer_is_current(
                broker_offer_id,
                SelectionKind::Clipboard,
                source_generation,
                binding.target_id,
                source_key,
                &mime_type,
            )
        {
            return;
        }
        self.request_selection_data(SelectionKind::Clipboard, source_key, mime_type, fd);
    }

    fn retire_clipboard_selection_offers(&mut self) {
        self.data_offers
            .retain(|_, offer| offer.kind == DataOfferKind::DragAndDrop && offer.offer.is_alive());
    }
}
