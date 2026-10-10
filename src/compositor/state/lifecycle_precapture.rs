use crate::compositor::ResolvedEffectScene;
use crate::compositor::{
    CompositorState, FrozenLifecycleRenderLayout, RenderableSurface, WindowId, client_pacing_log,
};
use crate::presentation_animation::AnimationTime;
use crate::window_lifecycle_animation::{
    LifecycleDirection, LifecycleEffectKind, LifecycleFrameSnapshot, LifecycleSceneSample,
    LifecycleVisualGroup,
};
use std::collections::{HashMap, HashSet};

use super::lifecycle_animation::LifecycleAdmissionReason;

pub(super) struct LifecyclePreCapturePreparation {
    pub(super) surface_presentation:
        super::lifecycle_surface_snapshot::RetainedSurfacePresentationSnapshot,
    pub(super) surfaces: Vec<RenderableSurface>,
    pub(super) release_candidates: Vec<super::lifecycle_retained::LifecycleHeldReleaseObligation>,
    pub(super) render_layout: std::sync::Arc<FrozenLifecycleRenderLayout>,
}

impl CompositorState {
    pub(in crate::compositor) fn cancel_lifecycle_precapture_after_render_failure(&mut self) {
        let windows = self.retained_lifecycle_payloads.windows_with_precapture();
        for window_id in windows {
            self.lifecycle_cancel_window(window_id);
        }
    }

    pub(in crate::compositor) fn begin_lifecycle_lamp_for_test(
        &mut self,
        window_id: WindowId,
        root_surface_id: u32,
        visual_group: LifecycleVisualGroup,
    ) -> bool {
        let Some(scene_node_id) = self.scene_node_id_for_window_group(window_id) else {
            return false;
        };
        self.presentation_animator.set_enabled(true);
        self.lifecycle_animation_renderer_available = Some(true);
        for surface in &self.renderable_surfaces {
            self.surface_presentation_generations
                .entry(surface.surface_id)
                .or_insert(1);
        }
        let now = AnimationTime::monotonic_now().unwrap_or(AnimationTime::from_nanos(0));
        self.install_lifecycle_motion(
            scene_node_id,
            window_id,
            root_surface_id,
            Some(visual_group.presented_source_client_rect),
            Some(visual_group.canonical_client_rect),
            Some(visual_group),
            LifecycleDirection::Minimize,
            LifecycleEffectKind::Lamp,
            ResolvedEffectScene::default(),
            None,
            now,
            self.animation_control.configuration().speed,
        )
        .is_ok()
    }

    pub(in crate::compositor) fn lifecycle_renderable_surfaces(
        &self,
        sample: &LifecycleSceneSample,
    ) -> Vec<RenderableSurface> {
        let mut surfaces = Vec::new();
        let mut seen = HashSet::new();
        for lifecycle_sample in &sample.samples {
            let Some(entry) = self
                .retained_lifecycle_payloads
                .get_entry_exact(lifecycle_sample.presentation_identity)
                .filter(|payload| {
                    payload.payload.payload_id == lifecycle_sample.payload_id
                        && payload.payload.window_id == lifecycle_sample.window_id
                        && payload.payload.root_surface_id == lifecycle_sample.root_surface_id
                })
            else {
                continue;
            };
            let Some(lease) = entry.pre_capture.as_ref() else {
                continue;
            };
            let Some(projected) = entry
                .payload
                .surface_presentation
                .project_captured_source(&lease.surfaces)
            else {
                debug_assert!(
                    false,
                    "frozen lifecycle source no longer matches its snapshot"
                );
                continue;
            };
            for surface in projected {
                let Some(node) = entry
                    .payload
                    .surface_presentation
                    .nodes
                    .iter()
                    .find(|node| node.key.surface_id == surface.surface_id)
                else {
                    debug_assert!(false, "frozen lifecycle source has no captured key");
                    continue;
                };
                let key = node.key;
                if seen.insert(key) {
                    surfaces.push(surface);
                }
            }
        }
        surfaces
    }

