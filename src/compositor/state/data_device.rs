use super::*;
use crate::xwayland::CanonicalDndSessionId;

#[cfg(test)]
use crate::render_backend::buffer::CommittedSurfaceBuffer;

pub(super) const DND_COPY: u32 = 1;
pub(super) const DND_MOVE: u32 = 2;
pub(super) const DND_ASK: u32 = 4;
pub(super) const MAX_PENDING_XWAYLAND_DND_DATA_REQUESTS: usize = 64;

pub(super) fn xdnd_action_from_wayland_mask(
    mask: u32,
) -> Option<crate::xwayland::XwaylandDndAction> {
    match mask {
        DND_COPY => Some(crate::xwayland::XwaylandDndAction::Copy),
        DND_MOVE => Some(crate::xwayland::XwaylandDndAction::Move),
        DND_ASK => Some(crate::xwayland::XwaylandDndAction::Ask),
        _ => None,
    }
}

fn xdnd_actions_from_wayland_mask(mask: u32) -> Vec<crate::xwayland::XwaylandDndAction> {
    [
        (DND_COPY, crate::xwayland::XwaylandDndAction::Copy),
        (DND_MOVE, crate::xwayland::XwaylandDndAction::Move),
        (DND_ASK, crate::xwayland::XwaylandDndAction::Ask),
    ]
    .into_iter()
    .filter_map(|(bit, action)| (mask & bit != 0).then_some(action))
    .collect()
}

fn first_dnd_action(mask: u32) -> u32 {
    [DND_COPY, DND_MOVE, DND_ASK]
        .into_iter()
        .find(|action| mask & action != 0)
        .unwrap_or_default()
}

