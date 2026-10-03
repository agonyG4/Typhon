use super::*;

const MAX_PENDING_XWAYLAND_SELECTION_REQUESTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionDataRequestFailure {
    NoActiveSelection,
    SourceMismatch,
    MimeMismatch,
    MissingBackend,
    SourceBindingMissing,
    SourceClientMismatch,
    SourceDead,
    ClipboardBridgeMissing,
    ClipboardBridgeFailure,
    XwaylandMailboxFull,
    SendEventFailure,
}

impl SelectionDataRequestFailure {
    const fn reason(self) -> &'static str {
        match self {
            Self::NoActiveSelection => "no_active_selection",
            Self::SourceMismatch => "source_mismatch",
            Self::MimeMismatch => "mime_mismatch",
            Self::MissingBackend => "missing_backend",
            Self::SourceBindingMissing => "source_binding_missing",
            Self::SourceClientMismatch => "source_client_mismatch",
            Self::SourceDead => "source_dead",
            Self::ClipboardBridgeMissing => "host_bridge_missing",
            Self::ClipboardBridgeFailure => "host_bridge_failure",
            Self::XwaylandMailboxFull => "xwayland_mailbox_full",
            Self::SendEventFailure => "wl_data_source_send_event_failure",
        }
    }
}

#[derive(Debug, Clone)]
struct ClipboardReceiveTraceContext {
    client_id: ClientId,
    offer_protocol_id: u32,
    broker_offer_id: u64,
    target_device_id: u32,
    source_key: SelectionSourceKey,
    source_generation: u64,
    offer_kind: DataOfferKind,
}

#[derive(Default)]
struct ClipboardTraceEvent<'a> {
    client_id: Option<&'a ClientId>,
    expected_target_client_id: Option<&'a ClientId>,
    target_client_matches: Option<bool>,
    offer_protocol_id: Option<u32>,
    source_protocol_id: Option<u32>,
    broker_offer_id: Option<u64>,
    target_device_id: Option<u32>,
    source_key: Option<SelectionSourceKey>,
    source_generation: Option<u64>,
    mime_type: Option<&'a str>,
    offer_kind: Option<DataOfferKind>,
    backend_kind: Option<&'a str>,
    source_alive: Option<bool>,
    source_binding_present: Option<bool>,
    source_client_matches: Option<bool>,
    data_offer_binding_present: Option<bool>,
    broker_offer_present: Option<bool>,
    offer_current: Option<bool>,
    send_attempted: Option<bool>,
    send_succeeded: Option<bool>,
    fd_path: Option<&'a str>,
    reason: Option<&'a str>,
}

#[derive(Default)]
struct ClipboardTraceStatus<'a> {
    backend_kind: Option<&'a str>,
    source_alive: Option<bool>,
    source_binding_present: Option<bool>,
    source_client_matches: Option<bool>,
    broker_offer_present: Option<bool>,
    offer_current: Option<bool>,
    send_attempted: Option<bool>,
    send_succeeded: Option<bool>,
    fd_path: Option<&'a str>,
    reason: Option<&'a str>,
}

impl CompositorState {
    fn trace_clipboard_diagnostic(&mut self, stage: &'static str, event: ClipboardTraceEvent<'_>) {
        if !self.lifecycle_compatibility_trace.clipboard_trace_enabled() {
            return;
        }

        let selection = self
            .selection_state
            .active_selection(SelectionKind::Clipboard);
        let source_key = event.source_key;
        let (
            detected_backend_kind,
            detected_source_alive,
            detected_binding_present,
            detected_source_client_matches,
            detected_source_protocol_id,
        ) = source_key.map_or((None, None, None, None, None), |source_key| {
            self.clipboard_trace_backend_state(source_key)
        });
        let source_protocol_id = event.source_protocol_id.or(detected_source_protocol_id);
        let backend_kind = event.backend_kind.or(detected_backend_kind);
        let source_alive = event.source_alive.or(detected_source_alive);
        let source_binding_present = event.source_binding_present.or(detected_binding_present);
        let source_client_matches = event
            .source_client_matches
            .or(detected_source_client_matches);
        let broker_offer_present = event.broker_offer_present.or_else(|| {
            event.broker_offer_id.map(|offer_id| {
                self.selection_state
                    .has_offer(SelectionKind::Clipboard, offer_id)
            })
        });
        let offer_current = event.offer_current;
        let target_client_matches = event
            .target_client_matches
            .or_else(|| Some(event.client_id? == event.expected_target_client_id?));

        let details = format!(
            "stage={stage} client={:?} expected_target_client={:?} target_client_matches={target_client_matches:?} offer_protocol_id={:?} source_protocol_id={:?} broker_offer_id={:?} target_data_device_id={:?} source_key={source_key:?} source_generation={:?} current_selection_generation={:?} current_selection_source_key={:?} mime={:?} offer_kind={:?} data_offer_binding_present={:?} source_backend_kind={backend_kind:?} source_alive={source_alive:?} source_binding_present={source_binding_present:?} source_client_matches={source_client_matches:?} broker_offer_present={broker_offer_present:?} offer_is_current={offer_current:?} source_send_event_attempted={:?} send_operation_succeeded={:?} fd_path={:?} reason={:?}",
            event.client_id,
            event.expected_target_client_id,
            event.offer_protocol_id,
            source_protocol_id,
            event.broker_offer_id,
            event.target_device_id,
            event.source_generation,
            selection.map(|selection| selection.generation),
            selection.map(|selection| selection.source_key),
            event.mime_type,
            event.offer_kind,
            event.data_offer_binding_present,
            event.send_attempted,
            event.send_succeeded,
            event.fd_path,
            event.reason,
        );
        self.lifecycle_compatibility_trace
            .record_clipboard_trace(|| details);
    }