    pub(in crate::compositor) fn lifecycle_precapture_dmabuf_release_count(&self) -> usize {
        self.retained_lifecycle_payloads
            .precapture_dmabuf_release_count()
    }

    pub(in crate::compositor) fn settle_lifecycle_precapture_for_rendered_frame(
        &mut self,
        frame_batch_id: crate::compositor::CompositorFrameBatchId,
        snapshot: &LifecycleFrameSnapshot,
    ) -> usize {
        if !self.frame_batches.contains_key(&frame_batch_id) {
            return 0;
        }
        let mut settled_releases = 0;
        for sample in &snapshot.samples {
            let Some((payload, mut lease)) = self
                .retained_lifecycle_payloads
                .get_entry_exact_mut(sample.presentation_identity)
                .filter(|entry| {
                    entry.payload.payload_id == sample.payload_id
                        && entry.payload.window_id == sample.window_id
                        && entry.payload.root_surface_id == sample.root_surface_id
                })
                .and_then(|entry| {
                    entry
                        .pre_capture
                        .take()
                        .map(|lease| (std::sync::Arc::clone(&entry.payload), lease))
                })
            else {
                continue;
            };
            let conflicts = lease
                .held_release_obligations
                .iter()
                .any(|held| self.buffer_release_is_owned(&held.obligation));
            if conflicts {
                if let Some(entry) = self
                    .retained_lifecycle_payloads
                    .get_entry_exact_mut(sample.presentation_identity)
                {
                    entry.pre_capture = Some(lease);
                }
                continue;
            }

            let surface_count = lease.surfaces.len();
            let dmabuf_surface_count = lease.dmabuf_surface_count();
            let held_release_count = lease.held_release_obligations.len();
            let Some(batch) = self.frame_batches.get_mut(&frame_batch_id) else {
                if let Some(entry) = self
                    .retained_lifecycle_payloads
                    .get_entry_exact_mut(sample.presentation_identity)
                {
                    entry.pre_capture = Some(lease);
                }
                continue;
            };
            let obligations = std::mem::take(&mut lease.held_release_obligations)
                .into_iter()
                .map(|held| held.obligation)
                .collect::<Vec<_>>();
            batch
                .dmabuf_releases_to_complete_on_present
                .extend(obligations);
            if held_release_count > 0 {
                self.buffer_release_metrics.buffer_releases_captured = self
                    .buffer_release_metrics
                    .buffer_releases_captured
                    .saturating_add(held_release_count as u64);
                client_pacing_log(
                    "buffer_releases_captured",
                    &[
                        ("frame_batch_id", frame_batch_id.get().to_string()),
                        ("count", held_release_count.to_string()),
                        ("dmabuf_count", held_release_count.to_string()),
                        ("source", "lifecycle_precapture".to_string()),
                    ],
                );
            }
            settled_releases += held_release_count;
            self.trace_lifecycle_precapture_event(
                super::lifecycle_retained::LifecyclePreCaptureEvent::TransferredToFrame,
                sample.presentation_identity,
                &payload,
                surface_count,
                dmabuf_surface_count,
                held_release_count,
                Some(frame_batch_id.get()),
                None,
            );
            self.trace_lifecycle_precapture_event(
                super::lifecycle_retained::LifecyclePreCaptureEvent::ReleasedAfterCapture,
                sample.presentation_identity,
                &payload,
                surface_count,
                dmabuf_surface_count,
                held_release_count,
                Some(frame_batch_id.get()),
                None,
            );
        }
        settled_releases
    }

    pub(in crate::compositor) fn retire_lifecycle_payload_exact(
        &mut self,
        identity: crate::presentation_animation::PresentationRetainedVisualIdentity,
        reason: &str,
    ) -> Option<std::sync::Arc<super::lifecycle_retained::RetainedLifecyclePayload>> {
        let entry = self.retained_lifecycle_payloads.retire_exact(identity)?;
        Some(self.settle_retired_lifecycle_entry(identity, entry, reason))
    }

