use super::*;
use crate::xwayland::trace::{self, TraceFields};

impl Xwm {
    pub(crate) fn next_resize_sync_deadline_ns(&self) -> Option<u64> {
        self.resize_sync.next_deadline_ns()
    }

    pub(crate) fn next_focus_deadline_ns(&self) -> Option<u64> {
        self.focus.next_focus_deadline_ns()
    }

    pub fn note_x11_surface_serial(
        &mut self,
        handle: X11WindowHandle,
        serial_lo: u32,
        serial_hi: u32,
    ) -> Result<(), XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        let Some(serial) = crate::xwayland::serial_from_parts(serial_lo, serial_hi) else {
            return Err(XwmError::Association(
                SurfaceAssociationJoinError::InvalidSerial,
            ));
        };
        let map_serial = self
            .windows
            .get(handle)
            .map(|record| record.map_serial)
            .unwrap_or_default();
        trace::emit("association_x11_serial_observed", || {
            TraceFields::new()
                .field("source", "x11")
                .field("xid", handle.xid())
                .field("association_serial", serial.get())
                .field("map_serial", map_serial)
                .field("surface_id", "pending")
        });
        let deadline = crate::native::event_loop::monotonic_now_ns()
            .unwrap_or_default()
            .saturating_add(adoption::ADOPTION_TIMEOUT_NS);
        self.adoption
            .observe(handle, adoption::AdoptionWait::SerialPair, deadline);
        self.association
            .note_x11_serial_for_map(handle, serial, map_serial)
            .map_err(XwmError::Association)?;
        self.sync_completed_associations();
        Ok(())
    }

    pub fn ingest_wayland_association(
        &mut self,
        event: XwaylandAssociationEvent,
    ) -> Result<(), XwmError> {
        let generation = match event {
            XwaylandAssociationEvent::Committed { generation, .. }
            | XwaylandAssociationEvent::Removed { generation, .. } => generation,
        };
        if generation != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        match event {
            XwaylandAssociationEvent::Committed {
                generation,
                serial,
                surface_id,
            } => {
                trace::emit("association_wayland_committed", || {
                    TraceFields::new()
                        .field("source", "wayland")
                        .field("generation", generation.get())
                        .field("association_serial", serial.get())
                        .field("surface_id", surface_id)
                });
                self.association
                    .commit_wayland(generation, serial, surface_id)
                    .map_err(XwmError::Association)?;
            }
            XwaylandAssociationEvent::Removed { surface_id, .. } => {
                trace::emit("association_wayland_removed", || {
                    TraceFields::new()
                        .field("source", "wayland")
                        .field("surface_id", surface_id)
                });
                let owner = self
                    .association
                    .completed
                    .iter()
                    .find_map(|(handle, association)| {
                        (association.surface_id == surface_id).then_some(*handle)
                    });
                let removed_association =
                    owner.and_then(|handle| self.association.completed.get(&handle).copied());
                self.association.remove_wayland_surface(surface_id);
                self.clear_surface_buffer_ready(surface_id);
                if let Some(handle) = owner {
                    let preserve_identity = self.windows.get(handle).is_some_and(|record| {
                        record.lifecycle == X11WindowLifecycle::Iconic
                            || removed_association.is_some_and(|association| {
                                record.map_serial > association.map_serial
                            })
                    });
                    let cleared = self
                        .windows
                        .clear_association(handle, surface_id, preserve_identity)
                        .map_err(XwmError::InvalidCommand)?;
                    if cleared {
                        self.clear_configure_timeline(handle);
                    }
                    if cleared && !preserve_identity {
                        self.outgoing_events
                            .push_back(XwmEvent::WindowWithdrawn(handle));
                    }
                }
            }
        }
        self.sync_completed_associations();
        Ok(())
    }

    pub fn remove_x11_association(&mut self, handle: X11WindowHandle) -> Result<(), XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        self.association.remove_x11_window(handle);
        Ok(())
    }

    pub fn take_association_events(&mut self) -> Vec<XwmAssociationEvent> {
        self.association.take_events()
    }

    pub fn set_window_lifecycle(
        &mut self,
        handle: X11WindowHandle,
        lifecycle: X11WindowLifecycle,
    ) -> Result<(), XwmError> {
        if handle.generation() != self.generation {
            return Err(XwmError::StaleGeneration);
        }
        if !self.windows.contains(handle) {
            return Err(XwmError::InvalidCommand("unknown X11 window"));
        }
        self.windows
            .get_mut(handle)
            .expect("validated X11 window")
            .lifecycle = lifecycle;
        Ok(())
    }

    pub fn drain_events(&mut self, budget: usize) -> Result<XwmDrain, XwmError> {
        let budget = budget.min(XWM_EVENT_BUDGET);
        let mut events_processed = 0;
        let mut replies_processed = 0;
        let mut events_quiescent = budget != 0;
        let mut property_replies_quiescent = budget != 0;
        let mut budget_exhausted = false;
        loop {
            let event_budget = budget.saturating_sub(events_processed);
            let event_drain = if event_budget == 0 {
                events_quiescent = false;
                budget_exhausted = budget_exhausted || budget != 0;
                None
            } else {
                let drain = events::drain(self, event_budget)?;
                events_processed = events_processed.saturating_add(drain.processed);
                events_quiescent &= drain.events_quiescent;
                budget_exhausted |= drain.budget_exhausted;
                self.poll_root_event_mask()?;
                Some(drain)
            };
            let reply_budget = budget.saturating_sub(replies_processed);
            let reply_drain = if reply_budget == 0 {
                property_replies_quiescent = false;
                budget_exhausted = budget_exhausted || budget != 0;
                None
            } else {
                let drain = properties::poll_replies(self, reply_budget)?;
                replies_processed = replies_processed.saturating_add(drain.processed);
                property_replies_quiescent &= drain.quiescent;
                budget_exhausted |= drain.budget_exhausted;
                Some(drain)
            };
            if event_drain.is_none_or(|drain| drain.processed == 0)
                && reply_drain.is_none_or(|drain| drain.processed == 0)
            {
                break;
            }
        }
        let quiescent = events_quiescent && property_replies_quiescent;
        if quiescent {
            self.reconcile_override_redirect_stack()?;
        }
        Ok(XwmDrain {
            processed: events_processed.saturating_add(replies_processed),
            budget_exhausted,
            events_processed,
            property_replies_processed: replies_processed,
            events_quiescent,
            property_replies_quiescent,
            quiescent,
        })
    }

    pub fn execute(&mut self, command: XwmCommand) -> Result<XwmCommandOutcome, XwmError> {
        commands::execute(self, command)
    }

    pub fn flush(&self) -> Result<(), XwmError> {
        commands::flush(self)?;
        let _ = self.flush_output()?;
        Ok(())
    }

    pub fn take_events(&mut self) -> impl Iterator<Item = XwmEvent> + '_ {
        self.outgoing_events.drain(..)
    }

    pub(crate) fn next_adoption_deadline_ns(&self) -> Option<u64> {
        self.adoption.next_deadline_ns()
    }
}
