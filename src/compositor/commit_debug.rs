use std::collections::HashMap;
use std::num::NonZeroU64;
use wayland_server::{Resource, backend::ObjectId, protocol::wl_callback};

use super::PendingAcquireState;

macro_rules! commit_debug_println {
    ($($arg:tt)*) => {
        super::pacing::commit_debug_log(format!($($arg)*));
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct SurfaceCommitId(NonZeroU64);

impl SurfaceCommitId {
    pub(crate) const fn get(self) -> u64 {
        self.0.get()
    }
    pub(crate) fn for_tests(value: u64) -> Self {
        Self(NonZeroU64::new(value).unwrap())
    }
    pub(crate) fn from_sequence(sequence: super::SurfaceCommitSequence) -> Self {
        Self(NonZeroU64::new(sequence.get()).expect("live surface commit sequence must be nonzero"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SurfaceCommitDisposition {
    Published,
    SupersededWhileUnready,
    SupersededWhileReady,
    Rejected,
    SurfaceDestroyed,
    MergedIntoNewer,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExplicitSyncCommitMetrics {
    pub(crate) explicit_sync_commits_captured: u64,
    pub(crate) explicit_sync_commits_became_ready: u64,
    pub(crate) explicit_sync_commits_published: u64,
    pub(crate) ready_commits_superseded: u64,
    pub(crate) unready_commits_superseded: u64,
    pub(crate) ready_commits_rejected_stale: u64,
    pub(crate) ready_commits_rejected_newer_attachment: u64,
    pub(crate) unready_commits_rejected_stale: u64,
    pub(crate) unready_commits_rejected_newer_attachment: u64,
    pub(crate) callbacks_merged_from_superseded: u64,
    pub(crate) callbacks_completed_from_published: u64,
    pub(crate) callbacks_completed_from_unpublished: u64,
    pub(crate) published_commits_without_visual_generation: u64,
    pub(crate) visual_generations_from_explicit_sync: u64,
}

impl ExplicitSyncCommitMetrics {
    pub(in crate::compositor) fn note_superseded(
        &mut self,
        state: PendingAcquireState,
    ) -> SurfaceCommitDisposition {
        if state == PendingAcquireState::Ready {
            self.ready_commits_superseded = self.ready_commits_superseded.saturating_add(1);
            SurfaceCommitDisposition::SupersededWhileReady
        } else {
            self.unready_commits_superseded = self.unready_commits_superseded.saturating_add(1);
            SurfaceCommitDisposition::SupersededWhileUnready
        }
    }

    pub(in crate::compositor) fn note_publication_rejected(
        &mut self,
        state: PendingAcquireState,
        decision: super::SurfacePublicationDecision,
    ) {
        match (state == PendingAcquireState::Ready, decision) {
            (true, super::SurfacePublicationDecision::StaleAlreadyPublished) => {
                self.ready_commits_rejected_stale =
                    self.ready_commits_rejected_stale.saturating_add(1);
            }
            (true, super::SurfacePublicationDecision::SupersededByNewerAttachment) => {
                self.ready_commits_rejected_newer_attachment = self
                    .ready_commits_rejected_newer_attachment
                    .saturating_add(1);
            }
            (false, super::SurfacePublicationDecision::StaleAlreadyPublished) => {
                self.unready_commits_rejected_stale =
                    self.unready_commits_rejected_stale.saturating_add(1);
            }
            (false, super::SurfacePublicationDecision::SupersededByNewerAttachment) => {
                self.unready_commits_rejected_newer_attachment = self
                    .unready_commits_rejected_newer_attachment
                    .saturating_add(1);
            }
            (_, super::SurfacePublicationDecision::Publish) => {}
        }
    }
}

#[derive(Debug, Clone)]
struct LiveCommit {
    surface: u32,
    root: u32,
    sequence: u64,
    acquire_state: PendingAcquireState,
    visual_generation: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct CallbackOwner {
    commit_id: SurfaceCommitId,
    surface: u32,
    published: bool,
}

#[derive(Debug, Default)]
pub(crate) struct CommitDebugState {
    enabled: bool,
    initialized: bool,
    summary_emitted: bool,
    pageflip_pending: bool,
    live: HashMap<SurfaceCommitId, LiveCommit>,
    callbacks: HashMap<ObjectId, CallbackOwner>,
    metrics: ExplicitSyncCommitMetrics,
}

impl CommitDebugState {
    fn ensure_initialized(&mut self) {
        if !self.initialized {
            self.enabled = std::env::var("TYPHON_COMMIT_DEBUG").ok().is_some_and(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on" | "debug" | "trace"
                )
            });
            self.initialized = true;
        }
    }
}

impl super::CompositorState {
    pub(in crate::compositor) fn note_explicit_commit_captured(
        &mut self,
        id: SurfaceCommitId,
        surface: u32,
        sequence: u64,
        buffer_id: Option<u64>,
        callbacks: &[wl_callback::WlCallback],
    ) {
        self.commit_debug.ensure_initialized();
        let root = self.root_surface_id_for_surface(surface);
        self.commit_debug.metrics.explicit_sync_commits_captured = self
            .commit_debug
            .metrics
            .explicit_sync_commits_captured
            .saturating_add(1);
        let previous = self.commit_debug.live.insert(
            id,
            LiveCommit {
                surface,
                root,
                sequence,
                acquire_state: PendingAcquireState::RegistrationPending,
                visual_generation: None,
            },
        );
        debug_assert!(previous.is_none());
        for callback in callbacks {
            self.commit_debug.callbacks.insert(
                callback.id(),
                CallbackOwner {
                    commit_id: id,
                    surface,
                    published: false,
                },
            );
        }
        self.commit_log(
            "captured",
            id,
            surface,
            sequence,
            buffer_id,
            "captured",
            callbacks.len(),
            "wl_surface_commit",
        );
        for callback in callbacks {
            self.commit_log_callback("callback_requested", id, surface, callback, "captured");
        }
    }

    pub(in crate::compositor) fn note_explicit_commit_acquire_wait(
        &mut self,
        id: SurfaceCommitId,
        callback_count: usize,
    ) {
        let Some(live) = self.commit_debug.live.get(&id).cloned() else {
            return;
        };
        self.commit_log(
            "acquire_wait",
            id,
            live.surface,
            live.sequence,
            None,
            "pending",
            callback_count,
            "fence_unsignaled",
        );
    }

    pub(in crate::compositor) fn note_explicit_commit_ready(&mut self, id: SurfaceCommitId) {
        let Some(live) = self.commit_debug.live.get_mut(&id) else {
            return;
        };
        if live.acquire_state != PendingAcquireState::Ready {
            live.acquire_state = PendingAcquireState::Ready;
            self.commit_debug.metrics.explicit_sync_commits_became_ready = self
                .commit_debug
                .metrics
                .explicit_sync_commits_became_ready
                .saturating_add(1);
        }
        let live = live.clone();
        self.commit_log(
            "acquire_ready",
            id,
            live.surface,
            live.sequence,
            None,
            "ready",
            0,
            "fence_signaled",
        );
    }

    pub(in crate::compositor) fn note_explicit_commit_superseded(
        &mut self,
        id: SurfaceCommitId,
        state: PendingAcquireState,
        callback_count: usize,
        replacement: SurfaceCommitId,
        reason: &str,
    ) {
        let live = self.commit_debug.live.remove(&id);
        let disposition = live
            .as_ref()
            .map(|_| self.commit_debug.metrics.note_superseded(state));
        self.commit_debug.metrics.callbacks_merged_from_superseded = self
            .commit_debug
            .metrics
            .callbacks_merged_from_superseded
            .saturating_add(callback_count as u64);
        let moved = self
            .commit_debug
            .callbacks
            .iter()
            .filter_map(|(callback, owner)| {
                (owner.commit_id == id).then_some((callback.clone(), owner.surface))
            })
            .collect::<Vec<_>>();
        for (callback, surface) in moved {
            if let Some(owner) = self.commit_debug.callbacks.get_mut(&callback) {
                owner.commit_id = replacement;
            }
            if self.commit_debug.enabled {
                commit_debug_println!(
                    "typhon commit: event=callback_moved commit_id={} replacement_commit_id={} surface={surface} callback={callback:?} reason={reason}",
                    id.get(),
                    replacement.get()
                );
            }
        }
        if let (Some(live), Some(disposition)) = (live, disposition) {
            self.commit_log(
                match disposition {
                    SurfaceCommitDisposition::SupersededWhileReady => "superseded_ready",
                    _ => "superseded_unready",
                },
                id,
                live.surface,
                live.sequence,
                None,
                if state == PendingAcquireState::Ready {
                    "ready"
                } else {
                    "unready"
                },
                callback_count,
                reason,
            );
        }
    }

    pub(in crate::compositor) fn note_explicit_commit_merged(
        &mut self,
        id: SurfaceCommitId,
        replacement: SurfaceCommitId,
        callback_count: usize,
    ) {
        let _disposition = SurfaceCommitDisposition::MergedIntoNewer;
        let live = self.commit_debug.live.remove(&id);
        self.commit_debug.metrics.callbacks_merged_from_superseded = self
            .commit_debug
            .metrics
            .callbacks_merged_from_superseded
            .saturating_add(callback_count as u64);
        let moved = self
            .commit_debug
            .callbacks
            .iter()
            .filter_map(|(callback, owner)| {
                (owner.commit_id == id).then_some((callback.clone(), owner.surface))
            })
            .collect::<Vec<_>>();
        for (callback, surface) in moved {
            if let Some(owner) = self.commit_debug.callbacks.get_mut(&callback) {
                owner.commit_id = replacement;
            }
            if self.commit_debug.enabled {
                commit_debug_println!(
                    "typhon commit: event=callback_moved commit_id={} replacement_commit_id={} surface={} callback={callback:?} reason=surface_tree_commit_merged",
                    id.get(),
                    replacement.get(),
                    surface,
                );
            }
        }
        if let Some(live) = live {
            self.commit_log(
                "merged",
                id,
                live.surface,
                live.sequence,
                None,
                if live.acquire_state == PendingAcquireState::Ready {
                    "ready"
                } else {
                    "unready"
                },
                callback_count,
                "surface_tree_commit_merged",
            );
        }
    }

    pub(in crate::compositor) fn note_explicit_commit_visual_generation(
        &mut self,
        id: SurfaceCommitId,
        generation: u64,
    ) {
        let Some(live) = self.commit_debug.live.get_mut(&id) else {
            return;
        };
        live.visual_generation = Some(generation);
        self.commit_debug
            .metrics
            .visual_generations_from_explicit_sync = self
            .commit_debug
            .metrics
            .visual_generations_from_explicit_sync
            .saturating_add(1);
        let Some(live) = self.commit_debug.live.get(&id).cloned() else {
            return;
        };
        self.commit_log(
            "visual_generation",
            id,
            live.surface,
            live.sequence,
            None,
            "published",
            0,
            &generation.to_string(),
        );
    }

    pub(in crate::compositor) fn note_explicit_commit_published(&mut self, id: SurfaceCommitId) {
        let _disposition = SurfaceCommitDisposition::Published;
        for owner in self
            .commit_debug
            .callbacks
            .values_mut()
            .filter(|owner| owner.commit_id == id)
        {
            owner.published = true;
        }
        let Some(live) = self.commit_debug.live.remove(&id) else {
            return;
        };
        self.commit_debug.metrics.explicit_sync_commits_published = self
            .commit_debug
            .metrics
            .explicit_sync_commits_published
            .saturating_add(1);
        if live.visual_generation.is_none() {
            self.commit_debug
                .metrics
                .published_commits_without_visual_generation = self
                .commit_debug
                .metrics
                .published_commits_without_visual_generation
                .saturating_add(1);
        }
        self.commit_log(
            "published",
            id,
            live.surface,
            live.sequence,
            None,
            "ready",
            0,
            "surface_state_published",
        );
    }

    pub(in crate::compositor) fn note_explicit_commit_publication_rejected(
        &mut self,
        id: SurfaceCommitId,
        decision: super::SurfacePublicationDecision,
        latest_published: Option<super::SurfaceCommitSequence>,
        latest_attachment: Option<super::SurfaceCommitSequence>,
    ) {
        let _disposition = SurfaceCommitDisposition::Rejected;
        let Some(live) = self.commit_debug.live.remove(&id) else {
            return;
        };
        self.commit_debug
            .metrics
            .note_publication_rejected(live.acquire_state, decision);
        if self.commit_debug.enabled {
            commit_debug_println!(
                "typhon commit: event=publication_rejected commit_id={} surface={} root={} sequence={} acquire_state={} decision={} latest_published={} latest_attachment_received={}",
                id.get(),
                live.surface,
                live.root,
                live.sequence,
                if live.acquire_state == PendingAcquireState::Ready {
                    "ready"
                } else {
                    "unready"
                },
                match decision {
                    super::SurfacePublicationDecision::Publish => "publish",
                    super::SurfacePublicationDecision::StaleAlreadyPublished => "stale",
                    super::SurfacePublicationDecision::SupersededByNewerAttachment =>
                        "newer_attachment",
                },
                latest_published
                    .map_or_else(|| "none".to_string(), |value| value.get().to_string()),
                latest_attachment
                    .map_or_else(|| "none".to_string(), |value| value.get().to_string()),
            );
        }
    }

    pub(in crate::compositor) fn note_explicit_commit_rejected(
        &mut self,
        id: SurfaceCommitId,
        reason: &str,
    ) {
        let _disposition = SurfaceCommitDisposition::Rejected;
        let Some(live) = self.commit_debug.live.remove(&id) else {
            return;
        };
        self.commit_log(
            "destroyed",
            id,
            live.surface,
            live.sequence,
            None,
            "rejected",
            0,
            reason,
        );
    }

    pub(in crate::compositor) fn note_explicit_commit_destroyed(
        &mut self,
        id: SurfaceCommitId,
        reason: &str,
    ) {
        let _disposition = SurfaceCommitDisposition::SurfaceDestroyed;
        let Some(live) = self.commit_debug.live.remove(&id) else {
            return;
        };
        self.commit_log(
            "destroyed",
            id,
            live.surface,
            live.sequence,
            None,
            "destroyed",
            0,
            reason,
        );
    }

    pub(in crate::compositor) fn note_callbacks_completed(
        &mut self,
        callbacks: &[wl_callback::WlCallback],
    ) {
        for callback in callbacks {
            let Some(owner) = self.commit_debug.callbacks.remove(&callback.id()) else {
                continue;
            };
            if owner.published {
                self.commit_debug.metrics.callbacks_completed_from_published = self
                    .commit_debug
                    .metrics
                    .callbacks_completed_from_published
                    .saturating_add(1);
            } else {
                self.commit_debug
                    .metrics
                    .callbacks_completed_from_unpublished = self
                    .commit_debug
                    .metrics
                    .callbacks_completed_from_unpublished
                    .saturating_add(1);
            }
            self.commit_log_callback(
                "callback_completed",
                owner.commit_id,
                owner.surface,
                callback,
                if owner.published {
                    "published"
                } else {
                    "unpublished"
                },
            );
        }
    }

    pub(in crate::compositor) fn set_commit_debug_pageflip_pending(&mut self, pending: bool) {
        self.commit_debug.pageflip_pending = pending;
    }
    pub(in crate::compositor) fn take_commit_debug_summary_line(&mut self) -> Option<String> {
        self.commit_debug.ensure_initialized();
        if !self.commit_debug.enabled || self.commit_debug.summary_emitted {
            return None;
        }
        self.commit_debug.summary_emitted = true;
        let m = self.commit_debug.metrics;
        Some(format!(
            "typhon commit: event=summary captured={} became_ready={} published={} ready_superseded={} unready_superseded={} ready_rejected_stale={} ready_rejected_newer_attachment={} unready_rejected_stale={} unready_rejected_newer_attachment={} callbacks_moved={} callbacks_completed_from_published={} callbacks_completed_from_unpublished={} published_without_visual_generation={} visual_generations={} queue_overflow={} max_queue_depth={} all_ready_pressure={} unready_retirements={} live_commits={} live_callbacks={}",
            m.explicit_sync_commits_captured,
            m.explicit_sync_commits_became_ready,
            m.explicit_sync_commits_published,
            m.ready_commits_superseded,
            m.unready_commits_superseded,
            m.ready_commits_rejected_stale,
            m.ready_commits_rejected_newer_attachment,
            m.unready_commits_rejected_stale,
            m.unready_commits_rejected_newer_attachment,
            m.callbacks_merged_from_superseded,
            m.callbacks_completed_from_published,
            m.callbacks_completed_from_unpublished,
            m.published_commits_without_visual_generation,
            m.visual_generations_from_explicit_sync,
            self.subsurface_transaction_metrics
                .explicit_sync_queue_overflow,
            self.subsurface_transaction_metrics
                .maximum_explicit_sync_queue_depth,
            self.subsurface_transaction_metrics.all_ready_queue_pressure,
            m.unready_commits_superseded,
            self.commit_debug.live.len(),
            self.commit_debug.callbacks.len()
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_log(
        &self,
        event: &str,
        id: SurfaceCommitId,
        surface: u32,
        sequence: u64,
        buffer: Option<u64>,
        acquire: &str,
        callbacks: usize,
        reason: &str,
    ) {
        if !self.commit_debug.enabled {
            return;
        }
        let live = self.commit_debug.live.get(&id);
        commit_debug_println!(
            "typhon commit: event={event} commit_id={} surface={surface} root={} sequence={sequence} buffer_id={} acquire_state={acquire} callback_count={callbacks} pageflip_pending={} pending_queue_depth={} ready_queue_depth={} visual_generation={} reason={reason}",
            id.get(),
            live.map_or_else(|| self.root_surface_id_for_surface(surface), |l| l.root),
            buffer.map_or_else(|| "none".to_string(), |b| b.to_string()),
            self.commit_debug.pageflip_pending,
            self.pending_explicit_sync_commits.len(),
            self.pending_explicit_sync_commits
                .iter()
                .filter(|c| c.acquire_state == PendingAcquireState::Ready)
                .count(),
            live.and_then(|l| l.visual_generation)
                .map_or_else(|| "none".to_string(), |g| g.to_string())
        );
    }
    fn commit_log_callback(
        &self,
        event: &str,
        id: SurfaceCommitId,
        surface: u32,
        callback: &wl_callback::WlCallback,
        reason: &str,
    ) {
        if self.commit_debug.enabled {
            commit_debug_println!(
                "typhon commit: event={event} commit_id={} surface={surface} root={} sequence={} buffer_id=none acquire_state=none callback_count=1 callback={:?} pageflip_pending={} pending_queue_depth={} ready_queue_depth={} visual_generation=none reason={reason}",
                id.get(),
                self.root_surface_id_for_surface(surface),
                self.commit_debug.live.get(&id).map_or(0, |l| l.sequence),
                callback.id(),
                self.commit_debug.pageflip_pending,
                self.pending_explicit_sync_commits.len(),
                self.pending_explicit_sync_commits
                    .iter()
                    .filter(|c| c.acquire_state == PendingAcquireState::Ready)
                    .count()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_commits_receive_unique_ids() {
        let first = SurfaceCommitId::from_sequence(super::super::SurfaceCommitSequence(11));
        let second = SurfaceCommitId::from_sequence(super::super::SurfaceCommitSequence(13));
        assert_ne!(first, second);
        assert!(second.get() > first.get());
    }

    #[test]
    fn ready_and_unready_supersede_accounting_is_separate() {
        let mut metrics = ExplicitSyncCommitMetrics::default();
        metrics.note_superseded(PendingAcquireState::Ready);
        metrics.note_superseded(PendingAcquireState::RegistrationPending);
        assert_eq!(metrics.ready_commits_superseded, 1);
        assert_eq!(metrics.unready_commits_superseded, 1);
    }

    #[test]
    fn commit_debug_summary_is_emitted_once() {
        let mut state = super::super::CompositorState::default();
        state.commit_debug.initialized = true;
        state.commit_debug.enabled = true;

        assert!(state.take_commit_debug_summary_line().is_some());
        assert!(state.take_commit_debug_summary_line().is_none());
    }

    #[test]
    fn commit_id_survives_ready_visual_and_publication_transitions() {
        let mut state = super::super::CompositorState::default();
        let id = SurfaceCommitId::from_sequence(super::super::SurfaceCommitSequence(11));
        state.note_explicit_commit_captured(id, 7, 11, Some(19), &[]);
        state.note_explicit_commit_ready(id);
        state.note_explicit_commit_visual_generation(id, 23);
        state.note_explicit_commit_published(id);
        assert_eq!(state.commit_debug.metrics.explicit_sync_commits_captured, 1);
        assert_eq!(
            state
                .commit_debug
                .metrics
                .explicit_sync_commits_became_ready,
            1
        );
        assert_eq!(
            state
                .commit_debug
                .metrics
                .visual_generations_from_explicit_sync,
            1
        );
        assert_eq!(
            state.commit_debug.metrics.explicit_sync_commits_published,
            1
        );
        assert!(!state.commit_debug.live.contains_key(&id));
    }

    #[test]
    fn surface_destruction_retires_live_commit_id_without_reuse() {
        let mut state = super::super::CompositorState::default();
        let id = SurfaceCommitId::from_sequence(super::super::SurfaceCommitSequence(11));
        state.note_explicit_commit_captured(id, 7, 11, None, &[]);
        state.note_explicit_commit_destroyed(id, "surface_destroyed");
        let next = SurfaceCommitId::from_sequence(super::super::SurfaceCommitSequence(13));
        assert!(next.get() > id.get());
        assert!(!state.commit_debug.live.contains_key(&id));
    }

    #[test]
    fn ready_newer_attachment_publication_rejection_is_counted() {
        let mut metrics = ExplicitSyncCommitMetrics::default();
        metrics.note_publication_rejected(
            PendingAcquireState::Ready,
            super::super::SurfacePublicationDecision::SupersededByNewerAttachment,
        );

        assert_eq!(metrics.ready_commits_rejected_newer_attachment, 1);
        assert_eq!(metrics.ready_commits_rejected_stale, 0);
    }

    #[test]
    fn merged_commit_identity_retires_predecessor_before_successor_publication() {
        let mut state = super::super::CompositorState::default();
        let predecessor = SurfaceCommitId::for_tests(11);
        let successor = SurfaceCommitId::for_tests(13);
        state.note_explicit_commit_captured(predecessor, 7, 11, None, &[]);
        state.note_explicit_commit_captured(successor, 7, 13, None, &[]);

        state.note_explicit_commit_merged(predecessor, successor, 0);
        state.note_explicit_commit_published(successor);

        assert!(!state.commit_debug.live.contains_key(&predecessor));
        assert!(!state.commit_debug.live.contains_key(&successor));
        assert_eq!(
            state.commit_debug.metrics.explicit_sync_commits_published,
            1
        );
    }
}