    fn clipboard_trace_backend_state(
        &self,
        source_key: SelectionSourceKey,
    ) -> (
        Option<&'static str>,
        Option<bool>,
        Option<bool>,
        Option<bool>,
        Option<u32>,
    ) {
        let Some(backend) = self.selection_state.source_backend(source_key) else {
            return (Some("missing"), None, Some(false), Some(false), None);
        };
        match backend {
            SelectionSourceBackend::WaylandClipboard { source, client_id } => {
                let binding = self.data_sources.get(&source.id());
                let source_client_matches = binding.is_some_and(|binding| {
                    binding.selection_key == source_key
                        && binding.client_id == *client_id
                        && source
                            .client()
                            .is_some_and(|client| client.id() == *client_id)
                });
                (
                    Some("wayland_clipboard"),
                    Some(source.is_alive()),
                    Some(binding.is_some()),
                    Some(source_client_matches),
                    Some(source.id().protocol_id()),
                )
            }
            SelectionSourceBackend::WaylandPrimary { source, client_id } => {
                let binding = self.primary_sources.get(&source.id());
                let source_client_matches = binding.is_some_and(|binding| {
                    binding.selection_key == source_key
                        && binding.client_id == *client_id
                        && source
                            .client()
                            .is_some_and(|client| client.id() == *client_id)
                });
                (
                    Some("wayland_primary"),
                    Some(source.is_alive()),
                    Some(binding.is_some()),
                    Some(source_client_matches),
                    Some(source.id().protocol_id()),
                )
            }
            SelectionSourceBackend::DataControl { source, client_id } => {
                let binding = self.data_control_sources.get(&source.id());
                let source_client_matches = binding.is_some_and(|binding| {
                    binding.selection_key == source_key
                        && binding.client_id == *client_id
                        && source
                            .client()
                            .is_some_and(|client| client.id() == *client_id)
                });
                (
                    Some("data_control"),
                    Some(source.is_alive()),
                    Some(binding.is_some()),
                    Some(source_client_matches),
                    Some(source.id().protocol_id()),
                )
            }
            SelectionSourceBackend::HostClipboardBridge { .. } => {
                (Some("host_clipboard_bridge"), None, None, None, None)
            }
            SelectionSourceBackend::Xwayland { .. } => (Some("xwayland"), None, None, None, None),
        }
    }

    pub(in crate::compositor) fn trace_clipboard_receive_entry(
        &mut self,
        client_id: &ClientId,
        expected_target_client_id: &ClientId,
        offer: &wl_data_offer::WlDataOffer,
        source_generation: u64,
        offer_kind: DataOfferKind,
        mime_type: &str,
    ) {
        if offer_kind != DataOfferKind::Selection
            || !self.lifecycle_compatibility_trace.clipboard_trace_enabled()
        {
            return;
        }

        let binding = self.data_offers.get(&offer.id()).cloned();
        self.trace_clipboard_offer_validation(
            "receive_entry",
            client_id,
            Some(expected_target_client_id),
            offer,
            offer_kind,
            source_generation,
            mime_type,
            binding.as_ref(),
            None,
            None,
            None,
            None,
        );
        self.trace_clipboard_offer_validation(
            "target_client_validation",
            client_id,
            Some(expected_target_client_id),
            offer,
            offer_kind,
            source_generation,
            mime_type,
            binding.as_ref(),
            None,
            None,
            None,
            (client_id != expected_target_client_id).then_some("target_client_mismatch"),
        );
        self.trace_clipboard_offer_validation(
            "data_offer_lookup",
            client_id,
            Some(expected_target_client_id),
            offer,
            offer_kind,
            source_generation,
            mime_type,
            binding.as_ref(),
            None,
            binding.is_none().then_some(false),
            binding
                .is_none()
                .then_some("owned_fd_closed_on_receive_early_return"),
            binding.is_none().then_some("data_offer_binding_missing"),
        );
    }

    fn trace_clipboard_offer_validation(
        &mut self,
        stage: &'static str,
        client_id: &ClientId,
        expected_client_id: Option<&ClientId>,
        offer: &wl_data_offer::WlDataOffer,
        offer_kind: DataOfferKind,
        source_generation: u64,
        mime_type: &str,
        binding: Option<&ClipboardDataOffer>,
        broker_offer_present: Option<bool>,
        offer_current: Option<bool>,
        fd_path: Option<&'static str>,
        reason: Option<&'static str>,
    ) {
        let binding_client_id = binding.map(|binding| &binding.target_client_id);
        let expected_client_id = expected_client_id.or(binding_client_id);
        self.trace_clipboard_diagnostic(
            stage,
            ClipboardTraceEvent {
                client_id: Some(client_id),
                expected_target_client_id: expected_client_id,
                target_client_matches: expected_client_id.map(|expected| client_id == expected),
                offer_protocol_id: Some(offer.id().protocol_id()),
                broker_offer_id: binding.and_then(|binding| binding.broker_offer_id),
                target_device_id: binding.map(|binding| binding.target_id),
                source_key: binding.and_then(|binding| binding.source_key),
                source_generation: Some(source_generation),
                mime_type: Some(mime_type),
                offer_kind: Some(offer_kind),
                data_offer_binding_present: Some(binding.is_some()),
                broker_offer_present,
                offer_current,
                fd_path,
                reason,
                ..ClipboardTraceEvent::default()
            },
        );
    }

    pub(in crate::compositor) fn trace_clipboard_offer_snapshot(
        &mut self,
        binding: &ClipboardDataOffer,
        stage: &'static str,
        reason: &'static str,
    ) {
        if !self.lifecycle_compatibility_trace.clipboard_trace_enabled()
            || binding.kind != DataOfferKind::Selection
        {
            return;
        }
        let binding_client_id = binding.target_client_id.clone();
        let mime_type = binding.mime_types.first().map(String::as_str);
        let source_key = binding.source_key;
        let broker_offer_id = binding.broker_offer_id;
        let source_generation = binding.source_generation;
        let target_id = binding.target_id;
        let offer_protocol_id = binding.offer.id().protocol_id();
        let data_offer_binding_present = self.data_offers.contains_key(&binding.offer.id());
        let broker_offer_present = broker_offer_id.map(|broker_offer_id| {
            self.selection_state
                .has_offer(SelectionKind::Clipboard, broker_offer_id)
        });
        let offer_current = broker_offer_id.zip(source_key).zip(mime_type).map(
            |((broker_offer_id, source_key), mime_type)| {
                self.selection_state.offer_is_current(
                    broker_offer_id,
                    SelectionKind::Clipboard,
                    source_generation,
                    target_id,
                    source_key,
                    mime_type,
                )
            },
        );
        let trace_event = ClipboardTraceEvent {
            client_id: Some(&binding_client_id),
            expected_target_client_id: Some(&binding_client_id),
            target_client_matches: Some(true),
            offer_protocol_id: Some(offer_protocol_id),
            broker_offer_id,
            target_device_id: Some(target_id),
            source_key,
            source_generation: Some(source_generation),
            mime_type,
            offer_kind: Some(binding.kind),
            data_offer_binding_present: Some(data_offer_binding_present),
            broker_offer_present,
            offer_current,
            reason: Some(reason),
            ..ClipboardTraceEvent::default()
        };
        self.trace_clipboard_diagnostic(stage, trace_event);
    }

