use super::*;

impl CompositorState {
    pub(in crate::compositor) fn take_pending_resize_commit_placement(
        &self,
        surface_id: u32,
        pending: &PendingSurfaceBuffer,
    ) -> io::Result<Option<SurfacePlacement>> {
        let Some(resize) = pending.resize_commit.as_deref().copied() else {
            return Ok(None);
        };
        let buffer_width = pending.data.width()?;
        let buffer_height = pending.data.height()?;
        let committed_size = resize
            .committed_size
            .map(|(width, height)| BufferSize { width, height })
            .or(pending.surface_size)
            .unwrap_or(BufferSize {
                width: buffer_width,
                height: buffer_height,
            });
        let placement =
            resize.placement_for_committed_size(committed_size.width, committed_size.height);
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize commit surface={surface_id} decision=accepted serial={} requested={}x{} actual={}x{} placement={},{}",
                resize.serial,
                resize.width,
                resize.height,
                committed_size.width,
                committed_size.height,
                placement.local_x,
                placement.local_y,
            );
        }
        Ok(Some(placement))
    }

    pub(in crate::compositor) fn ack_xdg_surface_configure(
        &mut self,
        surface_id: u32,
        serial: u32,
    ) {
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: resize_flow surface={surface_id} acked_serial={serial} decision=acked reason=matched_other_configure"
                );
            }
            return;
        }
        let had_uncaptured_ack = self
            .resize_configure_flows
            .get(&surface_id)
            .is_some_and(ResizeConfigureFlow::has_acked_uncaptured);
        let captures_pending = self
            .resize_configure_flows
            .get(&surface_id)
            .map_or(0, ResizeConfigureFlow::captured_count);
        let resize_decision = self
            .resize_configure_flows
            .get_mut(&surface_id)
            .map_or(ResizeAckDecision::Unknown, |flow| flow.ack(serial));
        let serial_state = self.xdg_configure_serials.entry(surface_id).or_default();
        let matched_other = resize_decision == ResizeAckDecision::Unknown
            && serial == serial_state.latest_sent
            && serial > serial_state.latest_acked;
        let decision = if matched_other {
            "matched_other_configure"
        } else {
            match resize_decision {
                ResizeAckDecision::Matched => "matched_in_flight",
                ResizeAckDecision::Duplicate => "duplicate_serial",
                ResizeAckDecision::Stale => "stale_serial",
                ResizeAckDecision::Unknown if serial <= serial_state.latest_sent => "stale_serial",
                ResizeAckDecision::Unknown => "unknown_serial",
            }
        };
        if matched_other || resize_decision == ResizeAckDecision::Matched {
            serial_state.latest_acked = serial_state.latest_acked.max(serial);
        }
        match resize_decision {
            ResizeAckDecision::Matched => {
                self.resize_flow_metrics.acks_matched =
                    self.resize_flow_metrics.acks_matched.saturating_add(1);
                if had_uncaptured_ack {
                    self.resize_flow_metrics.resize_acks_replaced_uncaptured = self
                        .resize_flow_metrics
                        .resize_acks_replaced_uncaptured
                        .saturating_add(1);
                }
                if captures_pending > 0 {
                    self.resize_flow_metrics
                        .resize_acks_preserved_while_captures_pending = self
                        .resize_flow_metrics
                        .resize_acks_preserved_while_captures_pending
                        .saturating_add(1);
                }
            }
            ResizeAckDecision::Stale | ResizeAckDecision::Duplicate => {
                self.resize_flow_metrics.acks_stale =
                    self.resize_flow_metrics.acks_stale.saturating_add(1);
            }
            ResizeAckDecision::Unknown => {
                if !matched_other && serial > serial_state.latest_sent {
                    self.resize_flow_metrics.acks_unknown =
                        self.resize_flow_metrics.acks_unknown.saturating_add(1);
                } else if !matched_other {
                    self.resize_flow_metrics.acks_stale =
                        self.resize_flow_metrics.acks_stale.saturating_add(1);
                }
            }
        }
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} acked_serial={serial} decision={} reason={decision}",
                if resize_decision == ResizeAckDecision::Matched || matched_other {
                    "acked"
                } else {
                    "ignored"
                },
            );
        }
    }

    pub(in crate::compositor) fn capture_acked_resize_for_surface_commit(
        &mut self,
        surface_id: u32,
    ) -> Option<ResizeCommitSnapshot> {
        if !self.toplevel_surfaces.contains_key(&surface_id) {
            return None;
        }
        self.next_surface_commit_sequence = self.next_surface_commit_sequence.saturating_add(1);
        let commit_sequence = self.next_surface_commit_sequence;
        let snapshot = self
            .resize_configure_flows
            .get_mut(&surface_id)
            .and_then(|flow| flow.capture(commit_sequence));
        if let Some(snapshot) = snapshot {
            self.resize_flow_metrics.commits_captured =
                self.resize_flow_metrics.commits_captured.saturating_add(1);
            self.update_resize_captures_pending_metrics(surface_id);
            if compositor_debug_surface_logging_enabled() {
                eprintln!(
                    "oblivion-one compositor: resize_capture surface={surface_id} serial={} sequence={} commit_sequence={} content_size={:?} size_source=pending xdg_geometry={:?} captured_pending={}",
                    snapshot.serial,
                    snapshot.sequence,
                    snapshot.commit_sequence,
                    snapshot.committed_size,
                    self.surface_window_geometries.get(&surface_id),
                    self.resize_configure_flows
                        .get(&surface_id)
                        .map_or(0, ResizeConfigureFlow::captured_count),
                );
            }
        }
        snapshot
    }

    pub(in crate::compositor) fn snapshot_resize_commit_for_buffer(
        &self,
        surface_id: u32,
        snapshot: ResizeCommitSnapshot,
        pending: &PendingSurfaceBuffer,
    ) -> ResizeCommitSnapshot {
        let raw_buffer_size = || {
            Some(BufferSize {
                width: pending.data.width().ok()?,
                height: pending.data.height().ok()?,
            })
        };
        self.snapshot_resize_commit_for_pending_buffer_size(
            surface_id,
            snapshot,
            pending.data.buffer_id().get(),
            pending.surface_size,
            raw_buffer_size(),
        )
    }

    pub(in crate::compositor) fn snapshot_resize_commit_for_pending_buffer_size(
        &self,
        _surface_id: u32,
        snapshot: ResizeCommitSnapshot,
        buffer_id: u64,
        surface_size: Option<BufferSize>,
        raw_buffer_size: Option<BufferSize>,
    ) -> ResizeCommitSnapshot {
        let snapshot = snapshot.with_buffer_id(buffer_id);
        let committed_size = surface_size.or(raw_buffer_size);
        committed_size.map_or(snapshot, |size| {
            snapshot.with_committed_size(size.width, size.height)
        })
    }

    pub(in crate::compositor) fn current_committed_surface_content_size(
        &self,
        surface_id: u32,
    ) -> Option<BufferSize> {
        self.renderable_surfaces
            .iter()
            .find(|surface| surface.surface_id == surface_id)
            .map(|surface| {
                BufferSize::new(surface.width, surface.height).unwrap_or(BufferSize {
                    width: surface.width,
                    height: surface.height,
                })
            })
    }

    pub(in crate::compositor) fn finalize_pending_buffer_resize_capture(
        &mut self,
        surface_id: u32,
        pending: &mut PendingSurfaceBuffer,
    ) {
        if pending.resize_capture_finalized {
            return;
        }
        pending.resize_commit = self
            .capture_acked_resize_for_surface_commit(surface_id)
            .map(|snapshot| self.snapshot_resize_commit_for_buffer(surface_id, snapshot, pending))
            .map(Box::new);
        pending.resize_capture_finalized = true;
    }

    pub(in crate::compositor) fn complete_applied_resize_transaction(
        &mut self,
        surface_id: u32,
        snapshot: ResizeCommitSnapshot,
    ) -> bool {
        self.resize_flow_metrics.resize_captures_completed = self
            .resize_flow_metrics
            .resize_captures_completed
            .saturating_add(1);
        self.update_resize_captures_pending_metrics(surface_id);
        let resize_metadata = self.active_toplevel_resizes.get(&surface_id).copied();
        let _resize_edges = resize_metadata.map(|metadata| metadata.edges);
        let owns_preview = resize_metadata
            .is_some_and(|metadata| metadata.interaction_id == snapshot.interaction_id);
        let (preview_sequence, preview_age_ms, preview_active) = if owns_preview {
            if snapshot.resizing {
                (
                    resize_metadata.map(|metadata| metadata.flow_sequence),
                    0,
                    true,
                )
            } else {
                let resize_metadata = self.active_toplevel_resizes.remove(&surface_id);
                if let Some(visual) = self.toplevel_visual_geometries.get_mut(&surface_id) {
                    visual.active_resize = None;
                }
                self.resize_flow_metrics.preview_completions = self
                    .resize_flow_metrics
                    .preview_completions
                    .saturating_add(1);
                self.resize_flow_metrics.resize_interactions_completed = self
                    .resize_flow_metrics
                    .resize_interactions_completed
                    .saturating_add(1);
                let preview_sequence = resize_metadata.map(|metadata| metadata.flow_sequence);
                let preview_age = resize_metadata
                    .map(|metadata| metadata.activated_at.elapsed())
                    .unwrap_or_else(|| snapshot.emitted_at.elapsed());
                let preview_age_ms = u64::try_from(preview_age.as_millis()).unwrap_or(u64::MAX);
                self.resize_flow_metrics.max_preview_age_ms = self
                    .resize_flow_metrics
                    .max_preview_age_ms
                    .max(preview_age_ms);
                (preview_sequence, preview_age_ms, false)
            }
        } else {
            let preview_active = resize_metadata.is_some();
            if preview_active {
                self.resize_flow_metrics.stale_commits_preserved_preview = self
                    .resize_flow_metrics
                    .stale_commits_preserved_preview
                    .saturating_add(1);
                self.resize_flow_metrics.stale_interaction_commits_applied = self
                    .resize_flow_metrics
                    .stale_interaction_commits_applied
                    .saturating_add(1);
            }
            (
                resize_metadata.map(|metadata| metadata.flow_sequence),
                0,
                preview_active,
            )
        };
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: resize_flow surface={surface_id} decision=applied serial={} sequence={} commit_generation={} interaction_id={} buffer_id={:?} preview_sequence={preview_sequence:?} preview_active={preview_active} preview_age_ms={preview_age_ms}",
                snapshot.serial,
                snapshot.sequence,
                snapshot.commit_sequence,
                snapshot.interaction_id.get(),
                snapshot.buffer_id,
            );
        }
        self.flush_pending_resize_configure();
        if self
            .resize_configure_flows
            .get(&surface_id)
            .is_some_and(ResizeConfigureFlow::is_empty)
        {
            self.resize_configure_flows.remove(&surface_id);
        }
        true
    }

    pub(in crate::compositor) fn update_resize_captures_pending_metrics(
        &mut self,
        surface_id: u32,
    ) {
        let pending = self
            .resize_configure_flows
            .get(&surface_id)
            .map_or(0, ResizeConfigureFlow::captured_count);
        self.resize_flow_metrics.resize_captures_pending = pending;
        self.resize_flow_metrics.resize_captures_pending_peak = self
            .resize_flow_metrics
            .resize_captures_pending_peak
            .max(pending);
    }

    pub(in crate::compositor) fn release_resize_capture(
        &mut self,
        surface_id: u32,
        commit_sequence: u64,
    ) -> bool {
        let released = self
            .resize_configure_flows
            .get_mut(&surface_id)
            .is_some_and(|flow| flow.release_capture(commit_sequence));
        if released {
            self.resize_flow_metrics.resize_captures_released = self
                .resize_flow_metrics
                .resize_captures_released
                .saturating_add(1);
            self.update_resize_captures_pending_metrics(surface_id);
        }
        released
    }

    pub(in crate::compositor) fn update_resize_retained_configure_peak(&mut self, surface_id: u32) {
        let retained = self
            .resize_configure_flows
            .get(&surface_id)
            .map_or(0, ResizeConfigureFlow::retained_configure_count);
        self.resize_flow_metrics.maximum_retained_configures = self
            .resize_flow_metrics
            .maximum_retained_configures
            .max(retained);
    }

    pub(in crate::compositor) fn send_resize_root_window_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
    ) -> bool {
        self.send_configure_root_window_to(surface_id, width, height, &[])
            .is_some()
    }

    pub(in crate::compositor) fn send_configure_root_window_to(
        &mut self,
        surface_id: u32,
        width: u32,
        height: u32,
        states: &[xdg_toplevel::State],
    ) -> Option<u32> {
        let width = self.clamp_toplevel_width(surface_id, width);
        let height = self.clamp_toplevel_height(surface_id, height);
        let toplevel = self.toplevel_surfaces.get(&surface_id).cloned()?;

        let _ = toplevel
            .toplevel
            .send_event(xdg_toplevel::Event::Configure {
                width: width as i32,
                height: height as i32,
                states: xdg_toplevel_state_bytes(states),
            });
        let serial = self.next_configure_serial();
        let _ = toplevel
            .xdg_surface
            .send_event(xdg_surface::Event::Configure { serial });
        self.xdg_configure_serials
            .entry(surface_id)
            .or_default()
            .latest_sent = serial;
        Some(serial)
    }
}