    pub(in crate::compositor) fn settle_retired_lifecycle_entry(
        &mut self,
        identity: crate::presentation_animation::PresentationRetainedVisualIdentity,
        mut entry: super::lifecycle_retained::RetainedLifecycleEntry,
        reason: &str,
    ) -> std::sync::Arc<super::lifecycle_retained::RetainedLifecyclePayload> {
        if let Some(lease) = entry.pre_capture.take() {
            self.restore_lifecycle_precapture_lease(identity, &entry.payload, lease, reason);
        }
        entry.payload
    }

    pub(super) fn capture_lifecycle_precapture_source(
        &self,
        window_id: WindowId,
        root_surface_id: u32,
    ) -> Result<LifecyclePreCapturePreparation, LifecycleAdmissionReason> {
        let surfaces = self.lifecycle_surface_candidates(window_id, root_surface_id);
        if !surfaces
            .iter()
            .any(|surface| surface.surface_id == root_surface_id)
        {
            return Err(LifecycleAdmissionReason::NoRenderableRoot);
        }
        let surface_presentation =
            super::lifecycle_surface_snapshot::RetainedSurfacePresentationSnapshot::capture(
                root_surface_id,
                &surfaces,
                &self.surface_presentation_generations,
            )
            .map_err(|failure| match failure {
            super::lifecycle_surface_snapshot::LifecycleSurfaceCaptureFailure::NoRenderableRoot => {
                LifecycleAdmissionReason::NoRenderableRoot
            }
            super::lifecycle_surface_snapshot::LifecycleSurfaceCaptureFailure::MissingPresentationGeneration => {
                LifecycleAdmissionReason::MissingPresentationGeneration
            }
            super::lifecycle_surface_snapshot::LifecycleSurfaceCaptureFailure::InvalidSurfaceTopology => {
                LifecycleAdmissionReason::InvalidSurfaceTopology
            }
        })?;
        if surface_presentation.nodes.len()
            > super::lifecycle_retained::MAX_LIFECYCLE_PRECAPTURE_SURFACES
        {
            return Err(LifecycleAdmissionReason::SourceLeaseUnavailable);
        }
        let Some(surfaces) = surface_presentation.project_captured_source(&surfaces) else {
            return Err(LifecycleAdmissionReason::InvalidSurfaceTopology);
        };
        let Some(render_layout) = FrozenLifecycleRenderLayout::capture(
            self.active_scene_surfaces(),
            &surfaces,
            &self.surface_presentation_generations,
        ) else {
            return Err(LifecycleAdmissionReason::InvalidSurfaceTopology);
        };

        if !surface_presentation.matches_render_layout(&render_layout) {
            return Err(LifecycleAdmissionReason::InvalidSurfaceTopology);
        }

        let mut release_candidates = Vec::new();
        for surface in &surfaces {
            if surface.dmabuf_handle().is_none() {
                continue;
            }
            let Some(obligation) = self.active_dmabuf_buffers.get(&surface.surface_id).cloned()
            else {
                return Err(LifecycleAdmissionReason::SourceLeaseUnavailable);
            };
            if obligation.buffer_id != surface.buffer_id()
                || self.buffer_release_is_owned(&obligation)
                || self
                    .active_dmabuf_buffers
                    .iter()
                    .any(|(owner_surface_id, other)| {
                        *owner_surface_id != surface.surface_id
                            && other.same_release_token(&obligation)
                    })
                || release_candidates.iter().any(
                    |held: &super::lifecycle_retained::LifecycleHeldReleaseObligation| {
                        held.obligation.same_release_token(&obligation)
                    },
                )
            {
                return Err(LifecycleAdmissionReason::SourceLeaseUnavailable);
            }
            release_candidates.push(super::lifecycle_retained::LifecycleHeldReleaseObligation {
                surface_id: surface.surface_id,
                obligation,
            });
        }

        Ok(LifecyclePreCapturePreparation {
            surface_presentation,
            surfaces,
            release_candidates,
            render_layout: std::sync::Arc::new(render_layout),
        })
    }

