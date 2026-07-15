use super::*;

#[cfg(test)]
use crate::render_backend::buffer::CommittedSurfaceBuffer;

const DND_COPY: u32 = 1;
const DND_MOVE: u32 = 2;
const DND_ASK: u32 = 4;

fn first_dnd_action(mask: u32) -> u32 {
    [DND_COPY, DND_MOVE, DND_ASK]
        .into_iter()
        .find(|action| mask & action != 0)
        .unwrap_or_default()
}

fn select_dnd_action(source_actions: u32, destination_actions: u32, preferred: u32) -> u32 {
    let common = source_actions & destination_actions;
    if common == 0 {
        return 0;
    }
    if preferred != 0 && common & preferred == preferred {
        preferred
    } else {
        first_dnd_action(common)
    }
}

impl CompositorState {
    #[cfg(test)]
    pub(in crate::compositor) fn test_create_unmapped_surface_resource_at_version(
        &mut self,
        client: &Client,
        handle: &DisplayHandle,
        version: u32,
    ) -> wl_surface::WlSurface {
        let surface = client
            .create_resource::<wl_surface::WlSurface, SurfaceData, CompositorState>(
                handle,
                version,
                SurfaceData::new(0),
            )
            .expect("test surface resource creation");
        let surface_id = compositor_surface_id(&surface);
        self.register_surface_resource(surface_id, surface.clone());
        self.register_surface_client(surface_id, client.id());
        surface
    }

    #[cfg(test)]
    pub(in crate::compositor) fn test_create_surface_resource(
        &mut self,
        client: &Client,
        handle: &DisplayHandle,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
    ) -> wl_surface::WlSurface {
        let surface = client
            .create_resource::<wl_surface::WlSurface, SurfaceData, CompositorState>(
                handle,
                6,
                SurfaceData::new(0),
            )
            .expect("test surface resource creation");
        let surface_id = compositor_surface_id(&surface);
        self.register_surface_resource(surface_id, surface.clone());
        self.register_surface_client(surface_id, client.id());
        self.test_publish_surface(surface_id, width, height, placement);
        surface
    }

    #[cfg(test)]
    pub(in crate::compositor) fn test_create_data_source(
        &mut self,
        client: &Client,
        handle: &DisplayHandle,
    ) -> wl_data_source::WlDataSource {
        let source = client
            .create_resource::<wl_data_source::WlDataSource, DataSourceData, CompositorState>(
                handle,
                3,
                DataSourceData {
                    client_id: client.id(),
                },
            )
            .expect("test data source resource creation");
        self.register_data_source(source.clone(), client.id());
        source
    }

    #[cfg(test)]
    pub(in crate::compositor) fn test_create_data_device(
        &mut self,
        client: &Client,
        handle: &DisplayHandle,
    ) -> wl_data_device::WlDataDevice {
        let device = client
            .create_resource::<wl_data_device::WlDataDevice, DataDeviceData, CompositorState>(
                handle,
                3,
                DataDeviceData {
                    client_id: client.id(),
                    seat_id: ObjectId::null(),
                },
            )
            .expect("test data device resource creation");
        self.register_data_device(device.clone(), client.id(), ObjectId::null());
        device
    }

    #[cfg(test)]
    pub(in crate::compositor) fn test_publish_surface(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
    ) {
        let size = BufferSize::new(width, height).expect("test surface size");
        let identity = self
            .allocate_buffer_identity()
            .expect("test buffer identity");
        self.renderable_surfaces.push(RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width,
            height,
            placement,
            render_placement: None,
            visual_clip: None,
            generation: 0,
            commit_sequence: SurfaceCommitSequence::initial(),
            buffer: CommittedSurfaceBuffer::shm_snapshot(
                identity,
                size,
                vec![0; size.pixel_count().expect("test pixel count")],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::Full,
        });
        self.store_surface_placement(surface_id, placement);
        self.reconcile_all_surface_output_memberships();
    }

