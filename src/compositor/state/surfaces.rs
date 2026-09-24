use super::*;

fn popup_grab_input_kind_is_legal(kind: InputSerialKind) -> bool {
    matches!(
        kind,
        InputSerialKind::PointerButtonPress { .. }
            | InputSerialKind::KeyboardKeyPress { .. }
            | InputSerialKind::TouchDown { .. }
    )
}

struct PopupGrabSerialMetadata<'a> {
    input_serial: u32,
    serial: u32,
    kind: InputSerialKind,
    input_root_surface_id: u32,
    expected_root_surface_id: u32,
    input_client_id: &'a Option<ClientId>,
    expected_client_id: &'a Option<ClientId>,
    input_focus_generation: u64,
    focus_generation: u64,
}

fn popup_grab_serial_metadata_matches(metadata: PopupGrabSerialMetadata<'_>) -> bool {
    metadata.input_serial == metadata.serial
        && popup_grab_input_kind_is_legal(metadata.kind)
        && metadata.input_root_surface_id == metadata.expected_root_surface_id
        && metadata.input_client_id == metadata.expected_client_id
        && metadata.input_focus_generation == metadata.focus_generation
}
use crate::xwayland::trace::{self, TraceFields};
impl CompositorState {
    pub(in crate::compositor) fn renderable_surface_index(&self, surface_id: u32) -> Option<usize> {
        let mut metrics = self.locality_metrics.get();
        metrics.global_indexed_lookups = metrics.global_indexed_lookups.saturating_add(1);
        self.locality_metrics.set(metrics);
        self.renderable_surface_indices.get(&surface_id).copied()
    }

    pub(in crate::compositor) fn content_renderable_surface_index(
        &self,
        surface_id: u32,
    ) -> Option<usize> {
        let mut metrics = self.locality_metrics.get();
        metrics.content_indexed_lookups = metrics.content_indexed_lookups.saturating_add(1);
        self.locality_metrics.set(metrics);
        self.renderable_surface_index(surface_id)
    }

    pub(in crate::compositor) fn renderable_surface(
        &self,
        surface_id: u32,
    ) -> Option<&RenderableSurface> {
        self.renderable_surface_index(surface_id)
            .and_then(|index| self.renderable_surfaces.get(index))
    }

    pub(in crate::compositor) fn surface_resource_sync_states(
        &self,
        surface_ids: impl IntoIterator<Item = u32>,
    ) -> Vec<SurfaceResourceSyncState> {
        let mut surface_ids = surface_ids.into_iter().collect::<Vec<_>>();
        surface_ids.sort_unstable();
        surface_ids.dedup();
        surface_ids
            .into_iter()
            .map(|surface_id| {
                // `SurfacePresentationKey::generation` is the authority for
                // presented damage baselines. RenderableSurface::generation
                // belongs to the render-state namespace and is unrelated.
                let presentation_generation = self
                    .surface_presentation_generations
                    .get(&surface_id)
                    .copied();
                let journal = self.surface_damage_journals.get(&surface_id);
                let current_commit = journal.map_or_else(
                    SurfaceCommitCounter::default,
                    SurfaceDamageJournal::current_commit,
                );
                let complete_since = match (
                    presentation_generation,
                    self.presented_surface_commit_generations.get(&surface_id),
                    self.presented_surface_commits.get(&surface_id),
                ) {
                    (Some(presentation_generation), Some(presented_generation), Some(commit))
                        if presentation_generation == *presented_generation =>
                    {
                        Some(*commit)
                    }
                    _ => None,
                };
                let surface_size = self
                    .renderable_surface_indices
                    .get(&surface_id)
                    .and_then(|index| self.renderable_surfaces.get(*index))
                    .map(RenderableSurface::buffer_size)
                    .or_else(|| {
                        self.client_cursor_surfaces
                            .get(&surface_id)
                            .map(RenderableSurface::buffer_size)
                    });
                let complete_since = complete_since.filter(|complete_since| {
                    journal.is_none_or(|journal| {
                        surface_size.is_none_or(|size| {
                            !matches!(
                                journal.damage_since(*complete_since, size.width, size.height),
                                DamageSince::HistoryLost
                            )
                        })
                    })
                });
                SurfaceResourceSyncState {
                    surface_id,
                    complete_since,
                    current_commit,
                    authoritative: journal.is_some(),
                }
            })
            .collect()
    }

    #[allow(dead_code)]
    pub(in crate::compositor) fn renderable_surface_mut(
        &mut self,
        surface_id: u32,
    ) -> Option<&mut RenderableSurface> {
        let index = self.renderable_surface_index(surface_id)?;
        self.renderable_surfaces.get_mut(index)
    }

    pub(in crate::compositor) fn append_renderable_surface(&mut self, surface: RenderableSurface) {
        assert!(
            !self
                .renderable_surface_indices
                .contains_key(&surface.surface_id),
            "duplicate RenderableSurface ID appended"
        );
        #[cfg(test)]
        self.ensure_surface_scene_node(surface.surface_id);
        let index = self.renderable_surfaces.len();
        let surface_id = surface.surface_id;
        self.renderable_surfaces.push(surface);
        self.renderable_surface_indices.insert(surface_id, index);
    }

    pub(in crate::compositor) fn replace_renderable_surface(
        &mut self,
        surface_id: u32,
        surface: RenderableSurface,
    ) -> Option<RenderableSurface> {
        assert_eq!(surface.surface_id, surface_id);
        let index = self.renderable_surface_indices.get(&surface_id).copied()?;
        Some(std::mem::replace(
            &mut self.renderable_surfaces[index],
            surface,
        ))
    }

    #[allow(dead_code)]
    pub(in crate::compositor) fn remove_renderable_surface(
        &mut self,
        surface_id: u32,
    ) -> Option<RenderableSurface> {
        let index = self.renderable_surface_indices.get(&surface_id).copied()?;
        let removed = self.renderable_surfaces.remove(index);
        self.rebuild_renderable_surface_index();
        Some(removed)
    }

    pub(in crate::compositor) fn retain_renderable_surfaces(
        &mut self,
        mut keep: impl FnMut(&RenderableSurface) -> bool,
    ) -> bool {
        let previous_len = self.renderable_surfaces.len();
        self.renderable_surfaces.retain(|surface| keep(surface));
        let changed = previous_len != self.renderable_surfaces.len();
        if changed {
            self.rebuild_renderable_surface_index();
        }
        changed
    }

    pub(in crate::compositor) fn rebuild_renderable_surface_index(&mut self) {
        self.renderable_surface_indices = self
            .renderable_surfaces
            .iter()
            .enumerate()
            .map(|(index, surface)| (surface.surface_id, index))
            .collect();
        let mut metrics = self.locality_metrics.get();
        metrics.global_renderable_index_rebuilds =
            metrics.global_renderable_index_rebuilds.saturating_add(1);
        self.locality_metrics.set(metrics);
        #[cfg(any(test, debug_assertions))]
        self.assert_renderable_surface_index_invariant();
    }

    #[cfg(any(test, debug_assertions))]
    fn assert_renderable_surface_index_invariant(&self) {
        assert_eq!(
            self.renderable_surface_indices.len(),
            self.renderable_surfaces.len(),
            "global RenderableSurface index length mismatch"
        );
        for (index, surface) in self.renderable_surfaces.iter().enumerate() {
            assert_eq!(
                self.renderable_surface_indices.get(&surface.surface_id),
                Some(&index),
                "global RenderableSurface index points at the wrong position"
            );
        }
        for (surface_id, index) in &self.renderable_surface_indices {
            assert_eq!(
                self.renderable_surfaces
                    .get(*index)
                    .map(|surface| surface.surface_id),
                Some(*surface_id),
                "global RenderableSurface index contains a stale entry"
            );
        }
    }

    #[cfg(test)]
    pub(in crate::compositor) fn assert_renderable_surface_index_invariant_for_test(&self) {
        self.assert_renderable_surface_index_invariant();
    }

    #[cfg(test)]
    pub(in crate::compositor) fn presentation_global_scan_count_for_test(&self) -> u64 {
        self.locality_metrics.get().presentation_global_scans
    }

    #[cfg(test)]
    pub(in crate::compositor) fn presentation_sampled_entry_count_for_test(&self) -> u64 {
        self.locality_metrics.get().presentation_sampled_entries
    }

    #[cfg(test)]
    pub(in crate::compositor) fn presentation_journal_lookup_count_for_test(&self) -> u64 {
        self.locality_metrics.get().presentation_journal_lookups
    }

    pub(in crate::compositor) const fn output_dimensions(&self) -> (u32, u32) {
        (self.output_size.width, self.output_size.height)
    }

    pub(in crate::compositor) fn surface_content_epoch(
        &self,
        surface_id: u32,
    ) -> Option<SurfaceCommitSequence> {
        let publication = self.surface_publications.get(&surface_id)?;
        publication
            .latest_published_buffer_id
            .and(publication.latest_published)
    }