    fn trace_clipboard_source_lifecycle(
        &mut self,
        source: &wl_data_source::WlDataSource,
        source_client_id: Option<&ClientId>,
        source_key: Option<SelectionSourceKey>,
        stage: &'static str,
        reason: &'static str,
    ) {
        if !self.lifecycle_compatibility_trace.clipboard_trace_enabled() {
            return;
        }
        let is_clipboard_source = source_key.is_some_and(|source_key| {
            matches!(
                self.selection_state.source_backend(source_key),
                Some(SelectionSourceBackend::WaylandClipboard { source: bound, .. })
                    if bound.id() == source.id()
            )
        });
        if !is_clipboard_source {
            return;
        }
        let active = self
            .selection_state
            .active_selection(SelectionKind::Clipboard)
            .filter(|selection| Some(selection.source_key) == source_key);
        let binding = self.data_sources.get(&source.id());
        let client_id = source_client_id
            .cloned()
            .or_else(|| source.client().map(|client| client.id()));
        let source_client_matches = client_id
            .as_ref()
            .map(|client_id| binding.map_or(true, |binding| binding.client_id == *client_id));
        let source_alive = source.is_alive();
        let source_binding_present = binding.is_some();
        let event = ClipboardTraceEvent {
            client_id: client_id.as_ref(),
            source_protocol_id: Some(source.id().protocol_id()),
            source_key,
            source_generation: active.map(|selection| selection.generation),
            offer_kind: Some(DataOfferKind::Selection),
            source_alive: Some(source_alive),
            source_binding_present: Some(source_binding_present),
            source_client_matches,
            reason: Some(reason),
            ..ClipboardTraceEvent::default()
        };
        self.trace_clipboard_diagnostic(stage, event);
    }

    fn trace_clipboard_context(
        &mut self,
        stage: &'static str,
        context: &ClipboardReceiveTraceContext,
        mime_type: &str,
        status: ClipboardTraceStatus<'_>,
    ) {
        self.trace_clipboard_diagnostic(
            stage,
            ClipboardTraceEvent {
                client_id: Some(&context.client_id),
                expected_target_client_id: Some(&context.client_id),
                target_client_matches: Some(true),
                offer_protocol_id: Some(context.offer_protocol_id),
                broker_offer_id: Some(context.broker_offer_id),
                target_device_id: Some(context.target_device_id),
                source_key: Some(context.source_key),
                source_generation: Some(context.source_generation),
                mime_type: Some(mime_type),
                offer_kind: Some(context.offer_kind),
                backend_kind: status.backend_kind,
                source_alive: status.source_alive,
                source_binding_present: status.source_binding_present,
                source_client_matches: status.source_client_matches,
                broker_offer_present: status.broker_offer_present,
                offer_current: status.offer_current,
                send_attempted: status.send_attempted,
                send_succeeded: status.send_succeeded,
                fd_path: status.fd_path,
                reason: status.reason,
                ..ClipboardTraceEvent::default()
            },
        );
    }

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

    pub(in crate::compositor) fn retire_wayland_clipboard_source(
        &mut self,
        source_key: SelectionSourceKey,
    ) {
        if !matches!(
            self.selection_state.source_backend(source_key),
            Some(SelectionSourceBackend::WaylandClipboard { .. })
        ) {
            return;
        }
        if let Some(binding) = self
            .data_sources
            .values_mut()
            .find(|binding| binding.selection_key == source_key)
        {
            binding.use_state = DataSourceUse::Retired;
        }
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
            self.note_selection_admission_rejection(
                SelectionKind::Clipboard,
                client_id,
                serial,
                SelectionAdmissionRejection::UnfocusedClient,
            );
            return false;
        }