pub(super) fn select_dnd_action(
    source_actions: u32,
    destination_actions: u32,
    preferred: u32,
) -> u32 {
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
                SurfaceData::new(self.allocate_surface_id()),
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
                SurfaceData::new(self.allocate_surface_id()),
            )
            .expect("test surface resource creation");
        let surface_id = compositor_surface_id(&surface);
        self.register_surface_resource(surface_id, surface.clone());
        self.register_surface_client(surface_id, client.id());
        self.test_publish_surface(surface_id, width, height, placement);
        self.refresh_active_scene_surface(surface_id);
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
        self.append_renderable_surface(RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width,
            height,
            placement,
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
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
            drag.target
                .as_ref()
                .and_then(ActiveDragTarget::wayland_offer)
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
        let Some(session_serial) = self.next_dnd_session_serial.checked_add(1) else {
            return;
        };
        let Some(session_serial) = std::num::NonZeroU64::new(session_serial) else {
            return;
        };
        let origin = match source {
            Some(source) => ActiveDragOrigin::WaylandSource {
                source,
                origin_surface,
                initiating_serial: serial,
            },
            None => {
                let Some(initiating_client) = origin_surface.client().map(|client| client.id())
                else {
                    return;
                };
                ActiveDragOrigin::WaylandSourceless {
                    initiating_client,
                    origin_surface,
                    initiating_serial: serial,
                }
            }
        };
        self.next_dnd_session_serial = session_serial.get();
        self.begin_drag_with_origin(
            CanonicalDndSessionId::Wayland(session_serial),
            origin,
            DragLifecycleDriver::WaylandImplicitPointerGrab,
            icon_surface,
        );
    }

    pub(super) fn begin_drag_with_origin(
        &mut self,
        id: CanonicalDndSessionId,
        origin: ActiveDragOrigin,
        lifecycle_driver: DragLifecycleDriver,
        icon_surface: Option<wl_surface::WlSurface>,
    ) {
        self.cancel_drag_session("replaced");
        self.compliance_metrics.dnd_last_terminal_phase = None;
        self.compliance_metrics.dnd_sessions_started = self
            .compliance_metrics
            .dnd_sessions_started
            .saturating_add(1);
        self.active_drag = Some(ActiveDrag {
            id,
            xwayland_dnd_generation: origin.xwayland_offer().map(|offer| offer.id().generation()),
            origin,
            lifecycle_driver,
            icon_surface,
            target: None,
            accepted_mime: None,
            target_action: None,
            selected_action: 0,
            destination_actions: None,
            last_offer_action: None,
            last_source_action: None,
            phase: DragSessionPhase::Dragging,
        });
    }

    pub(super) fn drag_source_mime_types(&self, origin: &ActiveDragOrigin) -> Vec<String> {
        match origin {
            ActiveDragOrigin::WaylandSource { source, .. } => self
                .data_sources
                .get(&source.id())
                .map(|source| source.mime_types.clone())
                .unwrap_or_default(),
            ActiveDragOrigin::Xwayland { offer } => offer.mime_types().as_slice().to_vec(),
            ActiveDragOrigin::WaylandSourceless { .. } => Vec::new(),
        }
    }

    pub(super) fn drag_source_actions(&self, origin: &ActiveDragOrigin) -> u32 {
        match origin {
            ActiveDragOrigin::WaylandSource { source, .. } => self
                .data_sources
                .get(&source.id())
                .map(|source| source.actions)
                .unwrap_or_default(),
            ActiveDragOrigin::Xwayland { offer } => offer.wayland_source_actions_mask(),
            ActiveDragOrigin::WaylandSourceless { .. } => 0,
        }
    }

    pub(in crate::compositor) fn update_drag_target_at(&mut self, x: f64, y: f64) {
        let Some(active) = self.active_drag.as_ref() else {
            return;
        };
        if active.phase != DragSessionPhase::Dragging {
            return;
        }
        let session_id = active.id;
        let origin = active.origin.clone();
        let previous_target = active.target.clone();
        let target = self.pointer_target_at(x, y);

        if let Some(target) = target.as_ref() {
            let surface_id = compositor_surface_id(&target.surface);
            let root_surface_id = self.root_surface_id_for_surface(surface_id);
            let x11_window = self
                .window_id_for_surface(root_surface_id)
                .and_then(|window_id| self.window(window_id))
                .and_then(|window| match window.backend {
                    WindowBackend::X11(handle) => Some(handle),
                    WindowBackend::Xdg(_) => None,
                })
                .filter(|handle| {
                    self.xwayland
                        .client_identity
                        .as_ref()
                        .is_some_and(|identity| identity.generation == handle.generation())
                });
            if let Some(window) = x11_window {
                if previous_target.as_ref().is_some_and(|previous| {
                    matches!(previous, ActiveDragTarget::Xwayland { window: current } if *current == window)
                }) {
                    self.queue_xwayland_dnd_position(session_id, window, x, y);
                    return;
                }
                self.leave_drag_target();
                if !self.active_drag.as_ref().is_some_and(|active| {
                    active.id == session_id && active.phase == DragSessionPhase::Dragging
                }) {
                    return;
                }
                if origin.is_wayland_sourceless()
                    && target.surface.client().map(|client| client.id())
                        != origin.wayland_initiating_client()
                {
                    return;
                }
                let mime_types = crate::xwayland::XwaylandDndMimeCatalog::bounded_from_iter(
                    self.drag_source_mime_types(&origin),
                );
                let source_actions = match &origin {
                    ActiveDragOrigin::Xwayland { offer } => offer.source_actions().to_vec(),
                    ActiveDragOrigin::WaylandSource { .. } => {
                        xdnd_actions_from_wayland_mask(self.drag_source_actions(&origin))
                    }
                    ActiveDragOrigin::WaylandSourceless { .. } => Vec::new(),
                };
                if !self.queue_xwayland_dnd_transition(
                    crate::xwayland::XwaylandDndTransition::TargetEntered {
                        session_id,
                        target: window,
                        x,
                        y,
                        mime_types,
                        source_actions,
                    },
                ) {
                    return;
                }
                if let Some(active) = self.active_drag.as_mut() {
                    active.xwayland_dnd_generation = Some(window.generation());
                    active.target = Some(ActiveDragTarget::Xwayland { window });
                    active.accepted_mime = None;
                    active.target_action = None;
                    active.selected_action = 0;
                    active.destination_actions = None;
                    active.last_offer_action = None;
                    active.last_source_action = None;
                }
                return;
            }

            if previous_target.as_ref().is_some_and(|previous| {
                matches!(previous, ActiveDragTarget::Wayland { surface, .. } if same_surface_resource(surface, &target.surface))
            }) {
                self.send_drag_motion_to_current_target(Some(target));
                return;
            }
        }

        self.leave_drag_target();
        if !self.active_drag.as_ref().is_some_and(|active| {
            active.id == session_id && active.phase == DragSessionPhase::Dragging
        }) {
            return;
        }
        let Some(target) = target else {
            return;
        };
        let Some(target_client) = target.surface.client().map(|client| client.id()) else {
            return;
        };
        if origin.is_wayland_sourceless()
            && origin.wayland_initiating_client().as_ref() != Some(&target_client)
        {
            // Sourceless core Wayland drags remain private to their initiating
            // client, including when the pointer is over an X11-backed window.
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

        let mime_types = self.drag_source_mime_types(&origin);
        let source_actions = self.drag_source_actions(&origin);
        let has_source = !origin.is_wayland_sourceless();
        let Some(client) = device.client() else {
            return;
        };
        let Some(handle) = device.handle().upgrade() else {
            return;
        };
        let display = DisplayHandle::from(handle);
        let offer = if has_source {
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
                    target_id: device.id().protocol_id(),
                    source_generation: 0,
                    broker_offer_id: None,
                    source_key: None,
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
            active.target = Some(ActiveDragTarget::Wayland {
                surface: target.surface.clone(),
                client_id: target_client,
                offer,
            });
            active.target_action = None;
            active.selected_action = 0;
            active.destination_actions = None;
            active.last_offer_action = None;
            active.last_source_action = None;
        }
    }

    fn queue_xwayland_dnd_position(
        &mut self,
        session_id: crate::xwayland::CanonicalDndSessionId,
        target_window: crate::xwayland::X11WindowHandle,
        x: f64,
        y: f64,
    ) {
        let current = self.active_drag.as_ref().filter(|active| {
            active.id == session_id
                && matches!(
                    active.target.as_ref(),
                    Some(ActiveDragTarget::Xwayland { window }) if *window == target_window
                )
        });
        let accepted_mime = current.and_then(|active| active.accepted_mime.clone());
        let action = current.and_then(|active| active.target_action);
        let mime_types = current
            .map(|active| {
                crate::xwayland::XwaylandDndMimeCatalog::bounded_from_iter(
                    self.drag_source_mime_types(&active.origin),
                )
            })
            .unwrap_or_default();
        let source_actions = current
            .map(|active| match &active.origin {
                ActiveDragOrigin::Xwayland { offer } => offer.source_actions().to_vec(),
                ActiveDragOrigin::WaylandSource { .. } => {
                    xdnd_actions_from_wayland_mask(self.drag_source_actions(&active.origin))
                }
                ActiveDragOrigin::WaylandSourceless { .. } => Vec::new(),
            })
            .unwrap_or_default();
        let _ = self.queue_xwayland_dnd_transition(
            crate::xwayland::XwaylandDndTransition::TargetPositioned {
                session_id,
                target: target_window,
                x,
                y,
                accepted_mime,
                action,
                mime_types,
                source_actions,
            },
        );
    }

    fn send_drag_motion_to_current_target(&mut self, target: Option<&PointerTarget>) {
        let Some(target) = target else {
            return;
        };
        let Some(active) = self.active_drag.as_ref() else {
            return;
        };
        match active.target.as_ref() {
            Some(ActiveDragTarget::Wayland { client_id, .. }) => {
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
            Some(ActiveDragTarget::Xwayland { window }) => {
                self.queue_xwayland_dnd_position(
                    active.id,
                    *window,
                    self.last_pointer_x,
                    self.last_pointer_y,
                );
            }
            None => {}
        }
    }

    pub(in crate::compositor) fn leave_drag_target(&mut self) {
        let transition = {
            let Some(active) = self.active_drag.as_mut() else {
                return;
            };
            let Some(target) = active.target.take() else {
                active.accepted_mime = None;
                active.target_action = None;
                active.selected_action = 0;
                active.destination_actions = None;
                active.last_offer_action = None;
                active.last_source_action = None;
                return;
            };
            let transition = match target {
                ActiveDragTarget::Wayland {
                    client_id, offer, ..
                } => {
                    if let Some(device) = self
                        .data_devices
                        .iter()
                        .find(|binding| binding.client_id == client_id && binding.device.is_alive())
                        .map(|binding| binding.device.clone())
                    {
                        let _ = device.send_event(wl_data_device::Event::Leave);
                    }
                    if let Some(offer) = offer {
                        self.data_offers.remove(&offer.id());
                    }
                    match &active.origin {
                        ActiveDragOrigin::WaylandSource { source, .. } if source.is_alive() => {
                            let _ = source
                                .send_event(wl_data_source::Event::Target { mime_type: None });
                            None
                        }
                        ActiveDragOrigin::Xwayland { offer } => {
                            Some(crate::xwayland::XwaylandDndTransition::SourceFeedback {
                                offer_id: offer.id(),
                                accepted_mime: None,
                                action: None,
                            })
                        }
                        ActiveDragOrigin::WaylandSource { .. }
                        | ActiveDragOrigin::WaylandSourceless { .. } => None,
                    }
                }
                ActiveDragTarget::Xwayland { window } => {
                    if let ActiveDragOrigin::WaylandSource { source, .. } = &active.origin
                        && source.is_alive()
                    {
                        let _ =
                            source.send_event(wl_data_source::Event::Target { mime_type: None });
                    }
                    Some(crate::xwayland::XwaylandDndTransition::TargetLeft {
                        session_id: active.id,
                        target: window,
                    })
                }
            };
            active.accepted_mime = None;
            active.target_action = None;
            active.selected_action = 0;
            active.destination_actions = None;
            active.last_offer_action = None;
            active.last_source_action = None;
            transition
        };
        if let Some(transition) = transition {
            let _ = self.queue_xwayland_dnd_transition(transition);
        }
    }

    pub(in crate::compositor) fn send_drag_action_if_changed(&mut self) {
        let Some(active) = self.active_drag.as_ref() else {
            return;
        };
        if active.phase != DragSessionPhase::Dragging || active.destination_actions.is_none() {
            return;
        }
        let action = active.selected_action;
        let offer = active
            .target
            .as_ref()
            .and_then(ActiveDragTarget::wayland_offer)
            .cloned();
        let source = active.origin.wayland_source().cloned();
        let xwayland_offer_id = active.origin.xwayland_offer().map(|offer| offer.id());
        let xwayland_action = active
            .target_action
            .or_else(|| xdnd_action_from_wayland_mask(active.selected_action));
        let accepted_mime = active.accepted_mime.clone();
        let send_offer = offer
            .as_ref()
            .is_some_and(|offer| offer.version() >= 3 && active.last_offer_action != Some(action));
        let send_source = source.as_ref().is_some_and(|source| {
            source.version() >= 3 && active.last_source_action != Some(action)
        });
        let send_xwayland =
            xwayland_offer_id.is_some() && active.last_source_action != Some(action);
        if let Some(active) = self.active_drag.as_mut() {
            if send_offer {
                active.last_offer_action = Some(action);
            }
            if send_source || send_xwayland {
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
        if send_xwayland && let Some(offer_id) = xwayland_offer_id {
            let _ = self.queue_xwayland_dnd_transition(
                crate::xwayland::XwaylandDndTransition::SourceFeedback {
                    offer_id,
                    accepted_mime,
                    action: xwayland_action,
                },
            );
        }
    }

    pub(in crate::compositor) fn update_drag_acceptance(
        &mut self,
        offer: &wl_data_offer::WlDataOffer,
        mime_type: Option<String>,
    ) {
        let transition = {
            let Some(active) = self.active_drag.as_mut() else {
                return;
            };
            if active
                .target
                .as_ref()
                .and_then(ActiveDragTarget::wayland_offer)
                .is_none_or(|current| !same_wayland_resource(current, offer))
            {
                return;
            }
            active.accepted_mime = mime_type.clone();
            match &active.origin {
                ActiveDragOrigin::WaylandSource { source, .. } if source.is_alive() => {
                    let _ = source.send_event(wl_data_source::Event::Target { mime_type });
                    None
                }
                ActiveDragOrigin::Xwayland { offer } => {
                    Some(crate::xwayland::XwaylandDndTransition::SourceFeedback {
                        offer_id: offer.id(),
                        accepted_mime: mime_type,
                        action: xdnd_action_from_wayland_mask(active.selected_action),
                    })
                }
                ActiveDragOrigin::WaylandSource { .. }
                | ActiveDragOrigin::WaylandSourceless { .. } => None,
            }
        };
        if let Some(transition) = transition {
            let _ = self.queue_xwayland_dnd_transition(transition);
        }
    }

    /// Apply semantic status from an X11-backed target to the canonical drag.
    /// XDND actions are converted by meaning; Link and Private remain typed but
    /// have no Wayland core action mask.
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
                .target
                .as_ref()
                .and_then(ActiveDragTarget::wayland_offer)
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
            .origin
            .wayland_source()
            .is_none_or(|current| !same_wayland_resource(current, source))
        {
            return;
        }
        let offer = active
            .target
            .as_ref()
            .and_then(ActiveDragTarget::wayland_offer)
            .cloned();
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
        self.drop_drag_for_driver(DragLifecycleDriver::WaylandImplicitPointerGrab);
    }

    pub(super) fn drop_drag_for_driver(&mut self, driver: DragLifecycleDriver) {
        if self
            .active_drag
            .as_ref()
            .is_some_and(|active| active.lifecycle_driver != driver)
        {
            return;
        }
        if self
            .active_drag
            .as_ref()
            .is_some_and(|active| active.phase != DragSessionPhase::Dragging)
        {
            self.note_dnd_duplicate_terminal_attempt();
            return;
        }
        let Some(active) = self.active_drag.as_ref().cloned() else {
            return;
        };
        let Some(target) = active.target.clone() else {
            self.cancel_drag_session("drop_without_target");
            return;
        };
        if active.phase != DragSessionPhase::Dragging {
            return;
        }
        match target {
            ActiveDragTarget::Xwayland { window } => {
                let (Some(mime_type), Some(action)) =
                    (active.accepted_mime.clone(), active.target_action)
                else {
                    self.cancel_drag_session("xwayland_target_not_accepted");
                    return;
                };
                let action_supported = match &active.origin {
                    ActiveDragOrigin::WaylandSource { .. } => {
                        action.to_wayland_action().is_some() && active.selected_action != 0
                    }
                    ActiveDragOrigin::Xwayland { offer } => {
                        offer.source_actions().contains(&action)
                    }
                    ActiveDragOrigin::WaylandSourceless { .. } => false,
                };
                if !action_supported {
                    self.cancel_drag_session("xwayland_target_action_unsupported");
                    return;
                }
                let mime_types = crate::xwayland::XwaylandDndMimeCatalog::bounded_from_iter(
                    self.drag_source_mime_types(&active.origin),
                );
                let source_actions = match &active.origin {
                    ActiveDragOrigin::Xwayland { offer } => offer.source_actions().to_vec(),
                    ActiveDragOrigin::WaylandSource { .. } => {
                        xdnd_actions_from_wayland_mask(self.drag_source_actions(&active.origin))
                    }
                    ActiveDragOrigin::WaylandSourceless { .. } => Vec::new(),
                };
                if !self.queue_xwayland_dnd_transition(
                    crate::xwayland::XwaylandDndTransition::DropRequested {
                        session_id: active.id,
                        target: window,
                        mime_type,
                        action,
                        mime_types,
                        source_actions,
                    },
                ) {
                    return;
                }
                if let Some(source) = active.origin.wayland_source()
                    && source.version() >= 3
                    && source.is_alive()
                {
                    let _ = source.send_event(wl_data_source::Event::DndDropPerformed);
                }
                if let Some(active) = self.active_drag.as_mut() {
                    active.phase = DragSessionPhase::DropPendingXwaylandTarget;
                }
            }
            ActiveDragTarget::Wayland {
                client_id, offer, ..
            } => {
                let Some(device) = self
                    .data_devices
                    .iter()
                    .find(|binding| binding.client_id == client_id && binding.device.is_alive())
                    .map(|binding| binding.device.clone())
                else {
                    self.cancel_drag_session("target_device_gone");
                    return;
                };
                let sourceless = active.origin.is_wayland_sourceless();
                if !sourceless && (active.accepted_mime.is_none() || active.selected_action == 0) {
                    self.cancel_drag_session("drop_not_accepted");
                    return;
                }
                let _ = device.send_event(wl_data_device::Event::Drop);
                if let Some(source) = active.origin.wayland_source()
                    && source.version() >= 3
                    && source.is_alive()
                {
                    let _ = source.send_event(wl_data_source::Event::DndDropPerformed);
                }
                if let Some(offer) = offer.as_ref()
                    && let Some(binding) = self.data_offers.get_mut(&offer.id())
                {
                    binding.drag_phase = Some(DragOfferPhase::Dropped);
                }
                if sourceless {
                    if let Some(active) = self.active_drag.as_mut() {
                        active.phase = DragSessionPhase::Finished;
                    }
                    self.compliance_metrics.dnd_last_terminal_phase =
                        Some(DragSessionPhase::Finished);
                    self.complete_drag_session(false);
                    self.compliance_metrics.dnd_sessions_finished = self
                        .compliance_metrics
                        .dnd_sessions_finished
                        .saturating_add(1);
                    return;
                }
                if let Some(active) = self.active_drag.as_mut() {
                    active.phase = if active.selected_action == DND_ASK {
                        DragSessionPhase::DroppedAwaitingAskResolution
                    } else {
                        DragSessionPhase::DroppedAwaitingFinish
                    };
                }
            }
        }
    }

    pub(in crate::compositor) fn finish_drag_offer(
        &mut self,
        offer: &wl_data_offer::WlDataOffer,
    ) -> bool {
        if self.active_drag.as_ref().is_some_and(|active| {
            active
                .target
                .as_ref()
                .and_then(ActiveDragTarget::wayland_offer)
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
            .target
            .as_ref()
            .and_then(ActiveDragTarget::wayland_offer)
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
        let origin = active.origin.clone();
        if let ActiveDragOrigin::Xwayland {
            offer: source_offer,
        } = &origin
            && !self.queue_xwayland_dnd_transition(
                crate::xwayland::XwaylandDndTransition::SourceFinished {
                    offer_id: source_offer.id(),
                    accepted: true,
                    action: xdnd_action_from_wayland_mask(final_action),
                },
            )
        {
            return false;
        }
        if let ActiveDragOrigin::WaylandSource { source, .. } = &origin
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
                .origin
                .wayland_source()
                .is_some_and(|current| same_wayland_resource(current, source))
        }) {
            self.cancel_drag_session("source_destroyed");
        }
    }

    pub(in crate::compositor) fn cancel_drag_session(&mut self, _reason: &'static str) {
        if self.xwayland_dnd_cancel_in_progress {
            return;
        }
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
        let session_id = active.id;
        let source = active.origin.wayland_source().cloned();
        let generation = active
            .xwayland_dnd_generation
            .or_else(|| {
                active
                    .origin
                    .xwayland_offer()
                    .map(|offer| offer.id().generation())
            })
            .or_else(|| match active.target.as_ref() {
                Some(ActiveDragTarget::Xwayland { window }) => Some(window.generation()),
                Some(ActiveDragTarget::Wayland { .. }) | None => None,
            });
        self.xwayland_dnd_cancel_in_progress = true;
        self.xwayland_dnd_cancel_outbox_overflowed = false;
        self.leave_drag_target();
        if let Some(generation) = generation {
            let _ = self.queue_xwayland_dnd_transition(
                crate::xwayland::XwaylandDndTransition::Retired {
                    session_id,
                    generation,
                },
            );
            if self.xwayland_dnd_cancel_outbox_overflowed {
                self.xwayland_dnd_outbox
                    .replace_with_retired(session_id, generation);
            }
        }
        if let Some(active) = self.active_drag.as_mut() {
            active.phase = DragSessionPhase::Cancelled;
        }
        self.compliance_metrics.dnd_last_terminal_phase = Some(DragSessionPhase::Cancelled);
        if let Some(source) = source
            && source.is_alive()
            && source.send_event(wl_data_source::Event::Cancelled).is_ok()
        {
            self.compliance_metrics.dnd_source_cancelled_events = self
                .compliance_metrics
                .dnd_source_cancelled_events
                .saturating_add(1);
        }
        self.complete_drag_session(true);
        self.compliance_metrics.dnd_sessions_cancelled = self
            .compliance_metrics
            .dnd_sessions_cancelled
            .saturating_add(1);
        self.xwayland_dnd_cancel_in_progress = false;
        self.xwayland_dnd_cancel_outbox_overflowed = false;
    }

    pub(super) fn complete_drag_session(&mut self, remove_offer: bool) {
        let Some(active) = self.active_drag.take() else {
            return;
        };
        if let Some(offer) = active.origin.xwayland_offer() {
            self.xwayland_dnd_data_requests
                .retain(|request| request.offer_id != offer.id());
        }
        if let Some(icon) = active.icon_surface {
            self.deactivate_role_instance(compositor_surface_id(&icon));
        }
        if remove_offer
            && let Some(offer) = active
                .target
                .as_ref()
                .and_then(ActiveDragTarget::wayland_offer)
        {
            self.data_offers.remove(&offer.id());
        }
    }

    pub(in crate::compositor) fn retire_xwayland_drag_target(
        &mut self,
        window: crate::xwayland::X11WindowHandle,
    ) {
        let phase = self.active_drag.as_ref().and_then(|active| {
            matches!(
                active.target.as_ref(),
                Some(ActiveDragTarget::Xwayland { window: current }) if *current == window
            )
            .then_some(active.phase)
        });
        match phase {
            Some(DragSessionPhase::DropPendingXwaylandTarget) => {
                self.cancel_drag_session("xwayland_target_retired_after_drop");
            }
            Some(_) => self.leave_drag_target(),
            None => {}
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
