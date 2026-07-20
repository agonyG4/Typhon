use super::*;

impl Xwm {
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
                match self.resize_sync.note_commit(handle) {
                    ResizeSyncCommit::Presented | ResizeSyncCommit::FallbackPresented => self
                        .outgoing_events
                        .push_back(XwmEvent::ResizeSyncPresented(handle)),
                    ResizeSyncCommit::Deferred | ResizeSyncCommit::Ignored => {}
                }
            }
            let _ = self.emit_ready_if_complete(handle);
        }
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
