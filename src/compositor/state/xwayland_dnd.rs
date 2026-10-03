//! Canonical compositor integration for generation-qualified XDND sessions.
//!
//! This module adapts XWayland source/target events into the single active
//! compositor DND session; it does not own an independent drag lifecycle.

use super::data_device::{DragTargetLeaveReason, select_dnd_action, xdnd_action_from_wayland_mask};
use super::*;
use crate::xwayland::CanonicalDndSessionId;

#[allow(clippy::enum_variant_names)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::compositor) enum XwaylandIncomingPositionDisposition {
    SameWaylandTarget,
    EnteredNewWaylandTarget,
    NoWaylandTarget,
}

impl CompositorState {
    /// Apply one XWayland Position as a single canonical state transition.
    /// Target retargeting cannot answer the new Position from the target being
    /// left, while an unchanged target can publish its current feedback.
    pub(in crate::compositor) fn apply_incoming_xwayland_position(
        &mut self,
        offer_id: crate::xwayland::XwaylandDndOfferId,
        position_id: crate::xwayland::XwaylandDndIncomingPositionId,
        source_actions: Vec<crate::xwayland::XwaylandDndAction>,
        x: f64,
        y: f64,
    ) -> Option<XwaylandIncomingPositionDisposition> {
        let active = self.active_drag.as_ref().filter(|active| {
            active.id == CanonicalDndSessionId::Xwayland(offer_id)
                && active.phase == DragSessionPhase::Dragging
                && active.lifecycle_driver == DragLifecycleDriver::Xwayland
                && position_id.offer_id() == offer_id
                && active
                    .origin
                    .xwayland_offer()
                    .is_some_and(|offer| offer.id() == offer_id)
                && self
                    .xwayland
                    .client_identity
                    .as_ref()
                    .is_some_and(|identity| identity.generation == offer_id.generation())
        })?;
        let previous_target = active.target.clone();
        let target = self.pointer_target_at(x, y);
        let target_is_x11 = target.as_ref().is_some_and(|target| {
            let surface_id = compositor_surface_id(&target.surface);
            let root_surface_id = self.root_surface_id_for_surface(surface_id);
            self.window_id_for_surface(root_surface_id)
                .and_then(|window_id| self.window(window_id))
                .and_then(|window| match window.backend {
                    WindowBackend::X11(handle) => Some(handle),
                    WindowBackend::Xdg(_) => None,
                })
                .is_some_and(|handle| {
                    self.xwayland
                        .client_identity
                        .as_ref()
                        .is_some_and(|identity| identity.generation == handle.generation())
                })
        });
        let source_actions_mask = {
            let active = self.active_drag.as_mut()?;
            let ActiveDragOrigin::Xwayland { offer } = &mut active.origin else {
                return None;
            };
            if offer.id() != offer_id || offer.replace_source_actions(source_actions).is_err() {
                return None;
            }
            active.xwayland_incoming_position_id = Some(position_id);
            active.last_source_action = None;
            offer.wayland_source_actions_mask()
        };

        let Some(target) = target.filter(|_| !target_is_x11) else {
            self.leave_drag_target_for_reason(DragTargetLeaveReason::NoWaylandTarget);
            return Some(XwaylandIncomingPositionDisposition::NoWaylandTarget);
        };
        if previous_target.as_ref().is_some_and(|previous| {
            matches!(previous, ActiveDragTarget::Wayland { surface, .. } if same_surface_resource(surface, &target.surface))
        }) {
            self.update_current_incoming_target_source_actions(source_actions_mask);
            self.send_drag_action_if_changed();
            self.send_drag_motion_to_current_target(Some(&target));
            return Some(XwaylandIncomingPositionDisposition::SameWaylandTarget);
        }

        self.leave_drag_target_for_reason(DragTargetLeaveReason::RetargetWithinPosition);
        if !self.active_drag.as_ref().is_some_and(|active| {
            active.id == CanonicalDndSessionId::Xwayland(offer_id)
                && active.phase == DragSessionPhase::Dragging
        }) {
            return None;
        }
        if self.enter_incoming_xwayland_wayland_target(offer_id, target, source_actions_mask) {
            Some(XwaylandIncomingPositionDisposition::EnteredNewWaylandTarget)
        } else {
            self.leave_drag_target_for_reason(DragTargetLeaveReason::NoWaylandTarget);
            Some(XwaylandIncomingPositionDisposition::NoWaylandTarget)
        }
    }

