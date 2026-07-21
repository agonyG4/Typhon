use super::*;
use crate::xwayland::trace::{self, TraceFields};
use x11rb::connection::Connection;

impl Xwm {
    pub(crate) fn handle_resize_sync_deadline(&mut self, now_ns: u64) -> Result<(), XwmError> {
        for handle in self.resize_sync.expired_handles(now_ns) {
            if self.resize_sync.timeout(handle, now_ns) {
                self.resize_sync.disable_after_timeout(handle);
                let transaction = self.resize_sync.transaction(handle);
                let counter_value = self
                    .resize_sync
                    .state(handle)
                    .counter_value()
                    .unwrap_or_default();
                let latest_desired = self
                    .resize_sync
                    .desired(handle)
                    .map(|desired| desired.geometry);
                let desired = self.resize_sync.take_desired(handle);
                let has_followup = desired.is_some();
                let fallback_geometry = desired
                    .map(|desired| desired.geometry)
                    .or_else(|| transaction.map(|(_, geometry, _)| geometry));
                let allow_result = commands::set_allow_commits(self, handle, true)
                    .and_then(|()| self.connection.flush().map_err(XwmError::Connection));
                self.timed_out_resize_counters.insert(handle, counter_value);
                self.resize_sync.finish_timeout(handle);
                allow_result?;
                trace::emit("resize_timeout", || {
                    TraceFields::new()
                        .field("source", "xwm")
                        .field("xid", handle.xid())
                        .field("resize_counter", counter_value)
                        .optional(
                            "transaction_id",
                            transaction.map(|(transaction_id, _, _)| transaction_id),
                        )
                        .optional(
                            "geometry",
                            transaction.map(|(_, geometry, _)| format!("{geometry:?}")),
                        )
                        .optional(
                            "latest_desired",
                            latest_desired.map(|geometry| format!("{geometry:?}")),
                        )
                        .field("allow_commits", true)
                });
                if std::env::var_os("TYPHON_XWAYLAND_LOG").is_some() {
                    let (transaction_id, geometry, _) = transaction.unwrap_or_default();
                    eprintln!(
                        "oblivion-one xwayland: event=x11_resize_timeout xid={} transaction_id={} counter={} geometry={:?} latest_desired={:?}",
                        handle.xid(),
                        transaction_id,
                        counter_value,
                        geometry,
                        latest_desired,
                    );
                }
                if let Some(geometry) = fallback_geometry {
                    commands::configure_immediate(self, handle, geometry, false)?;
                }
                self.outgoing_events.push_back(if has_followup {
                    XwmEvent::ResizeSyncTimedOutWithFollowup(handle)
                } else {
                    XwmEvent::ResizeSyncTimedOut(handle)
                });
            }
        }
        Ok(())
    }

    pub(crate) fn complete_resize_sync(&mut self, handle: X11WindowHandle) -> Result<(), XwmError> {
        if !self.resize_sync.complete(handle) {
            return Err(XwmError::InvalidCommand("resize sync is not presented"));
        }
        trace::emit("resize_presented_completion", || {
            TraceFields::new()
                .field("source", "xwm")
                .field("xid", handle.xid())
                .field(
                    "resize_state",
                    format!("{:?}", self.resize_sync.state(handle)),
                )
        });
        self.clear_resize_sync_alarm(handle);
        if let Some(desired) = self.resize_sync.take_desired(handle) {
            let now = crate::native::event_loop::monotonic_now_ns().unwrap_or_default();
            commands::begin_resize_sync(
                self,
                handle,
                desired.geometry,
                0,
                now.saturating_add(RESIZE_SYNC_TIMEOUT_NS),
                desired.final_pending,
            )?;
        }
        Ok(())
    }

    pub(crate) fn clear_resize_sync(&mut self, handle: X11WindowHandle) {
        self.resize_sync.clear(handle);
        self.clear_resize_sync_alarm(handle);
        self.timed_out_resize_counters.remove(&handle);
        self.expected_configures.remove(&handle);
        self.immediate_resize_windows.remove(&handle);
        self.fallback_resize_windows.remove(&handle);
        self.last_resize_geometries.remove(&handle);
    }

