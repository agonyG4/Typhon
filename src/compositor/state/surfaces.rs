use super::*;

impl CompositorState {
    pub(in crate::compositor) fn allocate_surface_commit_sequence(
        &mut self,
    ) -> SurfaceCommitSequence {
        self.next_surface_commit_sequence = self.next_surface_commit_sequence.saturating_add(1);
        SurfaceCommitSequence(self.next_surface_commit_sequence)
    }

    pub(in crate::compositor) fn record_surface_commit_received(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        has_attachment_change: bool,
    ) {
        let state = self.surface_publications.entry(surface_id).or_default();
        state.latest_received = state.latest_received.max(commit_sequence);
        if has_attachment_change {
            state.latest_attachment_received = Some(
                state
                    .latest_attachment_received
                    .map_or(commit_sequence, |latest| latest.max(commit_sequence)),
            );
        }
    }

    pub(in crate::compositor) fn surface_publication_decision(
        &self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        context: SurfacePublicationContext,
    ) -> SurfacePublicationDecision {
        let Some(state) = self.surface_publications.get(&surface_id) else {
            return SurfacePublicationDecision::Publish;
        };
        if state
            .latest_published
            .is_some_and(|published| commit_sequence <= published)
        {
            return SurfacePublicationDecision::StaleAlreadyPublished;
        }
        if context == SurfacePublicationContext::ImmediateLatestAttachment
            && state
                .latest_attachment_received
                .is_some_and(|attachment| commit_sequence < attachment)
        {
            return SurfacePublicationDecision::SupersededByNewerAttachment;
        }
        SurfacePublicationDecision::Publish
    }

    pub(in crate::compositor) fn record_surface_publication(
        &mut self,
        surface_id: u32,
        root_surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        buffer_id: Option<BufferId>,
        source: SurfacePublicationSource,
        size: Option<BufferSize>,
    ) {
        let state = self.surface_publications.entry(surface_id).or_default();
        let previous_sequence = state.latest_published;
        if previous_sequence.is_some_and(|previous| commit_sequence < previous) {
            self.resize_flow_metrics
                .surface_publication_sequence_regressions = self
                .resize_flow_metrics
                .surface_publication_sequence_regressions
                .saturating_add(1);
            return;
        }
        state.latest_published = Some(commit_sequence);
        state.latest_published_buffer_id = buffer_id;
        self.note_explicit_commit_published(SurfaceCommitId::from_sequence(commit_sequence));
        self.resize_flow_metrics.surface_content_publishes = self
            .resize_flow_metrics
            .surface_content_publishes
            .saturating_add(1);
        if source == SurfacePublicationSource::SurfaceTree {
            self.subsurface_transaction_metrics
                .surface_tree_publications = self
                .subsurface_transaction_metrics
                .surface_tree_publications
                .saturating_add(1);
        }
        if compositor_debug_surface_logging_enabled() {
            let size = size
                .map(|size| format!("{}x{}", size.width, size.height))
                .unwrap_or_else(|| "detached".to_string());
            eprintln!(
                "oblivion-one compositor: surface_publish surface={} root={} commit_sequence={} buffer_id={:?} source={} previous_sequence={:?} decision=publish size={}",
                surface_id,
                root_surface_id,
                commit_sequence.get(),
                buffer_id.map(BufferId::get),
                source.as_str(),
                previous_sequence.map(SurfaceCommitSequence::get),
                size,
            );
        }
    }

    pub(in crate::compositor) fn record_surface_publication_rejection(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
        buffer_id: Option<BufferId>,
        source: SurfacePublicationSource,
        decision: SurfacePublicationDecision,
    ) {
        let publication = self
            .surface_publications
            .get(&surface_id)
            .copied()
            .unwrap_or_default();
        self.note_explicit_commit_publication_rejected(
            SurfaceCommitId::from_sequence(commit_sequence),
            decision,
            publication.latest_published,
            publication.latest_attachment_received,
        );
        self.resize_flow_metrics.surface_content_stale_rejections = self
            .resize_flow_metrics
            .surface_content_stale_rejections
            .saturating_add(1);
        if source == SurfacePublicationSource::SurfaceTree {
            self.subsurface_transaction_metrics
                .surface_tree_stale_rejections = self
                .subsurface_transaction_metrics
                .surface_tree_stale_rejections
                .saturating_add(1);
        }
        if compositor_debug_surface_logging_enabled() {
            let state = self
                .surface_publications
                .get(&surface_id)
                .copied()
                .unwrap_or_default();
            eprintln!(
                "oblivion-one compositor: surface_publish surface={} commit_sequence={} buffer_id={:?} source={} latest_published={:?} latest_attachment={:?} decision={}",
                surface_id,
                commit_sequence.get(),
                buffer_id.map(BufferId::get),
                source.as_str(),
                state.latest_published.map(SurfaceCommitSequence::get),
                state
                    .latest_attachment_received
                    .map(SurfaceCommitSequence::get),
                match decision {
                    SurfacePublicationDecision::Publish => "publish",
                    SurfacePublicationDecision::StaleAlreadyPublished => "reject_stale",
                    SurfacePublicationDecision::SupersededByNewerAttachment => {
                        "reject_superseded_attachment"
                    }
                },
            );
        }
    }

