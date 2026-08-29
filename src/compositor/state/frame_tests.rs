use super::super::*;
use crate::wm::{WindowManagementState, WorkspaceId, WorkspaceLocation};

#[cfg(test)]
mod frame_consumption_tests {
    use super::*;

    #[test]
    fn empty_submitted_frame_batch_is_still_owned_until_completion() {
        let mut state = CompositorState::default();
        state.capture_frame_callbacks_for_render();
        state.mark_prepared_frame_submitted();

        assert!(state.has_submitted_frame_batch());
        state.complete_pending_presentation_feedbacks(
            FramePresentation::software_now(state.presentation_clock).unwrap(),
        );
        assert!(!state.has_submitted_frame_batch());
        assert!(state.frame_batches.is_empty());
    }

    #[test]
    fn prepare_publication_does_not_create_a_submitted_frame_batch() {
        let mut state = CompositorState::default();
        state.commit_ready_explicit_sync_buffers();
        assert!(!state.has_submitted_frame_batch());
    }

    #[test]
    fn fifo_head_publication_rechecks_next_wait_against_new_barrier() {
        let mut state = CompositorState::default();
        let mut first = empty_cached_subsurface_commit();
        first.pacing.fifo_set_barrier = true;
        let mut second = empty_cached_subsurface_commit();
        second.pacing.fifo_wait_barrier = true;
        state.pending_surface_tree_transactions.extend([
            PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(1),
                root_surface_id: 7,
                nodes: vec![(7, first)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
            PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(2),
                root_surface_id: 7,
                nodes: vec![(7, second)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
        ]);

        state.commit_ready_surface_tree_transactions();

        assert!(state.active_fifo_barriers.contains_key(&7));
        assert_eq!(state.pending_surface_tree_transactions.len(), 1);
        assert!(
            state.pending_surface_tree_transactions[0].nodes[0]
                .1
                .pacing
                .fifo_wait_barrier
        );
    }

    #[test]
    fn timed_head_cannot_be_superseded_by_untimed_work() {
        let mut state = CompositorState::default();
        let now = client_pacing_now_ns();
        let seconds = now / 1_000_000_000 + 60;
        let mut timed = empty_cached_subsurface_commit();
        timed.pacing.commit_timing = Some(
            CommitTimingConstraint::from_protocol(seconds, (now % 1_000_000_000) as u32).unwrap(),
        );
        state.pending_surface_tree_transactions.extend([
            PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(3),
                root_surface_id: 8,
                nodes: vec![(8, timed)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
            PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(4),
                root_surface_id: 8,
                nodes: vec![(8, empty_cached_subsurface_commit())],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            },
        ]);

        state.commit_ready_surface_tree_transactions();

        assert_eq!(state.pending_surface_tree_transactions.len(), 2);
    }

    #[test]
    fn stale_fifo_generation_cannot_clear_the_current_barrier() {
        let mut state = CompositorState::default();
        let current = ActiveFifoBarrier {
            surface_generation: 3,
            fifo_barrier_generation: FifoBarrierGeneration::new(2),
            commit_sequence: SurfaceCommitSequence::initial(),
            fallback_deadline_ns: u64::MAX,
        };
        state.active_fifo_barriers.insert(9, current);

        state.clear_fifo_barrier_claim(
            FifoBarrierClaim {
                surface_id: 9,
                surface_generation: 3,
                fifo_barrier_generation: FifoBarrierGeneration::new(1),
                commit_sequence: SurfaceCommitSequence::initial(),
            },
            FifoBarrierClearReason::Presented,
        );

        assert_eq!(state.active_fifo_barriers.get(&9), Some(&current));
        assert_eq!(state.surface_pacing_metrics.stale_barrier_clear_attempts, 1);
    }

    #[test]
    fn scene_work_index_tracks_hidden_prepare_work_by_typed_owner() {
        let mut state = CompositorState::new(None);
        let window_id = state.allocate_window_id().expect("window id");
        state
            .insert_desktop_window(DesktopWindow::new_xdg(window_id, 907))
            .expect("window");
        state.window_mut(window_id).expect("window").management = Some(WindowManagementState::new(
            crate::wm::WorkspaceLocation::Regular(WorkspaceId::new(2).expect("workspace two")),
        ));
        state.active_fifo_barriers.insert(
            907,
            ActiveFifoBarrier {
                surface_generation: 1,
                fifo_barrier_generation: FifoBarrierGeneration::new(1),
                commit_sequence: SurfaceCommitSequence::initial(),
                fallback_deadline_ns: u64::MAX,
            },
        );
        state.rebuild_active_scene_view();

        assert_eq!(
            state.scene_work_prepare_count(crate::wm::WorkspaceLocation::Regular(
                WorkspaceId::new(2).unwrap(),
            )),
            1
        );
        assert!(!state.has_pending_frame_prepare_work());
    }

    #[test]
    fn frame_owned_fifo_barrier_is_not_perpetual_prepare_work() {
        let mut state = CompositorState::new(None);
        let claim = FifoBarrierClaim {
            surface_id: 907,
            surface_generation: 1,
            fifo_barrier_generation: FifoBarrierGeneration::new(1),
            commit_sequence: SurfaceCommitSequence::initial(),
        };
        state.active_fifo_barriers.insert(
            claim.surface_id,
            ActiveFifoBarrier {
                surface_generation: claim.surface_generation,
                fifo_barrier_generation: claim.fifo_barrier_generation,
                commit_sequence: claim.commit_sequence,
                fallback_deadline_ns: u64::MAX,
            },
        );
        let batch = state.take_frame_batch_for_render(1);
        state
            .frame_batches
            .get_mut(&batch)
            .expect("frame batch")
            .fifo_barrier_claims
            .push(claim);
        state.rebuild_active_scene_view();

        assert!(!state.has_pending_frame_prepare_work());
    }

    #[test]
    fn future_commit_timing_is_planning_work_not_scene_prepare_work() {
        let mut state = CompositorState::new(None);
        let requested =
            CommitTimingConstraint::from_protocol(client_pacing_now_ns() / 1_000_000_000 + 60, 0)
                .expect("valid commit timing");
        let mut commit = empty_cached_subsurface_commit();
        commit.pacing.commit_timing = Some(requested);
        state
            .pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(1),
                root_surface_id: 8,
                nodes: vec![(8, commit)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            });
        state.rebuild_scene_work_index();

        assert!(!state.has_pending_frame_prepare_work());
        assert!(state.has_pending_commit_timing_planning());
        assert!(state.next_commit_timing_planning_deadline_ns().is_some());
        assert!(state.next_surface_pacing_deadline_ns().is_none());
    }

    #[test]
    fn future_commit_timing_is_not_explicit_sync_service_work() {
        let mut state = CompositorState::new(None);
        let requested =
            CommitTimingConstraint::from_protocol(client_pacing_now_ns() / 1_000_000_000 + 60, 0)
                .expect("valid commit timing");
        let mut commit = empty_cached_subsurface_commit();
        commit.pacing.commit_timing = Some(requested);
        state
            .pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(11),
                root_surface_id: 12,
                nodes: vec![(12, commit)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            });
        state.rebuild_scene_work_index();

        assert!(!state.has_pending_acquire_watch_changes());
    }

