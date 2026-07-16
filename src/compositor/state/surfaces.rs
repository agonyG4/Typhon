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
    pub(in crate::compositor) fn capture_surface_damage_presentation(
        &self,
    ) -> SurfaceDamagePresentation {
        let mut sampled_commits = Vec::new();
        for surface_id in self
            .renderable_surfaces
            .iter()
            .map(|surface| surface.surface_id)
            .chain(self.client_cursor_surfaces.keys().copied())
        {
            let Some(generation) = self
                .surface_presentation_generations
                .get(&surface_id)
                .copied()
            else {
                continue;
            };
            let Some(commit) = self
                .surface_damage_journals
                .get(&surface_id)
                .map(SurfaceDamageJournal::current_commit)
            else {
                continue;
            };
            let key = SurfacePresentationKey {
                surface_id,
                generation,
            };
            if !sampled_commits.iter().any(|(sampled, _)| *sampled == key) {
                sampled_commits.push((key, commit));
            }
        }
        SurfaceDamagePresentation { sampled_commits }
    }
    pub(in crate::compositor) fn capture_surface_damage_presentation_for_surface(
        &self,
        surface_id: u32,
    ) -> SurfaceDamagePresentation {
        let Some(generation) = self
            .surface_presentation_generations
            .get(&surface_id)
            .copied()
        else {
            return SurfaceDamagePresentation {
                sampled_commits: Vec::new(),
            };
        };
        let Some(commit) = self
            .surface_damage_journals
            .get(&surface_id)
            .map(SurfaceDamageJournal::current_commit)
        else {
            return SurfaceDamagePresentation {
                sampled_commits: Vec::new(),
            };
        };
        SurfaceDamagePresentation {
            sampled_commits: vec![(
                SurfacePresentationKey {
                    surface_id,
                    generation,
                },
                commit,
            )],
        }
    }
    pub(in crate::compositor) fn commit_surface_damage_presented(
        &mut self,
        token: SurfaceDamagePresentation,
    ) {
        for (key, sampled_commit) in token.sampled_commits {
            if self
                .surface_presentation_generations
                .get(&key.surface_id)
                .copied()
                != Some(key.generation)
            {
                continue;
            }
            self.presented_surface_commits
                .insert(key.surface_id, sampled_commit);
            let Some(journal) = self.surface_damage_journals.get(&key.surface_id) else {
                continue;
            };
            for surface in self
                .renderable_surfaces
                .iter_mut()
                .filter(|surface| surface.surface_id == key.surface_id)
                .chain(
                    self.client_cursor_surfaces
                        .values_mut()
                        .filter(|surface| surface.surface_id == key.surface_id),
                )
            {
                surface.damage = match journal.damage_since(
                    sampled_commit,
                    surface.buffer_size().width,
                    surface.buffer_size().height,
                ) {
                    DamageSince::Empty => RenderableSurfaceDamage::Empty,
                    DamageSince::Known(damage) => damage,
                    DamageSince::HistoryLost => RenderableSurfaceDamage::Full,
                };
            }
        }
    }
    pub(in crate::compositor) fn mark_render_damage_presented(&mut self) {
        let token = self.capture_surface_damage_presentation();
        self.commit_surface_damage_presented(token);
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
            next_window_id: 1,
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
        self.reconcile_all_surface_output_memberships();
        self.advance_render_generation(RenderGenerationCause::OutputChange);
        true
    }

    pub(in crate::compositor) fn set_output_preferred_transform(
        &mut self,
        transform: wl_output::Transform,
    ) -> bool {
        if self.preferred_output_transform == Some(transform) {
            return false;
        }
        self.preferred_output_transform = Some(transform);
        self.reconcile_all_surface_output_memberships();
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
        self.next_surface_presentation_generation = self
            .next_surface_presentation_generation
            .checked_add(1)
            .expect("surface presentation generation overflow");
        self.surface_presentation_generations.insert(
            self.next_surface_id,
            self.next_surface_presentation_generation,
        );
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
            self.focus_generation = self.focus_generation.wrapping_add(1);
            pointer_debug_log(format!(
                "focus change reason={} old={:?} new={}",
                reason, old_surface_id, new_surface_id
            ));
            focus_debug_log(|| {
                format!("focus_enter reason={reason} old={old_surface_id:?} new={new_surface_id}")
            });
        }
        self.focused_surface = Some(surface.clone());
        self.focused_window_id =
            self.window_id_for_surface(self.root_surface_id_for_surface(new_surface_id));
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
        kind: InputSerialKind,
    ) {
        let client_id = surface.client().map(|client| client.id());
        let root_surface_id = self.root_surface_id_for_surface(compositor_surface_id(&surface));
        self.recent_input_serials
            .retain(|input| input.serial != serial);
        self.recent_input_serials.push(InputSerial {
            serial,
            surface,
            client_id,
            root_surface_id,
            kind,
            focus_generation: self.focus_generation,
        });
        const MAX_RECENT_INPUT_SERIALS: usize = 16;
        let excess = self
            .recent_input_serials
            .len()
            .saturating_sub(MAX_RECENT_INPUT_SERIALS);
        if excess > 0 {
            self.recent_input_serials.drain(0..excess);
        }
    }

    pub(in crate::compositor) fn validate_activation_token_serial(
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
                && matches!(
                    input.kind,
                    InputSerialKind::PointerEnter
                        | InputSerialKind::PointerButtonPress { .. }
                        | InputSerialKind::KeyboardKeyPress { .. }
                        | InputSerialKind::TouchDown { .. }
                )
        })
    }

    pub(in crate::compositor) fn validate_set_cursor_serial(
        &self,
        serial: u32,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.recent_input_serials.iter().any(|input| {
            input.serial == serial
                && input.kind == InputSerialKind::PointerEnter
                && input.surface.id().same_client_as(&surface.id())
                && same_surface_resource(&input.surface, surface)
                && input.focus_generation <= self.focus_generation
        })
    }

    pub(in crate::compositor) fn validate_popup_grab_serial(
        &self,
        serial: u32,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        let surface_id = compositor_surface_id(surface);
        let expected_root_surface_id = self
            .popup_nodes
            .get(&surface_id)
            .map(|node| node.owner_root_id)
            .unwrap_or_else(|| self.root_surface_id_for_surface(surface_id));
        self.recent_input_serials.iter().any(|input| {
            input.serial == serial
                && matches!(input.kind, InputSerialKind::PointerButtonPress { .. })
                && input.root_surface_id == expected_root_surface_id
                && input.client_id == surface.client().map(|client| client.id())
                && input.focus_generation <= self.focus_generation
        })
    }

    pub(in crate::compositor) fn validate_start_drag_serial(
        &self,
        serial: u32,
        surface: &wl_surface::WlSurface,
    ) -> bool {
        self.validate_popup_grab_serial(serial, surface)
    }

    pub(in crate::compositor) fn validate_set_selection_serial(
        &self,
        client_id: &ClientId,
        serial: u32,
    ) -> bool {
        self.recent_input_serials.iter().any(|input| {
            input.serial == serial
                && input.client_id.as_ref() == Some(client_id)
                && matches!(
                    input.kind,
                    InputSerialKind::PointerButtonPress { .. }
                        | InputSerialKind::KeyboardKeyPress { .. }
                        | InputSerialKind::TouchDown { .. }
                )
                && input.focus_generation <= self.focus_generation
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
                use_state: DataSourceUse::Unused,
                actions: 0,
                actions_set: false,
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
        self.cancel_drag_for_source(source);
        if let Some(binding) = self.data_sources.get_mut(&source.id()) {
            binding.use_state = DataSourceUse::Retired;
        }
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
        if let Some(client_id) = device.client().map(|client| client.id())
            && self
                .active_drag
                .as_ref()
                .is_some_and(|drag| drag.target_client.as_ref() == Some(&client_id))
        {
            self.cancel_drag_session("data_device_destroyed");
        }
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
            || !self.validate_set_selection_serial(client_id, serial)
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
        if binding.use_state != DataSourceUse::Unused {
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
        if let Some(binding) = self.data_sources.get_mut(&source.id()) {
            binding.use_state = DataSourceUse::Selection;
        }
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
                    kind: DataOfferKind::Selection,
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
                kind: DataOfferKind::Selection,
                accepted_mime: None,
                selected_action: None,
                drag_phase: None,
                source_actions: 0,
                destination_actions: None,
                preferred_action: 0,
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
        if binding.kind == DataOfferKind::DragAndDrop {
            if binding.target_client_id != *client_id
                || !binding.mime_types.iter().any(|mime| mime == &mime_type)
            {
                return;
            }
            let Some(active) = self.active_drag.as_ref() else {
                return;
            };
            if active
                .offer
                .as_ref()
                .is_none_or(|current| !same_wayland_resource(current, offer))
            {
                return;
            }
            let Some(source) = active.source.as_ref() else {
                return;
            };
            let _ = source.send_event(wl_data_source::Event::Send {
                mime_type,
                fd: fd.as_fd(),
            });
            return;
        }
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

    pub(in crate::compositor) fn reconfigure_stateful_windows_for_output_size(&mut self) {
        let toplevels = self
            .toplevel_surfaces
            .iter()
            .filter_map(|(surface_id, _toplevel)| {
                let mode = self
                    .toplevel_window_state(*surface_id)
                    .map(WindowState::mode)
                    .unwrap_or(ToplevelMode::Floating);
                (mode != ToplevelMode::Floating
                    && !self
                        .toplevel_window_state(*surface_id)
                        .is_some_and(WindowState::is_minimized))
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
        self.surface_presentation_generations.remove(&surface_id);
        self.cancel_pending_surface_trees_for_surface(
            surface_id,
            AcquireWatchCancelReason::SurfaceDestroyed,
        );
        let callbacks = self.cancel_pending_acquire_commits_for_surface(
            surface_id,
            AcquireWatchCancelReason::SurfaceDestroyed,
        );
        self.complete_frame_callbacks(callbacks);
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
        self.scrub_surface_lifecycle(surface_id);
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
        self.xdg_surface_resources.remove(&surface_id);
        self.xdg_surface_wm_bases.remove(&surface_id);
        self.xdg_surface_lifecycles.remove(&surface_id);
        self.scrub_surface_output_membership(surface_id);
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
            self.focused_window_id = None;
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

    #[test]
    fn old_frame_completion_advances_only_its_sampled_damage_commit() {
        let mut state = CompositorState::default();
        state.surface_presentation_generations.insert(7, 1);
        let journal = state
            .surface_damage_journals
            .entry(7)
            .or_insert_with(|| SurfaceDamageJournal::new(64));
        let mut sampled = SurfaceCommitCounter::default();
        for _ in 0..10 {
            sampled = journal.record(RenderableSurfaceDamage::Full, 100, 80);
        }
        let token = SurfaceDamagePresentation {
            sampled_commits: vec![(
                SurfacePresentationKey {
                    surface_id: 7,
                    generation: 1,
                },
                sampled,
            )],
        };
        let newer = journal.record(RenderableSurfaceDamage::Full, 100, 80);
        state.commit_surface_damage_presented(token);

        assert_eq!(sampled, SurfaceCommitCounter(10));
        assert_eq!(newer, SurfaceCommitCounter(11));
        assert_eq!(state.presented_surface_commits.get(&7), Some(&sampled));
        assert!(matches!(
            state.surface_damage_journals[&7].damage_since(sampled, 100, 80),
            DamageSince::Known(RenderableSurfaceDamage::Full)
        ));
    }

    #[test]
    fn filtered_surface_damage_capture_samples_only_the_requested_surface() {
        let mut state = CompositorState::default();
        state.surface_presentation_generations.insert(7, 1);
        state.surface_presentation_generations.insert(8, 1);
        let mut direct_journal = SurfaceDamageJournal::new(4);
        let direct_commit = direct_journal.record(RenderableSurfaceDamage::Full, 100, 80);
        state.surface_damage_journals.insert(7, direct_journal);
        let mut unrelated_journal = SurfaceDamageJournal::new(4);
        unrelated_journal.record(RenderableSurfaceDamage::Full, 100, 80);
        state.surface_damage_journals.insert(8, unrelated_journal);
        let token = state.capture_surface_damage_presentation_for_surface(7);
        assert_eq!(
            token.sampled_commits,
            vec![(
                SurfacePresentationKey {
                    surface_id: 7,
                    generation: 1,
                },
                direct_commit,
            )]
        );
        assert!(
            state
                .capture_surface_damage_presentation_for_surface(99)
                .sampled_commits
                .is_empty()
        );
        state.commit_surface_damage_presented(token);
        assert_eq!(
            state.presented_surface_commits.get(&7),
            Some(&direct_commit)
        );
        assert!(!state.presented_surface_commits.contains_key(&8));
    }

    #[test]
    fn stale_surface_generation_cannot_advance_reused_surface_identity() {
        let mut state = CompositorState::default();
        state.surface_presentation_generations.insert(7, 2);
        state.surface_damage_journals.insert(7, {
            let mut journal = SurfaceDamageJournal::new(4);
            journal.record(RenderableSurfaceDamage::Full, 10, 10);
            journal
        });
        let stale = SurfaceDamagePresentation {
            sampled_commits: vec![(
                SurfacePresentationKey {
                    surface_id: 7,
                    generation: 1,
                },
                SurfaceCommitCounter(1),
            )],
        };

        state.commit_surface_damage_presented(stale);

        assert!(!state.presented_surface_commits.contains_key(&7));
    }

    #[test]
    fn compositor_owned_surface_keys_do_not_collide_across_clients() {
        let first = SurfacePresentationKey {
            surface_id: 7,
            generation: 11,
        };
        let second = SurfacePresentationKey {
            surface_id: 8,
            generation: 12,
        };
        let token = SurfaceDamagePresentation {
            sampled_commits: vec![
                (first, SurfaceCommitCounter(3)),
                (second, SurfaceCommitCounter(4)),
            ],
        };

        assert_ne!(first, second);
        assert_eq!(token.sampled_commits.len(), 2);
    }
}