    pub(in crate::compositor) fn supersede_older_pending_attachments_for_surface(
        &mut self,
        surface_id: u32,
        new_sequence: SurfaceCommitSequence,
    ) -> Vec<wl_callback::WlCallback> {
        let mut callbacks = Vec::new();
        let mut retained_explicit = Vec::new();
        for commit in std::mem::take(&mut self.pending_explicit_sync_commits) {
            if commit.surface_id == surface_id && commit.commit_sequence < new_sequence {
                if commit.acquire_state == PendingAcquireState::Ready {
                    retained_explicit.push(commit);
                    continue;
                }
                self.note_explicit_commit_superseded(
                    commit.surface_commit_id,
                    commit.acquire_state,
                    commit.frame_callbacks.len(),
                    SurfaceCommitId::from_sequence(new_sequence),
                    "newer_attachment_arrived",
                );
                if self.external_acquire_readiness {
                    self.pending_acquire_watch_changes
                        .push(AcquireWatchChange::Cancel {
                            commit_id: commit.commit_id,
                            reason: AcquireWatchCancelReason::Superseded,
                        });
                }
                if let Some(resize) = commit.pending.resize_commit.as_deref() {
                    self.release_resize_capture(surface_id, resize.commit_sequence);
                }
                commit.pending.release_target().release();
                callbacks.extend(commit.frame_callbacks);
                self.resize_flow_metrics
                    .surface_pending_attachments_superseded = self
                    .resize_flow_metrics
                    .surface_pending_attachments_superseded
                    .saturating_add(1);
                self.resize_flow_metrics.surface_cross_queue_supersessions = self
                    .resize_flow_metrics
                    .surface_cross_queue_supersessions
                    .saturating_add(1);
                if compositor_debug_surface_logging_enabled() {
                    eprintln!(
                        "oblivion-one compositor: surface_commit surface={} old_sequence={} new_sequence={} old_buffer_id={} decision=supersede_pending_attachment acquire_watch_canceled={}",
                        surface_id,
                        commit.commit_sequence.get(),
                        new_sequence.get(),
                        commit.pending.data.buffer_id().get(),
                        self.external_acquire_readiness,
                    );
                }
            } else {
                retained_explicit.push(commit);
            }
        }
        self.pending_explicit_sync_commits = retained_explicit;

        let mut retained_trees = Vec::new();
        for transaction in std::mem::take(&mut self.pending_surface_tree_transactions) {
            let supersedes = transaction.nodes.iter().any(|(node_surface_id, commit)| {
                *node_surface_id == surface_id
                    && commit.commit_sequence < new_sequence
                    && commit.attachment.is_some()
            });
            if supersedes {
                if transaction.is_ready() {
                    retained_trees.push(transaction);
                    continue;
                }
                let root_surface_id = transaction.root_surface_id;
                let replacement = SurfaceCommitId::from_sequence(new_sequence);
                let acquire_state = if transaction.is_ready() {
                    PendingAcquireState::Ready
                } else {
                    PendingAcquireState::RegistrationPending
                };
                for (_, commit) in &transaction.nodes {
                    if commit.attachment.is_some() {
                        self.note_explicit_commit_superseded(
                            commit.commit_id,
                            acquire_state,
                            commit.frame_callbacks.len(),
                            replacement,
                            "newer_surface_tree_attachment_arrived",
                        );
                    }
                }
                let released = self.release_pending_surface_tree_transaction(
                    transaction,
                    AcquireWatchCancelReason::Superseded,
                );
                callbacks.extend(released.callbacks);
                if let Some(resize_commit) = released.resize_commit {
                    self.release_detached_resize_capture(root_surface_id, resize_commit);
                }
                self.resize_flow_metrics
                    .surface_pending_attachments_superseded = self
                    .resize_flow_metrics
                    .surface_pending_attachments_superseded
                    .saturating_add(1);
                self.resize_flow_metrics.surface_cross_queue_supersessions = self
                    .resize_flow_metrics
                    .surface_cross_queue_supersessions
                    .saturating_add(1);
            } else {
                retained_trees.push(transaction);
            }
        }
        self.pending_surface_tree_transactions = retained_trees;
        callbacks
    }

    pub(in crate::compositor) fn mark_render_damage_presented(&mut self) {
        for surface in &mut self.renderable_surfaces {
            if let Some(journal) = self.surface_damage_journals.get(&surface.surface_id) {
                let last_seen = self
                    .presented_surface_commits
                    .get(&surface.surface_id)
                    .copied()
                    .unwrap_or_default();
                let _ = journal.damage_since(
                    last_seen,
                    surface.buffer_size().width,
                    surface.buffer_size().height,
                );
                self.presented_surface_commits
                    .insert(surface.surface_id, journal.current_commit());
            }
            surface.damage = RenderableSurfaceDamage::Empty;
        }
        for surface in self.client_cursor_surfaces.values_mut() {
            if let Some(journal) = self.surface_damage_journals.get(&surface.surface_id) {
                self.presented_surface_commits
                    .insert(surface.surface_id, journal.current_commit());
            }
            surface.damage = RenderableSurfaceDamage::Empty;
        }
    }