    #[cfg(test)]
    pub(in crate::compositor) fn test_resize_surface(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
    ) {
        if let Some(surface) = self
            .renderable_surfaces
            .iter_mut()
            .find(|surface| surface.surface_id == surface_id)
        {
            surface.width = width;
            surface.height = height;
            self.reconcile_all_surface_output_memberships();
        }
    }

    #[cfg(test)]
    pub(in crate::compositor) fn test_map_surface(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        placement: SurfacePlacement,
    ) {
        if self
            .renderable_surfaces
            .iter()
            .any(|surface| surface.surface_id == surface_id)
        {
            return;
        }
        self.test_publish_surface(surface_id, width, height, placement);
    }

    #[cfg(test)]
    pub(in crate::compositor) fn test_unmap_surface(&mut self, surface_id: u32) {
        self.unmap_surface_content(surface_id);
        self.reconcile_all_surface_output_memberships();
    }

    #[cfg(test)]
    pub(in crate::compositor) fn test_destroy_surface_resource(&mut self, surface_id: u32) {
        self.teardown_surface_resource(surface_id, SurfaceTeardownReason::ExplicitDestroy);
    }

    #[cfg(test)]
    pub(in crate::compositor) fn test_set_surface_placement(
        &mut self,
        surface_id: u32,
        placement: SurfacePlacement,
    ) {
        if !self.set_surface_placement(surface_id, placement) {
            self.store_surface_placement(surface_id, placement);
            self.reconcile_all_surface_output_memberships();
        } else {
            self.reconcile_all_surface_output_memberships();
        }
    }

    pub(in crate::compositor) fn destroy_data_offer(&mut self, offer: &wl_data_offer::WlDataOffer) {
        if let Some(binding) = self.data_offers.get_mut(&offer.id()) {
            binding.drag_phase = Some(DragOfferPhase::Destroyed);
        }
        if self.active_drag.as_ref().is_some_and(|drag| {
            drag.offer
                .as_ref()
                .is_some_and(|active_offer| active_offer.id() == offer.id())
        }) {
            self.cancel_drag_session("offer_destroyed");
        }
        self.data_offers.remove(&offer.id());
    }

    pub(in crate::compositor) fn note_dnd_duplicate_terminal_attempt(&mut self) {
        self.compliance_metrics
            .note_dnd_duplicate_terminal_attempt();
    }

    pub(in crate::compositor) fn begin_drag_session(
        &mut self,
        source: Option<wl_data_source::WlDataSource>,
        origin_surface: wl_surface::WlSurface,
        icon_surface: Option<wl_surface::WlSurface>,
        serial: u32,
    ) {
        self.cancel_drag_session("replaced");
        self.compliance_metrics.dnd_last_terminal_phase = None;
        self.compliance_metrics.dnd_sessions_started = self
            .compliance_metrics
            .dnd_sessions_started
            .saturating_add(1);
        self.active_drag = Some(ActiveDrag {
            source,
            origin_surface,
            icon_surface,
            initiating_serial: serial,
            target_surface: None,
            target_client: None,
            offer: None,
            accepted_mime: None,
            selected_action: 0,
            destination_actions: None,
            last_offer_action: None,
            last_source_action: None,
            phase: DragSessionPhase::Dragging,
        });
    }

