use super::*;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct ClientTeardownSummary {
    pub(in crate::compositor) surfaces_removed: usize,
    pub(in crate::compositor) renderables_removed: usize,
    pub(in crate::compositor) repaint_scheduled: bool,
}

impl CompositorState {
    pub(in crate::compositor) fn note_protocol_error_metric(&mut self) {
        self.compliance_metrics.note_protocol_error();
    }

    pub(in crate::compositor) fn post_protocol_error<I: Resource>(
        &mut self,
        client: &Client,
        resource: &I,
        code: impl Into<u32>,
        message: impl Into<String>,
    ) {
        self.post_protocol_error_with_cleanup(client, resource, code, message, true);
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
        self.post_protocol_error_with_cleanup(client, resource, code, message, false);
    }

    fn post_protocol_error_with_cleanup<I: Resource>(
        &mut self,
        client: &Client,
        resource: &I,
        code: impl Into<u32>,
        message: impl Into<String>,
        cleanup_now: bool,
    ) {
        let client_id = client.id();
        self.note_protocol_error_metric();
        resource.post_error(code, message);
        if cleanup_now {
            self.teardown_client_resources(&client_id);
        }
    }

    pub(in crate::compositor) fn teardown_client_resources(
        &mut self,
        client_id: &ClientId,
    ) -> ClientTeardownSummary {
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
        self.relative_pointer_resources.retain(|relative| {
            !resource_owned_by_client(&relative.resource, client_id)
                && !resource_owned_by_client(&relative.source_pointer, client_id)
        });
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

        let idle_before = self.idle_inhibitor_resources.len();
        self.idle_inhibitor_resources
            .retain(|inhibitor| !resource_owned_by_client(inhibitor, client_id));
        for _ in self.idle_inhibitor_resources.len()..idle_before {
            self.idle_manager.uninhibit();
        }

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