    pub(in crate::compositor) fn record_surface_damage_commit(
        &mut self,
        surface_id: u32,
        damage: RenderableSurfaceDamage,
        width: u32,
        height: u32,
    ) {
        self.surface_damage_journals
            .entry(surface_id)
            .or_insert_with(|| SurfaceDamageJournal::new(64))
            .record(damage, width, height);
    }

    pub(in crate::compositor) fn new(syncobj_device: Option<DrmSyncobjDevice>) -> Self {
        let default_dmabuf_device = default_dmabuf_main_device();
        Self {
            frame_clock_start: Some(Instant::now()),
            dmabuf_feedback: EglGlesDmabufFeedback::default(),
            dmabuf_main_device: default_dmabuf_device
                .as_ref()
                .map(|device| device.rdev)
                .unwrap_or(0),
            dmabuf_main_device_path: default_dmabuf_device.map(|device| device.path),
            syncobj_device,
            clipboard_bridge: Some(Box::new(NoopClipboardBridge)),
            ..Self::default()
        }
    }

    pub(in crate::compositor) fn allocate_buffer_identity(&mut self) -> Option<BufferIdentity> {
        self.buffer_ids.allocate()
    }

    pub(in crate::compositor) fn next_render_generation_value(&self) -> u64 {
        self.surface_tree_generation
            .unwrap_or_else(|| self.render_generation.saturating_add(1))
    }

    pub(in crate::compositor) fn begin_surface_tree_publication(&mut self) {
        debug_assert!(self.surface_tree_generation.is_none());
        self.surface_tree_generation = Some(self.render_generation.saturating_add(1));
    }

    pub(in crate::compositor) fn finish_surface_tree_publication(&mut self) {
        self.surface_tree_generation = None;
    }

    pub(in crate::compositor) fn set_render_generation(
        &mut self,
        generation: u64,
        cause: RenderGenerationCause,
    ) {
        self.render_generation = generation;
        self.render_generation_cause = cause;
        if !matches!(
            cause,
            RenderGenerationCause::CursorCommit
                | RenderGenerationCause::CursorMotion
                | RenderGenerationCause::CursorState
        ) {
            self.scene_render_generation = generation;
        }
    }

    pub(in crate::compositor) fn advance_render_generation(
        &mut self,
        cause: RenderGenerationCause,
    ) -> u64 {
        let generation = self.next_render_generation_value();
        self.set_render_generation(generation, cause);
        self.update_all_active_confined_pointer_regions(cause.as_str());
        generation
    }

    pub(in crate::compositor) fn render_generation_cause(&self) -> RenderGenerationCause {
        self.render_generation_cause
    }

    pub(in crate::compositor) fn set_dmabuf_feedback(
        &mut self,
        feedback: EglGlesDmabufFeedback,
        main_device: Option<u64>,
        main_device_path: Option<String>,
    ) {
        self.dmabuf_feedback = feedback;
        self.dmabuf_main_device = main_device.filter(|device| *device != 0).unwrap_or(0);
        self.dmabuf_main_device_path = main_device_path.filter(|path| !path.is_empty());
    }

    pub(in crate::compositor) fn set_output_size(&mut self, width: u32, height: u32) -> bool {
        let output_size = OutputSize::new(width, height);
        if self.output_size == output_size {
            return false;
        }

        self.output_size = output_size;
        self.send_output_mode_to_bound_outputs();
        self.reconfigure_layer_surfaces_for_output_change();
        self.reconfigure_stateful_windows_for_output_size();
        true
    }

    pub(in crate::compositor) fn set_output_scale_factor(&mut self, scale_factor: f64) -> bool {
        let output_scale = OutputScale::from_factor(scale_factor);
        if self.output_scale == output_scale {
            return false;
        }

        self.output_scale = output_scale;
        self.send_output_scale_to_bound_outputs();
        self.send_fractional_scale_to_bound_surfaces();
        self.advance_render_generation(RenderGenerationCause::OutputChange);
        true
    }

    pub(in crate::compositor) fn set_output_refresh_hz(&mut self, refresh_hz: u32) -> bool {
        let output_refresh = OutputRefreshRate::from_hz(refresh_hz);
        if self.output_refresh == output_refresh {
            return false;
        }

        self.output_refresh = output_refresh;
        self.send_output_mode_to_bound_outputs();
        true
    }

    pub fn note_xdg_toplevel_created(&mut self, app_id: impl Into<String>) {
        self.xdg_toplevels += 1;
        self.last_app_id = Some(app_id.into());
    }

    pub(in crate::compositor) fn note_xdg_popup_created(&mut self) {
        self.xdg_popups += 1;
    }

    pub(in crate::compositor) fn next_configure_serial(&mut self) -> u32 {
        self.next_configure_serial = self.next_configure_serial.saturating_add(1);
        self.next_configure_serial
    }

    pub(in crate::compositor) fn allocate_surface_id(&mut self) -> u32 {
        self.next_surface_id = self.next_surface_id.saturating_add(1).max(1);
        self.next_surface_id
    }

    pub(in crate::compositor) fn frame_callback_time_ms(&mut self) -> u32 {
        let start = self.frame_clock_start.get_or_insert_with(Instant::now);
        start.elapsed().as_millis() as u32
    }

    pub(in crate::compositor) fn focus_surface(&mut self, surface: wl_surface::WlSurface) {
        self.set_desktop_focus(surface, "focus");
    }