    fn update_current_incoming_target_source_actions(&mut self, source_actions: u32) {
        let Some(active) = self.active_drag.as_mut() else {
            return;
        };
        let wayland_offer = active
            .target
            .as_ref()
            .and_then(ActiveDragTarget::wayland_offer)
            .cloned();
        if let Some(offer) = wayland_offer.as_ref()
            && let Some(binding) = self.data_offers.get_mut(&offer.id())
        {
            binding.source_actions = source_actions;
            let selected = binding
                .destination_actions
                .map_or(0, |destination_actions| {
                    select_dnd_action(
                        source_actions,
                        destination_actions,
                        binding.preferred_action,
                    )
                });
            binding.selected_action = (selected != 0).then_some(selected);
            active.selected_action = selected;
        }
        if let Some(offer) = wayland_offer
            && offer.version() >= 3
        {
            let _ = offer.send_event(wl_data_offer::Event::SourceActions {
                source_actions: WEnum::Unknown(source_actions),
            });
        }
    }

    fn enter_incoming_xwayland_wayland_target(
        &mut self,
        offer_id: crate::xwayland::XwaylandDndOfferId,
        target: PointerTarget,
        source_actions: u32,
    ) -> bool {
        let Some(target_client) = target.surface.client().map(|client| client.id()) else {
            return false;
        };
        let Some(device) = self
            .data_devices
            .iter()
            .find(|binding| binding.client_id == target_client && binding.device.is_alive())
            .map(|binding| binding.device.clone())
        else {
            return false;
        };
        let Some(active) = self.active_drag.as_ref() else {
            return false;
        };
        if active.id != CanonicalDndSessionId::Xwayland(offer_id)
            || active.phase != DragSessionPhase::Dragging
        {
            return false;
        }
        let mime_types = self.drag_source_mime_types(&active.origin);
        let Some(client) = device.client() else {
            return false;
        };
        let Some(handle) = device.handle().upgrade() else {
            return false;
        };
        let display = DisplayHandle::from(handle);
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
            return false;
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
        let serial = self.next_configure_serial();
        let _ = device.send_event(wl_data_device::Event::Enter {
            serial,
            surface: target.surface.clone(),
            x: target.surface_x,
            y: target.surface_y,
            id: Some(offer.clone()),
        });
        if let Some(active) = self.active_drag.as_mut() {
            active.target = Some(ActiveDragTarget::Wayland {
                surface: target.surface,
                client_id: target_client,
                device_id: device.id().protocol_id(),
                offer: Some(offer),
            });
            active.target_action = None;
            active.selected_action = 0;
            active.destination_actions = None;
            active.last_offer_action = None;
            active.last_source_action = None;
        }
        true
    }

    /// Hand one XDND MIME read to the exact active Wayland source. Selection
    /// demand is advisory and never changes XdndStatus acceptance.
    pub(in crate::compositor) fn request_xwayland_dnd_source_data(
        &mut self,
        request: crate::xwayland::XwaylandDndSourceDataRequest,
    ) -> bool {
        use std::os::fd::AsFd;

        let adapter_id = request.transfer_id.source.adapter_id;
        if request.transfer_id.source.xid == 0
            || adapter_id.generation() != request.target.generation()
            || self
                .xwayland
                .client_identity
                .as_ref()
                .is_none_or(|identity| identity.generation != adapter_id.generation())
        {
            return false;
        }
        let Some(active) = self.active_drag.as_ref() else {
            return false;
        };
        if active.id != adapter_id.session_id()
            || active.lifecycle_driver != DragLifecycleDriver::WaylandImplicitPointerGrab
            || !matches!(
                active.phase,
                DragSessionPhase::Dragging | DragSessionPhase::DropPendingXwaylandTarget
            )
            || active.xwayland_dnd_generation != Some(adapter_id.generation())
            || !matches!(active.target.as_ref(), Some(ActiveDragTarget::Xwayland { window }) if *window == request.target)
            || !matches!(&active.origin, ActiveDragOrigin::WaylandSource { .. })
            || !self
                .drag_source_mime_types(&active.origin)
                .iter()
                .any(|mime| mime == &request.mime_type)
        {
            return false;
        }
        let ActiveDragOrigin::WaylandSource { source, .. } = &active.origin else {
            return false;
        };
        source
            .send_event(wayland_server::protocol::wl_data_source::Event::Send {
                mime_type: request.mime_type,
                fd: request.sink.as_fd(),
            })
            .is_ok()
    }