    pub(in crate::compositor) fn update_drag_target_at(&mut self, x: f64, y: f64) {
        let Some(active) = self.active_drag.as_ref() else {
            return;
        };
        let phase = active.phase;
        let previous_target = active.target_surface.clone();
        let source = active.source.clone();
        let origin_client = active.origin_surface.client().map(|client| client.id());
        if phase != DragSessionPhase::Dragging {
            return;
        }
        let target = self.pointer_target_at(x, y);
        let target_surface = target.as_ref().map(|target| target.surface.clone());
        let unchanged = previous_target
            .as_ref()
            .zip(target_surface.as_ref())
            .is_some_and(|(old, new)| same_surface_resource(old, new));

        if unchanged {
            self.send_drag_motion_to_current_target(target.as_ref());
            return;
        }

        self.leave_drag_target();
        let Some(target) = target else {
            return;
        };
        let Some(target_client) = target.surface.client().map(|client| client.id()) else {
            return;
        };
        if source.is_none() && Some(target_client.clone()) != origin_client {
            // Core source-less drags are private to the initiating client.  Do
            // not manufacture a wl_data_offer or leak enter/motion events to
            // another client under the pointer.
            return;
        }
        let Some(device) = self
            .data_devices
            .iter()
            .find(|binding| binding.client_id == target_client && binding.device.is_alive())
            .map(|binding| binding.device.clone())
        else {
            return;
        };

        let mime_types = source
            .as_ref()
            .and_then(|source| self.data_sources.get(&source.id()))
            .map(|source| source.mime_types.clone())
            .unwrap_or_default();
        let source_actions = source
            .as_ref()
            .and_then(|source| self.data_sources.get(&source.id()))
            .map(|source| source.actions)
            .unwrap_or_default();
        let Some(client) = device.client() else {
            return;
        };
        let Some(handle) = device.handle().upgrade() else {
            return;
        };
        let display = DisplayHandle::from(handle);
        let offer = if source.is_some() {
            let Ok(offer) = client
                .create_resource::<wl_data_offer::WlDataOffer, DataOfferData, CompositorState>(
                    &display,
                    device.version().min(3),
                    DataOfferData {
                        target_client_id: target_client.clone(),
                        source_generation: 0,
                        kind: DataOfferKind::DragAndDrop,
                    },
                )
            else {
                return;
            };

            self.data_offers.insert(
                offer.id(),
                ClipboardDataOffer {
                    offer: offer.clone(),
                    target_client_id: target_client.clone(),
                    source_generation: 0,
                    mime_types: mime_types.clone(),
                    kind: DataOfferKind::DragAndDrop,
                    accepted_mime: None,
                    selected_action: None,
                    drag_phase: Some(DragOfferPhase::Entered),
                    source_actions,
                    destination_actions: None,
                    preferred_action: 0,
                },
            );
            let _ = device.send_event(wl_data_device::Event::DataOffer { id: offer.clone() });
            for mime_type in mime_types {
                let _ = offer.send_event(wl_data_offer::Event::Offer { mime_type });
            }
            if offer.version() >= 3 {
                let _ = offer.send_event(wl_data_offer::Event::SourceActions {
                    source_actions: WEnum::Unknown(source_actions),
                });
            }
            Some(offer)
        } else {
            None
        };
        let serial = self.next_configure_serial();
        let _ = device.send_event(wl_data_device::Event::Enter {
            serial,
            surface: target.surface.clone(),
            x: target.surface_x,
            y: target.surface_y,
            id: offer.clone(),
        });
        if let Some(active) = self.active_drag.as_mut() {
            active.target_surface = Some(target.surface.clone());
            active.target_client = Some(target_client);
            active.offer = offer;
            active.selected_action = 0;
            active.destination_actions = None;
            active.last_offer_action = None;
            active.last_source_action = None;
        }
    }

    fn send_drag_motion_to_current_target(&mut self, target: Option<&PointerTarget>) {
        let Some(target) = target else {
            return;
        };
        let Some(active) = self.active_drag.as_ref() else {
            return;
        };
        let Some(client_id) = active.target_client.as_ref() else {
            return;
        };
        let Some(device) = self
            .data_devices
            .iter()
            .find(|binding| &binding.client_id == client_id && binding.device.is_alive())
            .map(|binding| binding.device.clone())
        else {
            return;
        };
        let _ = device.send_event(wl_data_device::Event::Motion {
            time: wayland_event_time(),
            x: target.surface_x,
            y: target.surface_y,
        });
    }