    pub(in crate::compositor) fn set_desktop_focus(
        &mut self,
        surface: wl_surface::WlSurface,
        reason: &'static str,
    ) {
        let old_surface_id = self.focused_surface.as_ref().map(compositor_surface_id);
        let new_surface_id = compositor_surface_id(&surface);
        let changed = !self
            .focused_surface
            .as_ref()
            .is_some_and(|current| same_surface_resource(current, &surface));
        if changed {
            pointer_debug_log(format!(
                "focus change reason={} old={:?} new={}",
                reason, old_surface_id, new_surface_id
            ));
            focus_debug_log(|| {
                format!("focus_enter reason={reason} old={old_surface_id:?} new={new_surface_id}")
            });
        }
        self.focused_surface = Some(surface.clone());
        self.ensure_keyboard_focus(&surface);
        self.apply_pending_pointer_constraint_state_for_surface(new_surface_id);
        if !self
            .layer_surfaces
            .contains_key(&self.root_surface_id_for_surface(new_surface_id))
        {
            self.last_application_keyboard_focus = Some(surface);
        }
    }

    pub(in crate::compositor) fn focused_client_id(&self) -> Option<ClientId> {
        self.focused_surface
            .as_ref()
            .and_then(Resource::client)
            .map(|client| client.id())
    }

    pub(in crate::compositor) fn client_has_focus(&self, client_id: &ClientId) -> bool {
        self.focused_client_id()
            .as_ref()
            .is_some_and(|focused_client_id| focused_client_id == client_id)
    }

    pub(in crate::compositor) fn remember_input_serial(
        &mut self,
        serial: u32,
        surface: wl_surface::WlSurface,
    ) {
        self.recent_input_serials
            .retain(|input| input.serial != serial);
        self.recent_input_serials
            .push(InputSerial { serial, surface });
        const MAX_RECENT_INPUT_SERIALS: usize = 16;
        let excess = self
            .recent_input_serials
            .len()
            .saturating_sub(MAX_RECENT_INPUT_SERIALS);
        if excess > 0 {
            self.recent_input_serials.drain(0..excess);
        }
    }

    pub(in crate::compositor) fn has_recent_input_serial_for_surface(
        &self,
        serial: u32,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.recent_input_serials
            .iter()
            .any(|input| input.serial == serial && input.surface.id().same_client_as(&surface.id()))
    }

    pub(in crate::compositor) fn client_has_recent_input_serial(
        &self,
        client_id: &ClientId,
        serial: u32,
    ) -> bool {
        self.recent_input_serials.iter().any(|input| {
            input.serial == serial
                && input
                    .surface
                    .client()
                    .is_some_and(|client| client.id() == *client_id)
        })
    }

    pub(in crate::compositor) fn register_data_source(
        &mut self,
        source: wl_data_source::WlDataSource,
        client_id: ClientId,
    ) {
        self.selection_state.begin_source(source.id().protocol_id());
        self.data_sources.insert(
            source.id(),
            ClipboardDataSource {
                source,
                client_id,
                mime_types: Vec::new(),
            },
        );
    }

    pub(in crate::compositor) fn offer_data_source_mime_type(
        &mut self,
        source: &wl_data_source::WlDataSource,
        mime_type: String,
    ) {
        self.selection_state
            .offer_source_mime_type(source.id().protocol_id(), mime_type.clone());
        let Some(binding) = self.data_sources.get_mut(&source.id()) else {
            return;
        };
        if mime_type.is_empty()
            || mime_type.len() > 4096
            || binding.mime_types.len() >= 128
            || binding
                .mime_types
                .iter()
                .any(|existing| existing == &mime_type)
        {
            return;
        }
        binding.mime_types.push(mime_type);
    }

    pub(in crate::compositor) fn remove_data_source(
        &mut self,
        source: &wl_data_source::WlDataSource,
    ) {
        self.data_sources.remove(&source.id());
        self.selection_state
            .remove_source(source.id().protocol_id());
        if self
            .active_clipboard
            .as_ref()
            .is_some_and(|selection| match &selection.source {
                ClipboardSourceBackend::InternalWayland {
                    source: active_source,
                    ..
                } => same_wayland_resource(active_source, source),
                ClipboardSourceBackend::HostBridge { .. } => false,
            })
        {
            self.active_clipboard = None;
            self.next_clipboard_generation = self.next_clipboard_generation.saturating_add(1);
            if let Some(bridge) = self.clipboard_bridge.as_mut() {
                let _ = bridge.clear_internal_selection();
            }
            self.data_offers.clear();
            self.publish_clipboard_clear_to_data_devices();
        }
    }

    pub(in crate::compositor) fn clear_dead_active_clipboard_source(&mut self) {
        let active_source = self.active_clipboard.as_ref().and_then(|selection| {
            if let ClipboardSourceBackend::InternalWayland { source, .. } = &selection.source {
                Some(source.clone())
            } else {
                None
            }
        });
        let Some(source) = active_source else {
            return;
        };
        if source.is_alive() && source.client().is_some() {
            return;
        }
        self.remove_data_source(&source);
    }