    pub(in crate::compositor) fn take_xwayland_dnd_transitions(
        &mut self,
    ) -> Vec<crate::xwayland::XwaylandDndTransition> {
        self.xwayland_dnd_outbox.drain()
    }

    pub(in crate::compositor) fn queue_xwayland_dnd_transition(
        &mut self,
        transition: crate::xwayland::XwaylandDndTransition,
    ) -> bool {
        let overflowed = match self.xwayland_dnd_outbox.push(transition) {
            Ok(()) => return true,
            Err(transition) => transition,
        };
        let session_id = overflowed.canonical_session_id();
        let generation = overflowed.generation();

        if self.xwayland_dnd_outbox_overflow_in_progress {
            return false;
        }
        if self.xwayland_dnd_cancel_in_progress {
            self.xwayland_dnd_cancel_outbox_overflowed = true;
            return false;
        }

        let exact_active_session = self.active_drag.as_ref().is_some_and(|active| {
            active.id == session_id
                && !matches!(
                    active.phase,
                    DragSessionPhase::Finished | DragSessionPhase::Cancelled
                )
        });
        if !exact_active_session {
            return false;
        }

        self.xwayland_dnd_outbox_overflow_in_progress = true;
        self.cancel_drag_session("xwayland_dnd_outbox_overflow");
        self.xwayland_dnd_outbox_overflow_in_progress = false;
        self.xwayland_dnd_outbox
            .replace_with_retired(session_id, generation);
        false
    }

    pub(in crate::compositor) fn begin_xwayland_drag_session(
        &mut self,
        offer: crate::xwayland::XwaylandDndOffer,
    ) -> bool {
        let offer_id = offer.id();
        if offer.source().generation() != offer_id.generation()
            || self
                .xwayland
                .client_identity
                .as_ref()
                .is_none_or(|identity| identity.generation != offer_id.generation())
            || self.last_xwayland_dnd_offer_id.is_some_and(|previous| {
                previous.generation() == offer_id.generation()
                    && previous.serial() >= offer_id.serial()
            })
        {
            return false;
        }
        self.last_xwayland_dnd_offer_id = Some(offer_id);
        self.begin_drag_with_origin(
            CanonicalDndSessionId::Xwayland(offer_id),
            ActiveDragOrigin::Xwayland { offer },
            DragLifecycleDriver::Xwayland,
            None,
        );
        true
    }