    pub(in crate::compositor) fn presentation_commit_key_for_surface_commit(
        &self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
    ) -> Option<SurfacePresentationCommitKey> {
        let presentation_generation = self
            .surface_presentation_generations
            .get(&surface_id)
            .copied()?;
        self.presentation_commit_key_for_surface_commit_with_generation(
            surface_id,
            presentation_generation,
            commit_sequence,
        )
    }

    pub(in crate::compositor) fn presentation_commit_key_for_surface_commit_with_generation(
        &self,
        surface_id: u32,
        presentation_generation: u64,
        commit_sequence: SurfaceCommitSequence,
    ) -> Option<SurfacePresentationCommitKey> {
        if self
            .surface_presentation_generations
            .get(&surface_id)
            .copied()
            != Some(presentation_generation)
        {
            return None;
        }
        let active = self.active_surface_presentation_commits.get(&surface_id)?;
        (active.commit_sequence == commit_sequence).then_some(SurfacePresentationCommitKey {
            surface_id,
            presentation_generation,
            commit_sequence,
        })
    }

    pub(in crate::compositor) fn presentation_commit_key_for_renderable_surface(
        &self,
        surface: &RenderableSurface,
    ) -> Option<SurfacePresentationCommitKey> {
        self.presentation_commit_key_for_surface_commit(surface.surface_id, surface.commit_sequence)
    }

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
        captured_buffer_id: Option<u64>,
    ) {
        if matches!(self.surface_role(surface_id), SurfaceRole::Xwayland) {
            let current = self.current_surface_buffers.get(&surface_id);
            let buffer_size =
                current.and_then(|buffer| buffer.width().ok().zip(buffer.height().ok()));
            let buffer_id = captured_buffer_id.or_else(|| {
                (!has_attachment_change)
                    .then(|| current.map(|buffer| buffer.buffer_id().get()))
                    .flatten()
            });
            let association_serial = self
                .xwayland
                .associations
                .serial_for_surface(surface_id)
                .map(|(_, serial)| serial.get());
            trace::emit("xwayland_commit_received", || {
                TraceFields::new()
                    .field("source", "wayland")
                    .field("surface_id", surface_id)
                    .field("commit_sequence", commit_sequence.get())
                    .field("has_attachment_change", has_attachment_change)
                    .optional("buffer_id", buffer_id)
                    .optional("buffer_width", buffer_size.map(|(width, _)| width))
                    .optional("buffer_height", buffer_size.map(|(_, height)| height))
                    .optional("association_serial", association_serial)
            });
        }
        let state = self.surface_publications.entry(surface_id).or_default();
        state.latest_received = state.latest_received.max(commit_sequence);
        if has_attachment_change {
            state.latest_attachment_received = Some(
                state
                    .latest_attachment_received
                    .map_or(commit_sequence, |latest| latest.max(commit_sequence)),
            );
        }
        self.trace_surface_pipeline_event(
            SurfacePipelineEvent::CommitCaptured,
            surface_id,
            commit_sequence,
            captured_buffer_id,
            None,
            None,
            None,
            None,
            None,
        );
    }

    pub(in crate::compositor) fn record_surface_content_update_terminal(
        &mut self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
    ) {
        let state = self.surface_publications.entry(surface_id).or_default();
        state.latest_terminal = Some(
            state
                .latest_terminal
                .map_or(commit_sequence, |latest| latest.max(commit_sequence)),
        );
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

    pub(in crate::compositor) fn async_surface_publication_decision(
        &self,
        surface_id: u32,
        owner_client_id: &ClientId,
        surface_presentation_generation: u64,
        commit_sequence: SurfaceCommitSequence,
        context: SurfacePublicationContext,
    ) -> SurfacePublicationDecision {
        if let Some(rejection) = self.async_surface_lifecycle_rejection(surface_id, owner_client_id)
        {
            return rejection;
        }
        if self
            .surface_presentation_generations
            .get(&surface_id)
            .copied()
            != Some(surface_presentation_generation)
        {
            return SurfacePublicationDecision::StaleSurfaceGeneration;
        }
        self.surface_publication_decision(surface_id, commit_sequence, context)
    }

    pub(in crate::compositor) fn async_surface_lifecycle_rejection(
        &self,
        surface_id: u32,
        owner_client_id: &ClientId,
    ) -> Option<SurfacePublicationDecision> {
        if self.terminal_client_ids.contains(owner_client_id) {
            return Some(SurfacePublicationDecision::TerminalClient);
        }
        let Some(current_owner) = self.surface_client_ids.get(&surface_id) else {
            return Some(SurfacePublicationDecision::SurfaceGone);
        };
        if current_owner != owner_client_id {
            return Some(SurfacePublicationDecision::OwnerGone);
        }
        let Some(surface) = self.surface_resource_by_id(surface_id) else {
            return Some(SurfacePublicationDecision::SurfaceGone);
        };
        if !surface.is_alive() || surface.client().is_none() {
            return Some(SurfacePublicationDecision::OwnerGone);
        }
        None
    }

    pub(in crate::compositor) fn capture_surface_publication_lifetime(
        &self,
        surface_id: u32,
    ) -> Option<(ClientId, u64)> {
        Some((
            self.surface_client_ids.get(&surface_id)?.clone(),
            self.surface_presentation_generations
                .get(&surface_id)
                .copied()?,
        ))
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
        if surface_id == root_surface_id {
            self.update_toplevel_visual_render_assignment_after_root_commit(
                root_surface_id,
                commit_sequence,
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
        if let Some(reason) = decision.pipeline_rejection_reason() {
            self.trace_surface_pipeline_event_with_reason(
                SurfacePipelineEvent::PublicationRejected,
                surface_id,
                commit_sequence,
                buffer_id.map(BufferId::get),
                None,
                None,
                None,
                None,
                None,
                Some(reason),
            );
        }
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
                decision
                    .pipeline_rejection_reason()
                    .map(SurfacePipelineRejectionReason::as_str)
                    .unwrap_or("publish"),
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
                let old_buffer_id = commit.pending.data.buffer_id().get();
                self.release_pending_surface_buffer(commit.pending);
                callbacks.extend(commit.frame_callbacks);
                self.discard_presentation_feedbacks(commit.presentation_feedbacks);
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
                        old_buffer_id,
                        self.external_acquire_readiness,
                    );
                }
            } else {
                retained_explicit.push(commit);
            }
        }
        self.pending_explicit_sync_commits = retained_explicit;
        callbacks
    }
    pub(in crate::compositor) fn capture_surface_damage_presentation(
        &self,
    ) -> SurfaceDamagePresentation {
        self.capture_surface_damage_presentation_for_surface_ids(
            self.active_scene_surfaces()
                .iter()
                .map(|surface| surface.surface_id),
        )
    }
    pub(in crate::compositor) fn capture_surface_damage_presentation_for_surface(
        &self,
        surface_id: u32,
    ) -> SurfaceDamagePresentation {
        self.capture_surface_damage_presentation_for_surface_ids([surface_id])
    }

    pub(in crate::compositor) fn capture_surface_damage_presentation_for_surface_ids(
        &self,
        surface_ids: impl IntoIterator<Item = u32>,
    ) -> SurfaceDamagePresentation {
        let mut sampled = HashSet::new();
        let mut sampled_commits = Vec::new();
        for surface_id in surface_ids {
            self.append_surface_damage_sample(surface_id, None, &mut sampled, &mut sampled_commits);
        }
        SurfaceDamagePresentation { sampled_commits }
    }

    pub(in crate::compositor) fn capture_surface_damage_presentation_for_surface_ids_and_commit(
        &self,
        surface_ids: impl IntoIterator<Item = u32>,
        exact_surface_commit: Option<(u32, SurfaceCommitSequence)>,
    ) -> SurfaceDamagePresentation {
        let mut sampled = HashSet::new();
        let mut sampled_commits = Vec::new();
        // An exact commit is a frozen render-time identity. Give it precedence
        // over the current journal entry if a caller includes the same surface
        // in the primary scene list as well.
        if let Some((surface_id, commit_sequence)) = exact_surface_commit {
            self.append_surface_damage_sample(
                surface_id,
                Some(commit_sequence),
                &mut sampled,
                &mut sampled_commits,
            );
        }
        for surface_id in surface_ids {
            self.append_surface_damage_sample(surface_id, None, &mut sampled, &mut sampled_commits);
        }
        SurfaceDamagePresentation { sampled_commits }
    }

    fn append_surface_damage_sample(
        &self,
        surface_id: u32,
        commit_sequence: Option<SurfaceCommitSequence>,
        sampled: &mut HashSet<u32>,
        sampled_commits: &mut Vec<SurfaceDamageSample>,
    ) {
        if !sampled.insert(surface_id) {
            return;
        }
        let Some(generation) = self
            .surface_presentation_generations
            .get(&surface_id)
            .copied()
        else {
            return;
        };
        let Some(journal) = self.surface_damage_journals.get(&surface_id) else {
            return;
        };
        let commit = match commit_sequence {
            Some(commit_sequence) => journal.commit_counter_for_sequence(commit_sequence),
            None => Some(journal.current_commit()),
        };
        let Some(commit) = commit else {
            return;
        };
        let change = match (
            self.presented_surface_commit_generations
                .get(&surface_id)
                .copied(),
            self.presented_surface_commits.get(&surface_id).copied(),
        ) {
            (Some(presented_generation), Some(presented_commit))
                if presented_generation == generation && commit > presented_commit =>
            {
                SurfacePresentationChange::Advanced
            }
            (Some(presented_generation), Some(presented_commit))
                if presented_generation == generation && commit == presented_commit =>
            {
                SurfacePresentationChange::Unchanged
            }
            _ => SurfacePresentationChange::Unknown,
        };
        let mut metrics = self.locality_metrics.get();
        metrics.presentation_journal_lookups =
            metrics.presentation_journal_lookups.saturating_add(1);
        metrics.presentation_sampled_entries =
            metrics.presentation_sampled_entries.saturating_add(1);
        self.locality_metrics.set(metrics);
        sampled_commits.push(SurfaceDamageSample {
            key: SurfacePresentationKey {
                surface_id,
                generation,
            },
            commit,
            change,
        });
    }

    #[cfg(test)]
    pub(in crate::compositor) fn surface_locality_metrics_for_test(
        &self,
    ) -> SurfaceLocalityMetrics {
        self.locality_metrics.get()
    }

    pub(in crate::compositor) fn note_client_cursor_surface_sample(&self, hardware: bool) {
        let mut metrics = self.locality_metrics.get();
        if hardware {
            metrics.cursor_surface_samples_hardware =
                metrics.cursor_surface_samples_hardware.saturating_add(1);
        } else {
            metrics.cursor_surface_samples_software =
                metrics.cursor_surface_samples_software.saturating_add(1);
        }
        self.locality_metrics.set(metrics);
    }

    pub(in crate::compositor) fn capture_surface_damage_presentation_for_surface_commit(
        &self,
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
    ) -> SurfaceDamagePresentation {
        self.capture_surface_damage_presentation_for_surface_ids_and_commit(
            [],
            Some((surface_id, commit_sequence)),
        )
    }
    pub(in crate::compositor) fn commit_surface_damage_presented(
        &mut self,
        token: SurfaceDamagePresentation,
    ) {
        self.settle_surface_damage(token, SurfaceDamageSettlement::Presented);
    }
    pub(in crate::compositor) fn commit_surface_damage_no_visual_change(
        &mut self,
        token: SurfaceDamagePresentation,
    ) {
        self.settle_surface_damage(token, SurfaceDamageSettlement::NoVisualChange);
    }
    fn settle_surface_damage(
        &mut self,
        token: SurfaceDamagePresentation,
        settlement: SurfaceDamageSettlement,
    ) {
        let sampled_entries = token.sampled_commits.len() as u64;
        let mut metrics = self.locality_metrics.get();
        match settlement {
            SurfaceDamageSettlement::Presented => {
                metrics.surface_damage_settlement_presented = metrics
                    .surface_damage_settlement_presented
                    .saturating_add(sampled_entries);
            }
            SurfaceDamageSettlement::NoVisualChange => {
                metrics.surface_damage_settlement_no_visual_change = metrics
                    .surface_damage_settlement_no_visual_change
                    .saturating_add(sampled_entries);
            }
        }
        self.locality_metrics.set(metrics);
        for sample in token.sampled_commits {
            let SurfaceDamageSample {
                key,
                commit: sampled_commit,
                ..
            } = sample;
            let mut metrics = self.locality_metrics.get();
            metrics.presentation_settlement_entries =
                metrics.presentation_settlement_entries.saturating_add(1);
            self.locality_metrics.set(metrics);
            if self
                .surface_presentation_generations
                .get(&key.surface_id)
                .copied()
                != Some(key.generation)
            {
                continue;
            }
            let Some(journal) = self.surface_damage_journals.get(&key.surface_id) else {
                continue;
            };
            if self
                .presented_surface_commits
                .get(&key.surface_id)
                .is_some_and(|presented| sampled_commit < *presented)
            {
                continue;
            }
            self.presented_surface_commits
                .insert(key.surface_id, sampled_commit);
            self.presented_surface_commit_generations
                .insert(key.surface_id, key.generation);
            let mut metrics = self.locality_metrics.get();
            metrics.presentation_settlement_journal_lookups = metrics
                .presentation_settlement_journal_lookups
                .saturating_add(1);
            self.locality_metrics.set(metrics);
            let surface_size = self
                .renderable_surface_indices
                .get(&key.surface_id)
                .copied()
                .and_then(|index| self.renderable_surfaces.get(index))
                .map(|surface| surface.buffer_size());
            let surface_size = surface_size.or_else(|| {
                self.client_cursor_surfaces
                    .get(&key.surface_id)
                    .map(|surface| surface.buffer_size())
            });
            let Some(surface_size) = surface_size else {
                continue;
            };
            let damage_since =
                journal.damage_since(sampled_commit, surface_size.width, surface_size.height);
            let mut metrics = self.locality_metrics.get();
            match &damage_since {
                DamageSince::Empty => {
                    metrics.damage_authoritative_empty =
                        metrics.damage_authoritative_empty.saturating_add(1);
                }
                DamageSince::HistoryLost => {
                    metrics.damage_history_lost_repairs =
                        metrics.damage_history_lost_repairs.saturating_add(1);
                }
                DamageSince::Known(_) => {}
            }
            self.locality_metrics.set(metrics);
            let damage = match damage_since {
                DamageSince::Empty => RenderableSurfaceDamage::Empty,
                DamageSince::Known(damage) => damage,
                DamageSince::HistoryLost => RenderableSurfaceDamage::HistoryLost,
            };
            if let Some(index) = self
                .renderable_surface_indices
                .get(&key.surface_id)
                .copied()
            {
                if let Some(surface) = self.renderable_surfaces.get_mut(index) {
                    surface.damage = damage;
                }
            } else if let Some(surface) = self.client_cursor_surfaces.get_mut(&key.surface_id) {
                surface.damage = damage;
            }
        }
    }
    pub(in crate::compositor) fn mark_render_damage_presented(&mut self) {
        let token = self.capture_surface_damage_presentation();
        self.commit_surface_damage_presented(token);
    }
    #[allow(dead_code)]
    pub(in crate::compositor) fn record_surface_damage_commit(
        &mut self,
        surface_id: u32,
        damage: RenderableSurfaceDamage,
        width: u32,
        height: u32,
    ) {
        self.record_surface_damage_commit_at(surface_id, None, damage, width, height);
    }

    pub(in crate::compositor) fn record_surface_damage_commit_at(
        &mut self,
        surface_id: u32,
        commit_sequence: Option<SurfaceCommitSequence>,
        damage: RenderableSurfaceDamage,
        width: u32,
        height: u32,
    ) {
        self.surface_damage_journals
            .entry(surface_id)
            .or_insert_with(|| SurfaceDamageJournal::new(64))
            .record_with_sequence(commit_sequence, damage, width, height);
    }
    pub(in crate::compositor) fn new(syncobj_device: Option<DrmSyncobjDevice>) -> Self {
        let mut state = Self {
            native_output_id: None,
            output_id_allocator: OutputIdAllocator::default(),
            frame_clock_start: Some(Instant::now()),
            next_window_id: 1,
            dmabuf_feedback: EglGlesDmabufFeedback::default(),
            dmabuf_main_device: 0,
            dmabuf_main_device_path: None,
            dmabuf_scanout_capabilities: None,
            dmabuf_scanout_target_device_override: None,
            syncobj_device,
            clipboard_bridge: Some(Box::new(NoopClipboardBridge)),
            trusted_effect_registry:
                crate::effects::TrustedEffectRegistry::with_builtin_background_blur(),
            pointer_hit_instrumentation_enabled: pointer_debug_enabled(),
            ..Self::default()
        };
        state.native_output_id = state.output_id_allocator.allocate().ok();
        // PresentationAnimator has its own compatibility defaults for older
        // callers. The compositor-owned control plane is authoritative for
        // the real runtime configuration, including persisted disablement.
        state
            .presentation_animator
            .set_enabled(state.animation_control.enabled());
        state.set_lifecycle_animation_enabled(state.animation_control.enabled());
        state.rebuild_active_scene_view();
        state
    }

    pub(in crate::compositor) fn native_output_id(&self) -> Option<OutputId> {
        self.native_output_id
    }

    pub(in crate::compositor) fn ensure_native_output_id(&mut self) -> Option<OutputId> {
        if self.native_output_id.is_none() {
            self.native_output_id = self.output_id_allocator.allocate().ok();
        }
        self.native_output_id
    }
    pub(in crate::compositor) fn allocate_buffer_identity(&mut self) -> Option<BufferIdentity> {
        self.buffer_ids.allocate()
    }

    pub(in crate::compositor) fn next_render_generation_value(&self) -> u64 {
        self.surface_tree_generation
            .unwrap_or_else(|| self.render_generation.saturating_add(1))
    }

    pub(in crate::compositor) fn advance_pointer_hit_generation(&mut self) -> u64 {
        self.pointer_hit_generation = advance_nonzero_serial(self.pointer_hit_generation);
        self.pointer_hit_metrics
            .pointer_hit_generation_invalidations = self
            .pointer_hit_metrics
            .pointer_hit_generation_invalidations
            .saturating_add(1);
        self.pointer_hit_generation
    }

    pub(in crate::compositor) fn begin_surface_tree_publication(&mut self) {
        debug_assert!(self.surface_tree_generation.is_none());
        self.surface_tree_generation = Some(self.render_generation.saturating_add(1));
        self.surface_tree_pointer_focus_refresh_pending = false;
        self.surface_tree_confined_region_refresh_pending = false;
    }

    pub(in crate::compositor) fn finish_surface_tree_publication(&mut self) {
        self.surface_tree_generation = None;
        if std::mem::take(&mut self.surface_tree_confined_region_refresh_pending) {
            self.update_all_active_confined_pointer_regions("surface_tree_publication");
        }
        if std::mem::take(&mut self.surface_tree_pointer_focus_refresh_pending) {
            self.refresh_pointer_focus_at_last_position();
        }
    }

    pub(in crate::compositor) fn set_render_generation(
        &mut self,
        generation: u64,
        cause: RenderGenerationCause,
    ) {
        self.set_render_generation_with_scene_effect(generation, cause, true);
    }

    fn set_render_generation_with_scene_effect(
        &mut self,
        generation: u64,
        cause: RenderGenerationCause,
        scene_effect: bool,
    ) {
        self.render_generation = generation;
        self.render_generation_cause = cause;
        self.note_cursor_generation(cause);
        if scene_effect
            && !matches!(
                cause,
                RenderGenerationCause::CursorCommit
                    | RenderGenerationCause::CursorMotion
                    | RenderGenerationCause::CursorState
            )
        {
            self.scene_render_generation = advance_nonzero_serial(self.scene_render_generation);
        }
    }

    pub(in crate::compositor) fn publish_surface_generation(
        &mut self,
        surface_id: u32,
        generation: u64,
        cause: RenderGenerationCause,
    ) {
        let scene_effect = self.surface_is_visible_in_active_scene(surface_id);
        self.set_render_generation_with_scene_effect(generation, cause, scene_effect);
        if scene_effect {
            let root_surface_id = self.root_surface_id_for_surface(surface_id);
            if root_surface_id == surface_id {
                self.refresh_active_scene_surface(root_surface_id);
            } else {
                self.refresh_active_scene_surface(surface_id);
            }
            self.reconcile_active_confined_pointer_regions(cause.as_str());
        }
    }

    pub(in crate::compositor) fn advance_render_generation(
        &mut self,
        cause: RenderGenerationCause,
    ) -> u64 {
        self.advance_render_generation_with_scene_effect(cause, true)
    }

    pub(in crate::compositor) fn advance_render_generation_with_scene_effect(
        &mut self,
        cause: RenderGenerationCause,
        scene_effect: bool,
    ) -> u64 {
        if self.layout_batch_depth > 0 {
            self.layout_batch_scene_effect |= scene_effect;
            return self.next_render_generation_value();
        }
        let generation = self.next_render_generation_value();
        self.set_render_generation_with_scene_effect(generation, cause, scene_effect);
        self.reconcile_active_confined_pointer_regions(cause.as_str());
        generation
    }

    fn reconcile_active_confined_pointer_regions(&mut self, reason: &'static str) {
        if self.surface_tree_generation.is_some() {
            self.surface_tree_confined_region_refresh_pending = true;
        } else {
            self.update_all_active_confined_pointer_regions(reason);
        }
    }

    pub(in crate::compositor) fn begin_layout_reflow_batch(&mut self) {
        self.layout_batch_depth = self.layout_batch_depth.saturating_add(1);
        if self.layout_batch_depth == 1 {
            self.layout_batch_scene_effect = false;
            let started_at = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
            self.layout_animation_epoch = Some(started_at);
            self.pending_presentation_geometry_transaction = Some(
                super::active_scene::PendingPresentationGeometryTransaction {
                    started_at,
                    members: Vec::new(),
                },
            );
        }
    }

    pub(in crate::compositor) fn finish_layout_reflow_batch(&mut self) -> bool {
        debug_assert!(self.layout_batch_depth > 0);
        self.layout_batch_depth = self.layout_batch_depth.saturating_sub(1);
        if self.layout_batch_depth > 0 {
            return false;
        }
        let scene_effect = self.layout_batch_scene_effect;
        self.layout_batch_scene_effect = false;
        if let Some(pending) = self.pending_presentation_geometry_transaction.take()
            && !pending.members.is_empty()
        {
            let result = self.presentation_animator.commit(
                crate::presentation_animation::PresentationTransactionRequest::geometry(
                    pending.started_at,
                    pending.members,
                ),
            );
            debug_assert!(
                matches!(
                    result,
                    Ok(_) | Err(crate::presentation_animation::PresentationTransactionError::Empty)
                ),
                "layout presentation transaction must validate or be an effective no-op"
            );
        }
        self.layout_animation_epoch = None;
        if scene_effect {
            self.advance_render_generation_with_scene_effect(
                RenderGenerationCause::LayoutReflow,
                true,
            );
            self.layout_generation = self.layout_generation.next();
        }
        scene_effect
    }

    pub(in crate::compositor) fn render_generation_cause(&self) -> RenderGenerationCause {
        self.render_generation_cause
    }

    pub(in crate::compositor) fn set_gpu_protocol_capabilities(
        &mut self,
        capabilities: GpuProtocolCapabilities,
    ) {
        self.dmabuf_main_device = capabilities.dmabuf_device().unwrap_or(0);
        self.dmabuf_main_device_path = capabilities.wl_drm_device().map(ToOwned::to_owned);
        self.gpu_protocol_capabilities = capabilities;
    }

    pub(in crate::compositor) fn set_output_size(&mut self, width: u32, height: u32) -> bool {
        let output_size = OutputSize::new(width, height);
        if self.output_size == output_size {
            return false;
        }

        self.output_size = output_size;
        self.send_output_mode_to_bound_outputs();
        self.reconfigure_layer_surfaces_for_output_change();
        let _ = self.reflow_usable_output_geometry();
        self.settle_lifecycle_no_visual_change();
        true
    }

    pub(in crate::compositor) fn set_output_refresh_hz(&mut self, refresh_hz: u32) -> bool {
        let output_refresh = OutputRefreshRate::from_hz(refresh_hz);
        if self.output_refresh == output_refresh {
            return false;
        }

        self.output_refresh = output_refresh;
        self.invalidate_surface_pacing_deadline_cache();
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

    pub(in crate::compositor) fn allocate_selection_source_key(&mut self) -> SelectionSourceKey {
        self.next_selection_source_key = self.next_selection_source_key.wrapping_add(1).max(1);
        SelectionSourceKey(self.next_selection_source_key)
    }

    pub(in crate::compositor) fn remember_input_serial(
        &mut self,
        serial: u32,
        surface: wl_surface::WlSurface,
        kind: InputSerialKind,
    ) {
        let client_id = surface.client().map(|client| client.id());
        let root_surface_id = self.root_surface_id_for_surface(compositor_surface_id(&surface));
        let epoch = self.selection_state.allocate_mutation_epoch();
        self.recent_input_serials
            .retain(|input| input.serial != serial);
        self.recent_input_serials.push(InputSerial {
            serial,
            epoch,
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
        let expected_client_id = surface.client().map(|client| client.id());
        self.recent_input_serials.iter().any(|input| {
            popup_grab_serial_metadata_matches(PopupGrabSerialMetadata {
                input_serial: input.serial,
                serial,
                kind: input.kind,
                input_root_surface_id: input.root_surface_id,
                expected_root_surface_id,
                input_client_id: &input.client_id,
                expected_client_id: &expected_client_id,
                input_focus_generation: input.focus_generation,
                focus_generation: self.focus_generation,
            })
        })
    }

    pub(in crate::compositor) fn validate_start_drag_serial(
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
        let expected_client_id = surface.client().map(|client| client.id());
        self.recent_input_serials.iter().any(|input| {
            let kind = match input.kind {
                InputSerialKind::PointerButtonPress { .. } => input.kind,
                _ => InputSerialKind::PointerEnter,
            };
            popup_grab_serial_metadata_matches(PopupGrabSerialMetadata {
                input_serial: input.serial,
                serial,
                kind,
                input_root_surface_id: input.root_surface_id,
                expected_root_surface_id,
                input_client_id: &input.client_id,
                expected_client_id: &expected_client_id,
                input_focus_generation: input.focus_generation,
                focus_generation: self.focus_generation,
            })
        })
    }

    pub(in crate::compositor) fn selection_input_epoch(
        &self,
        client_id: &ClientId,
        serial: u32,
    ) -> Option<SelectionMutationEpoch> {
        self.recent_input_serials
            .iter()
            .find(|input| {
                input.serial == serial
                    && input.client_id.as_ref() == Some(client_id)
                    && matches!(
                        input.kind,
                        InputSerialKind::PointerButtonPress { .. }
                            | InputSerialKind::KeyboardKeyPress { .. }
                            | InputSerialKind::TouchDown { .. }
                    )
                    && input.focus_generation == self.focus_generation
            })
            .map(|input| input.epoch)
    }

    pub(in crate::compositor) fn register_surface_resource(
        &mut self,
        surface_id: u32,
        surface: wl_surface::WlSurface,
    ) {
        self.surface_resources.entry(surface_id).or_insert(surface);
        self.ensure_surface_scene_node(surface_id);
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
                    .unwrap_or(ToplevelMode::Normal);
                (mode != ToplevelMode::Normal
                    && !self
                        .toplevel_window_state(*surface_id)
                        .is_some_and(WindowState::is_minimized))
                .then_some((*surface_id, mode))
            })
            .collect::<Vec<_>>();

        for (surface_id, mode) in toplevels {
            let geometry = self.window_geometry_for_surface_mode(surface_id, mode);
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
            self.install_toplevel_visual_geometry(surface_id, geometry);
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
            || self.renderable_surface_index(surface_id).is_some();
        let before = self.renderable_surfaces.len();
        self.unregister_surface_resource_with_reason(surface_id, reason);
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

    #[allow(dead_code)]
    pub(in crate::compositor) fn unregister_surface_resource(&mut self, surface_id: u32) {
        self.unregister_surface_resource_with_reason(
            surface_id,
            SurfaceTeardownReason::ExplicitDestroy,
        );
    }

    fn unregister_surface_resource_with_reason(
        &mut self,
        surface_id: u32,
        reason: SurfaceTeardownReason,
    ) {
        self.detach_dmabuf_surface(surface_id);
        let commit_sequence = self
            .active_surface_presentation_commits
            .get(&surface_id)
            .map(|active| active.commit_sequence)
            .or_else(|| {
                self.surface_publications
                    .get(&surface_id)
                    .and_then(|publication| publication.latest_published)
            })
            .unwrap_or_else(SurfaceCommitSequence::initial);
        let buffer_id = self
            .current_surface_buffers
            .get(&surface_id)
            .map(CurrentSurfaceBuffer::buffer_id)
            .map(|buffer_id| buffer_id.get());
        self.trace_surface_pipeline_event(
            SurfacePipelineEvent::SurfaceDetached,
            surface_id,
            commit_sequence,
            buffer_id,
            None,
            None,
            None,
            None,
            None,
        );
        // A root surface is only the current frame adapter. XWayland may
        // replace it while the logical WindowGroup and its geometry track
        // remain alive; logical window teardown cancels the track instead.
        self.remove_keyboard_shortcut_inhibitors_for_surface(surface_id);
        self.surface_frame_clock.remove(&surface_id);
        if reason != SurfaceTeardownReason::ClientDisconnected {
            self.discard_frame_callbacks_for_surface(surface_id);
        }
        if let Some(active) = self.active_fifo_barriers.get(&surface_id).copied() {
            self.clear_fifo_barrier_claim(
                FifoBarrierClaim {
                    surface_id,
                    surface_generation: active.surface_generation,
                    fifo_barrier_generation: active.fifo_barrier_generation,
                    commit_sequence: active.commit_sequence,
                },
                FifoBarrierClearReason::SurfaceTeardown,
            );
        }
        self.active_commit_timing_targets.remove(&surface_id);
        self.fifo_resources.remove(&surface_id);
        self.release_protocol_surface_effects_for_surface(surface_id);
        self.internal_surface_effects.remove(&surface_id);
        self.background_effect_resources.remove(&surface_id);
        self.background_effect_surface_ids.remove(&surface_id);
        self.commit_timer_resources.remove(&surface_id);
        self.surface_damage_journals.remove(&surface_id);
        self.presented_surface_commits.remove(&surface_id);
        self.presented_surface_commit_generations
            .remove(&surface_id);
        self.surface_presentation_generations.remove(&surface_id);
        self.active_surface_presentation_commits.remove(&surface_id);
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
        self.update_synchronized_cache_metrics();
        self.release_cached_subsurface_commits(cached);
        self.cleanup_subsurface_stack_state_for_surface(surface_id);
        self.surface_resources.remove(&surface_id);
        self.surface_client_ids.remove(&surface_id);
        self.scrub_surface_lifecycle(surface_id);
        self.cursor_surface_ids.remove(&surface_id);
        let removed_cursor_content = self.client_cursor_surfaces.remove(&surface_id).is_some();
        let active_cursor_pointer = self
            .focused_client_cursor
            .as_ref()
            .and_then(ClientCursorChoice::surface)
            .filter(|active| active.surface_id == surface_id)
            .map(|active| active.pointer.clone());
        if let Some(pointer) = active_cursor_pointer {
            self.focused_client_cursor = Some(ClientCursorChoice::Hidden {
                pointer: pointer.clone(),
            });
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
        self.xwayland.retired_surface_ids.remove(&surface_id);
        self.remove_current_surface_buffer(surface_id);
        self.surface_window_geometries.remove(&surface_id);
        self.pending_surface_window_geometries.remove(&surface_id);
        self.xdg_surface_resources.remove(&surface_id);
        self.xdg_surface_wm_bases.remove(&surface_id);
        self.xdg_surface_lifecycles.remove(&surface_id);
        self.scrub_surface_output_membership(surface_id);
        self.unregister_toplevel_surface(surface_id);
        self.unregister_popup_surface(surface_id);
        self.teardown_layer_surface(surface_id);
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
            self.detach_dmabuf_surface(*removed_surface_id);
            self.unregister_popup_surface(*removed_surface_id);
            if let Some(buffer) = self.active_dmabuf_buffers.remove(removed_surface_id) {
                self.queue_dmabuf_buffer_release(buffer);
            }
        }
        let previous_renderable_count = self.renderable_surfaces.len();
        self.retain_renderable_surfaces(|surface| {
            surface.surface_id != surface_id
                && surface.placement.parent_surface_id != Some(surface_id)
        });
        if self.renderable_surfaces.len() != previous_renderable_count {
            self.rebuild_active_scene_view();
            self.advance_render_generation(RenderGenerationCause::SurfaceUnmap);
        }
        self.clear_resize_state_for_surfaces_with_reason(
            &[surface_id],
            WindowInteractionEndReason::SurfaceDestroyed,
        );

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
        self.remove_surface_scene_node(surface_id);
    }
}

#[cfg(test)]
mod ordered_publication_tests {
    use std::os::unix::net::UnixStream;

    use super::*;
    use crate::render_backend::buffer::CommittedSurfaceBuffer;

    fn test_client_id() -> ClientId {
        let (stream, _peer) = UnixStream::pair().expect("test client socket");
        let display = wayland_server::Display::<CompositorState>::new().expect("test display");
        display
            .handle()
            .insert_client(stream, Arc::new(()))
            .expect("test client")
            .id()
    }

    #[test]
    fn popup_grab_accepts_only_user_action_serial_kinds() {
        assert!(popup_grab_input_kind_is_legal(
            InputSerialKind::PointerButtonPress { button: 1 }
        ));
        assert!(popup_grab_input_kind_is_legal(
            InputSerialKind::KeyboardKeyPress { key: 30 }
        ));
        assert!(popup_grab_input_kind_is_legal(InputSerialKind::TouchDown {
            touch_id: 0,
        }));
        assert!(!popup_grab_input_kind_is_legal(
            InputSerialKind::PointerEnter
        ));
    }

    #[test]
    fn popup_grab_serial_validation_preserves_ownership_and_focus_checks() {
        let client_a = test_client_id();
        let client_b = test_client_id();
        let expected_client = Some(client_a.clone());
        let matching = |serial, kind, root, client, focus| {
            popup_grab_serial_metadata_matches(PopupGrabSerialMetadata {
                input_serial: serial,
                serial: 77,
                kind,
                input_root_surface_id: root,
                expected_root_surface_id: 11,
                input_client_id: &client,
                expected_client_id: &expected_client,
                input_focus_generation: focus,
                focus_generation: 9,
            })
        };

        assert!(matching(
            77,
            InputSerialKind::PointerButtonPress { button: 1 },
            11,
            Some(client_a.clone()),
            9
        ));
        assert!(matching(
            77,
            InputSerialKind::KeyboardKeyPress { key: 30 },
            11,
            Some(client_a.clone()),
            9
        ));
        assert!(matching(
            77,
            InputSerialKind::TouchDown { touch_id: 0 },
            11,
            Some(client_a.clone()),
            9
        ));
        assert!(!matching(
            77,
            InputSerialKind::PointerEnter,
            11,
            Some(client_a.clone()),
            9
        ));
        assert!(!matching(
            77,
            InputSerialKind::PointerButtonPress { button: 1 },
            11,
            Some(client_b),
            9
        ));
        assert!(!matching(
            77,
            InputSerialKind::PointerButtonPress { button: 1 },
            12,
            Some(client_a.clone()),
            9
        ));
        assert!(!matching(
            77,
            InputSerialKind::PointerButtonPress { button: 1 },
            11,
            Some(client_a.clone()),
            8
        ));
        assert!(!matching(
            78,
            InputSerialKind::PointerButtonPress { button: 1 },
            11,
            Some(client_a),
            9
        ));
    }

    fn test_cursor_surface(
        surface_id: u32,
        commit_sequence: SurfaceCommitSequence,
    ) -> RenderableSurface {
        let identity = BufferIdAllocator::default()
            .allocate()
            .expect("cursor buffer identity");
        RenderableSurface {
            surface_id,
            x: 0,
            y: 0,
            width: 2,
            height: 2,
            placement: SurfacePlacement::root(),
            render_backend: SurfaceRenderBackend::NativeWayland,
            render_placement: None,
            visual_clip: None,
            render_target_size: None,
            generation: 1,
            commit_sequence,
            buffer: CommittedSurfaceBuffer::shm_snapshot(
                identity,
                BufferSize::new(2, 2).expect("cursor size"),
                vec![0; 4],
            ),
            viewport_source: None,
            viewport_destination: None,
            buffer_scale: 1,
            buffer_transform: wl_output::Transform::Normal,
            damage: RenderableSurfaceDamage::Full,
        }
    }

    #[test]
    fn ordered_ready_commit_ignores_newer_received_attachment() {
        let mut state = CompositorState::default();
        state.record_surface_commit_received(7, SurfaceCommitSequence(10), true, None);
        state.record_surface_commit_received(7, SurfaceCommitSequence(11), true, None);

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
    fn async_publication_rejects_terminal_owner_before_surface_lookup() {
        let mut state = CompositorState {
            surface_pipeline_trace:
                crate::compositor::surface_pipeline_trace::SurfacePipelineTrace::new(true, 8),
            ..Default::default()
        };
        let client_id = test_client_id();
        state.mark_client_terminal(client_id.clone());

        let decision = state.async_surface_publication_decision(
            150,
            &client_id,
            1,
            SurfaceCommitSequence(1),
            SurfacePublicationContext::OrderedExplicitSyncQueue,
        );
        assert_eq!(decision, SurfacePublicationDecision::TerminalClient);
        assert!(state.renderable_surface(150).is_none());

        state.record_surface_publication_rejection(
            150,
            SurfaceCommitSequence(1),
            None,
            SurfacePublicationSource::ExplicitSync,
            decision,
        );
        let records = state.surface_pipeline_trace.records().collect::<Vec<_>>();
        let rejection = records
            .iter()
            .find(|record| record.kind == SurfacePipelineEvent::PublicationRejected)
            .expect("terminal rejection must be traceable");
        assert_eq!(
            rejection.rejection_reason,
            Some(SurfacePipelineRejectionReason::TerminalClient)
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
    fn resource_sync_baseline_uses_presentation_generation_namespace() {
        let mut state = CompositorState::default();
        let surface_id = 7;
        let mut surface = test_cursor_surface(surface_id, SurfaceCommitSequence(2));
        surface.generation = 42;
        state.append_renderable_surface(surface);
        state.surface_presentation_generations.insert(surface_id, 7);

        let mut journal = SurfaceDamageJournal::new(4);
        let baseline = journal.record_for_surface_commit(
            SurfaceCommitSequence(1),
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }]),
            2,
            2,
        );
        let current = journal.record_for_surface_commit(
            SurfaceCommitSequence(2),
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 1,
                y: 1,
                width: 1,
                height: 1,
            }]),
            2,
            2,
        );
        state.surface_damage_journals.insert(surface_id, journal);
        state.presented_surface_commits.insert(surface_id, baseline);
        state
            .presented_surface_commit_generations
            .insert(surface_id, 7);

        assert_eq!(
            state.surface_resource_sync_states([surface_id]),
            vec![SurfaceResourceSyncState {
                surface_id,
                complete_since: Some(baseline),
                current_commit: current,
                authoritative: true,
            }]
        );
    }

    #[test]
    fn resource_sync_baseline_rejects_stale_presentation_generation() {
        let mut state = CompositorState::default();
        let surface_id = 7;
        state.append_renderable_surface(test_cursor_surface(surface_id, SurfaceCommitSequence(1)));
        state.surface_presentation_generations.insert(surface_id, 8);
        let mut journal = SurfaceDamageJournal::new(4);
        let baseline = journal.record(RenderableSurfaceDamage::Full, 2, 2);
        state.surface_damage_journals.insert(surface_id, journal);
        state.presented_surface_commits.insert(surface_id, baseline);
        state
            .presented_surface_commit_generations
            .insert(surface_id, 7);

        assert_eq!(
            state.surface_resource_sync_states([surface_id])[0].complete_since,
            None
        );
    }

    #[test]
    fn resource_sync_baseline_rejects_history_lost_presented_commit() {
        let mut state = CompositorState::default();
        let surface_id = 7;
        state.append_renderable_surface(test_cursor_surface(surface_id, SurfaceCommitSequence(1)));
        state.surface_presentation_generations.insert(surface_id, 7);
        let mut journal = SurfaceDamageJournal::new(1);
        let baseline = journal.record(RenderableSurfaceDamage::Full, 2, 2);
        journal.record(RenderableSurfaceDamage::Empty, 2, 2);
        journal.record(RenderableSurfaceDamage::Empty, 2, 2);
        let current = journal.current_commit();
        state.surface_damage_journals.insert(surface_id, journal);
        state.presented_surface_commits.insert(surface_id, baseline);
        state
            .presented_surface_commit_generations
            .insert(surface_id, 7);

        let sync_state = state.surface_resource_sync_states([surface_id]);
        assert_eq!(sync_state[0].complete_since, None);
        assert_eq!(sync_state[0].current_commit, current);
        assert!(sync_state[0].authoritative);
    }

    #[test]
    fn resource_sync_baseline_can_follow_no_visual_change_settlement() {
        let mut state = CompositorState::default();
        let surface_id = 7;
        state.append_renderable_surface(test_cursor_surface(surface_id, SurfaceCommitSequence(2)));
        state.surface_presentation_generations.insert(surface_id, 7);
        let mut journal = SurfaceDamageJournal::new(4);
        let resource_baseline = journal.record(RenderableSurfaceDamage::Full, 2, 2);
        let settled = journal.record(RenderableSurfaceDamage::Empty, 2, 2);
        state.surface_damage_journals.insert(surface_id, journal);
        state.presented_surface_commits.insert(surface_id, settled);
        state
            .presented_surface_commit_generations
            .insert(surface_id, 7);

        let sync_state = state.surface_resource_sync_states([surface_id]);
        assert_eq!(resource_baseline, SurfaceCommitCounter(1));
        assert_eq!(sync_state[0].complete_since, Some(settled));
        assert_eq!(sync_state[0].current_commit, settled);
    }

    #[test]
    fn resource_sync_baseline_keeps_accumulated_hidden_damage_complete_from_presented_commit() {
        let mut state = CompositorState::default();
        let surface_id = 7;
        state.append_renderable_surface(test_cursor_surface(surface_id, SurfaceCommitSequence(3)));
        state.surface_presentation_generations.insert(surface_id, 7);
        let mut journal = SurfaceDamageJournal::new(4);
        let baseline = journal.record(RenderableSurfaceDamage::Full, 2, 2);
        journal.record(
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }]),
            2,
            2,
        );
        let current = journal.record(
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 1,
                y: 1,
                width: 1,
                height: 1,
            }]),
            2,
            2,
        );
        state.surface_damage_journals.insert(surface_id, journal);
        state.presented_surface_commits.insert(surface_id, baseline);
        state
            .presented_surface_commit_generations
            .insert(surface_id, 7);

        let sync_state = state.surface_resource_sync_states([surface_id]);
        assert_eq!(sync_state[0].complete_since, Some(baseline));
        assert_eq!(sync_state[0].current_commit, current);
    }

    #[test]
    fn resource_sync_baseline_proves_empty_damage_through_current_commit() {
        let mut state = CompositorState::default();
        let surface_id = 7;
        state.append_renderable_surface(test_cursor_surface(surface_id, SurfaceCommitSequence(2)));
        state.surface_presentation_generations.insert(surface_id, 7);
        let mut journal = SurfaceDamageJournal::new(4);
        let baseline = journal.record(RenderableSurfaceDamage::Full, 2, 2);
        let current = journal.record(RenderableSurfaceDamage::Empty, 2, 2);
        state.surface_damage_journals.insert(surface_id, journal);
        state.presented_surface_commits.insert(surface_id, baseline);
        state
            .presented_surface_commit_generations
            .insert(surface_id, 7);

        let sync_state = state.surface_resource_sync_states([surface_id]);
        assert_eq!(sync_state[0].complete_since, Some(baseline));
        assert_eq!(sync_state[0].current_commit, current);
    }

    #[test]
    fn presented_settlement_preserves_history_lost_as_surface_local_evidence() {
        let mut state = CompositorState::default();
        let surface_id = 910;
        state.append_renderable_surface(test_cursor_surface(surface_id, SurfaceCommitSequence(1)));
        state.surface_presentation_generations.insert(surface_id, 1);
        let mut journal = SurfaceDamageJournal::new(1);
        let sampled_commit = journal.record_for_surface_commit(
            SurfaceCommitSequence(1),
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            }]),
            2,
            2,
        );
        state.surface_damage_journals.insert(surface_id, journal);
        let token = state.capture_surface_damage_presentation_for_surface_commit(
            surface_id,
            SurfaceCommitSequence(1),
        );
        let journal = state
            .surface_damage_journals
            .get_mut(&surface_id)
            .expect("test journal remains registered");
        journal.record_for_surface_commit(
            SurfaceCommitSequence(2),
            RenderableSurfaceDamage::Empty,
            2,
            2,
        );
        journal.record_for_surface_commit(
            SurfaceCommitSequence(3),
            RenderableSurfaceDamage::Empty,
            2,
            2,
        );

        state.commit_surface_damage_presented(token);

        assert_eq!(
            state.presented_surface_commits.get(&surface_id),
            Some(&sampled_commit)
        );
        assert_eq!(
            state
                .renderable_surface(surface_id)
                .expect("surface remains")
                .damage,
            RenderableSurfaceDamage::HistoryLost
        );
        assert_eq!(
            state
                .surface_locality_metrics_for_test()
                .damage_history_lost_repairs,
            1
        );
    }

    #[test]
    fn content_and_metadata_commits_have_distinct_epoch_behavior() {
        let mut state = CompositorState::default();
        let buffer_id = state
            .allocate_buffer_identity()
            .expect("test buffer identity")
            .id();
        state.record_surface_publication(
            7,
            7,
            SurfaceCommitSequence(10),
            Some(buffer_id),
            SurfacePublicationSource::Immediate,
            None,
        );
        let content_epoch = state.surface_content_epoch(7);

        state.record_surface_publication(
            7,
            7,
            SurfaceCommitSequence(11),
            Some(buffer_id),
            SurfacePublicationSource::Immediate,
            None,
        );
        let metadata_epoch = state.surface_content_epoch(7);

        assert_ne!(metadata_epoch, content_epoch);

        state.record_surface_commit_received(7, SurfaceCommitSequence(12), false, None);

        assert_eq!(state.surface_content_epoch(7), metadata_epoch);
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
            sampled_commits: vec![SurfaceDamageSample {
                key: SurfacePresentationKey {
                    surface_id: 7,
                    generation: 1,
                },
                commit: sampled,
                change: SurfacePresentationChange::Unknown,
            }],
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
    fn exact_surface_commit_capture_does_not_consume_a_newer_commit() {
        let mut state = CompositorState::default();
        state.surface_presentation_generations.insert(7, 1);
        let mut journal = SurfaceDamageJournal::new(8);
        let sampled = journal.record_for_surface_commit(
            SurfaceCommitSequence(41),
            RenderableSurfaceDamage::Full,
            100,
            80,
        );
        let newer = journal.record_for_surface_commit(
            SurfaceCommitSequence(42),
            RenderableSurfaceDamage::Full,
            100,
            80,
        );
        state.surface_damage_journals.insert(7, journal);

        let token = state
            .capture_surface_damage_presentation_for_surface_commit(7, SurfaceCommitSequence(41));
        assert_eq!(token.sampled_commits[0].commit, sampled);
        state.commit_surface_damage_presented(token);

        assert_eq!(state.presented_surface_commits.get(&7), Some(&sampled));
        assert!(matches!(
            state.surface_damage_journals[&7].damage_since(sampled, 100, 80),
            DamageSince::Known(RenderableSurfaceDamage::Full)
        ));
        assert_ne!(sampled, newer);
    }

    #[test]
    fn older_pageflip_cannot_regress_presented_surface_commit() {
        let mut state = CompositorState::default();
        state.surface_presentation_generations.insert(7, 1);
        let mut journal = SurfaceDamageJournal::new(8);
        let old = journal.record_for_surface_commit(
            SurfaceCommitSequence(41),
            RenderableSurfaceDamage::Full,
            100,
            80,
        );
        let new = journal.record_for_surface_commit(
            SurfaceCommitSequence(42),
            RenderableSurfaceDamage::Full,
            100,
            80,
        );
        state.surface_damage_journals.insert(7, journal);

        let new_token = state
            .capture_surface_damage_presentation_for_surface_commit(7, SurfaceCommitSequence(42));
        state.commit_surface_damage_presented(new_token);
        let old_token = state
            .capture_surface_damage_presentation_for_surface_commit(7, SurfaceCommitSequence(41));
        state.commit_surface_damage_presented(old_token);

        assert_eq!(state.presented_surface_commits.get(&7), Some(&new));
        assert_ne!(old, new);
    }

    #[test]
    fn exact_cursor_commit_settles_only_the_frozen_cursor_content() {
        let mut state = CompositorState::default();
        let surface_id = 77;
        state.surface_presentation_generations.insert(surface_id, 1);
        state.client_cursor_surfaces.insert(
            surface_id,
            test_cursor_surface(surface_id, SurfaceCommitSequence(41)),
        );
        let mut journal = SurfaceDamageJournal::new(8);
        let sampled = journal.record_for_surface_commit(
            SurfaceCommitSequence(41),
            RenderableSurfaceDamage::Full,
            2,
            2,
        );
        journal.record_for_surface_commit(
            SurfaceCommitSequence(42),
            RenderableSurfaceDamage::Full,
            2,
            2,
        );
        state.surface_damage_journals.insert(surface_id, journal);

        let token = state.capture_surface_damage_presentation_for_surface_commit(
            surface_id,
            SurfaceCommitSequence(41),
        );
        state
            .client_cursor_surfaces
            .get_mut(&surface_id)
            .unwrap()
            .commit_sequence = SurfaceCommitSequence(42);
        state.commit_surface_damage_presented(token);

        assert_eq!(
            state.presented_surface_commits.get(&surface_id),
            Some(&sampled)
        );
        assert_eq!(
            state.client_cursor_surfaces[&surface_id].commit_sequence,
            SurfaceCommitSequence(42)
        );
        assert_eq!(
            state.client_cursor_surfaces[&surface_id].damage,
            RenderableSurfaceDamage::Full
        );
    }

    #[test]
    fn composed_frame_token_owns_exact_primary_and_client_cursor_commits() {
        let mut state = CompositorState::default();
        let primary_id = 11;
        let cursor_id = 77;
        state.surface_presentation_generations.insert(primary_id, 1);
        state.surface_presentation_generations.insert(cursor_id, 1);
        state.client_cursor_surfaces.insert(
            cursor_id,
            test_cursor_surface(cursor_id, SurfaceCommitSequence(41)),
        );

        let mut primary_journal = SurfaceDamageJournal::new(8);
        let primary_commit = primary_journal.record_for_surface_commit(
            SurfaceCommitSequence(9),
            RenderableSurfaceDamage::Full,
            100,
            80,
        );
        state
            .surface_damage_journals
            .insert(primary_id, primary_journal);

        let mut cursor_journal = SurfaceDamageJournal::new(8);
        let cursor_commit = cursor_journal.record_for_surface_commit(
            SurfaceCommitSequence(41),
            RenderableSurfaceDamage::Full,
            2,
            2,
        );
        cursor_journal.record_for_surface_commit(
            SurfaceCommitSequence(42),
            RenderableSurfaceDamage::Full,
            2,
            2,
        );
        state
            .surface_damage_journals
            .insert(cursor_id, cursor_journal);

        let token = state.capture_surface_damage_presentation_for_surface_ids_and_commit(
            [primary_id],
            Some((cursor_id, SurfaceCommitSequence(41))),
        );
        assert_eq!(
            token.sampled_surface_ids_for_test(),
            vec![cursor_id, primary_id]
        );
        assert_eq!(token.sampled_commits[0].commit, cursor_commit);
        assert_eq!(token.sampled_commits[1].commit, primary_commit);

        state
            .client_cursor_surfaces
            .get_mut(&cursor_id)
            .expect("cursor surface remains mapped")
            .commit_sequence = SurfaceCommitSequence(42);
        state.commit_surface_damage_presented(token);

        assert_eq!(
            state.presented_surface_commits.get(&primary_id),
            Some(&primary_commit)
        );
        assert_eq!(
            state.presented_surface_commits.get(&cursor_id),
            Some(&cursor_commit)
        );
        assert_eq!(
            state.client_cursor_surfaces[&cursor_id].commit_sequence,
            SurfaceCommitSequence(42)
        );
        assert_eq!(
            state.client_cursor_surfaces[&cursor_id].damage,
            RenderableSurfaceDamage::Full
        );
    }

    #[test]
    fn presentation_capture_and_settlement_scale_with_sample_count() {
        let mut state = CompositorState::default();
        for surface_id in 1..=1_000 {
            state.append_renderable_surface(test_cursor_surface(
                surface_id,
                SurfaceCommitSequence(1),
            ));
            state.surface_presentation_generations.insert(surface_id, 1);
            let mut journal = SurfaceDamageJournal::new(4);
            journal.record(RenderableSurfaceDamage::Full, 10, 10);
            state.surface_damage_journals.insert(surface_id, journal);
        }
        let before = state.surface_locality_metrics_for_test();
        let token = state.capture_surface_damage_presentation_for_surface_ids([7, 17, 27, 37]);
        state.commit_surface_damage_presented(token);
        let after = state.surface_locality_metrics_for_test();

        assert_eq!(
            after.presentation_sampled_entries - before.presentation_sampled_entries,
            4
        );
        assert_eq!(
            after.presentation_journal_lookups - before.presentation_journal_lookups,
            4
        );
        assert_eq!(
            after.presentation_settlement_entries - before.presentation_settlement_entries,
            4
        );
        assert_eq!(
            after.presentation_settlement_journal_lookups
                - before.presentation_settlement_journal_lookups,
            4
        );
        assert_eq!(
            after.presentation_global_scans - before.presentation_global_scans,
            0
        );
        assert_eq!(
            after.damage_authoritative_empty - before.damage_authoritative_empty,
            4
        );
        assert_eq!(
            after.damage_history_lost_repairs - before.damage_history_lost_repairs,
            0
        );
    }

    #[test]
    fn abandoned_damage_capture_remains_unpresented_and_pending() {
        let mut state = CompositorState::default();
        state.surface_presentation_generations.insert(7, 1);
        let mut journal = SurfaceDamageJournal::new(4);
        let pending = journal.record(RenderableSurfaceDamage::Full, 100, 80);
        state.surface_damage_journals.insert(7, journal);
        drop(state.capture_surface_damage_presentation_for_surface(7));

        assert!(!state.presented_surface_commits.contains_key(&7));
        assert_eq!(state.surface_damage_journals[&7].current_commit(), pending);
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
            vec![SurfaceDamageSample {
                key: SurfacePresentationKey {
                    surface_id: 7,
                    generation: 1,
                },
                commit: direct_commit,
                change: SurfacePresentationChange::Unknown,
            }]
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
    fn filtered_surface_damage_capture_is_keyed_and_deduplicated() {
        let mut state = CompositorState::default();
        for surface_id in 1..=1_000 {
            state.surface_presentation_generations.insert(surface_id, 1);
            let mut journal = SurfaceDamageJournal::new(4);
            journal.record(RenderableSurfaceDamage::Full, 10, 10);
            state.surface_damage_journals.insert(surface_id, journal);
        }

        let token = state.capture_surface_damage_presentation_for_surface_ids([7, 7, 8, 9]);

        assert_eq!(token.sampled_surface_ids_for_test(), vec![7, 8, 9]);
        assert_eq!(state.presentation_global_scan_count_for_test(), 0);
        assert_eq!(state.presentation_sampled_entry_count_for_test(), 3);
        assert_eq!(state.presentation_journal_lookup_count_for_test(), 3);
    }

    #[test]
    fn presentation_capture_follows_the_final_resolved_surface_set() {
        let mut state = CompositorState::default();
        for surface_id in 1..=1_000 {
            state.surface_presentation_generations.insert(surface_id, 1);
            let mut journal = SurfaceDamageJournal::new(4);
            journal.record(RenderableSurfaceDamage::Full, 10, 10);
            state.surface_damage_journals.insert(surface_id, journal);
        }

        // This is the final scene authority after workspace selection,
        // fullscreen culling, and popup/subsurface expansion have completed.
        let final_resolved_surface_ids = [7, 17, 27, 37, 47, 57];
        let token =
            state.capture_surface_damage_presentation_for_surface_ids(final_resolved_surface_ids);

        assert_eq!(
            token.sampled_surface_ids_for_test(),
            final_resolved_surface_ids
        );
        assert_eq!(state.presentation_global_scan_count_for_test(), 0);
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
            sampled_commits: vec![SurfaceDamageSample {
                key: SurfacePresentationKey {
                    surface_id: 7,
                    generation: 1,
                },
                commit: SurfaceCommitCounter(1),
                change: SurfacePresentationChange::Unknown,
            }],
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
                SurfaceDamageSample {
                    key: first,
                    commit: SurfaceCommitCounter(3),
                    change: SurfacePresentationChange::Unknown,
                },
                SurfaceDamageSample {
                    key: second,
                    commit: SurfaceCommitCounter(4),
                    change: SurfacePresentationChange::Unknown,
                },
            ],
        };

        assert_ne!(first, second);
        assert_eq!(token.sampled_commits.len(), 2);
    }

    fn seed_presented_surface(state: &mut CompositorState, surface_id: u32) {
        state.append_renderable_surface(test_cursor_surface(surface_id, SurfaceCommitSequence(1)));
        state.surface_presentation_generations.insert(surface_id, 1);
        let mut journal = SurfaceDamageJournal::new(8);
        let baseline = journal.record(RenderableSurfaceDamage::Full, 10, 10);
        state.surface_damage_journals.insert(surface_id, journal);
        state.presented_surface_commits.insert(surface_id, baseline);
        state
            .presented_surface_commit_generations
            .insert(surface_id, 1);
    }

    fn advance_surface(state: &mut CompositorState, surface_id: u32) {
        state
            .surface_damage_journals
            .get_mut(&surface_id)
            .expect("test surface journal")
            .record(RenderableSurfaceDamage::Full, 10, 10);
    }

    #[test]
    fn fast_candidate_allows_static_competing_surfaces() {
        let mut state = CompositorState::default();
        seed_presented_surface(&mut state, 7);
        seed_presented_surface(&mut state, 8);
        seed_presented_surface(&mut state, 9);
        advance_surface(&mut state, 7);

        let token = state.capture_surface_damage_presentation_for_surface_ids([7, 8, 9]);

        assert_eq!(
            token.surface_change_for_id(7),
            Some(SurfacePresentationChange::Advanced)
        );
        assert_eq!(
            token.surface_change_for_id(8),
            Some(SurfacePresentationChange::Unchanged)
        );
        assert!(token.is_exclusive_surface_id(7));
    }

    #[test]
    fn fast_candidate_rejects_two_advanced_surfaces() {
        let mut state = CompositorState::default();
        seed_presented_surface(&mut state, 7);
        seed_presented_surface(&mut state, 8);
        advance_surface(&mut state, 7);
        advance_surface(&mut state, 8);

        let token = state.capture_surface_damage_presentation_for_surface_ids([7, 8]);

        assert!(!token.is_exclusive_surface_id(7));
    }

    #[test]
    fn fast_candidate_rejects_missing_presented_baseline_as_unknown() {
        let mut state = CompositorState::default();
        state.append_renderable_surface(test_cursor_surface(7, SurfaceCommitSequence(1)));
        state.surface_presentation_generations.insert(7, 1);
        let mut journal = SurfaceDamageJournal::new(8);
        journal.record(RenderableSurfaceDamage::Full, 10, 10);
        state.surface_damage_journals.insert(7, journal);

        let token = state.capture_surface_damage_presentation_for_surface(7);

        assert_eq!(
            token.surface_change_for_id(7),
            Some(SurfacePresentationChange::Unknown)
        );
        assert!(!token.is_exclusive_surface_id(7));
    }

    #[test]
    fn fast_candidate_rejects_stale_presentation_generation_as_unknown() {
        let mut state = CompositorState::default();
        seed_presented_surface(&mut state, 7);
        state.surface_presentation_generations.insert(7, 2);
        advance_surface(&mut state, 7);

        let token = state.capture_surface_damage_presentation_for_surface(7);

        assert_eq!(
            token.surface_change_for_id(7),
            Some(SurfacePresentationChange::Unknown)
        );
        assert!(!token.is_exclusive_surface_id(7));
    }

    #[test]
    fn unchanged_callback_surface_is_not_fast_candidate() {
        let mut state = CompositorState::default();
        seed_presented_surface(&mut state, 7);

        let token = state.capture_surface_damage_presentation_for_surface(7);

        assert_eq!(
            token.surface_change_for_id(7),
            Some(SurfacePresentationChange::Unchanged)
        );
        assert!(!token.is_exclusive_surface_id(7));
    }

    #[test]
    fn hardware_cursor_does_not_compete_with_advanced_primary_surface() {
        let mut state = CompositorState::default();
        seed_presented_surface(&mut state, 7);
        seed_presented_surface(&mut state, 8);
        advance_surface(&mut state, 7);
        advance_surface(&mut state, 8);

        let token = state.capture_surface_damage_presentation_for_surface_ids([7, 8]);

        assert!(!token.is_exclusive_surface_id(7));
        assert!(token.is_exclusive_surface_id_excluding(7, Some(8)));
    }
}