    pub(in crate::compositor) fn register_data_device(
        &mut self,
        device: wl_data_device::WlDataDevice,
        client_id: ClientId,
        seat_id: ObjectId,
    ) {
        self.data_devices
            .retain(|binding| binding.device.is_alive());
        self.data_devices.push(ClipboardDataDevice {
            device: device.clone(),
            client_id: client_id.clone(),
            seat_id,
        });
        if self.client_has_focus(&client_id) {
            self.publish_clipboard_to_data_device(&device);
        }
    }

    pub(in crate::compositor) fn remove_data_device(
        &mut self,
        device: &wl_data_device::WlDataDevice,
    ) {
        self.data_devices
            .retain(|binding| !same_wayland_resource(&binding.device, device));
        self.data_offers.retain(|_, offer| {
            offer.offer.is_alive() && !offer.offer.id().same_client_as(&device.id())
        });
    }

    pub(in crate::compositor) fn set_clipboard_selection(
        &mut self,
        client_id: &ClientId,
        source: Option<wl_data_source::WlDataSource>,
        serial: u32,
    ) -> bool {
        if !self.client_has_focus(client_id)
            || !self.client_has_recent_input_serial(client_id, serial)
        {
            return false;
        }

        let Some(source) = source else {
            self.active_clipboard = None;
            self.selection_state.clear_clipboard_selection();
            self.next_clipboard_generation = self.next_clipboard_generation.saturating_add(1);
            if let Some(bridge) = self.clipboard_bridge.as_mut() {
                let _ = bridge.clear_internal_selection();
            }
            self.data_offers.clear();
            self.publish_clipboard_to_focused_client();
            return true;
        };

        let Some(binding) = self.data_sources.get(&source.id()).cloned() else {
            return false;
        };
        if binding.client_id != *client_id || !source.is_alive() || binding.mime_types.is_empty() {
            return false;
        }

        if let Some(previous) = self.active_clipboard.as_ref()
            && let ClipboardSourceBackend::InternalWayland {
                source: previous_source,
                ..
            } = &previous.source
            && !same_wayland_resource(previous_source, &source)
            && previous_source.is_alive()
        {
            previous_source.cancelled();
        }

        self.next_clipboard_generation = self.next_clipboard_generation.saturating_add(1);
        let generation = self.next_clipboard_generation;
        self.selection_state
            .set_clipboard_selection_from_source(source.id().protocol_id());
        self.active_clipboard = Some(ActiveClipboard {
            generation,
            source: ClipboardSourceBackend::InternalWayland {
                source: binding.source,
                client_id: binding.client_id,
            },
            mime_types: binding.mime_types.clone(),
        });
        if let Some(bridge) = self.clipboard_bridge.as_mut() {
            let _ = bridge.publish_internal_selection(generation, binding.mime_types);
        }
        self.data_offers.clear();
        self.publish_clipboard_to_focused_client();
        true
    }

    pub(in crate::compositor) fn install_host_clipboard_selection(
        &mut self,
        offer_id: HostClipboardOfferId,
        mime_types: Vec<String>,
    ) {
        let mime_types = normalize_selection_mime_types(mime_types);
        if mime_types.is_empty() {
            self.clear_host_clipboard_selection();
            return;
        }
        self.next_clipboard_generation = self.next_clipboard_generation.saturating_add(1);
        self.active_clipboard = Some(ActiveClipboard {
            generation: self.next_clipboard_generation,
            source: ClipboardSourceBackend::HostBridge { offer_id },
            mime_types,
        });
        self.data_offers.clear();
        self.publish_clipboard_to_focused_client();
    }

    pub(in crate::compositor) fn clear_host_clipboard_selection(&mut self) {
        self.next_clipboard_generation = self.next_clipboard_generation.saturating_add(1);
        if self.active_clipboard.as_ref().is_some_and(|selection| {
            matches!(selection.source, ClipboardSourceBackend::HostBridge { .. })
        }) {
            self.active_clipboard = None;
            self.data_offers.clear();
            self.publish_clipboard_to_focused_client();
        }
    }

    pub(in crate::compositor) fn poll_clipboard_bridge(&mut self) {
        let Some(bridge) = self.clipboard_bridge.as_mut() else {
            return;
        };
        let events = bridge.poll_events();
        for event in events {
            match event {
                ClipboardBridgeEvent::HostSelectionChanged {
                    offer_id,
                    mime_types,
                } => self.install_host_clipboard_selection(offer_id, mime_types),
                ClipboardBridgeEvent::HostSelectionCleared => self.clear_host_clipboard_selection(),
            }
        }
    }

    pub(in crate::compositor) fn publish_clipboard_to_focused_client(&mut self) {
        let Some(client_id) = self.focused_client_id() else {
            return;
        };
        let devices = self
            .data_devices
            .iter()
            .filter(|binding| {
                binding.client_id == client_id
                    && binding.device.is_alive()
                    && binding.seat_id.interface().name == "wl_seat"
            })
            .map(|binding| binding.device.clone())
            .collect::<Vec<_>>();
        for device in devices {
            self.publish_clipboard_to_data_device(&device);
        }
    }

    pub(in crate::compositor) fn publish_clipboard_clear_to_data_devices(&mut self) {
        let devices = self
            .data_devices
            .iter()
            .filter(|binding| {
                binding.device.is_alive() && binding.seat_id.interface().name == "wl_seat"
            })
            .map(|binding| binding.device.clone())
            .collect::<Vec<_>>();
        for device in devices {
            let _ = device.send_event(wl_data_device::Event::Selection { id: None });
        }
    }