    pub(in crate::compositor) fn update_xwayland_drag_target_status(
        &mut self,
        session_id: CanonicalDndSessionId,
        target: crate::xwayland::X11WindowHandle,
        accepted: bool,
        action: Option<crate::xwayland::XwaylandDndAction>,
    ) -> bool {
        let Some(active) = self.active_drag.as_ref() else {
            return false;
        };
        let exact_target = matches!(
            active.target.as_ref(),
            Some(ActiveDragTarget::Xwayland { window }) if *window == target
        );
        if active.id != session_id
            || !exact_target
            || !matches!(
                active.phase,
                DragSessionPhase::Dragging | DragSessionPhase::DropPendingXwaylandTarget
            )
            || (active.phase == DragSessionPhase::DropPendingXwaylandTarget
                && (!matches!(&active.origin, ActiveDragOrigin::WaylandSource { .. })
                    || active.drop_action.is_none()))
            || self
                .xwayland
                .client_identity
                .as_ref()
                .is_none_or(|identity| identity.generation != target.generation())
        {
            return false;
        }
        let Some(origin) = self
            .active_drag
            .as_ref()
            .map(|active| active.origin.clone())
        else {
            return false;
        };
        let action = if accepted { action } else { None };
        if accepted && action.is_none() {
            return false;
        }
        let action_supported = match &origin {
            ActiveDragOrigin::WaylandSource { .. } => action.is_none_or(|action| {
                action
                    .to_wayland_action()
                    .is_some_and(|wayland| self.drag_source_actions(&origin) & wayland.mask() != 0)
            }),
            ActiveDragOrigin::Xwayland { offer } => {
                action.is_none_or(|action| offer.source_actions().contains(&action))
            }
            ActiveDragOrigin::WaylandSourceless { .. } => false,
        };
        if !action_supported {
            return false;
        }
        if active.phase == DragSessionPhase::DropPendingXwaylandTarget {
            // Status after DndDropPerformed is reconciliation evidence only.
            // The terminally significant action was frozen at physical drop.
            return true;
        }
        let action_mask = action
            .and_then(crate::xwayland::XwaylandDndAction::to_wayland_action)
            .map_or(0, crate::xwayland::WaylandDndAction::mask);
        let source_actions = self.drag_source_actions(&origin);
        let selected_action = if action_mask == 0 {
            0
        } else {
            select_dnd_action(source_actions, action_mask, action_mask)
        };
        let Some(active) = self.active_drag.as_mut() else {
            return false;
        };
        // XDND Status has no MIME field. Keep MIME acceptance owned by the
        // Wayland-target path and represent X11 wire acceptance through its
        // action mask and selected action only.
        active.accepted_mime = None;
        active.target_action = action;
        active.destination_actions = Some(action_mask);
        active.selected_action = selected_action;
        self.send_drag_action_if_changed();
        true
    }

    pub(in crate::compositor) fn drop_xwayland_drag(
        &mut self,
        offer_id: crate::xwayland::XwaylandDndOfferId,
    ) -> bool {
        let Some(active) = self.active_drag.as_ref() else {
            return false;
        };
        if active.lifecycle_driver != DragLifecycleDriver::Xwayland
            || active
                .origin
                .xwayland_offer()
                .is_none_or(|offer| offer.id() != offer_id)
            || self
                .xwayland
                .client_identity
                .as_ref()
                .is_none_or(|identity| identity.generation != offer_id.generation())
        {
            return false;
        }
        if active.phase != DragSessionPhase::Dragging {
            self.note_dnd_duplicate_terminal_attempt();
            return false;
        }
        self.drop_drag_for_driver(DragLifecycleDriver::Xwayland);
        true
    }

    pub(in crate::compositor) fn cancel_xwayland_drag_target(
        &mut self,
        session_id: CanonicalDndSessionId,
        target: crate::xwayland::X11WindowHandle,
    ) -> bool {
        if !self.active_drag.as_ref().is_some_and(|active| {
            active.id == session_id
                && active.phase == DragSessionPhase::Dragging
                && matches!(
                    active.target.as_ref(),
                    Some(ActiveDragTarget::Xwayland { window }) if *window == target
                )
        }) || self
            .xwayland
            .client_identity
            .as_ref()
            .is_none_or(|identity| identity.generation != target.generation())
        {
            return false;
        }
        self.cancel_drag_session("xwayland_target_cancel");
        true
    }