    pub(in crate::compositor) fn leave_drag_target(&mut self) {
        let Some(active) = self.active_drag.as_mut() else {
            return;
        };
        let Some(client_id) = active.target_client.take() else {
            active.target_surface = None;
            active.offer = None;
            return;
        };
        if let Some(device) = self
            .data_devices
            .iter()
            .find(|binding| binding.client_id == client_id && binding.device.is_alive())
            .map(|binding| binding.device.clone())
        {
            let _ = device.send_event(wl_data_device::Event::Leave);
        }
        if let Some(offer) = active.offer.take() {
            self.data_offers.remove(&offer.id());
        }
        if let Some(source) = active.source.as_ref()
            && source.is_alive()
        {
            let _ = source.send_event(wl_data_source::Event::Target { mime_type: None });
        }
        active.target_surface = None;
        active.accepted_mime = None;
        active.selected_action = 0;
        active.destination_actions = None;
        active.last_offer_action = None;
        active.last_source_action = None;
    }

    pub(in crate::compositor) fn send_drag_action_if_changed(&mut self) {
        let Some(active) = self.active_drag.as_ref() else {
            return;
        };
        if active.phase != DragSessionPhase::Dragging || active.destination_actions.is_none() {
            return;
        }
        let action = active.selected_action;
        let offer = active.offer.clone();
        let source = active.source.clone();
        let send_offer = offer
            .as_ref()
            .is_some_and(|offer| offer.version() >= 3 && active.last_offer_action != Some(action));
        let send_source = source.as_ref().is_some_and(|source| {
            source.version() >= 3 && active.last_source_action != Some(action)
        });
        if let Some(active) = self.active_drag.as_mut() {
            if send_offer {
                active.last_offer_action = Some(action);
            }
            if send_source {
                active.last_source_action = Some(action);
            }
        }
        if send_offer && let Some(offer) = offer {
            let _ = offer.send_event(wl_data_offer::Event::Action {
                dnd_action: WEnum::Unknown(action),
            });
            self.compliance_metrics.dnd_offer_action_events = self
                .compliance_metrics
                .dnd_offer_action_events
                .saturating_add(1);
        }
        if send_source && let Some(source) = source {
            let _ = source.send_event(wl_data_source::Event::Action {
                dnd_action: WEnum::Unknown(action),
            });
            self.compliance_metrics.dnd_source_action_events = self
                .compliance_metrics
                .dnd_source_action_events
                .saturating_add(1);
        }
    }

    pub(in crate::compositor) fn update_drag_acceptance(
        &mut self,
        offer: &wl_data_offer::WlDataOffer,
        mime_type: Option<String>,
    ) {
        let Some(active) = self.active_drag.as_mut() else {
            return;
        };
        if active
            .offer
            .as_ref()
            .is_none_or(|current| !same_wayland_resource(current, offer))
        {
            return;
        }
        active.accepted_mime = mime_type.clone();
        if let Some(source) = active.source.as_ref()
            && source.is_alive()
        {
            let _ = source.send_event(wl_data_source::Event::Target { mime_type });
        }
    }

    pub(in crate::compositor) fn update_drag_actions(
        &mut self,
        offer: &wl_data_offer::WlDataOffer,
        destination_actions: u32,
        preferred_action: u32,
    ) {
        let Some(binding) = self.data_offers.get_mut(&offer.id()) else {
            return;
        };
        binding.preferred_action = preferred_action;
        binding.destination_actions = Some(destination_actions);
        let selected = select_dnd_action(
            binding.source_actions,
            destination_actions,
            preferred_action,
        );
        binding.selected_action = (selected != 0).then_some(selected);
        if let Some(active) = self.active_drag.as_mut()
            && active
                .offer
                .as_ref()
                .is_some_and(|current| same_wayland_resource(current, offer))
        {
            active.selected_action = selected;
            active.destination_actions = Some(destination_actions);
        }
        self.send_drag_action_if_changed();
    }