    pub(in crate::compositor) fn publish_clipboard_to_data_device(
        &mut self,
        device: &wl_data_device::WlDataDevice,
    ) {
        if !device.is_alive() {
            return;
        }
        let Some(selection) = self.active_clipboard.clone() else {
            let _ = device.send_event(wl_data_device::Event::Selection { id: None });
            return;
        };
        if selection.mime_types.is_empty() {
            let _ = device.send_event(wl_data_device::Event::Selection { id: None });
            return;
        }
        let Some(client) = device.client() else {
            return;
        };
        let Some(handle) = device.handle().upgrade() else {
            return;
        };
        let display = DisplayHandle::from(handle);
        let Ok(offer) = client
            .create_resource::<wl_data_offer::WlDataOffer, DataOfferData, CompositorState>(
                &display,
                device.version().min(3),
                DataOfferData {
                    target_client_id: client.id(),
                    source_generation: selection.generation,
                },
            )
        else {
            return;
        };

        self.data_offers.insert(
            offer.id(),
            ClipboardDataOffer {
                offer: offer.clone(),
                target_client_id: client.id(),
                source_generation: selection.generation,
                mime_types: selection.mime_types.clone(),
            },
        );
        let _ = device.send_event(wl_data_device::Event::DataOffer { id: offer.clone() });
        for mime_type in selection.mime_types {
            let _ = offer.send_event(wl_data_offer::Event::Offer { mime_type });
        }
        let _ = device.send_event(wl_data_device::Event::Selection { id: Some(offer) });
    }

    pub(in crate::compositor) fn receive_clipboard_offer(
        &mut self,
        offer: &wl_data_offer::WlDataOffer,
        client_id: &ClientId,
        source_generation: u64,
        mime_type: String,
        fd: OwnedFd,
    ) {
        let Some(binding) = self.data_offers.get(&offer.id()) else {
            return;
        };
        let Some(selection) = self.active_clipboard.as_ref() else {
            return;
        };
        if binding.target_client_id != *client_id
            || binding.source_generation != selection.generation
            || source_generation != selection.generation
            || !binding.mime_types.iter().any(|mime| mime == &mime_type)
        {
            return;
        }
        match &selection.source {
            ClipboardSourceBackend::InternalWayland { source, client_id } => {
                let active_source_client_matches = self
                    .data_sources
                    .get(&source.id())
                    .is_some_and(|registered| registered.client_id == *client_id);
                if !active_source_client_matches || !source.is_alive() {
                    return;
                }
                let _ = source.send_event(wl_data_source::Event::Send {
                    mime_type,
                    fd: fd.as_fd(),
                });
            }
            ClipboardSourceBackend::HostBridge { offer_id } => {
                if let Some(bridge) = self.clipboard_bridge.as_mut() {
                    let _ = bridge.request_host_data(*offer_id, mime_type, fd);
                }
            }
        }
    }

    pub(in crate::compositor) fn register_surface_resource(
        &mut self,
        surface_id: u32,
        surface: wl_surface::WlSurface,
    ) {
        self.surface_resources.entry(surface_id).or_insert(surface);
    }

    pub(in crate::compositor) fn register_surface_client(
        &mut self,
        surface_id: u32,
        client_id: ClientId,
    ) {
        self.surface_client_ids
            .entry(surface_id)
            .or_insert(client_id);
    }

    pub(in crate::compositor) fn register_output_resource(&mut self, output: wl_output::WlOutput) {
        if self
            .output_resources
            .iter()
            .any(|resource| same_wayland_resource(resource, &output))
        {
            return;
        }

        send_output_description(
            &output,
            self.output_size,
            self.output_scale,
            self.output_refresh,
        );
        self.output_resources.push(output);
    }

    pub(in crate::compositor) fn unregister_output_resource(
        &mut self,
        output: &wl_output::WlOutput,
    ) {
        let output_id = output.id().protocol_id();
        self.output_resources
            .retain(|resource| !same_wayland_resource(resource, output));
        self.surface_entered_outputs
            .retain(|(_, entered_output_id)| *entered_output_id != output_id);
    }

    pub(in crate::compositor) fn send_output_mode_to_bound_outputs(&self) {
        for output in &self.output_resources {
            send_output_mode(output, self.output_size, self.output_refresh);
            send_output_done_if_supported(output);
        }
    }

    pub(in crate::compositor) fn send_output_scale_to_bound_outputs(&self) {
        for output in &self.output_resources {
            send_output_scale(output, self.output_scale);
            send_output_done_if_supported(output);
        }
    }

    pub(in crate::compositor) fn register_fractional_scale_resource(
        &mut self,
        surface: &wl_surface::WlSurface,
        fractional_scale: wp_fractional_scale_v1::WpFractionalScaleV1,
    ) {
        let surface_id = compositor_surface_id(surface);

        fractional_scale.preferred_scale(self.output_scale.preferred_scale());
        self.fractional_scale_resources
            .entry(surface_id)
            .or_default()
            .push(fractional_scale);
    }