    pub(in crate::compositor) fn finish_xwayland_drag_target(
        &mut self,
        session_id: CanonicalDndSessionId,
        target: crate::xwayland::X11WindowHandle,
        accepted: bool,
        final_action: Option<crate::xwayland::XwaylandDndAction>,
    ) -> bool {
        let Some(active) = self.active_drag.as_ref() else {
            return false;
        };
        if active.id != session_id
            || active.phase != DragSessionPhase::DropPendingXwaylandTarget
            || !matches!(
                active.target.as_ref(),
                Some(ActiveDragTarget::Xwayland { window }) if *window == target
            )
            || self
                .xwayland
                .client_identity
                .as_ref()
                .is_none_or(|identity| identity.generation != target.generation())
        {
            return false;
        }
        let source_actions = self.drag_source_actions(&active.origin);
        let origin = active.origin.clone();
        let Some(frozen_drop_action) = active.drop_action else {
            return false;
        };
        let operation_was_ask = frozen_drop_action == crate::xwayland::XwaylandDndAction::Ask;
        let action = if !accepted && matches!(&origin, ActiveDragOrigin::WaylandSource { .. }) {
            None
        } else if accepted
            && matches!(&origin, ActiveDragOrigin::WaylandSource { .. })
            && operation_was_ask
        {
            match final_action {
                Some(
                    action @ (crate::xwayland::XwaylandDndAction::Copy
                    | crate::xwayland::XwaylandDndAction::Move),
                ) => Some(action),
                _ => return false,
            }
        } else {
            final_action.or(Some(frozen_drop_action))
        };
        if accepted {
            let Some(action) = action else {
                return false;
            };
            let supported = match &origin {
                ActiveDragOrigin::WaylandSource { .. } => action
                    .to_wayland_action()
                    .is_some_and(|wayland| source_actions & wayland.mask() != 0),
                ActiveDragOrigin::Xwayland { offer } => offer.source_actions().contains(&action),
                ActiveDragOrigin::WaylandSourceless { .. } => false,
            };
            if !supported {
                return false;
            }
            if matches!(&origin, ActiveDragOrigin::WaylandSource { .. })
                && !operation_was_ask
                && action != frozen_drop_action
            {
                return false;
            }
        }

        if !self.queue_xwayland_dnd_transition(
            crate::xwayland::XwaylandDndTransition::TargetFinished {
                session_id,
                target,
                accepted,
                action,
            },
        ) {
            return false;
        }
        if let Some(active) = self.active_drag.as_mut() {
            if let Some(action) = action {
                active.selected_action = action
                    .to_wayland_action()
                    .map_or(0, crate::xwayland::WaylandDndAction::mask);
                active.target_action = Some(action);
            } else if !accepted && matches!(&origin, ActiveDragOrigin::WaylandSource { .. }) {
                active.selected_action = 0;
                active.target_action = None;
            }
            active.phase = if accepted {
                DragSessionPhase::Finished
            } else {
                DragSessionPhase::Cancelled
            };
        }
        if let ActiveDragOrigin::WaylandSource { source, .. } = &origin {
            if accepted && source.version() >= 3 && source.is_alive() {
                if operation_was_ask
                    && let Some(action) = action.and_then(|action| {
                        action
                            .to_wayland_action()
                            .map(crate::xwayland::WaylandDndAction::mask)
                    })
                {
                    let _ = source.send_event(wl_data_source::Event::Action {
                        dnd_action: WEnum::Unknown(action),
                    });
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
            } else if !accepted && source.is_alive() {
                let _ = source.send_event(wl_data_source::Event::Cancelled);
                self.compliance_metrics.dnd_source_cancelled_events = self
                    .compliance_metrics
                    .dnd_source_cancelled_events
                    .saturating_add(1);
            }
        }
        self.compliance_metrics.dnd_last_terminal_phase = Some(if accepted {
            DragSessionPhase::Finished
        } else {
            DragSessionPhase::Cancelled
        });
        if accepted {
            self.compliance_metrics.dnd_sessions_finished = self
                .compliance_metrics
                .dnd_sessions_finished
                .saturating_add(1);
        } else {
            self.compliance_metrics.dnd_sessions_cancelled = self
                .compliance_metrics
                .dnd_sessions_cancelled
                .saturating_add(1);
        }
        self.complete_drag_session(true);
        true
    }

    pub(in crate::compositor) fn cancel_xwayland_drag(
        &mut self,
        offer_id: crate::xwayland::XwaylandDndOfferId,
    ) -> bool {
        if !self.active_drag.as_ref().is_some_and(|active| {
            active.lifecycle_driver == DragLifecycleDriver::Xwayland
                && active.phase == DragSessionPhase::Dragging
                && active
                    .origin
                    .xwayland_offer()
                    .is_some_and(|offer| offer.id() == offer_id)
        }) || self
            .xwayland
            .client_identity
            .as_ref()
            .is_none_or(|identity| identity.generation != offer_id.generation())
        {
            return false;
        }
        self.cancel_drag_session("xwayland_cancel");
        true
    }

    /// Completion hook for the future XWayland source-side adapter. The
    /// canonical session remains the sole terminal authority; this method
    /// validates the exact offer and lifecycle phase before retiring it.
    pub(in crate::compositor) fn finish_xwayland_drag(
        &mut self,
        offer_id: crate::xwayland::XwaylandDndOfferId,
        accepted: bool,
    ) -> bool {
        let Some(active) = self.active_drag.as_ref() else {
            return false;
        };
        if active.lifecycle_driver != DragLifecycleDriver::Xwayland
            || active
                .origin
                .xwayland_offer()
                .is_none_or(|offer| offer.id() != offer_id)
            || self
                .xwayland
                .client_identity
                .as_ref()
                .is_none_or(|identity| identity.generation != offer_id.generation())
            || !matches!(
                active.phase,
                DragSessionPhase::DroppedAwaitingFinish
                    | DragSessionPhase::DroppedAwaitingAskResolution
            )
        {
            return false;
        }
        self.terminate_drag_from_xwayland(offer_id, accepted);
        true
    }

    pub(in crate::compositor) fn clear_xwayland_dnd_generation(
        &mut self,
        generation: crate::xwayland::XwaylandGeneration,
    ) {
        let source_matches = self.active_drag.as_ref().is_some_and(|active| {
            active
                .origin
                .xwayland_offer()
                .is_some_and(|offer| offer.id().generation() == generation)
        });
        let target = self
            .active_drag
            .as_ref()
            .and_then(|active| match active.target.as_ref() {
                Some(ActiveDragTarget::Xwayland { window })
                    if window.generation() == generation =>
                {
                    Some(*window)
                }
                _ => None,
            });
        if source_matches {
            self.cancel_drag_session("xwayland_generation_retired");
        } else if let Some(target) = target {
            self.retire_xwayland_drag_target(target);
        }

        self.xwayland_dnd_data_requests
            .retain(|request| request.offer_id.generation() != generation);
        if self
            .last_xwayland_dnd_offer_id
            .is_some_and(|offer_id| offer_id.generation() == generation)
        {
            self.last_xwayland_dnd_offer_id = None;
        }
        if let Some(active) = self.active_drag.as_mut()
            && active.xwayland_dnd_generation == Some(generation)
        {
            active.xwayland_dnd_generation = None;
        }
        self.xwayland_dnd_outbox.clear_generation(generation);
    }

    fn terminate_drag_from_xwayland(
        &mut self,
        offer_id: crate::xwayland::XwaylandDndOfferId,
        accepted: bool,
    ) {
        let Some(active) = self.active_drag.as_ref() else {
            return;
        };
        if active
            .origin
            .xwayland_offer()
            .is_none_or(|offer| offer.id() != offer_id)
            || !matches!(
                active.phase,
                DragSessionPhase::DroppedAwaitingFinish
                    | DragSessionPhase::DroppedAwaitingAskResolution
            )
        {
            return;
        }
        let final_action = active.selected_action;
        let action = xdnd_action_from_wayland_mask(final_action);
        if !self.queue_xwayland_dnd_transition(
            crate::xwayland::XwaylandDndTransition::SourceFinished {
                offer_id,
                accepted,
                action,
            },
        ) {
            return;
        }
        if let Some(active) = self.active_drag.as_mut() {
            active.phase = if accepted {
                DragSessionPhase::Finished
            } else {
                DragSessionPhase::Cancelled
            };
        }
        if let Some(offer) = self
            .active_drag
            .as_ref()
            .and_then(|active| active.target.as_ref())
            .and_then(ActiveDragTarget::wayland_offer)
            && let Some(binding) = self.data_offers.get_mut(&offer.id())
        {
            binding.drag_phase = Some(if accepted {
                DragOfferPhase::Finished
            } else {
                DragOfferPhase::Destroyed
            });
        }
        self.compliance_metrics.dnd_last_terminal_phase = Some(if accepted {
            DragSessionPhase::Finished
        } else {
            DragSessionPhase::Cancelled
        });
        if accepted {
            self.compliance_metrics.dnd_sessions_finished = self
                .compliance_metrics
                .dnd_sessions_finished
                .saturating_add(1);
        } else {
            self.compliance_metrics.dnd_sessions_cancelled = self
                .compliance_metrics
                .dnd_sessions_cancelled
                .saturating_add(1);
        }
        self.complete_drag_session(!accepted);
    }
}