    pub(in crate::compositor) fn source_drag_actions_changed(
        &mut self,
        source: &wl_data_source::WlDataSource,
        actions: u32,
    ) {
        let Some(active) = self.active_drag.as_ref() else {
            return;
        };
        if active
            .source
            .as_ref()
            .is_none_or(|current| !same_wayland_resource(current, source))
        {
            return;
        }
        let offer = active.offer.clone();
        if let Some(offer) = offer.as_ref()
            && let Some(binding) = self.data_offers.get_mut(&offer.id())
        {
            binding.source_actions = actions;
            let selected = if let Some(destination_actions) = binding.destination_actions {
                select_dnd_action(actions, destination_actions, binding.preferred_action)
            } else {
                0
            };
            binding.selected_action = (selected != 0).then_some(selected);
            if let Some(active) = self.active_drag.as_mut() {
                active.selected_action = selected;
            }
        }
        if let Some(offer) = offer
            && offer.version() >= 3
        {
            let _ = offer.send_event(wl_data_offer::Event::SourceActions {
                source_actions: WEnum::Unknown(actions),
            });
        }
        self.send_drag_action_if_changed();
    }

    pub(in crate::compositor) fn drop_active_drag(&mut self) {
        if self
            .active_drag
            .as_ref()
            .is_some_and(|active| active.phase != DragSessionPhase::Dragging)
        {
            self.note_dnd_duplicate_terminal_attempt();
            return;
        }
        let Some(active) = self.active_drag.as_mut() else {
            return;
        };
        let Some(client_id) = active.target_client.clone() else {
            self.cancel_drag_session("drop_without_target");
            return;
        };
        let Some(device) = self
            .data_devices
            .iter()
            .find(|binding| binding.client_id == client_id && binding.device.is_alive())
            .map(|binding| binding.device.clone())
        else {
            self.cancel_drag_session("target_device_gone");
            return;
        };
        if active.phase != DragSessionPhase::Dragging {
            return;
        }
        if active.source.is_none() {
            let _ = device.send_event(wl_data_device::Event::Drop);
            active.phase = DragSessionPhase::Finished;
            self.compliance_metrics.dnd_last_terminal_phase = Some(DragSessionPhase::Finished);
            self.complete_drag_session(false);
            self.compliance_metrics.dnd_sessions_finished = self
                .compliance_metrics
                .dnd_sessions_finished
                .saturating_add(1);
            return;
        }
        if active.accepted_mime.is_none() || active.selected_action == 0 {
            self.cancel_drag_session("drop_not_accepted");
            return;
        }
        let _ = device.send_event(wl_data_device::Event::Drop);
        if let Some(source) = active.source.as_ref()
            && source.version() >= 3
            && source.is_alive()
        {
            let _ = source.send_event(wl_data_source::Event::DndDropPerformed);
        }
        if let Some(offer) = active.offer.as_ref()
            && let Some(binding) = self.data_offers.get_mut(&offer.id())
        {
            binding.drag_phase = Some(DragOfferPhase::Dropped);
        }
        active.phase = if active.selected_action == DND_ASK {
            DragSessionPhase::DroppedAwaitingAskResolution
        } else {
            DragSessionPhase::DroppedAwaitingFinish
        };
    }