    #[test]
    fn fifo_only_transaction_is_not_explicit_sync_service_work() {
        let mut state = CompositorState::new(None);
        let mut commit = empty_cached_subsurface_commit();
        commit.pacing.fifo_set_barrier = true;
        state
            .pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(17),
                root_surface_id: 18,
                nodes: vec![(18, commit)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            });
        state.rebuild_scene_work_index();

        assert!(!state.has_pending_acquire_watch_changes());
    }

    #[test]
    fn unreadable_external_acquire_is_passive_explicit_sync_state() {
        let mut state = CompositorState::new(None);
        state.external_acquire_readiness = true;
        let acquire = ExplicitSyncPoint::for_tests(19, 1);
        state
            .pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(20),
                root_surface_id: 21,
                nodes: vec![(21, empty_cached_subsurface_commit())],
                dependencies: vec![SurfaceTreeAcquireDependency {
                    surface_commit_id: SurfaceCommitId::for_tests(22),
                    commit_id: AcquireCommitId::for_tests(23),
                    surface_id: 21,
                    buffer_id: 24,
                    acquire,
                    state: PendingAcquireState::EventfdBacked,
                }],
                commit_timing_readiness: None,
                received_at: Instant::now(),
            });
        state.rebuild_scene_work_index();

        assert!(!state.has_pending_acquire_watch_changes());
    }

    #[test]
    fn acquire_watch_mutation_is_explicit_sync_service_work() {
        let mut state = CompositorState::new(None);
        state
            .pending_acquire_watch_changes
            .push(AcquireWatchChange::Cancel {
                commit_id: AcquireCommitId::for_tests(25),
                reason: AcquireWatchCancelReason::Superseded,
            });

        assert!(state.has_pending_acquire_watch_changes());
    }

    #[test]
    fn commit_timing_planning_generation_tracks_candidate_set_not_pending_boolean() {
        let mut state = CompositorState::new(None);
        let requested =
            CommitTimingConstraint::from_protocol(client_pacing_now_ns() / 1_000_000_000 + 60, 0)
                .expect("valid commit timing");
        let mut first = empty_cached_subsurface_commit();
        first.pacing.commit_timing = Some(requested);
        state
            .pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(26),
                root_surface_id: 27,
                nodes: vec![(27, first)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            });
        state.rebuild_scene_work_index();
        let first_generation = state.commit_timing_planning_generation();
        assert!(state.has_pending_commit_timing_planning());

        let mut second = empty_cached_subsurface_commit();
        second.pacing.commit_timing = Some(requested);
        state
            .pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(28),
                root_surface_id: 29,
                nodes: vec![(29, second)],
                dependencies: Vec::new(),
                commit_timing_readiness: None,
                received_at: Instant::now(),
            });
        state.rebuild_scene_work_index();

        assert!(state.has_pending_commit_timing_planning());
        assert_ne!(state.commit_timing_planning_generation(), first_generation);
        let second_generation = state.commit_timing_planning_generation();
        state.rebuild_scene_work_index();
        assert_eq!(state.commit_timing_planning_generation(), second_generation);
    }

    #[test]
    fn ordinary_acquire_ready_does_not_create_surface_pacing_debt() {
        let mut state = CompositorState::new(None);
        let acquire = ExplicitSyncPoint::for_tests(12, 1);
        let commit_id = AcquireCommitId::for_tests(13);
        state
            .pending_surface_tree_transactions
            .push(PendingSurfaceTreeTransaction {
                id: SurfaceTreeTransactionId::new(14),
                root_surface_id: 15,
                nodes: vec![(15, empty_cached_subsurface_commit())],
                dependencies: vec![SurfaceTreeAcquireDependency {
                    surface_commit_id: SurfaceCommitId::for_tests(16),
                    commit_id,
                    surface_id: 15,
                    buffer_id: 17,
                    acquire: acquire.clone(),
                    state: PendingAcquireState::EventfdBacked,
                }],
                commit_timing_readiness: None,
                received_at: Instant::now(),
            });
        state.rebuild_scene_work_index();
        let pacing_generation = state.surface_pacing_readiness_generation();

        assert!(state.mark_acquire_commit_ready(commit_id, 15, &acquire));
        assert_eq!(
            state.surface_pacing_readiness_generation(),
            pacing_generation
        );
    }

    #[test]
    fn callback_only_ignores_hidden_prepare_work_but_rejects_visible_prepare_work() {
        let mut state = CompositorState::new(None);
        state.visible_pending_frame_callback_count = 1;
        state
            .scene_work_index
            .add_prepare_work(SceneWorkOwner::Location(WorkspaceLocation::Regular(
                WorkspaceId::new(2).expect("workspace two"),
            )));

        assert!(state.has_only_pending_surface_frame_callbacks());

        state
            .scene_work_index
            .add_prepare_work(SceneWorkOwner::Location(WorkspaceLocation::Regular(
                WorkspaceId::new(1).expect("active workspace"),
            )));
        assert!(!state.has_only_pending_surface_frame_callbacks());
    }

    #[test]
    fn empty_frame_batch_is_explicit_and_registry_is_bounded_to_two() {
        let mut state = CompositorState::default();
        let first = state.take_frame_batch_for_render(10);
        let second = state.take_frame_batch_for_render(11);
        assert_eq!(state.frame_batches.len(), 2);
        assert!(state.frame_batches[&first].callbacks.is_empty());
        assert!(
            state.frame_batches[&second]
                .presentation_feedbacks
                .is_empty()
        );

        let overflow = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.take_frame_batch_for_render(12)
        }));
        assert!(overflow.is_err());
        assert_eq!(state.frame_batches.len(), 2);
    }

    #[test]
    fn no_visual_change_batch_completes_without_presenting() {
        let mut state = CompositorState::default();
        let batch_id = state.take_frame_batch_for_render(12);

        state.complete_no_visual_change_frame_batch(batch_id);

        assert!(state.frame_batches.is_empty());
        assert!(!state.has_submitted_frame_batch());
    }

    #[test]
    fn no_visual_change_batch_settles_owned_surface_damage_without_presentation() {
        let mut state = CompositorState::default();
        let surface_id = 77;
        state.surface_presentation_generations.insert(surface_id, 1);
        let mut journal = SurfaceDamageJournal::new(2);
        let settled = journal.record_for_surface_commit(
            SurfaceCommitSequence(41),
            RenderableSurfaceDamage::Full,
            2,
            2,
        );
        state.surface_damage_journals.insert(surface_id, journal);
        let token = state.capture_surface_damage_presentation_for_surface_commit(
            surface_id,
            SurfaceCommitSequence(41),
        );
        let batch_id = state.take_frame_batch_for_render(13);
        state.set_frame_batch_surface_damage(batch_id, token);

        state.complete_no_visual_change_frame_batch(batch_id);

        assert_eq!(
            state.presented_surface_commits.get(&surface_id),
            Some(&settled)
        );
        assert!(matches!(
            state.surface_damage_journals[&surface_id].damage_since(settled, 2, 2,),
            DamageSince::Empty
        ));
        assert!(state.frame_batches.is_empty());
        let metrics = state.locality_metrics.get();
        assert_eq!(metrics.surface_damage_settlement_no_visual_change, 1);
        assert_eq!(metrics.surface_damage_settlement_presented, 0);
        assert!(settled.0 > 0);
    }

    #[test]
    fn no_visual_change_without_protocol_work_settles_lineage_without_a_batch() {
        let mut state = CompositorState::default();
        let surface_id = 80;
        state.surface_presentation_generations.insert(surface_id, 1);
        let mut journal = SurfaceDamageJournal::new(64);
        let settled = journal.record_for_surface_commit(
            SurfaceCommitSequence(1),
            RenderableSurfaceDamage::Empty,
            2,
            2,
        );
        state.surface_damage_journals.insert(surface_id, journal);
        let token = state.capture_surface_damage_presentation_for_surface_commit(
            surface_id,
            SurfaceCommitSequence(1),
        );

        assert!(state.settle_no_visual_change_work(Some(token), false));
        assert_eq!(
            state.presented_surface_commits.get(&surface_id),
            Some(&settled)
        );
        assert!(settled.0 > 0);
        assert!(state.frame_batches.is_empty());
        assert!(state.legacy_prepared_frame_batch.is_none());
    }

    #[test]
    fn no_visual_change_without_work_does_not_create_an_orphan_batch() {
        let mut state = CompositorState::default();

        assert!(!state.settle_no_visual_change_work(None, false));
        assert!(state.frame_batches.is_empty());
        assert!(state.legacy_prepared_frame_batch.is_none());
    }

    #[test]
    fn no_visual_change_with_protocol_work_owns_exactly_one_terminal_batch() {
        let mut state = CompositorState {
            visible_pending_frame_callback_count: 1,
            ..CompositorState::default()
        };

        assert!(state.settle_no_visual_change_work(None, true));
        assert!(state.frame_batches.is_empty());
        assert!(state.legacy_prepared_frame_batch.is_none());

        let mut state = CompositorState {
            visible_pending_frame_callback_count: 1,
            ..CompositorState::default()
        };
        assert!(state.settle_no_visual_change_work(None, true));
        assert!(state.frame_batches.is_empty());
        assert!(state.legacy_prepared_frame_batch.is_none());
    }

    #[test]
    fn no_visual_change_with_release_work_owns_one_terminal_batch() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(500));

        assert!(state.has_unowned_frame_work());
        assert!(state.settle_no_visual_change_work(None, true));
        assert!(state.frame_batches.is_empty());
        assert!(state.legacy_prepared_frame_batch.is_none());
        assert!(state.pending_dmabuf_buffer_releases.is_empty());
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
    }

    #[test]
    fn protocol_only_tick_drains_release_work_through_terminal_batch() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(501));

        assert_eq!(
            state.complete_protocol_only_frame_tick(FrameCallbackTime::new(1)),
            ProtocolOnlyCompletion::NoCallbacks
        );
        assert!(state.pending_dmabuf_buffer_releases.is_empty());
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
        assert!(state.frame_batches.is_empty());
    }

    #[test]
    fn repeated_surface_only_no_visual_settlement_keeps_lineage_bounded() {
        let mut state = CompositorState::default();
        let surface_id = 81;
        state.surface_presentation_generations.insert(surface_id, 1);
        state
            .surface_damage_journals
            .insert(surface_id, SurfaceDamageJournal::new(64));

        for sequence in 1..=128 {
            let commit_sequence = SurfaceCommitSequence(sequence);
            let journal_commit = state
                .surface_damage_journals
                .get_mut(&surface_id)
                .expect("test journal remains registered")
                .record_for_surface_commit(
                    commit_sequence,
                    RenderableSurfaceDamage::Empty,
                    100,
                    80,
                );
            let token = state.capture_surface_damage_presentation_for_surface_commit(
                surface_id,
                commit_sequence,
            );

            assert!(state.settle_no_visual_change_work(Some(token), false));
            assert!(state.frame_batches.is_empty());
            assert!(state.legacy_prepared_frame_batch.is_none());
            assert_eq!(
                state.presented_surface_commits.get(&surface_id),
                Some(&journal_commit)
            );
            let history = state
                .surface_damage_journals
                .get(&surface_id)
                .expect("test journal remains registered")
                .damage_since(journal_commit, 100, 80);
            assert!(matches!(history, DamageSince::Empty));
        }

        let partial_commit = SurfaceCommitSequence(129);
        let partial = RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 7,
            y: 9,
            width: 3,
            height: 5,
        }]);
        state
            .surface_damage_journals
            .get_mut(&surface_id)
            .expect("test journal remains registered")
            .record_for_surface_commit(partial_commit, partial.clone(), 100, 80);
        let baseline = state
            .presented_surface_commits
            .get(&surface_id)
            .copied()
            .expect("latest Empty commit is the logical baseline");
        assert_eq!(
            state
                .surface_damage_journals
                .get(&surface_id)
                .expect("test journal remains registered")
                .damage_since(baseline, 100, 80),
            DamageSince::Known(partial)
        );
    }

    #[test]
    fn rejected_non_empty_frame_does_not_settle_damage_before_retry() {
        let mut state = CompositorState::default();
        let surface_id = 78;
        state.surface_presentation_generations.insert(surface_id, 1);
        let mut journal = SurfaceDamageJournal::new(8);
        let sampled = journal.record_for_surface_commit(
            SurfaceCommitSequence(1),
            RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
                x: 1,
                y: 1,
                width: 1,
                height: 1,
            }]),
            4,
            4,
        );
        state.surface_damage_journals.insert(surface_id, journal);
        let token = state.capture_surface_damage_presentation_for_surface_commit(
            surface_id,
            SurfaceCommitSequence(1),
        );
        let rejected_batch = state.take_frame_batch_for_render(1);
        state.set_frame_batch_surface_damage(rejected_batch, token);

        state.restore_frame_batch_after_render_failure(rejected_batch);

        assert_eq!(state.presented_surface_commits.get(&surface_id), None);
        assert!(matches!(
            state.surface_damage_journals[&surface_id].damage_since(
                SurfaceCommitCounter::default(),
                4,
                4,
            ),
            DamageSince::Known(RenderableSurfaceDamage::Partial(_))
        ));

        let retry_token = state.capture_surface_damage_presentation_for_surface_commit(
            surface_id,
            SurfaceCommitSequence(1),
        );
        let retry_batch = state.take_frame_batch_for_render(2);
        state.set_frame_batch_surface_damage(retry_batch, retry_token);
        state.complete_presented_frame_batch(
            2,
            retry_batch,
            FramePresentation::software_now(state.presentation_clock).unwrap(),
        );

        assert_eq!(
            state.presented_surface_commits.get(&surface_id),
            Some(&sampled)
        );
        assert_eq!(
            state.surface_damage_journals[&surface_id].damage_since(sampled, 4, 4),
            DamageSince::Empty
        );
    }

    #[test]
    fn no_visual_change_batch_baseline_keeps_later_partial_damage_known() {
        let mut state = CompositorState::default();
        let surface_id = 79;
        state.surface_presentation_generations.insert(surface_id, 1);
        state
            .surface_damage_journals
            .insert(surface_id, SurfaceDamageJournal::new(64));

        for sequence in 1..=128 {
            let commit_sequence = SurfaceCommitSequence(sequence);
            let journal_commit = state
                .surface_damage_journals
                .get_mut(&surface_id)
                .expect("test journal remains registered")
                .record_for_surface_commit(
                    commit_sequence,
                    RenderableSurfaceDamage::Empty,
                    100,
                    80,
                );
            let token = state.capture_surface_damage_presentation_for_surface_commit(
                surface_id,
                commit_sequence,
            );
            let batch_id = state.take_frame_batch_for_render(sequence);
            state.set_frame_batch_surface_damage(batch_id, token);
            state.complete_no_visual_change_frame_batch(batch_id);
            assert_eq!(
                state.presented_surface_commits.get(&surface_id),
                Some(&journal_commit)
            );
        }

        let partial_commit = SurfaceCommitSequence(129);
        let partial = RenderableSurfaceDamage::Partial(vec![SurfaceDamageRect {
            x: 7,
            y: 9,
            width: 3,
            height: 5,
        }]);
        let journal = state
            .surface_damage_journals
            .get_mut(&surface_id)
            .expect("test journal remains registered");
        journal.record_for_surface_commit(partial_commit, partial.clone(), 100, 80);

        let baseline = state
            .presented_surface_commits
            .get(&surface_id)
            .copied()
            .expect("latest Empty commit is the logical baseline");
        assert_eq!(
            state.surface_damage_journals[&surface_id].damage_since(baseline, 100, 80),
            DamageSince::Known(partial)
        );
    }

    #[test]
    fn unrelated_completion_cannot_consume_ready_frame_batch() {
        let mut state = CompositorState::default();
        let submitted = state.take_frame_batch_for_render(20);
        let ready = state.take_frame_batch_for_render(21);
        let presentation = FramePresentation::software_now(state.presentation_clock).unwrap();

        state.complete_presented_frame_batch(20, submitted, presentation);

        assert!(!state.frame_batches.contains_key(&submitted));
        assert!(state.frame_batches.contains_key(&ready));
        state.restore_frame_batch_after_render_failure(ready);
        assert!(state.frame_batches.is_empty());
    }

    #[test]
    fn mismatched_frame_and_batch_identity_completes_nothing() {
        let mut state = CompositorState::default();
        let batch = state.take_frame_batch_for_render(30);
        let presentation = FramePresentation::software_now(state.presentation_clock).unwrap();

        let mismatch = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.complete_presented_frame_batch(31, batch, presentation)
        }));

        assert!(mismatch.is_err());
        assert!(state.frame_batches.contains_key(&batch));
    }

    fn test_dmabuf_release(point: u64) -> SurfaceBufferRelease {
        SurfaceBufferRelease::ExplicitSync(ExplicitSyncPoint::for_tests(99, point))
    }

    fn test_dmabuf_points(releases: &[SurfaceBufferRelease]) -> Vec<u64> {
        releases
            .iter()
            .map(|release| match release {
                SurfaceBufferRelease::ExplicitSync(point) => point.point,
                SurfaceBufferRelease::WlBuffer(_) => panic!("test release is not explicit sync"),
            })
            .collect()
    }

    #[test]
    fn frame_batch_captures_only_releases_pending_at_capture_and_restores_order() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(1));
        state.queue_dmabuf_buffer_release(test_dmabuf_release(2));
        let batch = state.take_frame_batch_for_render(40);
        state.queue_dmabuf_buffer_release(test_dmabuf_release(3));

        assert_eq!(
            test_dmabuf_points(&state.frame_batches[&batch].dmabuf_releases_to_complete_on_present),
            vec![1, 2]
        );
        assert_eq!(
            test_dmabuf_points(&state.pending_dmabuf_buffer_releases),
            vec![3]
        );

        state.restore_frame_batch_after_render_failure(batch);
        assert_eq!(
            test_dmabuf_points(&state.pending_dmabuf_buffer_releases),
            vec![1, 2, 3]
        );
        assert_eq!(state.buffer_release_metrics.buffer_releases_restored, 2);
    }

    #[test]
    fn pending_and_ready_batches_keep_release_sets_disjoint() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(10));
        let pending = state.take_frame_batch_for_render(50);
        state.queue_dmabuf_buffer_release(test_dmabuf_release(11));
        let ready = state.take_frame_batch_for_render(51);

        assert_eq!(
            test_dmabuf_points(
                &state.frame_batches[&pending].dmabuf_releases_to_complete_on_present
            ),
            vec![10]
        );
        assert_eq!(
            test_dmabuf_points(&state.frame_batches[&ready].dmabuf_releases_to_complete_on_present),
            vec![11]
        );

        let presentation = FramePresentation::software_now(state.presentation_clock).unwrap();
        state.complete_presented_frame_batch(50, pending, presentation);
        assert!(state.frame_batches.contains_key(&ready));
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
        assert_eq!(
            test_dmabuf_points(&state.frame_batches[&ready].dmabuf_releases_to_complete_on_present),
            vec![11]
        );
        state.restore_frame_batch_after_render_failure(ready);
    }

    #[test]
    fn frame_batch_completes_all_owned_dmabuf_releases_on_its_matching_presentation() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(20));
        state.queue_dmabuf_buffer_release(test_dmabuf_release(21));
        let batch = state.take_frame_batch_for_render(60);
        let presentation = FramePresentation::software_now(state.presentation_clock).unwrap();
        state.complete_presented_frame_batch(60, batch, presentation);

        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 2);
        assert_eq!(state.buffer_release_metrics.buffer_releases_deferred, 0);
    }

    #[test]
    fn direct_frame_batch_completion_releases_once_and_rejects_duplicate_completion() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(25));
        let batch = state.take_frame_batch_for_render(65);
        let presentation =
            FramePresentation::synchronized(state.presentation_clock, 1, 0, 1).unwrap();

        state.complete_direct_presented_frame_batch(65, batch, 7, presentation);
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
        assert!(state.frame_batches.is_empty());

        let duplicate = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.complete_direct_presented_frame_batch(65, batch, 7, presentation);
        }));
        assert!(duplicate.is_err());
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
    }

    #[test]
    fn failed_frame_retains_release_until_safe_teardown_without_duplication() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(30));
        let batch = state.take_frame_batch_for_render(70);
        state.discard_frame_batch(batch, FrameBatchDiscardReason::FatalOutputFailure);

        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 0);
        assert_eq!(state.retired_frame_batches.len(), 1);
        state.complete_frame_batch_after_safe_abandonment(
            batch,
            FrameBatchDiscardReason::OutputDestroyed,
        );
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
        assert!(state.retired_frame_batches.is_empty());
        state.release_client_buffers_for_shutdown();
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
    }

    #[test]
    fn three_buffer_terminal_sequence_releases_both_spares_without_a_drain_frame() {
        let mut state = CompositorState::default();
        // A is sampled. B replaces A, then C replaces B before output capture.
        state.queue_dmabuf_buffer_release(test_dmabuf_release(100));
        state.queue_dmabuf_buffer_release(test_dmabuf_release(101));
        let frame = state.take_frame_batch_for_render(1745);
        state.complete_presented_frame_batch(
            1745,
            frame,
            FramePresentation::software_now(state.presentation_clock).unwrap(),
        );

        assert_eq!(state.buffer_release_metrics.buffer_releases_captured, 2);
        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 2);
        assert!(state.pending_dmabuf_buffer_releases.is_empty());
        // Either released spare can be used immediately; no synthetic presentation intervenes.
        state.queue_dmabuf_buffer_release(test_dmabuf_release(102));
        assert_eq!(state.pending_dmabuf_buffer_releases.len(), 1);
    }

    #[test]
    fn release_queued_after_capture_is_not_completed_by_the_earlier_frame() {
        let mut state = CompositorState::default();
        state.queue_dmabuf_buffer_release(test_dmabuf_release(200));
        let frame_n = state.take_frame_batch_for_render(80);
        state.queue_dmabuf_buffer_release(test_dmabuf_release(201));

        state.complete_presented_frame_batch(
            80,
            frame_n,
            FramePresentation::software_now(state.presentation_clock).unwrap(),
        );

        assert_eq!(state.buffer_release_metrics.buffer_releases_completed, 1);
        assert_eq!(
            test_dmabuf_points(&state.pending_dmabuf_buffer_releases),
            vec![201]
        );
    }

    #[test]
    fn reused_buffer_with_distinct_explicit_sync_points_is_not_a_duplicate() {
        let mut state = CompositorState::default();
        let first = test_dmabuf_release(300);
        let second = match &first {
            SurfaceBufferRelease::ExplicitSync(point) => {
                SurfaceBufferRelease::ExplicitSync(ExplicitSyncPoint {
                    timeline: point.timeline.clone(),
                    point: 301,
                })
            }
            SurfaceBufferRelease::WlBuffer(_) => unreachable!(),
        };
        state.queue_dmabuf_buffer_release(first);
        state.queue_dmabuf_buffer_release(second);

        assert_eq!(state.pending_dmabuf_buffer_releases.len(), 2);
        assert_eq!(
            state
                .buffer_release_metrics
                .buffer_release_duplicate_attempts,
            0
        );
    }

    #[test]
    fn exact_explicit_sync_point_queued_twice_is_a_true_duplicate() {
        let mut state = CompositorState::default();
        let release = test_dmabuf_release(400);
        state.queue_dmabuf_buffer_release(release.clone());
        state.queue_dmabuf_buffer_release(release);

        assert_eq!(state.pending_dmabuf_buffer_releases.len(), 1);
        assert_eq!(
            state
                .buffer_release_metrics
                .buffer_release_duplicate_attempts,
            1
        );
    }

    #[test]
    fn adversarial_three_buffer_client_completes_one_thousand_presentations() {
        let mut state = CompositorState::default();
        let mut reusable = [false, true, true];
        let mut current = 0usize;
        let mut next_release_point = 1_000u64;

        for frame_id in 1..=1_000 {
            let mut released_this_frame = Vec::with_capacity(2);
            for _ in 0..2 {
                let Some(next) = reusable.iter().position(|available| *available) else {
                    panic!("three-buffer client starved before presentation {frame_id}");
                };
                reusable[next] = false;
                released_this_frame.push(current);
                current = next;
                state.queue_dmabuf_buffer_release(test_dmabuf_release(next_release_point));
                next_release_point += 1;
            }

            let completed_before = state.buffer_release_metrics.buffer_releases_completed;
            let batch = state.take_frame_batch_for_render(frame_id);
            state.complete_presented_frame_batch(
                frame_id,
                batch,
                FramePresentation::software_now(state.presentation_clock).unwrap(),
            );
            assert_eq!(
                state.buffer_release_metrics.buffer_releases_completed - completed_before,
                released_this_frame.len() as u64
            );
            for released in released_this_frame {
                reusable[released] = true;
            }
            reusable[current] = false;
        }

        assert_eq!(state.buffer_release_metrics.buffer_releases_captured, 2_000);
        assert_eq!(
            state.buffer_release_metrics.buffer_releases_completed,
            2_000
        );
        assert_eq!(
            state
                .buffer_release_metrics
                .buffer_release_duplicate_attempts,
            0
        );
        assert!(state.frame_batches.is_empty());
        assert!(state.pending_dmabuf_buffer_releases.is_empty());
    }

    fn terminal_callback_batch() -> (CompositorState, CompositorFrameBatchId) {
        let mut state = CompositorState::default();
        let batch = state.take_frame_batch_for_render(900);
        (state, batch)
    }

    #[test]
    fn no_visual_change_checks_callbacks_before_batch_removal() {
        let (mut state, batch) = terminal_callback_batch();

        assert_eq!(
            state.prepare_terminal_callback_ownership(
                batch,
                TerminalCallbackDisposition::NoVisualChange,
            ),
            TerminalCallbackOwnership::None
        );
        assert!(state.frame_batches.contains_key(&batch));
        state.complete_no_visual_change_frame_batch(batch);
        assert!(!state.frame_batches.contains_key(&batch));
    }

    #[test]
    fn retryable_rejection_validates_callback_transfer_target() {
        let (mut state, batch) = terminal_callback_batch();

        assert_eq!(
            state.prepare_terminal_callback_ownership(
                batch,
                TerminalCallbackDisposition::Retryable,
            ),
            TerminalCallbackOwnership::None
        );
        state.restore_frame_batch_after_render_failure(batch);
        assert!(!state.frame_batches.contains_key(&batch));
    }

    #[test]
    fn safe_abandonment_checks_callbacks_before_cancellation() {
        let (mut state, batch) = terminal_callback_batch();

        assert_eq!(
            state.prepare_terminal_callback_ownership(
                batch,
                TerminalCallbackDisposition::Cancelled,
            ),
            TerminalCallbackOwnership::None
        );
        state.complete_frame_batch_after_safe_abandonment(
            batch,
            FrameBatchDiscardReason::OutputDestroyed,
        );
    }

    #[test]
    fn disconnect_checks_callbacks_before_owner_removal() {
        let (mut state, batch) = terminal_callback_batch();

        assert_eq!(
            state.prepare_terminal_callback_ownership(
                batch,
                TerminalCallbackDisposition::Cancelled,
            ),
            TerminalCallbackOwnership::None
        );
    }

    #[test]
    fn presented_transaction_checks_callbacks_before_completion() {
        let (mut state, batch) = terminal_callback_batch();

        assert_eq!(
            state.prepare_terminal_callback_ownership(
                batch,
                TerminalCallbackDisposition::Presented,
            ),
            TerminalCallbackOwnership::None
        );
    }

    #[test]
    fn injected_terminal_callback_leak_increments_alarm() {
        let mut state = CompositorState::default();
        let missing_batch =
            CompositorFrameBatchId::new(std::num::NonZeroU64::new(901).expect("test batch ID"));

        assert!(matches!(
            state.prepare_terminal_callback_ownership(
                missing_batch,
                TerminalCallbackDisposition::Presented,
            ),
            TerminalCallbackOwnership::Leaked {
                owner,
                unresolved: 0,
                reason: TerminalCallbackLeakReason::MissingBatch,
            } if owner == missing_batch
        ));
    }

    #[test]
    fn callback_count_mismatch_increments_leak_alarm() {
        let (mut state, batch) = terminal_callback_batch();
        state
            .frame_batches
            .get_mut(&batch)
            .expect("terminal callback batch")
            .callback_settlement
            .originally_owned = 1;

        assert!(matches!(
            state.prepare_terminal_callback_ownership(
                batch,
                TerminalCallbackDisposition::Presented,
            ),
            TerminalCallbackOwnership::Leaked {
                owner,
                unresolved: 0,
                reason: TerminalCallbackLeakReason::CountMismatch,
            } if owner == batch
        ));
    }

    #[test]
    fn valid_callback_transfer_does_not_increment_alarm() {
        let (mut state, batch) = terminal_callback_batch();

        assert_eq!(
            state.prepare_terminal_callback_ownership(
                batch,
                TerminalCallbackDisposition::Retryable,
            ),
            TerminalCallbackOwnership::None
        );
    }

    #[test]
    fn callback_leak_check_runs_exactly_once() {
        let (mut state, batch) = terminal_callback_batch();

        let first = state
            .prepare_terminal_callback_ownership(batch, TerminalCallbackDisposition::Presented);
        let second = state
            .prepare_terminal_callback_ownership(batch, TerminalCallbackDisposition::Presented);
        assert_eq!(first, second);
    }

    #[test]
    fn callback_owner_terminal_check_is_prepared_once() {
        let (mut state, batch) = terminal_callback_batch();

        assert_eq!(
            state.prepare_terminal_callback_ownership(
                batch,
                TerminalCallbackDisposition::Presented,
            ),
            TerminalCallbackOwnership::None
        );
    }
}