    pub(super) fn lifecycle_surface_candidates(
        &self,
        window_id: WindowId,
        root_surface_id: u32,
    ) -> Vec<RenderableSurface> {
        let mut candidates = self.renderable_surfaces.iter().collect::<Vec<_>>();
        if let Some(window) = self.window(window_id)
            && window.state.is_minimized()
        {
            candidates.extend(window.state.minimized_surfaces());
        }
        let mut seen = HashSet::new();
        candidates.retain(|surface| seen.insert(surface.surface_id));
        let by_surface_id = candidates
            .iter()
            .map(|surface| (surface.surface_id, *surface))
            .collect::<HashMap<_, _>>();

        fn reaches_root(
            surface_id: u32,
            root_surface_id: u32,
            by_surface_id: &HashMap<u32, &RenderableSurface>,
            visiting: &mut HashSet<u32>,
            memo: &mut HashMap<u32, bool>,
        ) -> bool {
            if surface_id == root_surface_id {
                return true;
            }
            if let Some(result) = memo.get(&surface_id) {
                return *result;
            }
            if !visiting.insert(surface_id) {
                return false;
            }
            let result = by_surface_id
                .get(&surface_id)
                .and_then(|surface| surface.placement.parent_surface_id)
                .is_some_and(|parent_id| {
                    by_surface_id.contains_key(&parent_id)
                        && reaches_root(parent_id, root_surface_id, by_surface_id, visiting, memo)
                });
            visiting.remove(&surface_id);
            memo.insert(surface_id, result);
            result
        }

        let mut ancestry = HashMap::from([(root_surface_id, true)]);
        candidates
            .into_iter()
            .filter(|surface| {
                reaches_root(
                    surface.surface_id,
                    root_surface_id,
                    &by_surface_id,
                    &mut HashSet::new(),
                    &mut ancestry,
                )
            })
            .cloned()
            .collect()
    }

