//! Canonical compositor integration for generation-qualified XDND sessions.
//!
//! This module adapts XWayland source/target events into the single active
//! compositor DND session; it does not own an independent drag lifecycle.

use super::data_device::{select_dnd_action, xdnd_action_from_wayland_mask};
use super::*;
use crate::xwayland::CanonicalDndSessionId;

impl CompositorState {
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
        accepted_mime: Option<String>,
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
            || active.phase != DragSessionPhase::Dragging
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
        let mime_types = crate::xwayland::XwaylandDndMimeCatalog::bounded_from_iter(
            self.drag_source_mime_types(&origin),
        );
        if accepted_mime.as_ref().is_some_and(|mime| {
            !mime_types
                .as_slice()
                .iter()
                .any(|source_mime| source_mime == mime)
        }) || (accepted_mime.is_none() && action.is_some())
        {
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
        active.accepted_mime = accepted_mime.clone();
        active.target_action = action;
        active.destination_actions = Some(action_mask);
        active.selected_action = selected_action;
        if let Some(source) = active.origin.wayland_source()
            && source.is_alive()
        {
            let _ = source.send_event(wl_data_source::Event::Target {
                mime_type: accepted_mime,
            });
        }
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
            || !active
                .origin
                .xwayland_offer()
                .is_some_and(|offer| offer.id() == offer_id)
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
        let accepted_mime = active.accepted_mime.clone();
        let negotiated_action = active.target_action;
        let action = final_action.or(active.target_action);
        if accepted && accepted_mime.is_none() {
            return false;
        }
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
                && negotiated_action != Some(crate::xwayland::XwaylandDndAction::Ask)
                && Some(action) != negotiated_action
            {
                return false;
            }
        }

        let accepted = accepted && accepted_mime.is_some();
        if let Some(active) = self.active_drag.as_mut() {
            if let Some(action) = action {
                active.selected_action = action
                    .to_wayland_action()
                    .map_or(0, crate::xwayland::WaylandDndAction::mask);
                active.target_action = Some(action);
            }
            active.phase = if accepted {
                DragSessionPhase::Finished
            } else {
                DragSessionPhase::Cancelled
            };
        }
        if let ActiveDragOrigin::WaylandSource { source, .. } = &origin {
            if accepted && source.version() >= 3 && source.is_alive() {
                if negotiated_action == Some(crate::xwayland::XwaylandDndAction::Ask)
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
        if matches!(&origin, ActiveDragOrigin::Xwayland { .. }) {
            self.xwayland_dnd_transition =
                Some(crate::xwayland::XwaylandDndTransition::TargetFinished {
                    session_id,
                    target,
                    accepted,
                    action,
                });
        }
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

    pub(in crate::compositor) fn finish_xwayland_drag(
        &mut self,
        offer_id: crate::xwayland::XwaylandDndOfferId,
        accepted: bool,
    ) -> bool {
        let Some(active) = self.active_drag.as_ref() else {
            return false;
        };
        if active.lifecycle_driver != DragLifecycleDriver::Xwayland
            || !active
                .origin
                .xwayland_offer()
                .is_some_and(|offer| offer.id() == offer_id)
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
        let target_matches = self.active_drag.as_ref().is_some_and(|active| {
            matches!(
                active.target.as_ref(),
                Some(ActiveDragTarget::Xwayland { window })
                    if window.generation() == generation
            )
        });
        if source_matches {
            self.cancel_drag_session("xwayland_generation_retired");
        } else if target_matches {
            self.leave_drag_target();
        }

        self.xwayland_dnd_data_requests
            .retain(|request| request.offer_id.generation() != generation);
        if self
            .last_xwayland_dnd_offer_id
            .is_some_and(|offer_id| offer_id.generation() == generation)
        {
            self.last_xwayland_dnd_offer_id = None;
        }
        if self
            .xwayland_dnd_transition
            .as_ref()
            .is_some_and(|transition| match transition {
                crate::xwayland::XwaylandDndTransition::TargetEntered { target, .. }
                | crate::xwayland::XwaylandDndTransition::TargetPositioned { target, .. }
                | crate::xwayland::XwaylandDndTransition::TargetLeft { target, .. }
                | crate::xwayland::XwaylandDndTransition::DropRequested { target, .. }
                | crate::xwayland::XwaylandDndTransition::TargetFinished { target, .. } => {
                    target.generation() == generation
                }
                crate::xwayland::XwaylandDndTransition::SourceFeedback { offer_id, .. }
                | crate::xwayland::XwaylandDndTransition::SourceFinished { offer_id, .. } => {
                    offer_id.generation() == generation
                }
                crate::xwayland::XwaylandDndTransition::Retired {
                    generation: retired,
                    ..
                } => *retired == generation,
            })
        {
            self.xwayland_dnd_transition = None;
        }
    }

    fn terminate_drag_from_xwayland(
        &mut self,
        offer_id: crate::xwayland::XwaylandDndOfferId,
        accepted: bool,
    ) {
        let Some(active) = self.active_drag.as_ref() else {
            return;
        };
        if !active
            .origin
            .xwayland_offer()
            .is_some_and(|offer| offer.id() == offer_id)
            || !matches!(
                active.phase,
                DragSessionPhase::DroppedAwaitingFinish
                    | DragSessionPhase::DroppedAwaitingAskResolution
            )
        {
            return;
        }
        let final_action = active.selected_action;
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
        self.xwayland_dnd_transition =
            Some(crate::xwayland::XwaylandDndTransition::SourceFinished {
                offer_id,
                accepted,
                action: xdnd_action_from_wayland_mask(final_action),
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