    pub(in crate::compositor) fn unregister_fractional_scale_resources_for_surface(
        &mut self,
        surface_id: u32,
    ) {
        self.fractional_scale_resources.remove(&surface_id);
    }

    pub(in crate::compositor) fn unregister_fractional_scale_resource(
        &mut self,
        surface_id: u32,
        resource_id: u32,
    ) {
        if let Some(resources) = self.fractional_scale_resources.get_mut(&surface_id) {
            resources.retain(|resource| resource.id().protocol_id() != resource_id);
            if resources.is_empty() {
                self.fractional_scale_resources.remove(&surface_id);
            }
        }
    }

    pub(in crate::compositor) fn send_fractional_scale_to_bound_surfaces(&self) {
        for fractional_scales in self.fractional_scale_resources.values() {
            for fractional_scale in fractional_scales {
                fractional_scale.preferred_scale(self.output_scale.preferred_scale());
            }
        }
    }

    pub(in crate::compositor) fn ensure_surface_entered_outputs(
        &mut self,
        surface: &wl_surface::WlSurface,
    ) {
        let surface_id = compositor_surface_id(surface);
        for output in &self.output_resources {
            if !resource_belongs_to_surface_client(output, surface) {
                continue;
            }
            let output_id = output.id().protocol_id();
            if !self.surface_entered_outputs.insert((surface_id, output_id)) {
                continue;
            }
            let _ = surface.send_event(wl_surface::Event::Enter {
                output: output.clone(),
            });
        }
    }

    pub(in crate::compositor) fn reconfigure_stateful_windows_for_output_size(&mut self) {
        let toplevels = self
            .toplevel_surfaces
            .iter()
            .filter_map(|(surface_id, toplevel)| {
                let mode = toplevel.window.mode();
                (mode != ToplevelMode::Floating && !toplevel.window.is_minimized())
                    .then_some((*surface_id, mode))
            })
            .collect::<Vec<_>>();

        for (surface_id, mode) in toplevels {
            let geometry = self.window_geometry_for_mode(mode);
            self.send_configure_root_window_to(
                surface_id,
                geometry.width,
                geometry.height,
                mode.xdg_states(),
            );
            self.set_surface_placement_with_cause(
                surface_id,
                geometry.placement,
                RenderGenerationCause::OutputChange,
            );
            if mode == ToplevelMode::Fullscreen {
                self.refresh_fullscreen_presentation_owner(surface_id);
            }
        }
    }

    pub(in crate::compositor) fn teardown_surface_resource(
        &mut self,
        surface_id: u32,
        reason: SurfaceTeardownReason,
    ) -> SurfaceTeardownResult {
        let resource_known = self.surface_resources.contains_key(&surface_id)
            || self
                .renderable_surfaces
                .iter()
                .any(|surface| surface.surface_id == surface_id);
        let before = self.renderable_surfaces.len();
        self.unregister_surface_resource(surface_id);
        let removed = before.saturating_sub(self.renderable_surfaces.len());
        if compositor_debug_surface_logging_enabled() {
            eprintln!(
                "oblivion-one compositor: surface_teardown surface={} reason={:?} known={} removed_renderables={}",
                surface_id, reason, resource_known, removed
            );
        }
        SurfaceTeardownResult {
            removed_resource: resource_known,
            removed_renderables: removed,
        }
    }

