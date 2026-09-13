use super::*;

#[allow(clippy::mutable_key_type)]
pub(in crate::compositor) fn post_fatal_protocol_error<I: Resource>(
    metrics: &mut CoreComplianceMetrics,
    trace: &mut ProtocolErrorTrace,
    terminal_client_ids: &mut HashSet<ClientId>,
    resource: &I,
    code: impl Into<u32>,
    message: impl Into<String>,
    surface_id: Option<u32>,
    category: ProtocolErrorCategory,
    xwayland_generation: Option<u64>,
) -> bool {
    let code = code.into();
    let message = message.into();
    let Some(client) = resource.client() else {
        metrics.note_protocol_error();
        trace.record(ProtocolErrorRecord {
            timestamp_ns: protocol_error_timestamp_ns(),
            client_id: None,
            peer_pid: None,
            interface: ProtocolErrorInterface::for_resource::<I>(),
            resource_id: Some(resource.id().protocol_id()),
            error_code: Some(code),
            surface_id,
            xwayland_generation,
            category: ProtocolErrorCategory::Unavailable,
        });
        return false;
    };
    let client_id = client.id();
    terminal_client_ids.insert(client_id);
    metrics.note_protocol_error();
    trace.record(ProtocolErrorRecord {
        timestamp_ns: protocol_error_timestamp_ns(),
        client_id: Some(client.id()),
        peer_pid: None,
        interface: ProtocolErrorInterface::for_resource::<I>(),
        resource_id: Some(resource.id().protocol_id()),
        error_code: Some(code),
        surface_id,
        xwayland_generation,
        category,
    });
    // This is the only raw fatal emitter in compositor source. The owning client
    // is terminal and diagnostics are recorded before the wire is killed.
    resource.post_error(code, message);
    true
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct ClientTeardownSummary {
    pub(in crate::compositor) surfaces_removed: usize,
    pub(in crate::compositor) renderables_removed: usize,
    pub(in crate::compositor) repaint_scheduled: bool,
}

#[derive(Debug)]
pub(in crate::compositor) struct PendingClientResourceExhaustion {
    pub(in crate::compositor) client: Client,
    pub(in crate::compositor) _evidence_surface_id: u32,
}

impl CompositorState {
    pub(in crate::compositor) fn request_client_resource_exhaustion(
        &mut self,
        surface_id: u32,
    ) -> bool {
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            debug_assert!(
                false,
                "resource exhaustion requested without a live surface resource"
            );
            return false;
        };
        let Some(client) = surface.client() else {
            debug_assert!(
                false,
                "resource exhaustion surface has no owning client resource"
            );
            return false;
        };
        let client_id = client.id();
        debug_assert_eq!(
            self.surface_client_ids.get(&surface_id),
            Some(&client_id),
            "surface ownership maps disagree while scheduling resource exhaustion"
        );
        if !self
            .pending_client_resource_exhaustion_clients
            .insert(client_id)
        {
            return false;
        }
        self.pending_client_resource_exhaustions
            .push(PendingClientResourceExhaustion {
                client,
                _evidence_surface_id: surface_id,
            });
        self.surface_pacing_metrics.queue_resource_exhaustions = self
            .surface_pacing_metrics
            .queue_resource_exhaustions
            .saturating_add(1);
        true
    }

    pub(in crate::compositor) fn take_client_resource_exhaustions(
        &mut self,
    ) -> Vec<PendingClientResourceExhaustion> {
        self.pending_client_resource_exhaustion_clients.clear();
        std::mem::take(&mut self.pending_client_resource_exhaustions)
    }

    pub(in crate::compositor) fn client_resource_exhaustion_pending(
        &self,
        client_id: &ClientId,
    ) -> bool {
        self.pending_client_resource_exhaustion_clients
            .contains(client_id)
    }

    pub(in crate::compositor) fn note_protocol_error_metric(&mut self) {
        self.compliance_metrics.note_protocol_error();
        // Some generated or internal protocol paths do not retain the client/resource
        // that caused the error. Keep the counter and recorder coupled, while marking
        // those records explicitly unavailable instead of silently dropping them.
        self.protocol_error_trace.record(ProtocolErrorRecord {
            timestamp_ns: protocol_error_timestamp_ns(),
            client_id: None,
            peer_pid: None,
            interface: ProtocolErrorInterface::Other,
            resource_id: None,
            error_code: None,
            surface_id: None,
            xwayland_generation: None,
            category: ProtocolErrorCategory::Unavailable,
        });
    }

    pub(in crate::compositor) fn mark_client_terminal(&mut self, client_id: ClientId) {
        self.terminal_client_ids.insert(client_id);
    }

    pub(in crate::compositor) fn post_protocol_error<I: Resource>(
        &mut self,
        client: &Client,
        resource: &I,
        code: impl Into<u32>,
        message: impl Into<String>,
    ) {
        self.post_protocol_error_with_cleanup_and_details(
            client,
            resource,
            code,
            message,
            None,
            ProtocolErrorCategory::Wire,
            true,
        );
    }

    pub(in crate::compositor) fn post_protocol_error_deferred<I: Resource>(
        &mut self,
        client: &Client,
        resource: &I,
        code: impl Into<u32>,
        message: impl Into<String>,
    ) {
        // Pointer-constraint dispatch may have queued a valid earlier request in the
        // same wire batch. Preserve that request's backend ordering; normal client
        // disconnect teardown remains the terminal cleanup authority.
        self.post_protocol_error_with_cleanup_and_details(
            client,
            resource,
            code,
            message,
            None,
            ProtocolErrorCategory::Wire,
            false,
        );
    }

    pub(in crate::compositor) fn post_protocol_error_deferred_with_details<I: Resource>(
        &mut self,
        client: &Client,
        resource: &I,
        code: impl Into<u32>,
        message: impl Into<String>,
        surface_id: Option<u32>,
        category: ProtocolErrorCategory,
    ) {
        self.post_protocol_error_with_cleanup_and_details(
            client, resource, code, message, surface_id, category, false,
        );
    }

    fn post_protocol_error_with_cleanup_and_details<I: Resource>(
        &mut self,
        client: &Client,
        resource: &I,
        code: impl Into<u32>,
        message: impl Into<String>,
        surface_id: Option<u32>,
        category: ProtocolErrorCategory,
        cleanup_now: bool,
    ) {
        let client_id = client.id();
        // wayland-server kills the wire from inside post_error, while Typhon
        // drains the resulting disconnected-client notification later in the
        // dispatch cycle. Make that interval non-publishable immediately.
        let _ = post_fatal_protocol_error(
            &mut self.compliance_metrics,
            &mut self.protocol_error_trace,
            &mut self.terminal_client_ids,
            resource,
            code,
            message,
            surface_id,
            category,
            self.xwayland
                .client_identity
                .as_ref()
                .map(|identity| identity.generation.get()),
        );
        if cleanup_now {
            self.teardown_client_resources(&client_id);
        }
    }

    pub(in crate::compositor) fn teardown_client_resources(
        &mut self,
        client_id: &ClientId,
    ) -> ClientTeardownSummary {
        self.remove_workspace_client(client_id);
        let renderables_before = self.renderable_surfaces.len();
        let surfaces_removed = self.teardown_surfaces_for_client(client_id);
        self.teardown_non_surface_resources_for_client(client_id);
        self.scrub_dead_buffer_releases();
        self.audit_dnd_resource_ownership();
        let leaks = self.count_client_state_leaks(client_id);
        if leaks != 0 {
            self.compliance_metrics.client_state_leaks_detected = self
                .compliance_metrics
                .client_state_leaks_detected
                .saturating_add(leaks as u64);
            eprintln!(
                "oblivion-one compliance: client_state_leaks_detected client={client_id:?} count={leaks}"
            );
        }
        let renderables_removed = renderables_before.saturating_sub(self.renderable_surfaces.len());

        self.terminal_client_ids.remove(client_id);

        ClientTeardownSummary {
            surfaces_removed,
            renderables_removed,
            repaint_scheduled: renderables_removed > 0,
        }
    }

    fn teardown_surfaces_for_client(&mut self, client_id: &ClientId) -> usize {
        let mut surface_ids = self
            .surface_client_ids
            .iter()
            .filter_map(|(surface_id, owner)| (owner == client_id).then_some(*surface_id))
            .collect::<Vec<_>>();
        surface_ids.sort_unstable();
        surface_ids.dedup();

        let mut removed = 0usize;
        for surface_id in surface_ids {
            let result = self
                .teardown_surface_resource(surface_id, SurfaceTeardownReason::ClientDisconnected);
            if result.removed_resource || result.removed_renderables > 0 {
                removed = removed.saturating_add(1);
            }
        }
        removed
    }

    fn teardown_non_surface_resources_for_client(&mut self, client_id: &ClientId) {
        self.remove_keyboard_shortcut_inhibitors_for_client(client_id);
        self.remove_astrea_toplevel_client(client_id);
        if self.active_drag.as_ref().is_some_and(|drag| {
            drag.target_client.as_ref() == Some(client_id)
                || drag
                    .source
                    .as_ref()
                    .and_then(|source| source.client())
                    .is_some_and(|client| client.id() == *client_id)
        }) {
            self.cancel_drag_session("client_disconnected");
        }
        let pointers = self
            .pointer_resources
            .iter()
            .filter(|pointer| resource_owned_by_client(*pointer, client_id))
            .cloned()
            .collect::<Vec<_>>();
        for pointer in pointers {
            self.unregister_pointer(&pointer);
        }

        self.keyboard_resources
            .retain(|keyboard| !resource_owned_by_client(keyboard, client_id));
        let before_relative_resources = self.relative_pointer_resources.len();
        self.relative_pointer_resources.retain(|relative| {
            !resource_owned_by_client(&relative.resource, client_id)
                && !resource_owned_by_client(&relative.source_pointer, client_id)
        });
        if self.relative_pointer_resources.len() != before_relative_resources {
            self.advance_relative_pointer_resources_generation();
        }
        let outputs = self
            .output_resources
            .iter()
            .filter(|output| resource_owned_by_client(*output, client_id))
            .cloned()
            .collect::<Vec<_>>();
        for output in outputs {
            self.unregister_output_resource(&output);
        }
        self.data_devices
            .retain(|device| device.client_id != *client_id);
        self.data_offers.retain(|_, offer| {
            offer.target_client_id != *client_id
                && !resource_owned_by_client(&offer.offer, client_id)
        });
        self.activation_tokens
            .retain(|_, token| token.client_id != *client_id);
        self.pending_activation_tokens
            .retain(|_, token| token.client_id != *client_id);

        let sources = self
            .data_sources
            .values()
            .filter(|source| source.client_id == *client_id)
            .map(|source| source.source.clone())
            .collect::<Vec<_>>();
        for source in sources {
            self.remove_data_source(&source);
        }

        self.primary_devices
            .retain(|device| device.client_id != *client_id);
        self.primary_offers.retain(|_, offer| {
            offer.target_client_id != *client_id
                && !resource_owned_by_client(&offer.offer, client_id)
        });
        let primary_sources = self
            .primary_sources
            .values()
            .filter(|source| source.client_id == *client_id)
            .map(|source| source.source.clone())
            .collect::<Vec<_>>();
        for source in primary_sources {
            self.remove_primary_source(&source);
        }

        self.data_control_devices
            .retain(|device| device.client_id != *client_id);
        self.data_control_offers.retain(|_, offer| {
            offer.target_client_id != *client_id
                && !resource_owned_by_client(&offer.offer, client_id)
        });
        let data_control_sources = self
            .data_control_sources
            .values()
            .filter(|source| source.client_id == *client_id)
            .map(|source| source.source.clone())
            .collect::<Vec<_>>();
        for source in data_control_sources {
            self.remove_data_control_source(&source);
        }

        self.idle_inhibitor_resources
            .retain(|inhibitor| inhibitor.client_id != *client_id);
        self.reconcile_idle_inhibition();

        self.recent_input_serials
            .retain(|input| !resource_owned_by_client(&input.surface, client_id));
        self.pointer_enter_serials
            .retain(|entry| !resource_owned_by_client(&entry.surface, client_id));
        self.pointer_entered_surfaces
            .retain(|(_, surface)| !resource_owned_by_client(surface, client_id));

        if self
            .focused_surface
            .as_ref()
            .is_some_and(|surface| resource_owned_by_client(surface, client_id))
        {
            self.focused_surface = None;
            self.focused_window_id = None;
        }
        if self
            .keyboard_surface
            .as_ref()
            .is_some_and(|surface| resource_owned_by_client(surface, client_id))
        {
            self.keyboard_surface = None;
        }
        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| resource_owned_by_client(surface, client_id))
        {
            self.clear_pointer_focus();
        }
        if self
            .last_application_keyboard_focus
            .as_ref()
            .is_some_and(|surface| resource_owned_by_client(surface, client_id))
        {
            self.last_application_keyboard_focus = None;
        }

        debug_assert!(self.check_surface_output_membership_invariants());
    }

    fn count_client_state_leaks(&self, client_id: &ClientId) -> usize {
        let mut leaks = 0usize;
        leaks += self
            .surface_resources
            .values()
            .filter(|resource| resource_owned_by_client(*resource, client_id))
            .count();
        leaks += self
            .surface_client_ids
            .values()
            .filter(|owner| *owner == client_id)
            .count();
        leaks += self
            .surface_output_memberships
            .keys()
            .filter(|surface_id| {
                self.surface_client_ids
                    .get(surface_id)
                    .is_none_or(|owner| owner == client_id)
            })
            .count();
        leaks += self
            .output_resources
            .iter()
            .filter(|resource| resource_owned_by_client(*resource, client_id))
            .count();
        leaks += self
            .pointer_resources
            .iter()
            .filter(|resource| resource_owned_by_client(*resource, client_id))
            .count();
        leaks += self
            .keyboard_resources
            .iter()
            .filter(|resource| resource_owned_by_client(*resource, client_id))
            .count();
        leaks += self
            .relative_pointer_resources
            .iter()
            .filter(|resource| {
                resource_owned_by_client(&resource.resource, client_id)
                    || resource_owned_by_client(&resource.source_pointer, client_id)
            })
            .count();
        leaks += self
            .data_devices
            .iter()
            .filter(|device| device.client_id == *client_id)
            .count();
        leaks += self
            .data_offers
            .values()
            .filter(|offer| {
                offer.target_client_id == *client_id
                    || resource_owned_by_client(&offer.offer, client_id)
            })
            .count();
        leaks += self
            .data_sources
            .values()
            .filter(|source| source.client_id == *client_id)
            .count();
        leaks += self
            .activation_tokens
            .values()
            .filter(|token| token.client_id == *client_id)
            .count();
        leaks += self
            .pending_activation_tokens
            .values()
            .filter(|token| token.client_id == *client_id)
            .count();
        leaks += self
            .recent_input_serials
            .iter()
            .filter(|serial| resource_owned_by_client(&serial.surface, client_id))
            .count();
        leaks += self
            .pointer_enter_serials
            .iter()
            .filter(|serial| resource_owned_by_client(&serial.surface, client_id))
            .count();
        if self.active_drag.as_ref().is_some_and(|drag| {
            drag.target_client.as_ref() == Some(client_id)
                || drag
                    .source
                    .as_ref()
                    .and_then(|source| source.client())
                    .is_some_and(|client| client.id() == *client_id)
                || resource_owned_by_client(&drag.origin_surface, client_id)
                || drag
                    .icon_surface
                    .as_ref()
                    .is_some_and(|surface| resource_owned_by_client(surface, client_id))
        }) {
            leaks += 1;
        }
        leaks
    }

    fn audit_dnd_resource_ownership(&mut self) {
        let active_offer_id = self
            .active_drag
            .as_ref()
            .and_then(|drag| drag.offer.as_ref().map(|offer| offer.id()));
        let orphaned_offer_ids = self
            .data_offers
            .iter()
            .filter_map(|(id, offer)| {
                if offer.kind != DataOfferKind::DragAndDrop
                    || !matches!(
                        offer.drag_phase,
                        Some(DragOfferPhase::Entered | DragOfferPhase::Dropped)
                    )
                    || active_offer_id.as_ref() == Some(id)
                {
                    return None;
                }
                Some(id.clone())
            })
            .collect::<Vec<_>>();
        if orphaned_offer_ids.is_empty() {
            return;
        }
        self.compliance_metrics.dnd_orphaned_resources_detected = self
            .compliance_metrics
            .dnd_orphaned_resources_detected
            .saturating_add(orphaned_offer_ids.len() as u64);
        for offer_id in orphaned_offer_ids {
            self.data_offers.remove(&offer_id);
        }
    }
}