    pub(in crate::compositor) fn transfer_lifecycle_release_candidates(
        &mut self,
        candidates: Vec<super::lifecycle_retained::LifecycleHeldReleaseObligation>,
    ) -> Option<Vec<super::lifecycle_retained::LifecycleHeldReleaseObligation>> {
        let mut transferred = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            if self.buffer_release_is_owned(&candidate.obligation) {
                self.restore_lifecycle_active_releases(transferred);
                return None;
            }
            let Some(active) = self.active_dmabuf_buffers.remove(&candidate.surface_id) else {
                self.restore_lifecycle_active_releases(transferred);
                return None;
            };
            if active.buffer_id != candidate.obligation.buffer_id
                || !active.same_release_token(&candidate.obligation)
            {
                self.active_dmabuf_buffers
                    .insert(candidate.surface_id, active);
                self.restore_lifecycle_active_releases(transferred);
                return None;
            }
            transferred.push(super::lifecycle_retained::LifecycleHeldReleaseObligation {
                surface_id: candidate.surface_id,
                obligation: active,
            });
        }
        Some(transferred)
    }

    pub(in crate::compositor) fn restore_lifecycle_active_releases(
        &mut self,
        releases: Vec<super::lifecycle_retained::LifecycleHeldReleaseObligation>,
    ) {
        for held in releases {
            let same_active_owner = self
                .active_dmabuf_buffers
                .get(&held.surface_id)
                .is_some_and(|active| active.same_release_token(&held.obligation));
            if same_active_owner {
                continue;
            }
            let token_owned_by_another_surface =
                self.active_dmabuf_buffers
                    .iter()
                    .any(|(surface_id, active)| {
                        *surface_id != held.surface_id
                            && active.same_release_token(&held.obligation)
                    });
            if self.buffer_release_is_owned(&held.obligation) || token_owned_by_another_surface {
                self.note_buffer_release_duplicate_attempt();
                continue;
            }
            let release_to_queue = match self.active_dmabuf_buffers.entry(held.surface_id) {
                std::collections::hash_map::Entry::Occupied(_) => Some(held.obligation),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(held.obligation);
                    None
                }
            };
            if let Some(obligation) = release_to_queue {
                self.queue_dmabuf_buffer_release(obligation);
            }
        }
    }

    pub(in crate::compositor) fn restore_lifecycle_precapture_lease(
        &mut self,
        identity: crate::presentation_animation::PresentationRetainedVisualIdentity,
        payload: &super::lifecycle_retained::RetainedLifecyclePayload,
        mut lease: super::lifecycle_retained::LifecyclePreCaptureLease,
        reason: &str,
    ) {
        let surface_count = lease.surfaces.len();
        let dmabuf_surface_count = lease.dmabuf_surface_count();
        let held_release_count = lease.held_release_obligations.len();
        let mut restored = 0;
        let mut queued = 0;
        for held in std::mem::take(&mut lease.held_release_obligations) {
            match self.active_dmabuf_buffers.get(&held.surface_id) {
                Some(active) if active.same_release_token(&held.obligation) => {
                    restored += 1;
                }
                Some(_) => {
                    self.queue_dmabuf_buffer_release(held.obligation);
                    queued += 1;
                }
                None => {
                    let source_is_current = self
                        .current_surface_buffers
                        .get(&held.surface_id)
                        .is_some_and(|current| current.buffer_id() == held.obligation.buffer_id);
                    let token_owned_elsewhere = self.buffer_release_is_owned(&held.obligation)
                        || self
                            .active_dmabuf_buffers
                            .values()
                            .any(|active| active.same_release_token(&held.obligation));
                    if source_is_current && !token_owned_elsewhere {
                        self.active_dmabuf_buffers
                            .insert(held.surface_id, held.obligation);
                        restored += 1;
                    } else if token_owned_elsewhere {
                        self.note_buffer_release_duplicate_attempt();
                        queued += 1;
                    } else {
                        self.queue_dmabuf_buffer_release(held.obligation);
                        queued += 1;
                    }
                }
            }
        }
        if restored > 0 {
            self.trace_lifecycle_precapture_event(
                super::lifecycle_retained::LifecyclePreCaptureEvent::RestoredToLiveOwner,
                identity,
                payload,
                surface_count,
                dmabuf_surface_count,
                held_release_count,
                None,
                Some(reason),
            );
        }
        if queued > 0 || held_release_count == 0 {
            self.trace_lifecycle_precapture_event(
                super::lifecycle_retained::LifecyclePreCaptureEvent::QueuedAfterCancel,
                identity,
                payload,
                surface_count,
                dmabuf_surface_count,
                held_release_count,
                None,
                Some(reason),
            );
        }
        if restored > 0 {
            self.buffer_release_metrics.buffer_releases_restored = self
                .buffer_release_metrics
                .buffer_releases_restored
                .saturating_add(restored as u64);
            client_pacing_log(
                "buffer_releases_restored",
                &[
                    ("frame_batch_id", "none".to_string()),
                    ("count", restored.to_string()),
                    ("source", "lifecycle_precapture".to_string()),
                    ("reason", reason.to_string()),
                ],
            );
        }
        if queued > 0 {
            client_pacing_log(
                "buffer_releases_queued_after_lifecycle_cancel",
                &[
                    ("count", queued.to_string()),
                    ("source", "lifecycle_precapture".to_string()),
                    ("reason", reason.to_string()),
                ],
            );
        }
    }

    pub(in crate::compositor) fn note_lifecycle_precapture_source_withdrawal(
        &mut self,
        surface_ids: &HashSet<u32>,
    ) {
        let leases = self
            .retained_lifecycle_payloads
            .entries_for_source_withdrawal(surface_ids);
        for (identity, payload, surface_count, dmabuf_surface_count, held_release_count) in leases {
            self.trace_lifecycle_precapture_event(
                super::lifecycle_retained::LifecyclePreCaptureEvent::SourceWithdrawnWhileLeased,
                identity,
                &payload,
                surface_count,
                dmabuf_surface_count,
                held_release_count,
                None,
                Some("xwayland_withdrawal"),
            );
        }
    }
}