    pub(crate) fn clear_resize_sync_generation(&mut self, generation: XwaylandGeneration) {
        let handles = self
            .sync_alarms
            .keys()
            .filter(|handle| handle.generation() == generation)
            .copied()
            .collect::<Vec<_>>();
        self.resize_sync.clear_generation(generation);
        self.timed_out_resize_counters
            .retain(|handle, _| handle.generation() != generation);
        self.next_resize_counter_values
            .retain(|handle, _| handle.generation() != generation);
        self.expected_configures
            .retain(|handle, _| handle.generation() != generation);
        self.fallback_resize_windows
            .retain(|handle| handle.generation() != generation);
        for handle in handles {
            self.clear_resize_sync_alarm(handle);
        }
    }

    pub(crate) fn note_expected_configure(
        &mut self,
        handle: X11WindowHandle,
        geometry: X11Geometry,
    ) {
        self.expected_configures.insert(handle, geometry);
    }

    pub(crate) fn note_configure_notify(
        &mut self,
        handle: X11WindowHandle,
        geometry: X11Geometry,
    ) -> bool {
        let expected = self.expected_configures.get(&handle).copied();
        if expected == Some(geometry) {
            self.expected_configures.remove(&handle);
            true
        } else {
            false
        }
    }

    pub(crate) fn clear_resize_sync_alarm(&mut self, handle: X11WindowHandle) {
        let Some(alarm) = self.sync_alarms.remove(&handle) else {
            return;
        };
        self.sync_handles_by_counter
            .retain(|_, mapped_handle| *mapped_handle != handle);
        use x11rb::protocol::sync::ConnectionExt as _;
        let _ = self.connection.sync_destroy_alarm(alarm);
    }

    pub(crate) fn collect_adoption_expirations(&mut self, now_ns: u64) {
        for (handle, wait) in self.adoption.expired(now_ns) {
            eprintln!(
                "oblivion-one xwayland: event=adoption_timeout window={} wait={wait:?}",
                handle.xid()
            );
        }
    }

    pub(crate) fn sync_completed_associations(&mut self) {
        let associations = self
            .association
            .completed
            .iter()
            .map(|(handle, association)| (*handle, *association))
            .collect::<Vec<_>>();
        for (handle, association) in associations {
            if !self.windows.contains(handle) {
                continue;
            }
            let needs_association = self
                .windows
                .get(handle)
                .is_some_and(|record| record.association.is_none());
            if needs_association {
                let _ = self.windows.mark_associated(handle, association);
                trace::emit("association_complete", || {
                    TraceFields::new()
                        .field("source", "xwm")
                        .field("xid", handle.xid())
                        .field("association_serial", association.serial.get())
                        .field("surface_id", association.surface_id)
                        .field("lifecycle", "associated")
                });
                let deadline = crate::native::event_loop::monotonic_now_ns()
                    .unwrap_or_default()
                    .saturating_add(adoption::ADOPTION_TIMEOUT_NS);
                self.adoption.observe(
                    handle,
                    adoption::AdoptionWait::AssociationToBuffer,
                    deadline,
                );
            }
            if self.buffer_ready_surfaces.contains(&association.surface_id) {
                self.adoption.complete(handle);
                let _ = self.windows.mark_buffer_ready(handle);
            }
            let _ = self.emit_ready_if_complete(handle);
        }
        self.process_pending_resize_commits();
    }

    pub(crate) fn emit_ready_if_complete(
        &mut self,
        handle: X11WindowHandle,
    ) -> Result<bool, XwmError> {
        let Some((properties_ready, kind, map_authorized)) = self
            .windows
            .get(handle)
            .map(|record| (record.properties_ready, record.kind, record.map_authorized))
        else {
            return Ok(false);
        };
        if properties_ready
            && kind == DesktopWindowKind::Managed
            && !map_authorized
            && self
                .windows
                .mark_map_authorized(handle)
                .map_err(XwmError::InvalidCommand)?
        {
            self.outgoing_events
                .push_back(XwmEvent::WindowMapRequested(handle));
        }
        if !properties_ready {
            return Ok(false);
        }
        let snapshot = self
            .windows
            .try_ready(handle)
            .map_err(XwmError::InvalidCommand)?;
        if let Some(snapshot) = snapshot {
            self.outgoing_events
                .push_back(XwmEvent::WindowReady(snapshot));
            return Ok(true);
        }
        Ok(false)
    }
}