    pub(in crate::compositor) fn finish_drag_offer(
        &mut self,
        offer: &wl_data_offer::WlDataOffer,
    ) -> bool {
        if self.active_drag.as_ref().is_some_and(|active| {
            active
                .offer
                .as_ref()
                .is_some_and(|current| same_wayland_resource(current, offer))
                && matches!(
                    active.phase,
                    DragSessionPhase::Finished | DragSessionPhase::Cancelled
                )
        }) {
            self.note_dnd_duplicate_terminal_attempt();
            return false;
        }
        let Some(active) = self.active_drag.as_ref() else {
            return false;
        };
        if active
            .offer
            .as_ref()
            .is_none_or(|current| !same_wayland_resource(current, offer))
            || !matches!(
                active.phase,
                DragSessionPhase::DroppedAwaitingFinish
                    | DragSessionPhase::DroppedAwaitingAskResolution
            )
        {
            return false;
        }
        if active.phase == DragSessionPhase::DroppedAwaitingAskResolution
            && !matches!(active.selected_action, DND_COPY | DND_MOVE)
        {
            return false;
        }
        let ask_resolution = active.phase == DragSessionPhase::DroppedAwaitingAskResolution;
        let final_action = active.selected_action;
        if let Some(source) = active.source.as_ref()
            && source.version() >= 3
            && source.is_alive()
        {
            if ask_resolution {
                let _ = source.send_event(wl_data_source::Event::Action {
                    dnd_action: WEnum::Unknown(final_action),
                });
                self.compliance_metrics.dnd_source_action_events = self
                    .compliance_metrics
                    .dnd_source_action_events
                    .saturating_add(1);
            }
            if source
                .send_event(wl_data_source::Event::DndFinished)
                .is_ok()
            {
                self.compliance_metrics.dnd_source_finished_events = self
                    .compliance_metrics
                    .dnd_source_finished_events
                    .saturating_add(1);
            }
        }
        if let Some(binding) = self.data_offers.get_mut(&offer.id()) {
            binding.drag_phase = Some(DragOfferPhase::Finished);
        }
        if let Some(active) = self.active_drag.as_mut() {
            active.phase = DragSessionPhase::Finished;
        }
        self.compliance_metrics.dnd_last_terminal_phase = Some(DragSessionPhase::Finished);
        self.complete_drag_session(false);
        self.compliance_metrics.dnd_sessions_finished = self
            .compliance_metrics
            .dnd_sessions_finished
            .saturating_add(1);
        true
    }

    pub(in crate::compositor) fn cancel_drag_for_source(
        &mut self,
        source: &wl_data_source::WlDataSource,
    ) {
        if self.active_drag.as_ref().is_some_and(|active| {
            active
                .source
                .as_ref()
                .is_some_and(|current| same_wayland_resource(current, source))
        }) {
            self.cancel_drag_session("source_destroyed");
        }
    }

    pub(in crate::compositor) fn cancel_drag_session(&mut self, _reason: &'static str) {
        let Some(active) = self.active_drag.as_ref() else {
            if _reason == "explicit_cancel"
                && self.compliance_metrics.dnd_last_terminal_phase.is_some()
            {
                self.note_dnd_duplicate_terminal_attempt();
            }
            return;
        };
        if matches!(
            active.phase,
            DragSessionPhase::Finished | DragSessionPhase::Cancelled
        ) {
            self.note_dnd_duplicate_terminal_attempt();
            return;
        }
        if let Some(source) = active.source.as_ref()
            && source.is_alive()
            && active.phase != DragSessionPhase::Finished
            && source.send_event(wl_data_source::Event::Cancelled).is_ok()
        {
            self.compliance_metrics.dnd_source_cancelled_events = self
                .compliance_metrics
                .dnd_source_cancelled_events
                .saturating_add(1);
        }
        if let Some(active) = self.active_drag.as_mut() {
            active.phase = DragSessionPhase::Cancelled;
        }
        self.compliance_metrics.dnd_last_terminal_phase = Some(DragSessionPhase::Cancelled);
        self.leave_drag_target();
        self.complete_drag_session(true);
        self.compliance_metrics.dnd_sessions_cancelled = self
            .compliance_metrics
            .dnd_sessions_cancelled
            .saturating_add(1);
    }

    fn complete_drag_session(&mut self, remove_offer: bool) {
        let Some(active) = self.active_drag.take() else {
            return;
        };
        if let Some(icon) = active.icon_surface {
            self.deactivate_role_instance(compositor_surface_id(&icon));
        }
        if remove_offer && let Some(offer) = active.offer {
            self.data_offers.remove(&offer.id());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dnd_action_selection_waits_for_destination_actions() {
        assert_eq!(select_dnd_action(DND_COPY | DND_MOVE, 0, DND_COPY), 0);
        assert_eq!(
            select_dnd_action(DND_COPY | DND_MOVE, DND_MOVE, DND_MOVE),
            DND_MOVE
        );
        assert_eq!(
            select_dnd_action(DND_COPY | DND_MOVE, DND_MOVE, DND_COPY),
            DND_MOVE
        );
        assert_eq!(select_dnd_action(DND_COPY, DND_MOVE, DND_COPY), 0);
    }
}