        let Some(source) = source else {
            let mutation_epoch = self.selection_state.allocate_mutation_epoch();
            let Some(clear) = self
                .selection_state
                .clear_selection(SelectionKind::Clipboard, mutation_epoch)
            else {
                return false;
            };
            if let Some(source_key) = clear.cleared_source {
                self.retire_wayland_clipboard_source(source_key);
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
            self.note_selection_admission_rejection(
                SelectionKind::Clipboard,
                client_id,
                serial,
                SelectionAdmissionRejection::ForeignSource,
            );
            return false;
        };
        if binding.client_id != *client_id {
            self.note_selection_admission_rejection(
                SelectionKind::Clipboard,
                client_id,
                serial,
                SelectionAdmissionRejection::ForeignSource,
            );
            return false;
        }
        if !source.is_alive() {
            self.note_selection_admission_rejection(
                SelectionKind::Clipboard,
                client_id,
                serial,
                SelectionAdmissionRejection::DeadSource,
            );
            return false;
        }
        if binding.mime_types.is_empty() {
            self.note_selection_admission_rejection(
                SelectionKind::Clipboard,
                client_id,
                serial,
                SelectionAdmissionRejection::EmptyMimeCatalog,
            );
            return false;
        }
        if self.is_active_clipboard_source_reuse(client_id, &source) {
            self.record_lifecycle_compatibility_recovery(
                client_id.clone(),
                source.id().protocol_id(),
                "wl_data_source",
                None,
                LifecycleCompatibilityViolation::ActiveClipboardSourceReused,
                LifecycleCompatibilityAction::KeepActiveClipboardSelection,
            );
            return true;
        }
        if binding.actions_set {
            self.note_selection_admission_rejection(
                SelectionKind::Clipboard,
                client_id,
                serial,
                SelectionAdmissionRejection::InvalidSourcePurpose,
            );
            return false;
        }
        if binding.use_state != DataSourceUse::Unused {
            self.note_selection_admission_rejection(
                SelectionKind::Clipboard,
                client_id,
                serial,
                SelectionAdmissionRejection::UsedSource,
            );
            return false;
        }

        // Wire serial provenance is not the broker's mutation order. Allocate
        // this only after the core selection request has passed admission.
        let mutation_epoch = self.selection_state.allocate_mutation_epoch();
        let Some(commit) = self.selection_state.commit_selection(
            SelectionKind::Clipboard,
            binding.selection_key,
            mutation_epoch,
        ) else {
            return false;
        };
        if let Some(previous_source) = commit.replaced_source {
            self.retire_wayland_clipboard_source(previous_source);
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

    pub(in crate::compositor) fn is_active_clipboard_source_reuse(
        &self,
        client_id: &ClientId,
        source: &wl_data_source::WlDataSource,
    ) -> bool {
        self.data_sources.get(&source.id()).is_some_and(|binding| {
            binding.client_id == *client_id
                && binding.source.is_alive()
                && !binding.actions_set
                && !binding.mime_types.is_empty()
                && binding.use_state == DataSourceUse::Selection
                && self
                    .selection_state
                    .active_selection(SelectionKind::Clipboard)
                    .is_some_and(|active| active.source_key == binding.selection_key)
        })
    }

    pub(in crate::compositor) fn note_selection_admission_rejection(
        &self,
        kind: SelectionKind,
        client_id: &ClientId,
        serial: u32,
        reason: SelectionAdmissionRejection,
    ) {
        eprintln!(
            "oblivion-one selection: rejected {:?} set_selection client={client_id:?} serial={serial} reason={reason:?}",
            kind
        );
    }

    pub(in crate::compositor) fn publish_clipboard_clear_to_client(
        &mut self,
        client_id: &ClientId,
    ) {
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
            let _ = device.send_event(wl_data_device::Event::Selection { id: None });
        }
    }

    pub(in crate::compositor) fn retire_clipboard_selection_offers_for_client(
        &mut self,
        client_id: &ClientId,
    ) {
        let mut retired = Vec::new();
        self.data_offers.retain(|_, offer| {
            let retire =
                offer.kind == DataOfferKind::Selection && offer.target_client_id == *client_id;
            if retire {
                retired.extend(offer.broker_offer_id);
            }
            !retire
        });
        for offer_id in retired {
            self.selection_state
                .retire_offer(SelectionKind::Clipboard, offer_id);
        }
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
            self.retire_wayland_clipboard_source(previous_source);
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

    pub(in crate::compositor) fn apply_xwayland_selection_event(
        &mut self,
        event: crate::xwayland::XwaylandSelectionEvent,
    ) {
        use crate::xwayland::{XwaylandSelectionEvent, XwaylandSelectionKind};

        match event {
            XwaylandSelectionEvent::OfferChanged { kind, offer } => {
                if kind != offer.id.kind {
                    return;
                }
                let selection_kind = match kind {
                    XwaylandSelectionKind::Clipboard => SelectionKind::Clipboard,
                    XwaylandSelectionKind::Primary => SelectionKind::Primary,
                };
                let mime_types = normalize_selection_mime_types(offer.mime_types);
                if mime_types.is_empty() {
                    self.clear_xwayland_selection(selection_kind, offer.id.generation, kind);
                    return;
                }

                let mutation_epoch = self.selection_state.allocate_mutation_epoch();
                let source_key = self.allocate_selection_source_key();
                self.selection_state.register_source(
                    source_key,
                    SelectionSourceKind::Xwayland,
                    None,
                );
                self.selection_state.set_source_backend(
                    source_key,
                    SelectionSourceBackend::Xwayland { offer_id: offer.id },
                );
                for mime_type in &mime_types {
                    self.selection_state
                        .offer_source_mime_type_for_key(source_key, mime_type.clone());
                }
                let Some(commit) = self.selection_state.commit_selection(
                    selection_kind,
                    source_key,
                    mutation_epoch,
                ) else {
                    self.selection_state
                        .remove_source_key(source_key, mutation_epoch);
                    return;
                };
                if let Some(previous_source) = commit.replaced_source {
                    if selection_kind == SelectionKind::Clipboard {
                        self.retire_wayland_clipboard_source(previous_source);
                    }
                    self.cancel_selection_source(selection_kind, previous_source);
                    if previous_source != source_key {
                        self.selection_state
                            .remove_source_key(previous_source, mutation_epoch);
                    }
                }
                self.selection_state.mark_source_used(source_key);
                self.publish_xwayland_selection(selection_kind);
            }
            XwaylandSelectionEvent::Cleared { kind, generation } => {
                let selection_kind = match kind {
                    XwaylandSelectionKind::Clipboard => SelectionKind::Clipboard,
                    XwaylandSelectionKind::Primary => SelectionKind::Primary,
                };
                self.clear_xwayland_selection(selection_kind, generation, kind);
            }
        }
    }

    fn publish_xwayland_selection(&mut self, kind: SelectionKind) {
        match kind {
            SelectionKind::Clipboard => {
                self.retire_clipboard_selection_offers();
                self.publish_clipboard_to_keyboard_focused_client();
            }
            SelectionKind::Primary => self.publish_primary_to_keyboard_focused_client(),
        }
        self.publish_data_control_selection(kind);
    }

    fn clear_xwayland_selection(
        &mut self,
        selection_kind: SelectionKind,
        generation: crate::xwayland::XwaylandGeneration,
        event_kind: crate::xwayland::XwaylandSelectionKind,
    ) {
        let Some(active) = self
            .selection_state
            .active_selection(selection_kind)
            .cloned()
        else {
            return;
        };
        let Some(SelectionSourceBackend::Xwayland { offer_id }) =
            self.selection_state.source_backend(active.source_key)
        else {
            return;
        };
        if offer_id.generation != generation || offer_id.kind != event_kind {
            return;
        }
        let source_key = active.source_key;
        let mutation_epoch = self.selection_state.allocate_mutation_epoch();
        let Some(clear) = self
            .selection_state
            .clear_selection(selection_kind, mutation_epoch)
        else {
            return;
        };
        if let Some(source_key) = clear.cleared_source {
            self.cancel_selection_source(selection_kind, source_key);
            self.selection_state
                .remove_source_key(source_key, mutation_epoch);
        }
        let _ = source_key;
        self.publish_xwayland_selection(selection_kind);
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
        let target_id = device.id().protocol_id();
        let target_client_id = self
            .data_devices
            .iter()
            .find(|binding| same_wayland_resource(&binding.device, device))
            .map(|binding| binding.client_id.clone())
            .or_else(|| device.client().map(|client| client.id()));
        if let Some(client_id) = target_client_id.as_ref()
            && self.active_drag.as_ref().is_some_and(|drag| {
                drag.target
                    .as_ref()
                    .and_then(ActiveDragTarget::wayland_device_id)
                    == Some(target_id)
                    && drag
                        .target
                        .as_ref()
                        .and_then(ActiveDragTarget::wayland_client)
                        == Some(client_id)
            })
        {
            self.cancel_drag_session("data_device_destroyed");
        }
        let retiring_offers = if self.lifecycle_compatibility_trace.clipboard_trace_enabled() {
            self.data_offers
                .values()
                .filter(|offer| {
                    offer.kind == DataOfferKind::Selection
                        && offer.target_id == target_id
                        && target_client_id.as_ref() == Some(&offer.target_client_id)
                })
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for offer in &retiring_offers {
            self.trace_clipboard_offer_snapshot(
                offer,
                "offer_retirement_begin",
                "data_device_destroyed",
            );
        }
        self.data_devices
            .retain(|binding| !same_wayland_resource(&binding.device, device));
        let mut retired = Vec::new();
        self.data_offers.retain(|_, offer| {
            let belongs_to_device = offer.target_id == target_id
                && target_client_id.as_ref() == Some(&offer.target_client_id);
            if belongs_to_device {
                if offer.kind == DataOfferKind::Selection {
                    retired.extend(offer.broker_offer_id);
                }
                false
            } else {
                true
            }
        });
        for broker_offer_id in retired {
            self.selection_state
                .retire_offer(SelectionKind::Clipboard, broker_offer_id);
        }
        for offer in &retiring_offers {
            self.trace_clipboard_offer_snapshot(offer, "offer_retired", "data_device_destroyed");
        }
    }

    pub(in crate::compositor) fn remove_data_source(
        &mut self,
        source: &wl_data_source::WlDataSource,
    ) {
        self.cancel_drag_for_source(source);
        let source_binding = self
            .data_sources
            .get(&source.id())
            .map(|binding| (binding.selection_key, binding.client_id.clone()));
        let selection_key = source_binding
            .as_ref()
            .map(|(selection_key, _)| *selection_key);
        if let Some(selection_key) = selection_key {
            self.trace_clipboard_source_lifecycle(
                source,
                source_binding.as_ref().map(|(_, client_id)| client_id),
                Some(selection_key),
                "source_destruction_begin",
                "source_resource_destroyed",
            );
        }
        if let Some(binding) = self.data_sources.get_mut(&source.id()) {
            binding.use_state = DataSourceUse::Retired;
        }
        self.data_sources.remove(&source.id());
        if let Some(selection_key) = selection_key {
            self.trace_clipboard_source_lifecycle(
                source,
                source_binding.as_ref().map(|(_, client_id)| client_id),
                Some(selection_key),
                "source_binding_removed",
                "source_resource_destroyed",
            );
        }
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
    ) -> bool {
        self.request_selection_data_with_trace(kind, source_key, mime_type, fd, None)
            .is_ok()
    }

    fn trace_selection_data_stage(
        &mut self,
        trace_context: Option<&ClipboardReceiveTraceContext>,
        stage: &'static str,
        mime_type: &str,
        status: ClipboardTraceStatus<'_>,
    ) {
        if let Some(context) = trace_context {
            self.trace_clipboard_context(stage, context, mime_type, status);
        }
    }

    fn reject_selection_data_request(
        &mut self,
        fd: OwnedFd,
        failure: SelectionDataRequestFailure,
        trace_context: Option<&ClipboardReceiveTraceContext>,
        mime_type: &str,
        mut status: ClipboardTraceStatus<'_>,
    ) -> Result<(), SelectionDataRequestFailure> {
        drop(fd);
        status.fd_path = Some("owned_fd_closed_on_rejection");
        status.reason = Some(failure.reason());
        self.trace_selection_data_stage(
            trace_context,
            "request_selection_data_failed",
            mime_type,
            status,
        );
        Err(failure)
    }

    fn request_selection_data_with_trace(
        &mut self,
        kind: SelectionKind,
        source_key: SelectionSourceKey,
        mime_type: String,
        fd: OwnedFd,
        trace_context: Option<ClipboardReceiveTraceContext>,
    ) -> Result<(), SelectionDataRequestFailure> {
        self.trace_selection_data_stage(
            trace_context.as_ref(),
            "request_selection_data_entry",
            &mime_type,
            ClipboardTraceStatus::default(),
        );
        let Some(active) = self.selection_state.active_selection(kind) else {
            return self.reject_selection_data_request(
                fd,
                SelectionDataRequestFailure::NoActiveSelection,
                trace_context.as_ref(),
                &mime_type,
                ClipboardTraceStatus::default(),
            );
        };
        if active.source_key != source_key {
            return self.reject_selection_data_request(
                fd,
                SelectionDataRequestFailure::SourceMismatch,
                trace_context.as_ref(),
                &mime_type,
                ClipboardTraceStatus::default(),
            );
        }
        if !active.mime_types.iter().any(|mime| mime == &mime_type) {
            return self.reject_selection_data_request(
                fd,
                SelectionDataRequestFailure::MimeMismatch,
                trace_context.as_ref(),
                &mime_type,
                ClipboardTraceStatus::default(),
            );
        }
        let Some(backend) = self.selection_state.source_backend(source_key).cloned() else {
            let failure = SelectionDataRequestFailure::MissingBackend;
            return self.reject_selection_data_request(
                fd,
                failure,
                trace_context.as_ref(),
                &mime_type,
                ClipboardTraceStatus {
                    backend_kind: Some("missing"),
                    source_alive: Some(false),
                    source_binding_present: Some(false),
                    source_client_matches: Some(false),
                    ..ClipboardTraceStatus::default()
                },
            );
        };
        let backend_kind = match &backend {
            SelectionSourceBackend::WaylandClipboard { .. } => "wayland_clipboard",
            SelectionSourceBackend::WaylandPrimary { .. } => "wayland_primary",
            SelectionSourceBackend::DataControl { .. } => "data_control",
            SelectionSourceBackend::HostClipboardBridge { .. } => "host_clipboard_bridge",
            SelectionSourceBackend::Xwayland { .. } => "xwayland",
        };
        self.trace_selection_data_stage(
            trace_context.as_ref(),
            "source_backend_selected",
            &mime_type,
            ClipboardTraceStatus {
                backend_kind: Some(backend_kind),
                ..ClipboardTraceStatus::default()
            },
        );

        match backend {
            SelectionSourceBackend::WaylandClipboard { source, client_id } => {
                let binding_status = self.data_sources.get(&source.id()).map(|binding| {
                    (
                        binding.selection_key == source_key,
                        binding.client_id == client_id,
                        binding.source.is_alive(),
                    )
                });
                let source_binding_present = binding_status.is_some();
                let source_client_matches = binding_status.is_some_and(|(_, client_matches, _)| {
                    client_matches
                        && source
                            .client()
                            .is_some_and(|source_client| source_client.id() == client_id)
                });
                let source_alive = source.is_alive()
                    && binding_status.is_some_and(|(_, _, binding_alive)| binding_alive);
                self.trace_selection_data_stage(
                    trace_context.as_ref(),
                    "source_binding_lookup",
                    &mime_type,
                    ClipboardTraceStatus {
                        backend_kind: Some(backend_kind),
                        source_alive: Some(source_alive),
                        source_binding_present: Some(source_binding_present),
                        source_client_matches: Some(source_client_matches),
                        ..ClipboardTraceStatus::default()
                    },
                );
                self.trace_selection_data_stage(
                    trace_context.as_ref(),
                    "source_client_validation",
                    &mime_type,
                    ClipboardTraceStatus {
                        backend_kind: Some(backend_kind),
                        source_binding_present: Some(source_binding_present),
                        source_client_matches: Some(source_client_matches),
                        ..ClipboardTraceStatus::default()
                    },
                );
                self.trace_selection_data_stage(
                    trace_context.as_ref(),
                    "source_liveness",
                    &mime_type,
                    ClipboardTraceStatus {
                        backend_kind: Some(backend_kind),
                        source_alive: Some(source_alive),
                        source_binding_present: Some(source_binding_present),
                        ..ClipboardTraceStatus::default()
                    },
                );
                let failure = if binding_status.is_none() {
                    Some(SelectionDataRequestFailure::SourceBindingMissing)
                } else if !binding_status.is_some_and(|(key_matches, _, _)| key_matches) {
                    Some(SelectionDataRequestFailure::SourceMismatch)
                } else if !source_client_matches {
                    Some(SelectionDataRequestFailure::SourceClientMismatch)
                } else if !source_alive {
                    Some(SelectionDataRequestFailure::SourceDead)
                } else {
                    None
                };
                if let Some(failure) = failure {
                    return self.reject_selection_data_request(
                        fd,
                        failure,
                        trace_context.as_ref(),
                        &mime_type,
                        ClipboardTraceStatus {
                            backend_kind: Some(backend_kind),
                            source_alive: Some(source_alive),
                            source_binding_present: Some(source_binding_present),
                            source_client_matches: Some(source_client_matches),
                            ..ClipboardTraceStatus::default()
                        },
                    );
                }
                self.trace_selection_data_stage(
                    trace_context.as_ref(),
                    "wl_data_source_send_event_attempt",
                    &mime_type,
                    ClipboardTraceStatus {
                        backend_kind: Some(backend_kind),
                        source_alive: Some(true),
                        source_binding_present: Some(true),
                        source_client_matches: Some(true),
                        send_attempted: Some(true),
                        ..ClipboardTraceStatus::default()
                    },
                );
                let trace_mime_type = trace_context.as_ref().map(|_| mime_type.clone());
                let result = source.send_event(wl_data_source::Event::Send {
                    mime_type,
                    fd: fd.as_fd(),
                });
                let send_succeeded = result.is_ok();
                drop(fd);
                if let (Some(context), Some(trace_mime_type)) =
                    (trace_context.as_ref(), trace_mime_type.as_deref())
                {
                    self.trace_clipboard_context(
                        "wl_data_source_send_event_result",
                        context,
                        trace_mime_type,
                        ClipboardTraceStatus {
                            backend_kind: Some(backend_kind),
                            source_alive: Some(true),
                            source_binding_present: Some(true),
                            source_client_matches: Some(true),
                            send_attempted: Some(true),
                            send_succeeded: Some(send_succeeded),
                            fd_path: Some(if send_succeeded {
                                "event_fd_queued_original_owned_fd_closed"
                            } else {
                                "event_send_failed_original_owned_fd_closed"
                            }),
                            reason: (!send_succeeded)
                                .then_some("wl_data_source_send_event_failure"),
                            ..ClipboardTraceStatus::default()
                        },
                    );
                }
                result
                    .map(|()| ())
                    .map_err(|_| SelectionDataRequestFailure::SendEventFailure)
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
                    return self.reject_selection_data_request(
                        fd,
                        SelectionDataRequestFailure::SourceBindingMissing,
                        trace_context.as_ref(),
                        &mime_type,
                        ClipboardTraceStatus::default(),
                    );
                }
                let result = source.send_event(zwp_primary_selection_source_v1::Event::Send {
                    mime_type,
                    fd: fd.as_fd(),
                });
                drop(fd);
                result
                    .map(|()| ())
                    .map_err(|_| SelectionDataRequestFailure::SendEventFailure)
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
                    return self.reject_selection_data_request(
                        fd,
                        SelectionDataRequestFailure::SourceBindingMissing,
                        trace_context.as_ref(),
                        &mime_type,
                        ClipboardTraceStatus::default(),
                    );
                }
                let result = source.send_event(ext_data_control_source_v1::Event::Send {
                    mime_type,
                    fd: fd.as_fd(),
                });
                drop(fd);
                result
                    .map(|()| ())
                    .map_err(|_| SelectionDataRequestFailure::SendEventFailure)
            }
            SelectionSourceBackend::HostClipboardBridge { offer_id } => {
                let Some(bridge) = self.clipboard_bridge.as_mut() else {
                    return self.reject_selection_data_request(
                        fd,
                        SelectionDataRequestFailure::ClipboardBridgeMissing,
                        trace_context.as_ref(),
                        &mime_type,
                        ClipboardTraceStatus {
                            backend_kind: Some(backend_kind),
                            ..ClipboardTraceStatus::default()
                        },
                    );
                };
                let trace_mime_type = trace_context.as_ref().map(|_| mime_type.clone());
                let result = bridge.request_host_data(offer_id, mime_type, fd);
                let succeeded = result.is_ok();
                if let (Some(context), Some(trace_mime_type)) =
                    (trace_context.as_ref(), trace_mime_type.as_deref())
                {
                    self.trace_clipboard_context(
                        "host_bridge_request_result",
                        context,
                        trace_mime_type,
                        ClipboardTraceStatus {
                            backend_kind: Some(backend_kind),
                            send_succeeded: Some(succeeded),
                            fd_path: Some("owned_fd_moved_to_host_bridge"),
                            reason: (!succeeded).then_some("host_bridge_failure"),
                            ..ClipboardTraceStatus::default()
                        },
                    );
                }
                result.map_err(|_| SelectionDataRequestFailure::ClipboardBridgeFailure)
            }
            SelectionSourceBackend::Xwayland { offer_id } => {
                if self.xwayland_selection_data_requests.len()
                    >= MAX_PENDING_XWAYLAND_SELECTION_REQUESTS
                {
                    return self.reject_selection_data_request(
                        fd,
                        SelectionDataRequestFailure::XwaylandMailboxFull,
                        trace_context.as_ref(),
                        &mime_type,
                        ClipboardTraceStatus {
                            backend_kind: Some(backend_kind),
                            ..ClipboardTraceStatus::default()
                        },
                    );
                }
                let trace_mime_type = trace_context.as_ref().map(|_| mime_type.clone());
                self.xwayland_selection_data_requests.push_back(
                    crate::xwayland::XwaylandSelectionDataRequest {
                        offer_id,
                        mime_type,
                        sink: fd,
                    },
                );
                if let (Some(context), Some(trace_mime_type)) =
                    (trace_context.as_ref(), trace_mime_type.as_deref())
                {
                    self.trace_clipboard_context(
                        "xwayland_request_queued",
                        context,
                        trace_mime_type,
                        ClipboardTraceStatus {
                            backend_kind: Some(backend_kind),
                            send_attempted: Some(false),
                            fd_path: Some("owned_fd_moved_to_xwayland_mailbox"),
                            ..ClipboardTraceStatus::default()
                        },
                    );
                }
                Ok(())
            }
        }
    }

    pub(in crate::compositor) fn request_xwayland_proxy_selection_data(
        &mut self,
        proxy_id: crate::xwayland::XwaylandProxySelectionId,
        mime_type: String,
        fd: OwnedFd,
    ) -> bool {
        let kind = match proxy_id.kind {
            crate::xwayland::XwaylandSelectionKind::Clipboard => SelectionKind::Clipboard,
            crate::xwayland::XwaylandSelectionKind::Primary => SelectionKind::Primary,
        };
        let Some(active) = self.selection_state.active_selection(kind) else {
            return false;
        };
        if active.generation != proxy_id.selection_generation
            || active.source_key != proxy_id.source_key
            || active.source_kind == SelectionSourceKind::Xwayland
            || !active.mime_types.iter().any(|mime| mime == &mime_type)
            || self
                .selection_state
                .source_backend(proxy_id.source_key)
                .is_none()
        {
            return false;
        }
        self.request_selection_data(kind, proxy_id.source_key, mime_type, fd)
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
        let target_client_id = client.id();
        let Ok(offer) = client
            .create_resource::<wl_data_offer::WlDataOffer, DataOfferData, CompositorState>(
                &display,
                device.version().min(3),
                DataOfferData {
                    target_client_id: target_client_id.clone(),
                    source_generation: selection.generation,
                    kind: DataOfferKind::Selection,
                },
            )
        else {
            self.selection_state
                .retire_offer(SelectionKind::Clipboard, broker_offer_id);
            return;
        };
        self.data_offers.insert(
            offer.id(),
            ClipboardDataOffer {
                offer: offer.clone(),
                target_client_id: target_client_id.clone(),
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
        self.trace_clipboard_diagnostic(
            "offer_published",
            ClipboardTraceEvent {
                client_id: Some(&target_client_id),
                expected_target_client_id: Some(&target_client_id),
                target_client_matches: Some(true),
                offer_protocol_id: Some(offer.id().protocol_id()),
                broker_offer_id: Some(broker_offer_id),
                target_device_id: Some(device.id().protocol_id()),
                source_key: Some(selection.source_key),
                source_generation: Some(selection.generation),
                offer_kind: Some(DataOfferKind::Selection),
                ..ClipboardTraceEvent::default()
            },
        );
        let _ = device.send_event(wl_data_device::Event::DataOffer { id: offer.clone() });
        for mime_type in selection.mime_types {
            self.trace_clipboard_diagnostic(
                "offer_mime_published",
                ClipboardTraceEvent {
                    client_id: Some(&target_client_id),
                    expected_target_client_id: Some(&target_client_id),
                    target_client_matches: Some(true),
                    offer_protocol_id: Some(offer.id().protocol_id()),
                    broker_offer_id: Some(broker_offer_id),
                    target_device_id: Some(device.id().protocol_id()),
                    source_key: Some(selection.source_key),
                    source_generation: Some(selection.generation),
                    mime_type: Some(&mime_type),
                    offer_kind: Some(DataOfferKind::Selection),
                    ..ClipboardTraceEvent::default()
                },
            );
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
            drop(fd);
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
                .target
                .as_ref()
                .and_then(ActiveDragTarget::wayland_offer)
                .is_none_or(|current| !same_wayland_resource(current, offer))
                || active
                    .target
                    .as_ref()
                    .and_then(ActiveDragTarget::wayland_client)
                    != Some(client_id)
            {
                return;
            }
            match &active.origin {
                ActiveDragOrigin::WaylandSource { source, .. } => {
                    let _ = source.send_event(wl_data_source::Event::Send {
                        mime_type,
                        fd: fd.as_fd(),
                    });
                }
                ActiveDragOrigin::Xwayland {
                    offer: xwayland_offer,
                } => {
                    let offer_id = xwayland_offer.id();
                    if self
                        .xwayland
                        .client_identity
                        .as_ref()
                        .is_none_or(|identity| identity.generation != offer_id.generation())
                        || !xwayland_offer
                            .mime_types()
                            .as_slice()
                            .iter()
                            .any(|mime| mime == &mime_type)
                        || self.xwayland_dnd_data_requests.len()
                            >= super::data_device::MAX_PENDING_XWAYLAND_DND_DATA_REQUESTS
                    {
                        return;
                    }
                    self.xwayland_dnd_data_requests.push_back(
                        crate::xwayland::XwaylandDndDataRequest {
                            offer_id,
                            mime_type,
                            sink: fd,
                        },
                    );
                }
                ActiveDragOrigin::WaylandSourceless { .. } => {}
            }
            return;
        }

        let trace_enabled = self.lifecycle_compatibility_trace.clipboard_trace_enabled();
        let target_matches = binding.target_client_id == *client_id;
        if trace_enabled {
            self.trace_clipboard_offer_validation(
                "target_client_binding_validation",
                client_id,
                Some(&binding.target_client_id),
                offer,
                binding.kind,
                source_generation,
                &mime_type,
                Some(&binding),
                None,
                None,
                (!target_matches).then_some("owned_fd_closed_on_rejection"),
                (!target_matches).then_some("target_client_mismatch"),
            );
        }
        if !target_matches {
            drop(fd);
            return;
        }

        let generation_matches = binding.source_generation == source_generation;
        if trace_enabled {
            self.trace_clipboard_offer_validation(
                "source_generation_validation",
                client_id,
                Some(&binding.target_client_id),
                offer,
                binding.kind,
                source_generation,
                &mime_type,
                Some(&binding),
                None,
                None,
                (!generation_matches).then_some("owned_fd_closed_on_rejection"),
                (!generation_matches).then_some("source_generation_mismatch"),
            );
        }
        if !generation_matches {
            drop(fd);
            return;
        }

        let Some(source_key) = binding.source_key else {
            if trace_enabled {
                self.trace_clipboard_offer_validation(
                    "source_key_validation",
                    client_id,
                    Some(&binding.target_client_id),
                    offer,
                    binding.kind,
                    source_generation,
                    &mime_type,
                    Some(&binding),
                    None,
                    None,
                    Some("owned_fd_closed_on_rejection"),
                    Some("source_key_missing"),
                );
            }
            drop(fd);
            return;
        };

        let mime_matches = binding.mime_types.iter().any(|mime| mime == &mime_type);
        if trace_enabled {
            self.trace_clipboard_offer_validation(
                "mime_validation",
                client_id,
                Some(&binding.target_client_id),
                offer,
                binding.kind,
                source_generation,
                &mime_type,
                Some(&binding),
                None,
                None,
                (!mime_matches).then_some("owned_fd_closed_on_rejection"),
                (!mime_matches).then_some("mime_mismatch"),
            );
        }
        if !mime_matches {
            drop(fd);
            return;
        }

        let Some(broker_offer_id) = binding.broker_offer_id else {
            if trace_enabled {
                self.trace_clipboard_offer_validation(
                    "broker_offer_validation",
                    client_id,
                    Some(&binding.target_client_id),
                    offer,
                    binding.kind,
                    source_generation,
                    &mime_type,
                    Some(&binding),
                    Some(false),
                    None,
                    Some("owned_fd_closed_on_rejection"),
                    Some("broker_offer_id_missing"),
                );
            }
            drop(fd);
            return;
        };
        let broker_offer_present = self
            .selection_state
            .has_offer(SelectionKind::Clipboard, broker_offer_id);
        if trace_enabled {
            self.trace_clipboard_offer_validation(
                "broker_offer_validation",
                client_id,
                Some(&binding.target_client_id),
                offer,
                binding.kind,
                source_generation,
                &mime_type,
                Some(&binding),
                Some(broker_offer_present),
                None,
                (!broker_offer_present).then_some("owned_fd_closed_on_rejection"),
                (!broker_offer_present).then_some("broker_offer_missing"),
            );
        }
        if !broker_offer_present {
            drop(fd);
            return;
        }

        let offer_is_current = self.selection_state.offer_is_current(
            broker_offer_id,
            SelectionKind::Clipboard,
            source_generation,
            binding.target_id,
            source_key,
            &mime_type,
        );
        let stale_reason = if offer_is_current {
            None
        } else if self
            .selection_state
            .active_selection(SelectionKind::Clipboard)
            .is_none()
        {
            Some("no_active_selection")
        } else if self
            .selection_state
            .active_selection(SelectionKind::Clipboard)
            .is_some_and(|selection| selection.source_key != source_key)
            || self
                .selection_state
                .active_selection(SelectionKind::Clipboard)
                .is_some_and(|selection| selection.generation != source_generation)
        {
            Some("source_mismatch")
        } else if self
            .selection_state
            .active_selection(SelectionKind::Clipboard)
            .is_some_and(|selection| !selection.mime_types.iter().any(|mime| mime == &mime_type))
        {
            Some("mime_mismatch")
        } else {
            Some("offer_not_current")
        };
        if trace_enabled {
            self.trace_clipboard_offer_validation(
                "offer_is_current_validation",
                client_id,
                Some(&binding.target_client_id),
                offer,
                binding.kind,
                source_generation,
                &mime_type,
                Some(&binding),
                Some(true),
                Some(offer_is_current),
                (!offer_is_current).then_some("owned_fd_closed_on_rejection"),
                stale_reason,
            );
        }
        if !offer_is_current {
            drop(fd);
            return;
        }

        let context = trace_enabled.then(|| ClipboardReceiveTraceContext {
            client_id: client_id.clone(),
            offer_protocol_id: offer.id().protocol_id(),
            broker_offer_id,
            target_device_id: binding.target_id,
            source_key,
            source_generation,
            offer_kind: binding.kind,
        });
        let trace_mime_type = context.as_ref().map(|_| mime_type.clone());
        let result = self.request_selection_data_with_trace(
            SelectionKind::Clipboard,
            source_key,
            mime_type,
            fd,
            context.clone(),
        );
        if let (Some(context), Some(trace_mime_type)) =
            (context.as_ref(), trace_mime_type.as_deref())
        {
            let failure_reason = result.err().map(SelectionDataRequestFailure::reason);
            self.trace_clipboard_context(
                "receive_transfer_result",
                context,
                trace_mime_type,
                ClipboardTraceStatus {
                    send_succeeded: Some(failure_reason.is_none()),
                    reason: failure_reason,
                    ..ClipboardTraceStatus::default()
                },
            );
        }
    }

    fn retire_clipboard_selection_offers(&mut self) {
        let retiring_offers = if self.lifecycle_compatibility_trace.clipboard_trace_enabled() {
            self.data_offers
                .values()
                .filter(|offer| offer.kind == DataOfferKind::Selection)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        for offer in &retiring_offers {
            self.trace_clipboard_offer_snapshot(
                offer,
                "offer_retirement_begin",
                "selection_replaced_or_cleared",
            );
        }
        let mut retired = Vec::new();
        self.data_offers.retain(|_, offer| {
            if offer.kind == DataOfferKind::Selection {
                retired.extend(offer.broker_offer_id);
                false
            } else {
                offer.offer.is_alive()
            }
        });
        for offer_id in retired {
            self.selection_state
                .retire_offer(SelectionKind::Clipboard, offer_id);
        }
        for offer in &retiring_offers {
            self.trace_clipboard_offer_snapshot(
                offer,
                "offer_retired",
                "selection_replaced_or_cleared",
            );
        }
    }
}