    pub(in crate::compositor) fn unregister_surface_resource(&mut self, surface_id: u32) {
        self.surface_damage_journals.remove(&surface_id);
        self.presented_surface_commits.remove(&surface_id);
        self.cancel_pending_surface_trees_for_surface(
            surface_id,
            AcquireWatchCancelReason::SurfaceDestroyed,
        );
        self.cancel_pending_acquire_commits_for_surface(
            surface_id,
            AcquireWatchCancelReason::SurfaceDestroyed,
        );
        self.discard_pending_presentation_feedbacks_for_surface(surface_id);
        if let Some(feedbacks) = self
            .pending_surface_presentation_feedbacks
            .remove(&surface_id)
        {
            for feedback in feedbacks {
                feedback.feedback.discarded();
            }
        }
        self.deactivate_pointer_constraints_for_surface(surface_id, false);
        let cached = self.subsurface_transactions.remove_subtree(surface_id);
        self.release_cached_subsurface_commits(cached);
        self.cleanup_subsurface_stack_state_for_surface(surface_id);
        self.surface_resources.remove(&surface_id);
        self.surface_client_ids.remove(&surface_id);
        self.clear_surface_role(surface_id);
        self.cursor_surface_ids.remove(&surface_id);
        let removed_cursor_content = self.client_cursor_surfaces.remove(&surface_id).is_some();
        let active_cursor_pointer = self
            .active_client_cursor
            .as_ref()
            .filter(|active| active.surface_id == surface_id)
            .map(|active| active.pointer.clone());
        if let Some(pointer) = active_cursor_pointer {
            self.active_client_cursor = None;
            self.cursor_visibility.client_cursor_pointer = None;
            self.cursor_visibility.client_hidden_pointer = Some(pointer);
            pointer_debug_log(format!(
                "cursor cleanup surface={} reason=active-surface-destroyed",
                surface_id
            ));
            self.advance_render_generation(RenderGenerationCause::CursorState);
            self.sync_cursor_visibility_request();
        } else if removed_cursor_content {
            pointer_debug_log(format!(
                "cursor cleanup surface={} reason=inactive-surface-destroyed",
                surface_id
            ));
        }
        self.unregister_fractional_scale_resources_for_surface(surface_id);
        self.surface_placements.remove(&surface_id);
        self.current_surface_buffers.remove(&surface_id);
        self.surface_window_geometries.remove(&surface_id);
        self.pending_surface_window_geometries.remove(&surface_id);
        self.configured_xdg_surfaces.remove(&surface_id);
        self.surface_entered_outputs
            .retain(|(entered_surface_id, _)| *entered_surface_id != surface_id);
        self.unregister_toplevel_surface(surface_id);
        self.unregister_popup_surface(surface_id);
        self.teardown_layer_surface(surface_id);
        self.clear_resize_state_for_surfaces_with_reason(
            &[surface_id],
            WindowInteractionEndReason::SurfaceDestroyed,
        );
        self.surface_placements
            .retain(|_, placement| placement.parent_surface_id != Some(surface_id));
        let mut removed_surface_ids = vec![surface_id];
        removed_surface_ids.extend(
            self.renderable_surfaces
                .iter()
                .filter(|surface| surface.placement.parent_surface_id == Some(surface_id))
                .map(|surface| surface.surface_id),
        );
        removed_surface_ids.sort_unstable();
        removed_surface_ids.dedup();
        self.clear_popup_grab_for_surface_ids(&removed_surface_ids);
        self.popup_grab_stack
            .retain(|surface_id| !removed_surface_ids.contains(surface_id));
        self.recent_input_serials
            .retain(|input| !removed_surface_ids.contains(&compositor_surface_id(&input.surface)));
        self.clear_pointer_button_state_for_removed_surfaces(
            &removed_surface_ids,
            "surface-destroyed",
        );

        for removed_surface_id in &removed_surface_ids {
            self.unregister_popup_surface(*removed_surface_id);
            if let Some(buffer) = self.active_dmabuf_buffers.remove(removed_surface_id) {
                self.queue_dmabuf_buffer_release(buffer);
            }
        }
        let previous_renderable_count = self.renderable_surfaces.len();
        self.renderable_surfaces.retain(|surface| {
            surface.surface_id != surface_id
                && surface.placement.parent_surface_id != Some(surface_id)
        });
        if self.renderable_surfaces.len() != previous_renderable_count {
            self.advance_render_generation(RenderGenerationCause::SurfaceUnmap);
        }

        if self
            .focused_surface
            .as_ref()
            .is_some_and(|surface| compositor_surface_id(surface) == surface_id)
        {
            self.focused_surface = None;
            focus_debug_log(|| format!("focus_leave reason=surface_destroyed old={surface_id}"));
        }

        if self
            .keyboard_surface
            .as_ref()
            .is_some_and(|surface| compositor_surface_id(surface) == surface_id)
        {
            self.keyboard_surface = None;
        }

        if self
            .pointer_surface
            .as_ref()
            .is_some_and(|surface| removed_surface_ids.contains(&compositor_surface_id(surface)))
        {
            self.pointer_surface = None;
            self.clear_pointer_constraint();
            self.cursor_visibility.client_hidden_pointer = None;
            self.cursor_visibility.client_cursor_pointer = None;
            self.sync_cursor_visibility_request();
        }
        self.pointer_entered_surfaces
            .retain(|(_, surface)| !removed_surface_ids.contains(&compositor_surface_id(surface)));
        self.pointer_enter_serials
            .retain(|entry| !removed_surface_ids.contains(&compositor_surface_id(&entry.surface)));
    }
}

fn focus_debug_log(message: impl FnOnce() -> String) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ENABLED.get_or_init(|| std::env::var_os("OBLIVION_ONE_FOCUS_DEBUG").is_some()) {
        eprintln!("oblivion-one focus: {}", message());
    }
}

#[cfg(test)]
mod ordered_publication_tests {
    use super::*;

    #[test]
    fn ordered_ready_commit_ignores_newer_received_attachment() {
        let mut state = CompositorState::default();
        state.record_surface_commit_received(7, SurfaceCommitSequence(10), true);
        state.record_surface_commit_received(7, SurfaceCommitSequence(11), true);

        assert_eq!(
            state.surface_publication_decision(
                7,
                SurfaceCommitSequence(10),
                SurfacePublicationContext::OrderedExplicitSyncQueue,
            ),
            SurfacePublicationDecision::Publish
        );
        assert_eq!(
            state.surface_publication_decision(
                7,
                SurfaceCommitSequence(10),
                SurfacePublicationContext::ImmediateLatestAttachment,
            ),
            SurfacePublicationDecision::SupersededByNewerAttachment
        );
    }

    #[test]
    fn ordered_queue_rejects_already_published_sequence() {
        let mut state = CompositorState::default();
        state.surface_publications.insert(
            7,
            SurfacePublicationState {
                latest_published: Some(SurfaceCommitSequence(11)),
                ..SurfacePublicationState::default()
            },
        );

        assert_eq!(
            state.surface_publication_decision(
                7,
                SurfaceCommitSequence(11),
                SurfacePublicationContext::OrderedExplicitSyncQueue,
            ),
            SurfacePublicationDecision::StaleAlreadyPublished
        );
    }
}
