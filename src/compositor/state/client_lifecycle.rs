use super::*;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) struct ClientTeardownSummary {
    pub(in crate::compositor) surfaces_removed: usize,
    pub(in crate::compositor) renderables_removed: usize,
    pub(in crate::compositor) repaint_scheduled: bool,
}

impl CompositorState {
    pub(in crate::compositor) fn teardown_client_resources(
        &mut self,
        client_id: &ClientId,
    ) -> ClientTeardownSummary {
        let renderables_before = self.renderable_surfaces.len();
        let surfaces_removed = self.teardown_surfaces_for_client(client_id);
        self.teardown_non_surface_resources_for_client(client_id);
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
        self.output_resources
            .retain(|output| !resource_owned_by_client(output, client_id));
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
    }
}
