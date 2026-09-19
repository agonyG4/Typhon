#[cfg(test)]
#[allow(unused_must_use)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn ticket_frame_id(ticket: Option<WorkerPacingTicket>) -> Option<NativeOutputFrameId> {
        ticket.map(WorkerPacingTicket::frame_id)
    }

    fn predictive_physical_identity(
        frame_id: u64,
        render_generation: u64,
    ) -> OutputFrameIdentitySnapshot {
        OutputFrameIdentitySnapshot {
            output_id: oblivion_one::core::OutputId::from_raw(1).unwrap(),
            frame_id,
            protocol_batch_id: oblivion_one::compositor::CompositorFrameBatchId::new(
                std::num::NonZeroU64::new(frame_id).unwrap(),
            ),
            transaction_id: crate::native_output::OutputTransactionId::new(
                std::num::NonZeroU64::new(frame_id).unwrap(),
            ),
            slot: super::super::scanout::OutputSlotId::new(1).unwrap(),
            framebuffer_id: oblivion_one::native::kms::FramebufferId::new(frame_id as u32).unwrap(),
            render_generation,
            pool_generation: 1,
            target: None,
        }
    }

    #[test]
    fn frame_ids_are_nonzero_and_wrap_to_one() {
        let mut ids = NativeOutputFrameIdSequence::new(u64::MAX);
        assert_eq!(ids.next().get(), u64::MAX);
        assert_eq!(ids.next().get(), 1);
    }

    #[test]
    fn worker_reservation_ids_are_nonzero_and_wrap_to_one() {
        let mut ids = WorkerPacingReservationIdSequence::new(u64::MAX);
        assert_eq!(ids.next().get(), u64::MAX);
        assert_eq!(ids.next().get(), 1);
    }

    #[test]
    fn bounded_samples_report_nearest_rank_percentiles() {
        let mut samples = BoundedSamples::<4>::default();
        for sample in [40, 10, 30, 20, 50] {
            samples.record(sample);
        }
        assert_eq!(samples.len(), 4);
        assert_eq!(samples.percentiles(), (20, 50, 50));
    }

    #[test]
    fn refresh_misses_use_documented_half_interval_tolerance() {
        let mut misses = RefreshMissBuckets::default();
        for interval in [9_000, 9_001, 15_000, 15_001, 21_000, 21_001] {
            misses.record(interval, 6_000);
        }
        assert_eq!(misses.on_time, 1);
        assert_eq!(misses.missed_1x, 2);
        assert_eq!(misses.missed_2x, 2);
        assert_eq!(misses.missed_3x_or_more, 1);
    }

    #[test]
    fn long_idle_gap_is_not_classified_as_an_active_refresh_miss() {
        assert!(is_active_refresh_interval(18_181, 6_060));
        assert!(!is_active_refresh_interval(60_000, 6_060));
    }

    #[test]
    fn pacing_line_is_compact_and_prefixed() {
        let line = pacing_line(
            "wait_for_buffer",
            &[PacingField::u64("frame_id", 7), PacingField::none("ready")],
        );
        assert_eq!(
            line,
            "typhon pacing: event=wait_for_buffer frame_id=7 ready=none"
        );
    }

    #[test]
    fn snapshot_fields_use_stable_slot_values_only() {
        let fields = snapshot_fields(NativeScanoutBufferSnapshot {
            backend: super::super::scanout::NativeScanoutKind::AtomicEglGbmExplicit,
            capacity: None,
            current: None,
            pending: None,
            ready: None,
            free_count: None,
            gbm_surface_has_free_buffers: Some(false),
        });
        assert_eq!(
            pacing_line("decision", &fields),
            "typhon pacing: event=decision backend=atomic-egl-gbm-explicit capacity=none current=none pending=none ready=none free_count=none gbm_surface_has_free_buffers=false"
        );
    }

    #[test]
    fn verbose_trace_drops_when_full_without_blocking() {
        let (sender, _receiver) = sync_channel(1);
        let sink = NativeTraceSink {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
        };
        sink.send("queued".to_string());
        let started = Instant::now();
        sink.send("dropped".to_string());

        assert_eq!(sink.dropped_entries(), 1);
        assert!(started.elapsed().as_millis() < 50);
    }

    #[test]
    fn reactive_double_metrics_never_report_predictive_or_ready_work() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::ReactiveDouble, false);
        pacing.note_submit(41, 2, false, NativeOutputPacingMode::ReactiveDouble);

        assert_eq!(pacing.reactive_double_frames, 1);
        assert_eq!(pacing.reactive_double_immediate_submits, 1);
        assert_eq!(pacing.predictive_render_ahead_attempts, 0);
        assert_eq!(pacing.predictive_render_ahead_ready, 0);
        assert_eq!(pacing.predictive_ready_submits, 0);
        assert_eq!(pacing.normal_ready_wait_count, 0);
    }

    #[test]
    fn predictive_ready_count_cannot_exceed_attempt_count() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        pacing.note_submit(41, 3, true, NativeOutputPacingMode::PredictiveTriple);

        assert_eq!(pacing.predictive_render_ahead_attempts, 1);
        assert_eq!(pacing.predictive_render_ahead_ready, 1);
        assert_eq!(pacing.predictive_ready_submits, 1);
        assert_eq!(pacing.predictive_ready_created, 1);
        assert_eq!(pacing.predictive_ready_submitted, 1);
        assert_eq!(
            pacing.predictive_ready_created,
            pacing.predictive_ready_submitted
                + pacing.predictive_ready_overtaken_ready
                + pacing.predictive_ready_overtaken_worker_queued
                + pacing.predictive_ready_other_safe_abandonment
                + pacing.predictive_ready_failed
                + pacing.predictive_ready_current_at_shutdown
        );
        assert!(pacing.predictive_render_ahead_ready <= pacing.predictive_render_ahead_attempts);
    }

    #[test]
    fn unbound_counters_only_track_predictive_unbound_lifecycle() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        pacing.note_ready_frame(2, true);
        assert_eq!(pacing.predictive_unbound_created, 0);
        assert_eq!(pacing.predictive_unbound_ready, 0);

        pacing.note_predictive_unbound_created();
        pacing.note_predictive_unbound_ready();
        assert_eq!(pacing.predictive_unbound_created, 1);
        assert_eq!(pacing.predictive_unbound_ready, 1);
    }

    #[test]
    fn predictive_ready_lifecycle_reconciles_safe_overtake_failure_and_shutdown() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        assert!(pacing.abandon_ready_frame());

        pacing.queue_visual(3, 2);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(4, true);
        let worker_overtaken = pacing.ready.expect("worker-overtaken ready frame").get();
        pacing.note_predictive_ready_overtaken_worker_queued(Some(worker_overtaken));

        pacing.queue_visual(5, 3);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(6, true);
        let safely_abandoned = pacing.ready.expect("safely abandoned ready frame").get();
        pacing.note_predictive_ready_other_safe_abandonment(Some(safely_abandoned));

        pacing.queue_visual(7, 4);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(8, true);
        let reserved = pacing
            .reserve_worker_submission(true)
            .expect("reserve predictive ready worker submission")
            .expect("predictive ready frame identity");
        assert!(pacing.cancel_worker_submission(Some(reserved)));

        pacing.queue_visual(9, 5);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(10, true);
        pacing.note_predictive_o1_current_at_shutdown();

        assert_eq!(pacing.predictive_ready_created, 5);
        assert_eq!(pacing.predictive_ready_submitted, 0);
        assert_eq!(pacing.predictive_ready_overtaken_ready, 1);
        assert_eq!(pacing.predictive_ready_overtaken_worker_queued, 1);
        assert_eq!(pacing.predictive_ready_other_safe_abandonment, 1);
        assert_eq!(pacing.predictive_ready_failed, 1);
        assert_eq!(pacing.predictive_ready_current_at_shutdown, 1);
        assert_eq!(
            pacing.predictive_ready_created,
            pacing.predictive_ready_submitted
                + pacing.predictive_ready_overtaken_ready
                + pacing.predictive_ready_overtaken_worker_queued
                + pacing.predictive_ready_other_safe_abandonment
                + pacing.predictive_ready_failed
                + pacing.predictive_ready_current_at_shutdown
        );
    }

    #[test]
    fn worker_pacing_submit_records_pending_only_after_success() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);

        assert!(pacing.pending.is_none());
        pacing.note_worker_submit(41, 3, false, NativeOutputPacingMode::ReactiveDouble);
        assert!(pacing.pending.is_some());
    }

    #[test]
    fn worker_submit_settles_reserved_frame_after_active_becomes_ready() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let reserved = pacing
            .reserve_worker_submission(false)
            .expect("worker reservation should be available");

        pacing.note_ready_frame(2, true);
        assert_eq!(
            ticket_frame_id(pacing.worker_reservation),
            reserved.map(WorkerPacingTicket::frame_id)
        );

        pacing
            .note_worker_submit_exact(reserved, 41, 3, NativeOutputPacingMode::PredictiveTriple)
            .expect("the immutable worker reservation should settle once");
        assert!(pacing.ready.is_none());
        assert_eq!(pacing.pending, reserved.map(WorkerPacingTicket::frame_id));
    }

    #[test]
    fn worker_success_does_not_clear_same_logical_id_predictive_successor() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let predecessor = pacing.active.expect("normal predecessor");
        let predecessor_reservation = pacing
            .reserve_worker_submission(false)
            .expect("predecessor worker reservation")
            .expect("predecessor ticket");

        pacing
            .note_render_started(NativeOutputPacingMode::PredictiveTriple, true)
            .expect("same-logical-ID predictive successor");
        let successor_attempt = pacing
            .active_predictive_attempt
            .expect("successor predictive attempt");
        let successor_physical = predictive_physical_identity(5_263, 2);
        pacing
            .bind_predictive_o1(successor_physical)
            .expect("bind successor physical frame");
        pacing.note_render_ready();
        pacing.note_ready_frame(2, true);

        assert_eq!(pacing.ready, Some(predecessor));
        assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));
        assert_eq!(
            pacing.ready_physical_key,
            Some(OutputFrameKey::from(&successor_physical))
        );

        pacing
            .note_worker_submit_exact(
                Some(predecessor_reservation),
                41,
                3,
                NativeOutputPacingMode::PredictiveTriple,
            )
            .expect("predecessor worker success");

        assert_eq!(pacing.pending, Some(predecessor));
        assert!(pacing.pending_predictive_attempt.is_none());
        assert_eq!(pacing.ready, Some(predecessor));
        assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));
        assert_eq!(
            pacing.ready_physical_key,
            Some(OutputFrameKey::from(&successor_physical))
        );

        pacing.note_pageflip_exact(None, 4, 3, 41, 6_060);
        let successor_reservation = pacing
            .reserve_worker_submission(true)
            .expect("successor worker reservation")
            .expect("successor ticket");
        pacing
            .note_worker_submit_exact(
                Some(successor_reservation),
                42,
                5,
                NativeOutputPacingMode::PredictiveTriple,
            )
            .expect("successor worker success");
        pacing.note_pageflip_exact(Some(successor_physical), 6, 5, 42, 6_060);

        assert_eq!(pacing.predictive_o1_presented, 1);
        assert_eq!(pacing.predictive_o1_invalid_stage_transitions, 0);
        assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 0);
    }

    #[test]
    fn worker_cancel_does_not_clear_same_logical_id_predictive_successor() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let predecessor = pacing.active.expect("normal predecessor");
        let predecessor_reservation = pacing
            .reserve_worker_submission(false)
            .expect("predecessor worker reservation")
            .expect("predecessor frame ID");

        pacing
            .note_render_started(NativeOutputPacingMode::PredictiveTriple, true)
            .expect("same-logical-ID predictive successor");
        let successor_attempt = pacing
            .active_predictive_attempt
            .expect("successor predictive attempt");
        let successor_physical = predictive_physical_identity(5_263, 2);
        pacing
            .bind_predictive_o1(successor_physical)
            .expect("bind successor physical frame");
        pacing.note_render_ready();
        pacing.note_ready_frame(2, true);

        assert!(pacing.cancel_worker_submission(Some(predecessor_reservation)));

        assert_eq!(pacing.ready, Some(predecessor));
        assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));
        assert_eq!(
            pacing.ready_physical_key,
            Some(OutputFrameKey::from(&successor_physical))
        );
        assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 1);
    }

    #[test]
    fn worker_cancel_settles_reserved_frame_after_active_becomes_ready() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let reserved = pacing
            .reserve_worker_submission(false)
            .expect("worker reservation should be available");

        pacing.note_ready_frame(2, false);
        assert!(pacing.cancel_worker_submission(reserved));
        assert!(pacing.active.is_none());
        assert!(pacing.ready.is_none());
        assert!(pacing.ready_waiting_started_ns.is_none());
    }

    #[test]
    fn stale_worker_reservation_cannot_settle_or_remove_newer_frame() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let stale = pacing
            .reserve_worker_submission(false)
            .expect("worker reservation should be available");
        assert!(pacing.cancel_worker_submission(stale));

        pacing.queue_visual(2, 2);
        let current = pacing.active;
        assert_ne!(stale.map(WorkerPacingTicket::frame_id), current);
        assert!(
            pacing
                .note_worker_submit_exact(stale, 41, 3, NativeOutputPacingMode::ReactiveDouble,)
                .is_err()
        );
        assert_eq!(pacing.active, current);
        assert!(pacing.pending.is_none());
    }

    #[test]
    fn stale_worker_reservation_cannot_settle_same_logical_id_replacement() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.ids = NativeOutputFrameIdSequence::new(1);
        pacing.queue_visual(1, 1);
        let stale = pacing
            .reserve_worker_submission(false)
            .expect("first worker reservation should be available")
            .expect("first worker ticket");
        assert!(pacing.cancel_worker_submission(Some(stale)));

        pacing.ids = NativeOutputFrameIdSequence::new(stale.frame_id().get());
        pacing.queue_visual(2, 2);
        let current = pacing
            .reserve_worker_submission(false)
            .expect("replacement worker reservation should be available")
            .expect("replacement worker ticket");

        assert_eq!(stale.frame_id(), current.frame_id());
        assert_ne!(stale.reservation_id(), current.reservation_id());
        assert!(
            pacing
                .note_worker_submit_exact(
                    Some(stale),
                    41,
                    3,
                    NativeOutputPacingMode::ReactiveDouble,
                )
                .is_err()
        );
        assert_eq!(pacing.worker_reservation, Some(current));
        assert_eq!(pacing.active, Some(current.frame_id()));
        assert!(pacing.pending.is_none());

        assert!(pacing.cancel_worker_submission(Some(current)));
    }

    #[test]
    fn stale_worker_completion_cannot_clear_newer_colliding_active_timing() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.ids = NativeOutputFrameIdSequence::new(1);
        pacing.queue_visual(1, 1);
        let stale = pacing
            .reserve_worker_submission(false)
            .expect("first worker reservation should be available")
            .expect("first worker ticket");

        pacing
            .note_render_started(NativeOutputPacingMode::ReactiveDouble, false)
            .expect("replacement render attempt");
        pacing.active_queued_frame_id = Some(stale.frame_id());
        pacing.active_queued_ns = Some(99);

        pacing
            .note_worker_submit_exact(Some(stale), 41, 3, NativeOutputPacingMode::ReactiveDouble)
            .expect("old worker result should become pending");

        assert_eq!(pacing.active_queued_frame_id, Some(stale.frame_id()));
        assert_eq!(pacing.active_queued_ns, Some(99));
        assert_eq!(pacing.pending, Some(stale.frame_id()));

        pacing.cancel_unsubmitted_render();
        assert!(pacing.abandon_pending_submission(41));
    }

    fn post_join_pacing_overlap() -> (
        NativeFramePacing,
        WorkerPacingTicket,
        PredictiveO1AttemptId,
        OutputFrameKey,
    ) {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let predecessor_ticket = pacing
            .reserve_worker_submission(false)
            .expect("predecessor worker reservation")
            .expect("predecessor worker ticket");

        pacing
            .note_render_started(NativeOutputPacingMode::PredictiveTriple, true)
            .expect("same-logical-ID predictive successor");
        let successor_attempt = pacing
            .active_predictive_attempt
            .expect("successor predictive attempt");
        let successor_physical = predictive_physical_identity(5_263, 2);
        let successor_key = OutputFrameKey::from(&successor_physical);
        pacing
            .bind_predictive_o1(successor_physical)
            .expect("bind successor physical frame");
        pacing.note_render_ready();
        pacing.note_ready_frame(2, true);

        (pacing, predecessor_ticket, successor_attempt, successor_key)
    }

    #[test]
    fn post_join_returned_job_settles_exact_worker_ticket_and_preserves_successor() {
        let (mut pacing, predecessor_ticket, successor_attempt, successor_key) =
            post_join_pacing_overlap();

        crate::native_output::settle_returned_worker_pacing(&mut pacing, Some(predecessor_ticket))
            .expect("returned worker job settles its exact ticket");

        assert!(pacing.worker_reservation.is_none());
        assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));
        assert_eq!(pacing.ready_physical_key, Some(successor_key));
        assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 1);
    }

    #[test]
    fn post_join_submitted_job_abandons_exact_pending_ticket_and_preserves_successor() {
        let (mut pacing, predecessor_ticket, successor_attempt, successor_key) =
            post_join_pacing_overlap();

        crate::native_output::settle_submitted_worker_pacing(
            &mut pacing,
            Some(predecessor_ticket),
            41,
            3,
            NativeOutputPacingMode::PredictiveTriple,
        )
        .expect("submitted worker job settles and abandons its exact token");

        assert!(pacing.worker_reservation.is_none());
        assert!(pacing.pending.is_none());
        assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));
        assert_eq!(pacing.ready_physical_key, Some(successor_key));
        assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 1);
    }

    #[test]
    fn post_join_stale_worker_ticket_is_rejected_without_touching_collision_successor() {
        let (mut pacing, predecessor_ticket, successor_attempt, successor_key) =
            post_join_pacing_overlap();
        assert!(pacing.cancel_worker_submission(Some(predecessor_ticket)));
        let successor_ticket = pacing
            .reserve_worker_submission(true)
            .expect("successor worker reservation")
            .expect("successor worker ticket");

        assert!(
            crate::native_output::settle_returned_worker_pacing(
                &mut pacing,
                Some(predecessor_ticket),
            )
            .is_err()
        );
        assert_eq!(pacing.worker_reservation, Some(successor_ticket));
        assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));
        assert_eq!(pacing.ready_physical_key, Some(successor_key));
        assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 1);
    }

    #[test]
    fn uncertain_worker_submission_abandons_exact_ticket_and_preserves_successor() {
        let (mut pacing, predecessor_ticket, successor_attempt, successor_key) =
            post_join_pacing_overlap();

        assert!(pacing.abandon_worker_submission(Some(predecessor_ticket)));
        assert!(pacing.worker_reservation.is_none());
        assert_eq!(pacing.ready_predictive_attempt, Some(successor_attempt));
        assert_eq!(pacing.ready_physical_key, Some(successor_key));
        assert_eq!(pacing.predictive_o1_lifecycle.active_entries(), 1);
    }

    #[test]
    fn worker_reservation_settles_exactly_once() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let reserved = pacing
            .reserve_worker_submission(false)
            .expect("worker reservation should be available");
        pacing
            .note_worker_submit_exact(reserved, 41, 2, NativeOutputPacingMode::ReactiveDouble)
            .unwrap();

        assert!(
            pacing
                .note_worker_submit_exact(reserved, 42, 3, NativeOutputPacingMode::ReactiveDouble,)
                .is_err()
        );
        assert_eq!(pacing.pending, reserved.map(WorkerPacingTicket::frame_id));
    }

    #[test]
    fn unreserved_worker_submission_does_not_disturb_active_pacing() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let active = pacing.active;

        assert!(pacing.cancel_worker_submission(None));
        pacing
            .note_worker_submit_exact(None, 41, 2, NativeOutputPacingMode::ReactiveDouble)
            .expect("a compatibility job without a pacing reservation is valid");

        assert_eq!(pacing.active, active);
        assert!(pacing.ready.is_none());
        assert!(pacing.pending.is_none());
    }

    #[test]
    fn rejected_worker_submission_clears_active_identity_and_ready_timing() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        let active = pacing.reserve_worker_submission(false).unwrap();
        assert!(pacing.cancel_worker_submission(active));
        assert!(pacing.active.is_none());
        assert!(pacing.active_queued_ns.is_none());

        pacing.queue_visual(2, 2);
        pacing.note_ready_frame(3, false);
        let ready = pacing.reserve_worker_submission(true).unwrap();
        assert!(pacing.cancel_worker_submission(ready));
        assert!(pacing.ready.is_none());
        assert!(pacing.ready_waiting_started_ns.is_none());
    }

    #[test]
    fn rejected_worker_submission_does_not_leave_pending_presentation_state() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_worker_submit(41, 2, false, NativeOutputPacingMode::ReactiveDouble);
        assert!(pacing.pending.is_some());
        assert!(pacing.abandon_pending_submission(41));
        assert!(pacing.pending.is_none());
    }

    #[test]
    fn ready_worker_submit_records_wait_duration_before_clearing_timing() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_ready_frame(1_000, false);
        let reserved = pacing.reserve_worker_submission(true).unwrap();

        pacing
            .note_worker_submit_exact(
                reserved,
                41,
                51_000,
                NativeOutputPacingMode::PredictiveTriple,
            )
            .unwrap();

        assert_eq!(pacing.ready_waiting_for_target.percentiles(), (50, 50, 50));
        assert!(pacing.ready_waiting_started_ns.is_none());
    }

    #[test]
    fn pacing_summary_exports_reactive_and_deadline_owner_counters() {
        let summary = NativeFramePacing::from_env().summary_line(0, 0);
        for field in [
            "reactive_double_frames=0",
            "reactive_double_immediate_submits=0",
            "reactive_double_actual_misses=0",
            "advisory_dispatch_slips=0",
            "predictive_render_ahead_attempts=0",
            "predictive_render_ahead_ready=0",
            "predictive_ready_submits=0",
            "predictive_ready_created=0",
            "predictive_unbound_created=0",
            "predictive_unbound_ready=0",
            "predictive_bound_after_predecessor_pageflip=0",
            "predictive_bound_after_render_completion=0",
            "predictive_binding_advanced_intervals=0",
            "predictive_unbound_abandoned_identity=0",
            "predictive_unbound_abandoned_generation=0",
            "predictive_ready_submitted=0",
            "predictive_ready_overtaken_ready=0",
            "predictive_ready_overtaken_worker_queued=0",
            "predictive_ready_other_safe_abandonment=0",
            "predictive_ready_failed=0",
            "predictive_ready_current_at_shutdown=0",
            "normal_ready_wait_count=0",
            "scheduled_normal_target_count=0",
            "expired_deadline_wait_count=0",
            "repeated_immediate_timer_wake_count=0",
            "multiple_deadline_owner_violation_count=0",
            "target_identity_reuse_after_abandonment=0",
            "physical_claim_overtake_ready=0",
            "physical_claim_overtake_worker_queued=0",
            "physical_claim_overtake_recoveries=0",
            "physical_claim_overtake_recovery_failures=0",
            "physical_claim_fatal_violations=0",
            "active_pageflip_interval_p50_us=0",
            "active_pageflip_interval_p95_us=0",
            "active_pageflip_interval_p99_us=0",
            "adaptive_triple_entries_proven_presentation_miss=0",
        ] {
            assert!(summary.contains(field), "missing summary field {field}");
        }
    }

    #[test]
    fn advisory_dispatch_slips_are_bounded_in_the_pacing_summary() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.note_advisory_dispatch_slip();
        pacing.note_advisory_dispatch_slip();

        assert_eq!(pacing.advisory_dispatch_slips, 2);
        assert!(
            pacing
                .summary_line(0, 0)
                .contains("advisory_dispatch_slips=2")
        );
    }

    #[test]
    fn pacing_summary_exports_exact_pipeline_wait_reasons() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.note_pipeline_wait(PipelineWaitReason::FuturePrimaryDepthFull);
        pacing.note_pipeline_wait(PipelineWaitReason::KernelCommitPending);

        let summary = pacing.summary_line(0, 0);
        assert!(summary.contains("pipeline_wait_future_primary_depth_full=1"));
        assert!(summary.contains("pipeline_wait_kernel_commit_pending=1"));
        assert!(summary.contains("pipeline_wait_direct_steady_state=0"));
    }

    #[test]
    fn normal_ready_wait_does_not_count_as_predictive_o1() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, false);
        pacing.note_ready_frame(2, true);

        assert_eq!(pacing.normal_ready_wait_count, 1);
        assert_eq!(pacing.predictive_render_ahead_ready, 0);
        assert_eq!(pacing.predictive_ready_created, 0);
        assert!(
            pacing
                .summary_line(0, 0)
                .contains("predictive_o1_created=0")
        );
    }

    #[test]
    fn reactive_ready_wait_then_submit_counts_one_wait() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::ReactiveDouble, false);
        pacing.note_ready_frame(2, true);
        pacing.note_submit(41, 3, true, NativeOutputPacingMode::ReactiveDouble);

        assert_eq!(pacing.normal_ready_wait_count, 1);
    }

    #[test]
    fn repeated_submit_observation_does_not_recount_ready_wait() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::ReactiveDouble, false);
        pacing.note_ready_frame(2, true);
        pacing.note_submit(41, 3, true, NativeOutputPacingMode::ReactiveDouble);
        pacing.note_submit(42, 4, true, NativeOutputPacingMode::ReactiveDouble);

        assert_eq!(pacing.normal_ready_wait_count, 1);
    }

    #[test]
    fn multiple_normal_ready_waits_count_each_frame_once() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        for sequence in 1..=3 {
            pacing.queue_visual(sequence * 10, sequence);
            pacing.note_render_started(NativeOutputPacingMode::ReactiveDouble, false);
            pacing.note_ready_frame(sequence * 10 + 1, true);
            pacing.note_submit(
                sequence * 10 + 2,
                sequence * 10 + 2,
                true,
                NativeOutputPacingMode::ReactiveDouble,
            );
        }

        assert_eq!(pacing.normal_ready_wait_count, 3);
    }

    #[test]
    fn predictive_and_normal_ready_waits_reconcile_independently() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        for sequence in 1..=2 {
            pacing.queue_visual(sequence * 10, sequence);
            pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
            pacing.note_ready_frame(sequence * 10 + 1, true);
            pacing.note_submit(
                sequence * 10 + 2,
                sequence * 10 + 2,
                true,
                NativeOutputPacingMode::PredictiveTriple,
            );
            pacing.note_pageflip(
                sequence * 10 + 3,
                sequence * 10 + 2,
                sequence * 10 + 2,
                6_060,
            );
        }
        for sequence in 1..=3 {
            pacing.queue_visual(sequence * 20, sequence + 2);
            pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, false);
            pacing.note_ready_frame(sequence * 20 + 1, true);
            pacing.note_submit(
                sequence * 20 + 2,
                sequence * 20 + 2,
                true,
                NativeOutputPacingMode::PredictiveTriple,
            );
            pacing.note_pageflip(
                sequence * 20 + 3,
                sequence * 20 + 2,
                sequence * 20 + 2,
                6_060,
            );
        }

        assert_eq!(pacing.predictive_render_ahead_attempts, 2);
        assert_eq!(pacing.predictive_render_ahead_ready, 2);
        assert_eq!(pacing.predictive_ready_created, 2);
        assert_eq!(pacing.normal_ready_wait_count, 3);
        assert!(
            pacing
                .summary_line(0, 0)
                .contains("predictive_o1_render_ready=2")
        );
    }

    #[test]
    fn overlapping_predictive_ready_frames_reconcile_by_exact_worker_identity() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        let p1 = pacing
            .reserve_worker_submission(true)
            .unwrap()
            .expect("P1 worker reservation");

        pacing.queue_visual(3, 2);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(4, true);
        pacing
            .note_worker_submit_exact(Some(p1), 41, 5, NativeOutputPacingMode::PredictiveTriple)
            .unwrap();

        let p2 = pacing
            .reserve_worker_submission(true)
            .unwrap()
            .expect("P2 worker reservation");
        pacing
            .note_worker_submit_exact(Some(p2), 42, 6, NativeOutputPacingMode::PredictiveTriple)
            .unwrap();

        assert_eq!(pacing.predictive_ready_created, 2);
        assert_eq!(pacing.predictive_ready_submitted, 2);
        assert!(
            pacing
                .summary_line(0, 0)
                .contains("predictive_o1_submitted=2")
        );
    }

    #[test]
    fn ready_terminal_does_not_choose_older_worker_entry() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        let older_worker = pacing
            .reserve_worker_submission(true)
            .unwrap()
            .expect("older worker reservation");
        let older_attempt = older_worker
            .predictive_attempt_id()
            .expect("older worker predictive attempt");

        pacing.queue_visual(3, 2);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(4, true);
        let newer_ready = pacing
            .ready_predictive_attempt
            .expect("newer ready predictive attempt");

        pacing.note_predictive_ready_other_safe_abandonment(Some(newer_ready.get()));

        assert_eq!(pacing.predictive_o1_other_safe_abandonment, 1);
        assert!(
            pacing
                .predictive_o1_lifecycle
                .contains_attempt(older_attempt)
        );
        assert!(!pacing.predictive_o1_lifecycle.contains_attempt(newer_ready));
    }

    #[test]
    fn worker_terminal_does_not_choose_newer_ready_entry() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        let older_worker = pacing
            .reserve_worker_submission(true)
            .unwrap()
            .expect("older worker reservation");
        let older_attempt = older_worker
            .predictive_attempt_id()
            .expect("older worker predictive attempt");

        pacing.queue_visual(3, 2);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(4, true);
        let newer_ready = pacing
            .ready_predictive_attempt
            .expect("newer ready predictive attempt");

        pacing.note_predictive_ready_overtaken_worker_queued(Some(older_attempt.get()));

        assert_eq!(pacing.predictive_o1_other_safe_abandonment, 1);
        assert!(
            !pacing
                .predictive_o1_lifecycle
                .contains_attempt(older_attempt)
        );
        assert!(pacing.predictive_o1_lifecycle.contains_attempt(newer_ready));
    }

    #[test]
    fn unknown_predictive_terminal_id_does_not_mutate_live_entry() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        let live = pacing.ready.expect("live ready frame").get();

        pacing.note_predictive_ready_other_safe_abandonment(Some(live.saturating_add(999)));

        assert_eq!(pacing.predictive_o1_other_safe_abandonment, 0);
        assert!(
            pacing
                .predictive_o1_lifecycle
                .contains_attempt(PredictiveO1AttemptId::new(live))
        );

        pacing.note_predictive_ready_other_safe_abandonment(Some(live));

        assert_eq!(pacing.predictive_o1_other_safe_abandonment, 1);
        assert!(
            !pacing
                .predictive_o1_lifecycle
                .contains_attempt(PredictiveO1AttemptId::new(live))
        );
    }

    #[test]
    fn newer_predictive_ready_can_terminalize_before_older_pageflip() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        pacing.note_submit(41, 3, true, NativeOutputPacingMode::PredictiveTriple);

        pacing.queue_visual(4, 2);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(5, true);
        assert!(pacing.abandon_ready_frame());
        pacing.note_pageflip(6, 3, 41, 6_060);

        assert_eq!(pacing.predictive_ready_submitted, 1);
        assert_eq!(pacing.predictive_ready_overtaken_ready, 1);
        assert!(
            pacing
                .summary_line(0, 0)
                .contains("predictive_o1_terminal_reconciled=true")
        );
    }

    #[test]
    fn duplicate_predictive_submit_terminal_is_counted_once() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        let id = pacing.reserve_worker_submission(true).unwrap();

        pacing
            .note_worker_submit_exact(id, 41, 3, NativeOutputPacingMode::PredictiveTriple)
            .unwrap();
        assert!(
            pacing
                .note_worker_submit_exact(id, 42, 4, NativeOutputPacingMode::PredictiveTriple,)
                .is_err()
        );

        assert_eq!(pacing.predictive_ready_submitted, 1);
        assert!(
            pacing
                .summary_line(0, 0)
                .contains("predictive_o1_submitted=1")
        );
    }

    #[test]
    fn generation_abandonment_closes_exact_predictive_lifecycle() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        let frame_id = pacing.ready.map(NativeOutputFrameId::get);
        pacing.note_predictive_unbound_abandoned_generation(frame_id);

        assert_eq!(pacing.predictive_unbound_abandoned_generation, 1);
        assert!(
            pacing
                .summary_line(0, 0)
                .contains("predictive_o1_abandoned_generation=1")
        );
    }

    #[test]
    fn identity_abandonment_closes_exact_predictive_lifecycle() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        let frame_id = pacing.ready.map(NativeOutputFrameId::get);
        pacing.note_predictive_unbound_abandoned_identity(frame_id);

        assert_eq!(pacing.predictive_unbound_abandoned_identity, 1);
        assert!(
            pacing
                .summary_line(0, 0)
                .contains("predictive_o1_abandoned_identity=1")
        );
    }

    #[test]
    fn shutdown_reconciles_every_current_predictive_identity() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        pacing.queue_visual(3, 2);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);

        pacing.note_predictive_o1_current_at_shutdown();

        assert!(
            pacing
                .summary_line(0, 0)
                .contains("predictive_ready_current_at_shutdown=2")
        );
        assert!(
            pacing
                .summary_line(0, 0)
                .contains("predictive_o1_current_at_shutdown=2")
        );
    }

    #[test]
    fn successful_predictive_lifecycle_reports_each_applicable_stage() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, true);
        pacing.note_ready_frame(2, true);
        pacing.note_predictive_unbound_ready();
        pacing.note_predictive_binding_after_render_completion(0);
        let id = pacing.reserve_worker_submission(true).unwrap();
        pacing
            .note_worker_submit_exact(id, 41, 3, NativeOutputPacingMode::PredictiveTriple)
            .unwrap();
        pacing.note_pageflip(4, 3, 41, 6_060);

        let summary = pacing.summary_line(0, 0);
        for field in [
            "predictive_o1_created=1",
            "predictive_o1_render_ready=1",
            "predictive_o1_ready_unbound=1",
            "predictive_o1_bound=1",
            "predictive_o1_worker_queued=1",
            "predictive_o1_submitted=1",
            "predictive_o1_presented=1",
            "predictive_o1_terminal_reconciled=true",
        ] {
            assert!(summary.contains(field), "missing summary field {field}");
        }
    }

    #[test]
    fn normal_reactive_and_direct_paths_leave_predictive_o1_lifecycle_empty() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        pacing.queue_visual(1, 1);
        pacing.note_render_started(NativeOutputPacingMode::ReactiveDouble, false);
        pacing.note_submit(41, 2, false, NativeOutputPacingMode::ReactiveDouble);
        pacing.note_pageflip(3, 2, 41, 6_060);

        pacing.queue_visual(4, 2);
        pacing.note_render_started(NativeOutputPacingMode::PredictiveTriple, false);
        pacing.note_submit(42, 5, false, NativeOutputPacingMode::PredictiveTriple);
        pacing.note_pageflip(6, 5, 42, 6_060);

        pacing.queue_visual(7, 3);

        let summary = pacing.summary_line(0, 0);
        for field in [
            "predictive_o1_created=0",
            "predictive_o1_render_ready=0",
            "predictive_o1_submitted=0",
            "predictive_o1_presented=0",
        ] {
            assert!(summary.contains(field), "missing summary field {field}");
        }
    }

    #[test]
    fn presentation_miss_entry_has_a_dedicated_adaptive_metric() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;

        pacing.note_adaptive_transition(
            AdaptiveBufferingMode::Double,
            AdaptiveBufferingMode::Triple,
            Some(ProvenDeadlineMiss::KmsApplyGuard),
        );

        assert_eq!(pacing.adaptive_triple_entries_predicted, 0);
        assert_eq!(pacing.adaptive_triple_entries_proven_render_miss, 0);
        assert_eq!(pacing.adaptive_triple_entries_proven_submit_miss, 0);
        assert_eq!(pacing.adaptive_triple_entries_proven_presentation_miss, 1);
    }

    #[test]
    fn deadline_state_stress_has_no_expired_wait_or_immediate_wake_loop() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        for frame in 0..1_000_u64 {
            let now = frame * 6_060_606;
            pacing.note_deadline_state(SchedulerDecision::Render, now, None, None, false, false);
        }

        assert_eq!(pacing.expired_deadline_wait_count, 0);
        assert_eq!(pacing.repeated_immediate_timer_wake_count, 0);
        assert_eq!(pacing.multiple_deadline_owner_violation_count, 0);
    }

    #[test]
    fn deadline_diagnostics_count_each_forbidden_state() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.note_deadline_state(
            SchedulerDecision::WaitForRefresh,
            10,
            None,
            Some(9),
            true,
            true,
        );
        pacing.note_deadline_state(
            SchedulerDecision::WaitForRefresh,
            10,
            None,
            Some(9),
            true,
            true,
        );

        assert_eq!(pacing.expired_deadline_wait_count, 2);
        assert_eq!(pacing.repeated_immediate_timer_wake_count, 1);
        assert_eq!(pacing.multiple_deadline_owner_violation_count, 2);
    }

    #[test]
    fn active_pageflip_percentiles_exclude_idle_gaps() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        for now_ns in [6_060_000, 12_121_000, 24_241_000, 84_241_000, 90_301_000] {
            pacing.note_pageflip(now_ns, now_ns, 1, 6_060);
        }

        let timing = pacing.timing_metrics();
        assert_eq!(timing.active_pageflip_interval, (6_061, 12_120, 12_120));
        assert_eq!(pacing.idle_intervals_excluded, 1);
        assert_eq!(timing.pageflip_interval, (6_061, 60_000, 60_000));
    }

    #[test]
    fn content_clock_summary_exposes_bounded_stage_and_attribution_metrics() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.note_explicit_present(ExplicitPresentationObservation {
            planned_sequence: 4,
            actual_sequence: 2,
            target_ns: 1_018_181_818,
            presented_ns: 1_012_121_212,
            composite_started_ns: 1_010_000_000,
            rendered_ns: 1_011_000_000,
            submit_started_ns: 1_011_100_000,
            submit_returned_ns: 1_011_300_000,
            reactive_double: true,
            target_reason: oblivion_one::native::presentation_deadline::PresentationTargetReason::ReactiveDouble,
            target_selection: TargetSelectionEvidence {
                earliest_feasible_sequence: 2,
                binding: false,
            },
            previous_primary_sequence: Some(1),
            client_commit_ns: Some(1_009_500_000),
            callback_reaction_ns: Some(500_000),
            callback_admission_ns: None,
            callback_surface_id: Some(7),
            callback_surface_is_exclusive: true,
            refresh_interval_ns: 6_060_606,
            render_missed: false,
            submit_missed: false,
            kms_slipped: false,
        });

        let summary = pacing.content_summary_line();
        for field in [
            "event=native_content_frame_clock_summary",
            "primary_present_interval_p50_us=0",
            "callback_admission_to_next_commit_p50_us=500",
            "client_commit_to_render_start_p50_us=500",
            "render_start_to_ready_p50_us=1000",
            "ready_to_submit_p50_us=100",
            "submit_to_pageflip_p50_us=821",
            "selected_target_distance_intervals_p50=3",
            "actual_primary_distance_intervals_p50=1",
            "reactive_target_late_by_intervals=1",
            "fast_client_samples=1",
            "fast_candidate_seen=1",
            "fast_candidate_qualified=0",
            "fast_candidate_rejected_missing_admission=1",
            "content_attribution_target_hit=1",
            "prediction_total_cost_ns=0",
        ] {
            assert!(summary.contains(field), "missing content field {field}");
        }
    }

    #[test]
    fn prediction_summary_exposes_selected_estimator_arithmetic() {
        let journal = oblivion_one::native::adaptive_buffering::AdaptiveRenderJournal::default();
        let prediction =
            journal.prediction_with_kms_guard(std::time::Duration::from_millis(10), 100_000);
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        pacing.note_prediction(prediction);

        let summary = pacing.content_summary_line();
        for field in [
            "prediction_independent_total_cost_ns=5350000",
            "prediction_warm_paired_total_cost_ns=1350000",
            "prediction_independent_p90_floor_ns=1350000",
            "prediction_worker_non_ioctl_lead_ns=250000",
            "prediction_miss_recovery_remaining=0",
            "prediction_estimator_mode=cold_start",
            "prediction_total_cost_ns=5350000",
        ] {
            assert!(summary.contains(field), "missing estimator field {field}");
        }
    }

    #[test]
    fn fast_client_population_requires_continuous_exact_surface_content() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        let refresh_ns = 6_060_606_u64;
        let base_ns = 1_000_000_000_u64;

        for frame in 0..128_u64 {
            let presented_ns = base_ns + frame * refresh_ns;
            pacing.note_explicit_present(ExplicitPresentationObservation {
                planned_sequence: frame + 1,
                actual_sequence: frame + 1,
                target_ns: presented_ns,
                presented_ns,
                composite_started_ns: presented_ns.saturating_sub(2_000_000),
                rendered_ns: presented_ns.saturating_sub(1_000_000),
                submit_started_ns: presented_ns.saturating_sub(900_000),
                submit_returned_ns: presented_ns.saturating_sub(700_000),
                reactive_double: false,
                target_reason:
                    oblivion_one::native::presentation_deadline::PresentationTargetReason::Normal,
                target_selection: TargetSelectionEvidence {
                    earliest_feasible_sequence: frame + 1,
                    binding: false,
                },
                previous_primary_sequence: (frame > 0).then_some(frame),
                client_commit_ns: Some(presented_ns.saturating_sub(2_500_000)),
                callback_reaction_ns: Some(500_000),
                callback_admission_ns: Some(presented_ns.saturating_sub(3_000_000)),
                callback_surface_id: Some(7),
                callback_surface_is_exclusive: true,
                refresh_interval_ns: refresh_ns,
                render_missed: false,
                submit_missed: false,
                kms_slipped: false,
            });
        }

        let continuous_before_exclusions = pacing.fast_client_continuous_samples;
        assert_eq!(continuous_before_exclusions, 127);
        assert_eq!(
            pacing.fast_client_primary_present_intervals.percentiles(),
            (6_060, 6_060, 6_060)
        );
        assert_eq!(
            pacing
                .fast_client_actual_primary_distance_intervals
                .percentiles(),
            (1, 1, 1)
        );
        assert_eq!(pacing.fast_client_target_hit, 127);

        let summary = pacing.content_summary_line();
        for field in [
            "fast_client_continuous_samples=127",
            "fast_candidate_seen=128",
            "fast_candidate_qualified=128",
            "fast_continuity_seeded=1",
            "fast_continuity_sampled=127",
            "fast_client_primary_present_interval_p50_us=6060",
            "fast_client_actual_primary_distance_p50=1",
            "fast_client_missed_refresh_1x=0",
            "fast_client_target_hit=127",
        ] {
            assert!(summary.contains(field), "missing fast-client field {field}");
        }

        let idle_presented_ns = base_ns + 128 * refresh_ns + 100 * refresh_ns;
        pacing.note_explicit_present(ExplicitPresentationObservation {
            planned_sequence: 129,
            actual_sequence: 129,
            target_ns: idle_presented_ns,
            presented_ns: idle_presented_ns,
            composite_started_ns: idle_presented_ns.saturating_sub(2_000_000),
            rendered_ns: idle_presented_ns.saturating_sub(1_000_000),
            submit_started_ns: idle_presented_ns.saturating_sub(900_000),
            submit_returned_ns: idle_presented_ns.saturating_sub(700_000),
            reactive_double: false,
            target_reason:
                oblivion_one::native::presentation_deadline::PresentationTargetReason::Normal,
            target_selection: TargetSelectionEvidence {
                earliest_feasible_sequence: 129,
                binding: false,
            },
            previous_primary_sequence: Some(128),
            client_commit_ns: Some(idle_presented_ns.saturating_sub(2_500_000)),
            callback_reaction_ns: Some(
                idle_presented_ns.saturating_sub(base_ns + 127 * refresh_ns),
            ),
            callback_admission_ns: Some(base_ns + 127 * refresh_ns),
            callback_surface_id: Some(7),
            callback_surface_is_exclusive: true,
            refresh_interval_ns: refresh_ns,
            render_missed: false,
            submit_missed: false,
            kms_slipped: false,
        });
        assert_eq!(
            pacing.fast_client_continuous_samples,
            continuous_before_exclusions
        );

        let next_presented_ns = idle_presented_ns + refresh_ns;
        pacing.note_explicit_present(ExplicitPresentationObservation {
            planned_sequence: 130,
            actual_sequence: 130,
            target_ns: next_presented_ns,
            presented_ns: next_presented_ns,
            composite_started_ns: next_presented_ns.saturating_sub(2_000_000),
            rendered_ns: next_presented_ns.saturating_sub(1_000_000),
            submit_started_ns: next_presented_ns.saturating_sub(900_000),
            submit_returned_ns: next_presented_ns.saturating_sub(700_000),
            reactive_double: false,
            target_reason:
                oblivion_one::native::presentation_deadline::PresentationTargetReason::Normal,
            target_selection: TargetSelectionEvidence {
                earliest_feasible_sequence: 130,
                binding: false,
            },
            previous_primary_sequence: Some(129),
            client_commit_ns: Some(next_presented_ns.saturating_sub(2_500_000)),
            callback_reaction_ns: Some(3_000_000),
            callback_admission_ns: Some(next_presented_ns.saturating_sub(3_500_000)),
            callback_surface_id: Some(7),
            callback_surface_is_exclusive: true,
            refresh_interval_ns: refresh_ns,
            render_missed: false,
            submit_missed: false,
            kms_slipped: false,
        });
        assert_eq!(
            pacing.fast_client_continuous_samples,
            continuous_before_exclusions
        );

        let ambiguous_presented_ns = next_presented_ns + refresh_ns;
        pacing.note_explicit_present(ExplicitPresentationObservation {
            planned_sequence: 131,
            actual_sequence: 131,
            target_ns: ambiguous_presented_ns,
            presented_ns: ambiguous_presented_ns,
            composite_started_ns: ambiguous_presented_ns.saturating_sub(2_000_000),
            rendered_ns: ambiguous_presented_ns.saturating_sub(1_000_000),
            submit_started_ns: ambiguous_presented_ns.saturating_sub(900_000),
            submit_returned_ns: ambiguous_presented_ns.saturating_sub(700_000),
            reactive_double: false,
            target_reason:
                oblivion_one::native::presentation_deadline::PresentationTargetReason::Normal,
            target_selection: TargetSelectionEvidence {
                earliest_feasible_sequence: 131,
                binding: false,
            },
            previous_primary_sequence: Some(130),
            client_commit_ns: Some(ambiguous_presented_ns.saturating_sub(2_500_000)),
            callback_reaction_ns: Some(500_000),
            callback_admission_ns: Some(ambiguous_presented_ns.saturating_sub(3_000_000)),
            callback_surface_id: Some(7),
            callback_surface_is_exclusive: false,
            refresh_interval_ns: refresh_ns,
            render_missed: false,
            submit_missed: false,
            kms_slipped: false,
        });
        assert_eq!(
            pacing.fast_client_continuous_samples,
            continuous_before_exclusions
        );
    }

    #[test]
    fn fast_candidate_diagnostics_account_for_qualification_and_continuity_outcomes() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        let refresh_ns = 6_060_606_u64;
        let mut observation = ExplicitPresentationObservation {
            planned_sequence: 1,
            actual_sequence: 1,
            target_ns: 10_000_000,
            presented_ns: 10_000_000,
            composite_started_ns: 8_000_000,
            rendered_ns: 9_000_000,
            submit_started_ns: 9_100_000,
            submit_returned_ns: 9_300_000,
            reactive_double: false,
            target_reason:
                oblivion_one::native::presentation_deadline::PresentationTargetReason::Normal,
            target_selection: TargetSelectionEvidence {
                earliest_feasible_sequence: 1,
                binding: false,
            },
            previous_primary_sequence: None,
            client_commit_ns: Some(7_500_000),
            callback_reaction_ns: Some(500_000),
            callback_admission_ns: Some(7_000_000),
            callback_surface_id: Some(7),
            callback_surface_is_exclusive: true,
            refresh_interval_ns: refresh_ns,
            render_missed: false,
            submit_missed: false,
            kms_slipped: false,
        };

        observation.callback_surface_id = None;
        pacing.note_explicit_present(observation);
        observation.callback_surface_id = Some(7);
        observation.callback_surface_is_exclusive = false;
        pacing.note_explicit_present(observation);
        observation.callback_surface_is_exclusive = true;
        observation.client_commit_ns = None;
        pacing.note_explicit_present(observation);
        observation.client_commit_ns = Some(7_500_000);
        observation.callback_admission_ns = None;
        pacing.note_explicit_present(observation);

        pacing.note_pageflip(1_000_000, 1_000_000, 1, refresh_ns / 1_000);
        observation.callback_admission_ns = Some(8_000_000);
        observation.client_commit_ns = Some(8_500_000);
        pacing.note_explicit_present(observation);

        pacing.last_pageflip_ns = None;
        observation.presented_ns = 20_000_000;
        observation.target_ns = observation.presented_ns;
        observation.callback_admission_ns = Some(17_000_000);
        observation.client_commit_ns = Some(16_500_000);
        pacing.note_explicit_present(observation);
        observation.callback_surface_id = Some(8);
        observation.presented_ns = 30_000_000;
        observation.target_ns = observation.presented_ns;
        observation.callback_admission_ns = Some(27_000_000);
        observation.client_commit_ns = Some(26_500_000);
        pacing.note_explicit_present(observation);
        observation.presented_ns = 35_000_000;
        observation.target_ns = observation.presented_ns;
        observation.callback_admission_ns = Some(32_000_000);
        observation.client_commit_ns = Some(31_500_000);
        pacing.note_explicit_present(observation);
        observation.presented_ns = 40_000_000;
        observation.target_ns = observation.presented_ns;
        observation.client_commit_ns = Some(31_500_000);
        observation.callback_admission_ns = Some(37_000_000);
        pacing.note_explicit_present(observation);

        pacing.last_fast_client_surface_id = Some(8);
        pacing.last_fast_client_commit_ns = Some(26_500_000);
        pacing.last_fast_client_presented_ns = None;
        observation.presented_ns = 50_000_000;
        observation.target_ns = observation.presented_ns;
        observation.client_commit_ns = Some(46_500_000);
        observation.callback_admission_ns = Some(47_000_000);
        pacing.note_explicit_present(observation);

        assert_eq!(pacing.fast_candidate_seen, 10);
        assert_eq!(pacing.fast_candidate_qualified, 5);
        assert_eq!(pacing.fast_candidate_rejected_missing_surface, 1);
        assert_eq!(pacing.fast_candidate_rejected_nonexclusive_surface, 1);
        assert_eq!(pacing.fast_candidate_rejected_missing_commit, 1);
        assert_eq!(pacing.fast_candidate_rejected_missing_admission, 1);
        assert_eq!(pacing.fast_candidate_rejected_callback_handoff, 1);
        assert_eq!(pacing.fast_continuity_seeded, 1);
        assert_eq!(pacing.fast_continuity_sampled, 1);
        assert_eq!(pacing.fast_continuity_broken_surface_change, 1);
        assert_eq!(pacing.fast_continuity_broken_nonmonotonic_commit, 1);
        assert_eq!(pacing.fast_continuity_broken_missing_previous_present, 1);
        assert_eq!(
            pacing.fast_candidate_seen,
            pacing.fast_candidate_qualified
                + pacing.fast_candidate_rejected_missing_surface
                + pacing.fast_candidate_rejected_nonexclusive_surface
                + pacing.fast_candidate_rejected_missing_commit
                + pacing.fast_candidate_rejected_missing_admission
                + pacing.fast_candidate_rejected_callback_handoff
        );
    }

    #[test]
    fn fast_client_population_keeps_outstanding_five_refresh_compositor_tail() {
        let mut pacing = NativeFramePacing::from_env();
        pacing.enabled = true;
        let refresh_ns = 6_060_606_u64;
        let first_presented_ns = 1_000_000_000_u64;

        let observation = |presented_ns: u64, commit_ns: u64, admission_ns: u64| {
            ExplicitPresentationObservation {
                planned_sequence: 1,
                actual_sequence: 1,
                target_ns: presented_ns,
                presented_ns,
                composite_started_ns: presented_ns.saturating_sub(2_000_000),
                rendered_ns: presented_ns.saturating_sub(1_000_000),
                submit_started_ns: presented_ns.saturating_sub(900_000),
                submit_returned_ns: presented_ns.saturating_sub(700_000),
                reactive_double: false,
                target_reason:
                    oblivion_one::native::presentation_deadline::PresentationTargetReason::Normal,
                target_selection: TargetSelectionEvidence {
                    earliest_feasible_sequence: 1,
                    binding: false,
                },
                previous_primary_sequence: None,
                client_commit_ns: Some(commit_ns),
                callback_reaction_ns: Some(500_000),
                callback_admission_ns: Some(admission_ns),
                callback_surface_id: Some(7),
                callback_surface_is_exclusive: true,
                refresh_interval_ns: refresh_ns,
                render_missed: false,
                submit_missed: false,
                kms_slipped: false,
            }
        };

        pacing.note_explicit_present(observation(
            first_presented_ns,
            first_presented_ns.saturating_sub(2_500_000),
            first_presented_ns.saturating_sub(3_000_000),
        ));
        pacing.note_explicit_present(observation(
            first_presented_ns + 5 * refresh_ns,
            first_presented_ns + 1_000_000,
            first_presented_ns + 500_000,
        ));

        assert_eq!(pacing.fast_client_continuous_samples, 1);
        assert_eq!(
            pacing.fast_client_primary_present_intervals.percentiles(),
            (30_303, 30_303, 30_303)
        );
        assert_eq!(pacing.fast_client_misses.missed_3x_or_more, 1);
        assert_eq!(pacing.fast_client_samples, 2);
    }

    #[test]
    fn content_cadence_attribution_distinguishes_client_target_and_stage_limits() {
        let classify = |reaction_ns,
                        selected_distance,
                        target_feasible,
                        render_missed,
                        submit_missed,
                        kms_slipped| {
            classify_content_frame(
                false,
                reaction_ns,
                2_000_000,
                selected_distance,
                1,
                TargetSelectionEvidence {
                    earliest_feasible_sequence: if target_feasible {
                        selected_distance.saturating_sub(2)
                    } else {
                        selected_distance
                    },
                    binding: target_feasible,
                },
                1,
                render_missed,
                submit_missed,
                kms_slipped,
            )
        };

        assert_eq!(
            classify(Some(3_000_000), 1, false, false, false, false),
            ContentCadenceAttribution::ClientLimited
        );
        assert_eq!(
            classify(Some(500_000), 3, true, false, false, false),
            ContentCadenceAttribution::TargetHit
        );
        assert_eq!(
            classify_content_frame(
                false,
                Some(500_000),
                2_000_000,
                3,
                3,
                TargetSelectionEvidence {
                    earliest_feasible_sequence: 1,
                    binding: true,
                },
                1,
                false,
                false,
                false,
            ),
            ContentCadenceAttribution::TargetLimited
        );
        assert_eq!(
            classify(Some(500_000), 1, false, true, false, false),
            ContentCadenceAttribution::RenderLimited
        );
        assert_eq!(
            classify(Some(500_000), 1, false, false, true, false),
            ContentCadenceAttribution::SubmitLimited
        );
        assert_eq!(
            classify(Some(500_000), 1, false, false, false, true),
            ContentCadenceAttribution::KmsLimited
        );
        assert_eq!(
            classify(Some(500_000), 1, false, false, false, false),
            ContentCadenceAttribution::TargetHit
        );
    }
}
use super::scanout::{NativeScanoutBufferSnapshot, OutputFrameIdentitySnapshot, OutputFrameKey};
use oblivion_one::native::adaptive_buffering::{
    AdaptiveBufferingMode, FenceTimestampQuality, ProvenDeadlineMiss, RenderPrediction,
};
use oblivion_one::native::presentation_deadline::{
    PresentationTarget, PresentationTargetReason, TargetSelectionEvidence,
};
use oblivion_one::native::scheduler::{
    NativeOutputPacingMode, PipelineWaitReason, SchedulerDecision,
};
use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc::{SyncSender, sync_channel},
};
use std::thread;
#[path = "pacing_o1.rs"]
mod pacing_o1;
#[cfg(test)]
#[path = "predictive_o1_tests.rs"]
mod predictive_o1_tests;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct NativeOutputFrameId(u64);

impl NativeOutputFrameId {
    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

/// Identity allocated by pacing before the output backend has created a
/// concrete frame. It is a namespace of its own and is never reconstructed
/// from an ordinary scheduler frame ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PredictiveO1AttemptId(u64);

impl PredictiveO1AttemptId {
    #[cfg(test)]
    const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct WorkerPacingReservationId(u64);

impl WorkerPacingReservationId {
    pub(crate) const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WorkerPacingTicket {
    reservation_id: WorkerPacingReservationId,
    frame_id: NativeOutputFrameId,
    ready_submit: bool,
    predictive_attempt_id: Option<PredictiveO1AttemptId>,
    physical_identity: Option<OutputFrameIdentitySnapshot>,
    physical_key: Option<OutputFrameKey>,
}

impl WorkerPacingTicket {
    pub(crate) const fn reservation_id(self) -> WorkerPacingReservationId {
        self.reservation_id
    }

    pub(crate) const fn frame_id(self) -> NativeOutputFrameId {
        self.frame_id
    }

    pub(crate) const fn ready_submit(self) -> bool {
        self.ready_submit
    }

    pub(crate) const fn predictive_attempt_id(self) -> Option<PredictiveO1AttemptId> {
        self.predictive_attempt_id
    }

    pub(crate) const fn physical_identity(self) -> Option<OutputFrameIdentitySnapshot> {
        self.physical_identity
    }

    pub(crate) const fn physical_key(self) -> Option<OutputFrameKey> {
        self.physical_key
    }
}

#[derive(Debug)]
struct WorkerPacingReservationIdSequence {
    next: u64,
}

impl WorkerPacingReservationIdSequence {
    const fn new(next: u64) -> Self {
        Self { next }
    }

    fn next(&mut self) -> WorkerPacingReservationId {
        let id = WorkerPacingReservationId(self.next.max(1));
        self.next = id.0.checked_add(1).unwrap_or(1);
        id
    }
}

#[derive(Debug)]
struct PredictiveO1AttemptIdSequence {
    next: u64,
}

impl PredictiveO1AttemptIdSequence {
    const fn new(next: u64) -> Self {
        Self { next }
    }

    fn next(&mut self) -> PredictiveO1AttemptId {
        let id = PredictiveO1AttemptId(self.next.max(1));
        self.next = id.0.checked_add(1).unwrap_or(1);
        id
    }
}
#[derive(Debug)]
pub(crate) struct NativeOutputFrameIdSequence {
    next: u64,
}

impl NativeOutputFrameIdSequence {
    pub(crate) const fn new(next: u64) -> Self {
        Self { next }
    }

    pub(crate) fn next(&mut self) -> NativeOutputFrameId {
        let id = NativeOutputFrameId(self.next.max(1));
        self.next = id.0.checked_add(1).unwrap_or(1);
        id
    }
}

#[derive(Debug)]
pub(crate) struct BoundedSamples<const N: usize> {
    values: VecDeque<u64>,
}

impl<const N: usize> Default for BoundedSamples<N> {
    fn default() -> Self {
        Self {
            values: VecDeque::with_capacity(N),
        }
    }
}

impl<const N: usize> BoundedSamples<N> {
    pub(crate) fn record(&mut self, value: u64) {
        if N == 0 {
            return;
        }
        if self.values.len() == N {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.values.len()
    }

    pub(crate) fn percentiles(&self) -> (u64, u64, u64) {
        let mut values: Vec<_> = self.values.iter().copied().collect();
        values.sort_unstable();
        let percentile = |percent: usize| {
            if values.is_empty() {
                return 0;
            }
            let rank = (percent * values.len()).div_ceil(100).max(1);
            values[rank - 1]
        };
        (percentile(50), percentile(95), percentile(99))
    }
}

#[derive(Debug)]
pub(crate) struct BoundedSignedSamples<const N: usize> {
    values: VecDeque<i64>,
}

impl<const N: usize> Default for BoundedSignedSamples<N> {
    fn default() -> Self {
        Self {
            values: VecDeque::with_capacity(N),
        }
    }
}

impl<const N: usize> BoundedSignedSamples<N> {
    pub(crate) fn record(&mut self, value: i64) {
        if N == 0 {
            return;
        }
        if self.values.len() == N {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    pub(crate) fn percentiles(&self) -> (i64, i64, i64) {
        let mut values: Vec<_> = self.values.iter().copied().collect();
        values.sort_unstable();
        let percentile = |percent: usize| {
            if values.is_empty() {
                return 0;
            }
            let rank = (percent * values.len()).div_ceil(100).max(1);
            values[rank - 1]
        };
        (percentile(50), percentile(95), percentile(99))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RefreshMissBuckets {
    pub(crate) on_time: u64,
    pub(crate) missed_1x: u64,
    pub(crate) missed_2x: u64,
    pub(crate) missed_3x_or_more: u64,
}

impl RefreshMissBuckets {
    pub(crate) fn record(&mut self, elapsed_us: u64, refresh_interval_us: u64) {
        if refresh_interval_us == 0 {
            return;
        }
        let twice = elapsed_us.saturating_mul(2);
        if twice <= refresh_interval_us.saturating_mul(3) {
            self.on_time += 1;
        } else if twice <= refresh_interval_us.saturating_mul(5) {
            self.missed_1x += 1;
        } else if twice <= refresh_interval_us.saturating_mul(7) {
            self.missed_2x += 1;
        } else {
            self.missed_3x_or_more += 1;
        }
    }
}

fn is_active_refresh_interval(elapsed_us: u64, refresh_interval_us: u64) -> bool {
    refresh_interval_us != 0 && elapsed_us <= refresh_interval_us.saturating_mul(4)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PacingField {
    key: &'static str,
    value: String,
}

impl PacingField {
    pub(crate) fn str(key: &'static str, value: impl Into<String>) -> Self {
        Self {
            key,
            value: value.into(),
        }
    }
    pub(crate) fn u64(key: &'static str, value: u64) -> Self {
        Self::str(key, value.to_string())
    }
    pub(crate) fn option_u64(key: &'static str, value: Option<u64>) -> Self {
        value.map_or_else(|| Self::none(key), |v| Self::u64(key, v))
    }
    pub(crate) fn i64(key: &'static str, value: i64) -> Self {
        Self::str(key, value.to_string())
    }
    pub(crate) fn usize(key: &'static str, value: usize) -> Self {
        Self::str(key, value.to_string())
    }
    pub(crate) fn bool(key: &'static str, value: bool) -> Self {
        Self::str(key, if value { "true" } else { "false" })
    }
    pub(crate) fn option_usize(key: &'static str, value: Option<usize>) -> Self {
        value.map_or_else(|| Self::none(key), |v| Self::usize(key, v))
    }
    pub(crate) fn option_bool(key: &'static str, value: Option<bool>) -> Self {
        value.map_or_else(|| Self::none(key), |v| Self::bool(key, v))
    }
    pub(crate) fn none(key: &'static str) -> Self {
        Self::str(key, "none")
    }
}

pub(crate) fn pacing_line(event: &str, fields: &[PacingField]) -> String {
    let mut line = format!("typhon pacing: event={event}");
    for field in fields {
        line.push(' ');
        line.push_str(field.key);
        line.push('=');
        line.push_str(&field.value);
    }
    line
}

pub(crate) fn snapshot_fields(snapshot: NativeScanoutBufferSnapshot) -> Vec<PacingField> {
    vec![
        PacingField::str("backend", snapshot.backend.metric_name()),
        PacingField::option_usize("capacity", snapshot.capacity),
        PacingField::option_usize("current", snapshot.current),
        PacingField::option_usize("pending", snapshot.pending),
        PacingField::option_usize("ready", snapshot.ready),
        PacingField::option_usize("free_count", snapshot.free_count),
        PacingField::option_bool(
            "gbm_surface_has_free_buffers",
            snapshot.gbm_surface_has_free_buffers,
        ),
    ]
}

pub(crate) fn frame_id_field(frame_id: Option<NativeOutputFrameId>) -> PacingField {
    frame_id.map_or_else(
        || PacingField::none("frame_id"),
        |id| PacingField::u64("frame_id", id.get()),
    )
}

const PACING_SAMPLE_CAPACITY: usize = 4096;
const TARGET_TIMESTAMP_TOLERANCE_NS: u64 = 100_000;
const TRACE_QUEUE_CAPACITY: usize = 2_048;
const CONTENT_ATTRIBUTION_COUNT: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentCadenceAttribution {
    CallbackHandoffLimited,
    ClientLimited,
    TargetLimited,
    RenderLimited,
    SubmitLimited,
    KmsLimited,
    TargetHit,
}

impl ContentCadenceAttribution {
    const fn index(self) -> usize {
        match self {
            Self::CallbackHandoffLimited => 0,
            Self::ClientLimited => 1,
            Self::TargetLimited => 2,
            Self::RenderLimited => 3,
            Self::SubmitLimited => 4,
            Self::KmsLimited => 5,
            Self::TargetHit => 6,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn classify_content_frame(
    callback_handoff_limited: bool,
    callback_reaction_ns: Option<u64>,
    fast_client_threshold_ns: u64,
    selected_target_distance: u64,
    actual_primary_distance: u64,
    target_selection: TargetSelectionEvidence,
    earliest_feasible_distance: u64,
    render_missed: bool,
    submit_missed: bool,
    kms_slipped: bool,
) -> ContentCadenceAttribution {
    if callback_handoff_limited {
        ContentCadenceAttribution::CallbackHandoffLimited
    } else if callback_reaction_ns.is_some_and(|reaction| reaction > fast_client_threshold_ns) {
        ContentCadenceAttribution::ClientLimited
    } else if target_selection.binding
        && earliest_feasible_distance < selected_target_distance
        && actual_primary_distance == selected_target_distance
    {
        ContentCadenceAttribution::TargetLimited
    } else if render_missed {
        ContentCadenceAttribution::RenderLimited
    } else if submit_missed {
        ContentCadenceAttribution::SubmitLimited
    } else if kms_slipped {
        ContentCadenceAttribution::KmsLimited
    } else {
        ContentCadenceAttribution::TargetHit
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FastClientCandidateOutcome {
    NotSeen,
    MissingSurface,
    NonExclusiveSurface,
    MissingCommit,
    MissingAdmission,
    CallbackHandoff,
    Qualified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PreparedFrameOrigin {
    #[default]
    Normal,
    ReactiveDouble,
    PredictiveO1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PredictiveO1Stage {
    Rendering,
    RenderReady,
    ReadyUnbound,
    Bound,
    WorkerQueued,
    Submitted,
    Presented,
}

const PREDICTIVE_O1_LIFECYCLE_CAPACITY: usize = 4;

#[derive(Debug, Clone, Copy)]
struct PredictiveO1LifecycleEntry {
    attempt_id: PredictiveO1AttemptId,
    physical_key: Option<OutputFrameKey>,
    /// Retained for diagnostics and state validation. This is not lifecycle
    /// identity because its target field may change during deferred O1.
    physical_state: Option<OutputFrameIdentitySnapshot>,
    stage: PredictiveO1Stage,
    observed_stages: u8,
}

impl PredictiveO1LifecycleEntry {
    const fn new(attempt_id: PredictiveO1AttemptId) -> Self {
        Self {
            attempt_id,
            physical_key: None,
            physical_state: None,
            stage: PredictiveO1Stage::Rendering,
            observed_stages: 1u8 << (PredictiveO1Stage::Rendering as u8),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PredictiveO1LifecycleLedger {
    entries: [Option<PredictiveO1LifecycleEntry>; PREDICTIVE_O1_LIFECYCLE_CAPACITY],
    peak_entries: u64,
}

impl Default for PredictiveO1LifecycleLedger {
    fn default() -> Self {
        Self {
            entries: [None; PREDICTIVE_O1_LIFECYCLE_CAPACITY],
            peak_entries: 0,
        }
    }
}

impl PredictiveO1LifecycleLedger {
    fn insert(&mut self, attempt_id: PredictiveO1AttemptId) -> Result<(), &'static str> {
        if self.contains_attempt(attempt_id) {
            return Err("Predictive O1 frame identity was admitted twice");
        }
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.is_none()) else {
            return Err("Predictive O1 lifecycle capacity was exceeded");
        };
        *entry = Some(PredictiveO1LifecycleEntry::new(attempt_id));
        let active_entries = self.active_entries();
        self.peak_entries = self.peak_entries.max(active_entries);
        Ok(())
    }

    fn contains_attempt(&self, attempt_id: PredictiveO1AttemptId) -> bool {
        self.entries
            .iter()
            .flatten()
            .any(|entry| entry.attempt_id == attempt_id)
    }

    fn contains_identity(&self, identity: PredictiveO1LifecycleIdentity) -> bool {
        self.entries.iter().flatten().any(|entry| match identity {
            PredictiveO1LifecycleIdentity::Attempt(attempt_id) => entry.attempt_id == attempt_id,
            PredictiveO1LifecycleIdentity::Physical(physical_key) => {
                entry.physical_key == Some(physical_key)
            }
        })
    }

    fn has_observed_stage(
        &self,
        identity: PredictiveO1LifecycleIdentity,
        stage: PredictiveO1Stage,
    ) -> bool {
        self.entries
            .iter()
            .flatten()
            .find(|entry| match identity {
                PredictiveO1LifecycleIdentity::Attempt(attempt_id) => {
                    entry.attempt_id == attempt_id
                }
                PredictiveO1LifecycleIdentity::Physical(physical_key) => {
                    entry.physical_key == Some(physical_key)
                }
            })
            .is_some_and(|entry| entry.observed_stages & (1u8 << (stage as u8)) != 0)
    }

    fn bind(
        &mut self,
        attempt_id: PredictiveO1AttemptId,
        physical_state: OutputFrameIdentitySnapshot,
    ) -> Result<(), &'static str> {
        if self
            .entries
            .iter()
            .flatten()
            .any(|entry| entry.physical_key == Some(OutputFrameKey::from(&physical_state)))
        {
            return Err("Predictive O1 physical identity was bound twice");
        }
        let Some(entry) = self
            .entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.attempt_id == attempt_id)
        else {
            return Err("Predictive O1 attempt identity was not live");
        };
        if entry.physical_key.is_some() {
            return Err("Predictive O1 attempt identity was bound twice");
        }
        entry.physical_key = Some(OutputFrameKey::from(&physical_state));
        entry.physical_state = Some(physical_state);
        Ok(())
    }

    #[cfg(test)]
    fn physical_identity_for_attempt(
        &self,
        attempt_id: PredictiveO1AttemptId,
    ) -> Option<OutputFrameIdentitySnapshot> {
        self.entries
            .iter()
            .flatten()
            .find(|entry| entry.attempt_id == attempt_id)
            .and_then(|entry| entry.physical_state)
    }

    fn record_stage(
        &mut self,
        identity: PredictiveO1LifecycleIdentity,
        stage: PredictiveO1Stage,
    ) -> PredictiveO1StageRecord {
        let Some(entry) = self
            .entries
            .iter_mut()
            .flatten()
            .find(|entry| match identity {
                PredictiveO1LifecycleIdentity::Attempt(attempt_id) => {
                    entry.attempt_id == attempt_id
                }
                PredictiveO1LifecycleIdentity::Physical(physical_key) => {
                    entry.physical_key == Some(physical_key)
                }
            })
        else {
            return PredictiveO1StageRecord::Missing;
        };
        let bit = 1u8 << (stage as u8);
        if entry.observed_stages & bit != 0 {
            return PredictiveO1StageRecord::Duplicate;
        }
        if !stage_transition_is_valid(entry.stage, stage) {
            return PredictiveO1StageRecord::Invalid;
        }
        entry.stage = stage;
        entry.observed_stages |= bit;
        PredictiveO1StageRecord::Accepted
    }

    fn remove(&mut self, identity: PredictiveO1LifecycleIdentity) -> bool {
        let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.is_some_and(|entry| match identity {
                PredictiveO1LifecycleIdentity::Attempt(attempt_id) => {
                    entry.attempt_id == attempt_id
                }
                PredictiveO1LifecycleIdentity::Physical(physical_key) => {
                    entry.physical_key == Some(physical_key)
                }
            })
        }) else {
            return false;
        };
        *entry = None;
        true
    }

    fn active_entries(&self) -> u64 {
        self.entries.iter().filter(|entry| entry.is_some()).count() as u64
    }

    fn drain(&mut self) -> u64 {
        let count = self.active_entries();
        self.entries = [None; PREDICTIVE_O1_LIFECYCLE_CAPACITY];
        count
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PredictiveO1LifecycleIdentity {
    Attempt(PredictiveO1AttemptId),
    Physical(OutputFrameKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PredictiveO1StageRecord {
    Accepted,
    Duplicate,
    Invalid,
    Missing,
}

const fn lifecycle_identity(
    attempt_id: Option<PredictiveO1AttemptId>,
    physical_key: Option<OutputFrameKey>,
) -> Option<PredictiveO1LifecycleIdentity> {
    match (attempt_id, physical_key) {
        (_, Some(key)) => Some(PredictiveO1LifecycleIdentity::Physical(key)),
        (Some(attempt), None) => Some(PredictiveO1LifecycleIdentity::Attempt(attempt)),
        (None, None) => None,
    }
}

const fn stage_transition_is_valid(from: PredictiveO1Stage, to: PredictiveO1Stage) -> bool {
    matches!(
        (from, to),
        (PredictiveO1Stage::Rendering, PredictiveO1Stage::RenderReady)
            | (
                PredictiveO1Stage::RenderReady,
                PredictiveO1Stage::ReadyUnbound
            )
            | (PredictiveO1Stage::RenderReady, PredictiveO1Stage::Bound)
            | (PredictiveO1Stage::RenderReady, PredictiveO1Stage::Submitted)
            | (
                PredictiveO1Stage::RenderReady,
                PredictiveO1Stage::WorkerQueued
            )
            | (PredictiveO1Stage::ReadyUnbound, PredictiveO1Stage::Bound)
            | (PredictiveO1Stage::Bound, PredictiveO1Stage::WorkerQueued)
            | (PredictiveO1Stage::Bound, PredictiveO1Stage::Submitted)
            | (
                PredictiveO1Stage::WorkerQueued,
                PredictiveO1Stage::Submitted
            )
            | (PredictiveO1Stage::Submitted, PredictiveO1Stage::Presented)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PredictiveReadyTerminal {
    Presented,
    AbandonedIdentity,
    AbandonedGeneration,
    OvertakenReady,
    OvertakenWorkerQueued,
    OtherSafeAbandonment,
    Failed,
    InvalidStage,
}

#[derive(Debug)]
struct NativeTraceSink {
    sender: SyncSender<String>,
    dropped: Arc<AtomicU64>,
}

impl NativeTraceSink {
    fn new() -> Self {
        let (sender, receiver) = sync_channel(TRACE_QUEUE_CAPACITY);
        let _ = thread::Builder::new()
            .name("typhon-frame-pacing-trace".to_string())
            .spawn(move || {
                while let Ok(line) = receiver.recv() {
                    println!("{line}");
                }
            });
        Self {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    fn send(&self, line: String) {
        if self.sender.try_send(line).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn dropped_entries(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[derive(Debug)]
pub(crate) struct NativeFramePacing {
    enabled: bool,
    summary_enabled: bool,
    trace: Option<NativeTraceSink>,
    ids: NativeOutputFrameIdSequence,
    predictive_o1_attempt_ids: PredictiveO1AttemptIdSequence,
    worker_reservation_ids: WorkerPacingReservationIdSequence,
    pub(crate) active: Option<NativeOutputFrameId>,
    active_predictive_attempt: Option<PredictiveO1AttemptId>,
    active_physical_identity: Option<OutputFrameIdentitySnapshot>,
    active_physical_key: Option<OutputFrameKey>,
    active_origin: PreparedFrameOrigin,
    pub(crate) active_queued_ns: Option<u64>,
    active_queued_frame_id: Option<NativeOutputFrameId>,
    pub(crate) pending: Option<NativeOutputFrameId>,
    pending_predictive_attempt: Option<PredictiveO1AttemptId>,
    pending_physical_identity: Option<OutputFrameIdentitySnapshot>,
    pending_physical_key: Option<OutputFrameKey>,
    pending_token: Option<u64>,
    pub(crate) ready: Option<NativeOutputFrameId>,
    ready_predictive_attempt: Option<PredictiveO1AttemptId>,
    ready_physical_identity: Option<OutputFrameIdentitySnapshot>,
    ready_physical_key: Option<OutputFrameKey>,
    ready_waiting_frame_id: Option<NativeOutputFrameId>,
    worker_reservation: Option<WorkerPacingTicket>,
    active_worker_reservation_id: Option<WorkerPacingReservationId>,
    ready_worker_reservation_id: Option<WorkerPacingReservationId>,
    pub(crate) render_ahead_attempts: u64,
    pub(crate) render_ahead_successes: u64,
    pub(crate) wait_for_buffer_count: u64,
    pub(crate) ready_submit_count: u64,
    pub(crate) reactive_double_frames: u64,
    pub(crate) reactive_double_immediate_submits: u64,
    pub(crate) reactive_double_actual_misses: u64,
    pub(crate) advisory_dispatch_slips: u64,
    pub(crate) predictive_triple_frames: u64,
    pub(crate) predictive_render_ahead_attempts: u64,
    pub(crate) predictive_render_ahead_ready: u64,
    pub(crate) predictive_ready_submits: u64,
    pub(crate) predictive_ready_created: u64,
    pub(crate) predictive_ready_submitted: u64,
    pub(crate) predictive_o1_created: u64,
    pub(crate) predictive_o1_render_ready: u64,
    pub(crate) predictive_o1_ready_unbound: u64,
    pub(crate) predictive_o1_bound: u64,
    pub(crate) predictive_o1_worker_queued: u64,
    pub(crate) predictive_o1_submitted: u64,
    pub(crate) predictive_o1_presented: u64,
    pub(crate) predictive_o1_invalid_stage_transitions: u64,
    pub(crate) predictive_o1_stale_stage_events: u64,
    pub(crate) predictive_o1_abandoned_identity: u64,
    pub(crate) predictive_o1_abandoned_generation: u64,
    pub(crate) predictive_o1_other_safe_abandonment: u64,
    pub(crate) predictive_o1_failed: u64,
    pub(crate) predictive_o1_invalid_stage_terminalized: u64,
    pub(crate) predictive_o1_current_at_shutdown: u64,
    pub(crate) predictive_unbound_created: u64,
    pub(crate) predictive_unbound_ready: u64,
    pub(crate) predictive_bound_after_predecessor_pageflip: u64,
    pub(crate) predictive_bound_after_render_completion: u64,
    pub(crate) predictive_binding_advanced_intervals: u64,
    pub(crate) predictive_unbound_abandoned_identity: u64,
    pub(crate) predictive_unbound_abandoned_generation: u64,
    pub(crate) predictive_ready_overtaken_ready: u64,
    pub(crate) predictive_ready_overtaken_worker_queued: u64,
    pub(crate) predictive_ready_other_safe_abandonment: u64,
    pub(crate) predictive_ready_failed: u64,
    pub(crate) predictive_ready_current_at_shutdown: u64,
    pub(crate) normal_ready_wait_count: u64,
    pub(crate) ready_pull_in_attempts: u64,
    pub(crate) ready_pull_in_successes: u64,
    pub(crate) ready_pull_in_rejected_too_late: u64,
    pub(crate) ready_pull_in_rejected_owned: u64,
    pub(crate) ready_pull_in_rejected_identity: u64,
    pub(crate) ready_pull_in_advanced_intervals: u64,
    pub(crate) scheduled_normal_target_count: u64,
    pub(crate) expired_deadline_wait_count: u64,
    pub(crate) repeated_immediate_timer_wake_count: u64,
    pub(crate) multiple_deadline_owner_violation_count: u64,
    pub(crate) adaptive_triple_entries_predicted: u64,
    pub(crate) adaptive_triple_entries_proven_render_miss: u64,
    pub(crate) adaptive_triple_entries_proven_submit_miss: u64,
    pub(crate) adaptive_triple_entries_proven_presentation_miss: u64,
    pub(crate) adaptive_triple_exits: u64,
    pub(crate) o1_credit2_useful_hits: u64,
    pub(crate) o1_credit2_unnecessary_hits: u64,
    pub(crate) o1_credit2_ineffective_misses: u64,
    pub(crate) o1_credit2_granted_not_consumed: u64,
    pub(crate) o1_credit2_drain_events: u64,
    pub(crate) o1_credit2_refill_suppressed_while_draining: u64,
    o1_credit2_pending_grant: bool,
    pub(crate) sync_file_info_exact: u64,
    pub(crate) sync_file_info_approximate: u64,
    pipeline_waits: [u64; 10],
    wake_lateness: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    slot_hold: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    ready_age: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    target_error: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    target_error_signed: BoundedSignedSamples<PACING_SAMPLE_CAPACITY>,
    target_interval_distance: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    ready_waiting_for_target: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    atomic_submit: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    pageflip_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    active_pageflip_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    commit_to_present: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    callback_admission_to_next_commit: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    client_commit_to_render_start: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    render_start_to_ready: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    ready_to_submit: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    submit_to_pageflip: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    selected_target_distance_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    actual_primary_distance_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    fast_client_primary_present_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    fast_client_actual_primary_distance_intervals: BoundedSamples<PACING_SAMPLE_CAPACITY>,
    reactive_target_early_by_intervals: u64,
    predictive_target_early_by_intervals: u64,
    reactive_target_late_by_intervals: u64,
    predictive_target_late_by_intervals: u64,
    fast_client_samples: u64,
    slow_client_samples: u64,
    pub(crate) fast_candidate_seen: u64,
    pub(crate) fast_candidate_qualified: u64,
    pub(crate) fast_candidate_rejected_missing_surface: u64,
    pub(crate) fast_candidate_rejected_nonexclusive_surface: u64,
    pub(crate) fast_candidate_rejected_missing_commit: u64,
    pub(crate) fast_candidate_rejected_missing_admission: u64,
    pub(crate) fast_candidate_rejected_callback_handoff: u64,
    pub(crate) fast_continuity_seeded: u64,
    pub(crate) fast_continuity_sampled: u64,
    pub(crate) fast_continuity_broken_surface_change: u64,
    pub(crate) fast_continuity_broken_nonmonotonic_commit: u64,
    pub(crate) fast_continuity_broken_missing_previous_present: u64,
    fast_client_continuous_samples: u64,
    fast_client_target_hit: u64,
    fast_client_target_limited: u64,
    fast_client_render_limited: u64,
    fast_client_submit_limited: u64,
    fast_client_kms_limited: u64,
    fast_client_misses: RefreshMissBuckets,
    physical_claim_overtake_ready: u64,
    physical_claim_overtake_worker_queued: u64,
    physical_claim_overtake_recoveries: u64,
    physical_claim_overtake_recovery_failures: u64,
    physical_claim_fatal_violations: u64,
    content_attribution: [u64; CONTENT_ATTRIBUTION_COUNT],
    last_prediction: Option<RenderPrediction>,
    last_primary_sequence: Option<u64>,
    misses: RefreshMissBuckets,
    last_pageflip_ns: Option<u64>,
    last_fast_client_surface_id: Option<u32>,
    last_fast_client_commit_ns: Option<u64>,
    last_fast_client_presented_ns: Option<u64>,
    idle_intervals_excluded: u64,
    early_presentation_count: u64,
    late_presentation_count: u64,
    ready_waiting_for_target_count: u64,
    ready_waiting_started_ns: Option<u64>,
    last_immediate_timer_deadline: Option<u64>,
    predictive_o1_lifecycle: PredictiveO1LifecycleLedger,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ExplicitPresentationObservation {
    pub(crate) planned_sequence: u64,
    pub(crate) actual_sequence: u64,
    pub(crate) target_ns: u64,
    pub(crate) presented_ns: u64,
    pub(crate) composite_started_ns: u64,
    pub(crate) rendered_ns: u64,
    pub(crate) submit_started_ns: u64,
    pub(crate) submit_returned_ns: u64,
    pub(crate) reactive_double: bool,
    pub(crate) target_reason: PresentationTargetReason,
    pub(crate) target_selection: TargetSelectionEvidence,
    pub(crate) previous_primary_sequence: Option<u64>,
    pub(crate) client_commit_ns: Option<u64>,
    pub(crate) callback_reaction_ns: Option<u64>,
    pub(crate) callback_admission_ns: Option<u64>,
    pub(crate) callback_surface_id: Option<u32>,
    pub(crate) callback_surface_is_exclusive: bool,
    pub(crate) refresh_interval_ns: u64,
    pub(crate) render_missed: bool,
    pub(crate) submit_missed: bool,
    pub(crate) kms_slipped: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativeBufferingMetrics {
    pub(crate) reactive_double_frames: u64,
    pub(crate) predictive_triple_frames: u64,
    pub(crate) render_ahead_attempts: u64,
    pub(crate) render_ahead_ready: u64,
    pub(crate) ready_submits: u64,
    pub(crate) triple_entries_predicted: u64,
    pub(crate) triple_entries_render_miss: u64,
    pub(crate) triple_entries_submit_miss: u64,
    pub(crate) triple_entries_presentation_miss: u64,
    pub(crate) triple_exits: u64,
    pub(crate) o1_credit2_useful_hits: u64,
    pub(crate) o1_credit2_unnecessary_hits: u64,
    pub(crate) o1_credit2_ineffective_misses: u64,
    pub(crate) o1_credit2_granted_not_consumed: u64,
    pub(crate) o1_credit2_drain_events: u64,
    pub(crate) o1_credit2_refill_suppressed_while_draining: u64,
    pub(crate) ready_pull_in_attempts: u64,
    pub(crate) ready_pull_in_successes: u64,
    pub(crate) ready_pull_in_rejected_too_late: u64,
    pub(crate) ready_pull_in_rejected_owned: u64,
    pub(crate) ready_pull_in_rejected_identity: u64,
    pub(crate) ready_pull_in_advanced_intervals: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativePacingTimingMetrics {
    pub(crate) wake_lateness: (u64, u64, u64),
    pub(crate) target_error: (u64, u64, u64),
    pub(crate) pageflip_interval: (u64, u64, u64),
    pub(crate) active_pageflip_interval: (u64, u64, u64),
    pub(crate) commit_to_present: (u64, u64, u64),
    pub(crate) missed_refresh_1x: u64,
    pub(crate) missed_refresh_2x: u64,
    pub(crate) missed_refresh_3x_or_more: u64,
}

impl NativeFramePacing {
    pub(crate) fn from_env() -> Self {
        let summary_enabled = std::env::var("TYPHON_FRAME_PACING_DEBUG")
            .ok()
            .is_some_and(|value| super::perf::native_perf_log_value_enabled(&value));
        let trace_enabled = std::env::var("TYPHON_FRAME_PACING_TRACE")
            .ok()
            .is_some_and(|value| super::perf::native_perf_log_value_enabled(&value));
        Self {
            enabled: true,
            summary_enabled: summary_enabled || trace_enabled,
            trace: trace_enabled.then(NativeTraceSink::new),
            ids: NativeOutputFrameIdSequence::new(1),
            predictive_o1_attempt_ids: PredictiveO1AttemptIdSequence::new(1),
            worker_reservation_ids: WorkerPacingReservationIdSequence::new(1),
            active: None,
            active_predictive_attempt: None,
            active_physical_identity: None,
            active_physical_key: None,
            active_origin: PreparedFrameOrigin::Normal,
            active_queued_ns: None,
            active_queued_frame_id: None,
            pending: None,
            pending_predictive_attempt: None,
            pending_physical_identity: None,
            pending_physical_key: None,
            pending_token: None,
            ready: None,
            ready_predictive_attempt: None,
            ready_physical_identity: None,
            ready_physical_key: None,
            ready_waiting_frame_id: None,
            worker_reservation: None,
            active_worker_reservation_id: None,
            ready_worker_reservation_id: None,
            render_ahead_attempts: 0,
            render_ahead_successes: 0,
            wait_for_buffer_count: 0,
            ready_submit_count: 0,
            reactive_double_frames: 0,
            reactive_double_immediate_submits: 0,
            reactive_double_actual_misses: 0,
            advisory_dispatch_slips: 0,
            predictive_triple_frames: 0,
            predictive_render_ahead_attempts: 0,
            predictive_render_ahead_ready: 0,
            predictive_ready_submits: 0,
            predictive_ready_created: 0,
            predictive_ready_submitted: 0,
            predictive_o1_created: 0,
            predictive_o1_render_ready: 0,
            predictive_o1_ready_unbound: 0,
            predictive_o1_bound: 0,
            predictive_o1_worker_queued: 0,
            predictive_o1_submitted: 0,
            predictive_o1_presented: 0,
            predictive_o1_invalid_stage_transitions: 0,
            predictive_o1_stale_stage_events: 0,
            predictive_o1_abandoned_identity: 0,
            predictive_o1_abandoned_generation: 0,
            predictive_o1_other_safe_abandonment: 0,
            predictive_o1_failed: 0,
            predictive_o1_invalid_stage_terminalized: 0,
            predictive_o1_current_at_shutdown: 0,
            predictive_unbound_created: 0,
            predictive_unbound_ready: 0,
            predictive_bound_after_predecessor_pageflip: 0,
            predictive_bound_after_render_completion: 0,
            predictive_binding_advanced_intervals: 0,
            predictive_unbound_abandoned_identity: 0,
            predictive_unbound_abandoned_generation: 0,
            predictive_ready_overtaken_ready: 0,
            predictive_ready_overtaken_worker_queued: 0,
            predictive_ready_other_safe_abandonment: 0,
            predictive_ready_failed: 0,
            predictive_ready_current_at_shutdown: 0,
            normal_ready_wait_count: 0,
            ready_pull_in_attempts: 0,
            ready_pull_in_successes: 0,
            ready_pull_in_rejected_too_late: 0,
            ready_pull_in_rejected_owned: 0,
            ready_pull_in_rejected_identity: 0,
            ready_pull_in_advanced_intervals: 0,
            scheduled_normal_target_count: 0,
            expired_deadline_wait_count: 0,
            repeated_immediate_timer_wake_count: 0,
            multiple_deadline_owner_violation_count: 0,
            adaptive_triple_entries_predicted: 0,
            adaptive_triple_entries_proven_render_miss: 0,
            adaptive_triple_entries_proven_submit_miss: 0,
            adaptive_triple_entries_proven_presentation_miss: 0,
            adaptive_triple_exits: 0,
            o1_credit2_useful_hits: 0,
            o1_credit2_unnecessary_hits: 0,
            o1_credit2_ineffective_misses: 0,
            o1_credit2_granted_not_consumed: 0,
            o1_credit2_drain_events: 0,
            o1_credit2_refill_suppressed_while_draining: 0,
            o1_credit2_pending_grant: false,
            sync_file_info_exact: 0,
            sync_file_info_approximate: 0,
            pipeline_waits: [0; 10],
            wake_lateness: BoundedSamples::default(),
            slot_hold: BoundedSamples::default(),
            ready_age: BoundedSamples::default(),
            target_error: BoundedSamples::default(),
            target_error_signed: BoundedSignedSamples::default(),
            target_interval_distance: BoundedSamples::default(),
            ready_waiting_for_target: BoundedSamples::default(),
            atomic_submit: BoundedSamples::default(),
            pageflip_intervals: BoundedSamples::default(),
            active_pageflip_intervals: BoundedSamples::default(),
            commit_to_present: BoundedSamples::default(),
            callback_admission_to_next_commit: BoundedSamples::default(),
            client_commit_to_render_start: BoundedSamples::default(),
            render_start_to_ready: BoundedSamples::default(),
            ready_to_submit: BoundedSamples::default(),
            submit_to_pageflip: BoundedSamples::default(),
            selected_target_distance_intervals: BoundedSamples::default(),
            actual_primary_distance_intervals: BoundedSamples::default(),
            fast_client_primary_present_intervals: BoundedSamples::default(),
            fast_client_actual_primary_distance_intervals: BoundedSamples::default(),
            reactive_target_early_by_intervals: 0,
            predictive_target_early_by_intervals: 0,
            reactive_target_late_by_intervals: 0,
            predictive_target_late_by_intervals: 0,
            fast_client_samples: 0,
            slow_client_samples: 0,
            fast_candidate_seen: 0,
            fast_candidate_qualified: 0,
            fast_candidate_rejected_missing_surface: 0,
            fast_candidate_rejected_nonexclusive_surface: 0,
            fast_candidate_rejected_missing_commit: 0,
            fast_candidate_rejected_missing_admission: 0,
            fast_candidate_rejected_callback_handoff: 0,
            fast_continuity_seeded: 0,
            fast_continuity_sampled: 0,
            fast_continuity_broken_surface_change: 0,
            fast_continuity_broken_nonmonotonic_commit: 0,
            fast_continuity_broken_missing_previous_present: 0,
            fast_client_continuous_samples: 0,
            fast_client_target_hit: 0,
            fast_client_target_limited: 0,
            fast_client_render_limited: 0,
            fast_client_submit_limited: 0,
            fast_client_kms_limited: 0,
            fast_client_misses: RefreshMissBuckets::default(),
            physical_claim_overtake_ready: 0,
            physical_claim_overtake_worker_queued: 0,
            physical_claim_overtake_recoveries: 0,
            physical_claim_overtake_recovery_failures: 0,
            physical_claim_fatal_violations: 0,
            content_attribution: [0; CONTENT_ATTRIBUTION_COUNT],
            last_prediction: None,
            last_primary_sequence: None,
            misses: RefreshMissBuckets::default(),
            last_pageflip_ns: None,
            last_fast_client_surface_id: None,
            last_fast_client_commit_ns: None,
            last_fast_client_presented_ns: None,
            idle_intervals_excluded: 0,
            early_presentation_count: 0,
            late_presentation_count: 0,
            ready_waiting_for_target_count: 0,
            ready_waiting_started_ns: None,
            last_immediate_timer_deadline: None,
            predictive_o1_lifecycle: PredictiveO1LifecycleLedger::default(),
        }
    }

    pub(crate) const fn summary_enabled(&self) -> bool {
        self.summary_enabled
    }

    pub(crate) fn note_physical_claim_overtake_ready(&mut self) {
        if self.enabled {
            self.physical_claim_overtake_ready =
                self.physical_claim_overtake_ready.saturating_add(1);
        }
    }

    pub(crate) fn note_physical_claim_overtake_worker_queued(&mut self) {
        if self.enabled {
            self.physical_claim_overtake_worker_queued =
                self.physical_claim_overtake_worker_queued.saturating_add(1);
        }
    }

    pub(crate) fn note_physical_claim_overtake_recovery(&mut self, recovered: bool) {
        if !self.enabled {
            return;
        }
        let counter = if recovered {
            &mut self.physical_claim_overtake_recoveries
        } else {
            &mut self.physical_claim_overtake_recovery_failures
        };
        *counter = counter.saturating_add(1);
    }

    pub(crate) fn note_physical_claim_fatal_violation(&mut self) {
        if self.enabled {
            self.physical_claim_fatal_violations =
                self.physical_claim_fatal_violations.saturating_add(1);
        }
    }
    pub(crate) fn queue_visual(&mut self, now_ns: u64, render_generation: u64) {
        if !self.enabled {
            return;
        }
        if self.active.is_some() {
            return;
        }
        let id = self.ids.next();
        self.active = Some(id);
        self.active_worker_reservation_id = None;
        self.active_predictive_attempt = None;
        self.active_physical_identity = None;
        self.active_physical_key = None;
        self.active_origin = PreparedFrameOrigin::Normal;
        self.active_queued_ns = Some(now_ns);
        self.active_queued_frame_id = Some(id);
        self.log(
            "visual_queued",
            vec![
                PacingField::u64("frame_id", id.get()),
                PacingField::u64("render_generation", render_generation),
            ],
        );
    }
    pub(crate) fn log(&self, event: &str, fields: Vec<PacingField>) {
        if let Some(trace) = &self.trace {
            trace.send(pacing_line(event, &fields));
        }
    }

    pub(crate) fn note_prediction(&mut self, prediction: RenderPrediction) {
        if self.enabled {
            self.last_prediction = Some(prediction);
        }
    }

    /// Transfer a predictive attempt from the scheduler namespace to the
    /// exact output frame created by the physical scanout backend.
    pub(crate) fn bind_predictive_o1(
        &mut self,
        physical_identity: OutputFrameIdentitySnapshot,
    ) -> Result<(), &'static str> {
        if !self.enabled || self.active_origin != PreparedFrameOrigin::PredictiveO1 {
            return Ok(());
        }
        let attempt_id = self
            .active_predictive_attempt
            .ok_or("Predictive O1 physical frame has no active attempt")?;
        let physical_key = OutputFrameKey::from(&physical_identity);
        self.predictive_o1_lifecycle
            .bind(attempt_id, physical_identity)?;
        self.active_physical_identity = Some(physical_identity);
        self.active_physical_key = Some(physical_key);
        self.log(
            "predictive_attempt_bound",
            vec![
                PacingField::u64("predictive_attempt_id", attempt_id.get()),
                PacingField::u64("output_frame_id", physical_identity.frame_id),
                PacingField::u64("transaction_id", physical_identity.transaction_id.get()),
                PacingField::u64(
                    "protocol_batch_id",
                    physical_identity.protocol_batch_id.get(),
                ),
                PacingField::u64("pool_generation", physical_identity.pool_generation),
                PacingField::u64("render_generation", physical_identity.render_generation),
            ],
        );
        Ok(())
    }

    pub(crate) fn active_predictive_attempt_id(&self) -> Option<PredictiveO1AttemptId> {
        (self.active_origin == PreparedFrameOrigin::PredictiveO1)
            .then_some(self.active_predictive_attempt)
            .flatten()
    }

    pub(crate) const fn ready_predictive_attempt_id(&self) -> Option<PredictiveO1AttemptId> {
        self.ready_predictive_attempt
    }

    pub(crate) const fn ready_worker_submission_reserved(&self) -> bool {
        self.ready_worker_reservation_id.is_some()
    }

    #[cfg(test)]
    pub(crate) const fn ready_physical_key(&self) -> Option<OutputFrameKey> {
        self.ready_physical_key
    }

    #[cfg(test)]
    pub(crate) fn predictive_o1_active_entries_for_test(&self) -> u64 {
        self.predictive_o1_lifecycle.active_entries()
    }

    pub(crate) fn note_render_decision(
        &mut self,
        pacing_mode: NativeOutputPacingMode,
        render_ahead: bool,
    ) {
        if !self.enabled {
            return;
        }
        match (pacing_mode, render_ahead) {
            (NativeOutputPacingMode::ReactiveDouble, false) => {
                self.reactive_double_frames += 1;
            }
            (NativeOutputPacingMode::PredictiveTriple, true) => {
                self.predictive_triple_frames += 1;
                self.render_ahead_attempts += 1;
                self.predictive_render_ahead_attempts += 1;
            }
            (NativeOutputPacingMode::ReactiveDouble, true) => {
                self.multiple_deadline_owner_violation_count += 1;
            }
            (NativeOutputPacingMode::PredictiveTriple, false) => {
                self.predictive_triple_frames += 1;
            }
        }
        // A scheduler decision is not yet a render attempt. Keep the active
        // frame unowned until the caller has passed all recoverable
        // no-visual-change and submit-window exits.
        self.active_origin = PreparedFrameOrigin::Normal;
    }

    pub(crate) fn begin_render_attempt(
        &mut self,
        pacing_mode: NativeOutputPacingMode,
        render_ahead: bool,
    ) -> Result<(), &'static str> {
        if !self.enabled {
            return Ok(());
        }
        // A render attempt may legitimately reuse the active logical pacing
        // ID while an older worker submission is still in flight. The old
        // ticket remains valid, but its lane affiliation must not follow the
        // new attempt.
        self.active_worker_reservation_id = None;
        let origin = match (pacing_mode, render_ahead) {
            (NativeOutputPacingMode::ReactiveDouble, false) => PreparedFrameOrigin::ReactiveDouble,
            (NativeOutputPacingMode::PredictiveTriple, true) => PreparedFrameOrigin::PredictiveO1,
            (NativeOutputPacingMode::ReactiveDouble, true) => PreparedFrameOrigin::ReactiveDouble,
            (NativeOutputPacingMode::PredictiveTriple, false) => PreparedFrameOrigin::Normal,
        };
        if origin == PreparedFrameOrigin::PredictiveO1 {
            self.active
                .ok_or("Predictive O1 render started without an active frame")?;
            let attempt_id = self.predictive_o1_attempt_ids.next();
            self.predictive_o1_lifecycle.insert(attempt_id)?;
            self.active_predictive_attempt = Some(attempt_id);
            self.predictive_o1_created = self.predictive_o1_created.saturating_add(1);
            self.note_predictive_unbound_created();
        }
        self.active_origin = origin;
        Ok(())
    }

    /// Test-facing compatibility wrapper for the old combined operation.
    #[cfg(test)]
    pub(crate) fn note_render_started(
        &mut self,
        pacing_mode: NativeOutputPacingMode,
        render_ahead: bool,
    ) -> Result<(), &'static str> {
        self.note_render_decision(pacing_mode, render_ahead);
        self.begin_render_attempt(pacing_mode, render_ahead)
    }

    /// Drop a queued visual decision which never reached a backend render.
    ///
    /// Production callers use this for no-primary-work and recoverable
    /// submit-window exits. A pre-physical lifecycle should not exist here,
    /// but terminalizing one defensively keeps the bounded ledger exact if a
    /// future caller accidentally moves admission earlier again.
    fn clear_unsubmitted_render(&mut self, terminal: PredictiveReadyTerminal) {
        if !self.enabled {
            return;
        }
        if let Some(identity) =
            lifecycle_identity(self.active_predictive_attempt, self.active_physical_key)
        {
            self.terminalize_predictive_frame(identity, terminal);
        }
        self.active = None;
        self.active_worker_reservation_id = None;
        self.active_predictive_attempt = None;
        self.active_physical_identity = None;
        self.active_physical_key = None;
        self.active_origin = PreparedFrameOrigin::Normal;
        self.active_queued_frame_id = None;
        self.active_queued_ns = None;
    }

    pub(crate) fn cancel_unsubmitted_render(&mut self) {
        self.clear_unsubmitted_render(PredictiveReadyTerminal::Failed);
    }

    pub(crate) fn complete_unsubmitted_render_without_visual_change(&mut self) {
        self.clear_unsubmitted_render(PredictiveReadyTerminal::OtherSafeAbandonment);
    }
    pub(crate) fn note_submit(
        &mut self,
        token: u64,
        now_ns: u64,
        ready_submit: bool,
        pacing_mode: NativeOutputPacingMode,
    ) {
        if !self.enabled {
            return;
        }
        let current_id = if ready_submit {
            self.ready
        } else {
            self.active
        };
        if ready_submit {
            self.note_ready_submit_timing(current_id, now_ns, None);
            self.ready_worker_reservation_id = None;
        } else {
            self.clear_active_worker_timing_for_frame(current_id);
            self.active_worker_reservation_id = None;
        }
        let (id, physical_identity, predictive_attempt_id, physical_key) = if ready_submit {
            (
                self.ready.take(),
                self.ready_physical_identity.take(),
                self.ready_predictive_attempt.take(),
                self.ready_physical_key.take(),
            )
        } else {
            (
                self.active.take(),
                self.active_physical_identity.take(),
                self.active_predictive_attempt.take(),
                self.active_physical_key.take(),
            )
        };
        if !ready_submit {
            self.active_origin = PreparedFrameOrigin::Normal;
        }
        self.note_submit_frame(
            id,
            physical_identity,
            predictive_attempt_id,
            physical_key,
            token,
            now_ns,
            ready_submit,
            pacing_mode,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn note_submit_frame(
        &mut self,
        id: Option<NativeOutputFrameId>,
        physical_identity: Option<OutputFrameIdentitySnapshot>,
        predictive_attempt_id: Option<PredictiveO1AttemptId>,
        physical_key: Option<OutputFrameKey>,
        token: u64,
        now_ns: u64,
        ready_submit: bool,
        pacing_mode: NativeOutputPacingMode,
    ) {
        if ready_submit {
            self.ready_submit_count += 1;
            if predictive_attempt_id.is_some() {
                self.predictive_ready_submits += 1;
                if let Some(identity) = lifecycle_identity(predictive_attempt_id, physical_key)
                    && self.note_predictive_stage(identity, PredictiveO1Stage::Submitted)
                {
                    self.predictive_ready_submitted =
                        self.predictive_ready_submitted.saturating_add(1);
                }
            }
        }
        if !ready_submit
            && let Some(identity) = lifecycle_identity(predictive_attempt_id, physical_key)
        {
            self.note_predictive_stage(identity, PredictiveO1Stage::Submitted);
        }
        if pacing_mode == NativeOutputPacingMode::ReactiveDouble && !ready_submit {
            self.reactive_double_immediate_submits += 1;
        }
        let predictive_attempt_id_value = predictive_attempt_id.map(|attempt| attempt.get());
        self.pending = id;
        self.pending_predictive_attempt = predictive_attempt_id;
        self.pending_physical_identity = physical_identity;
        self.pending_physical_key = physical_key;
        self.pending_token = id.map(|_| token);
        self.log(
            "submit",
            vec![
                frame_id_field(id),
                PacingField::option_u64("predictive_attempt_id", predictive_attempt_id_value),
                PacingField::option_u64(
                    "output_frame_id",
                    physical_identity.map(|identity| identity.frame_id),
                ),
                PacingField::u64("pageflip_token", token),
                PacingField::u64("submit_ns", now_ns),
                PacingField::bool("ready_submit", ready_submit),
            ],
        );
    }

    fn note_predictive_stage(
        &mut self,
        identity: PredictiveO1LifecycleIdentity,
        stage: PredictiveO1Stage,
    ) -> bool {
        match self.predictive_o1_lifecycle.record_stage(identity, stage) {
            PredictiveO1StageRecord::Accepted => {}
            PredictiveO1StageRecord::Duplicate => {
                self.predictive_o1_stale_stage_events =
                    self.predictive_o1_stale_stage_events.saturating_add(1);
                return false;
            }
            PredictiveO1StageRecord::Missing => {
                self.predictive_o1_stale_stage_events =
                    self.predictive_o1_stale_stage_events.saturating_add(1);
                return false;
            }
            PredictiveO1StageRecord::Invalid => {
                self.predictive_o1_invalid_stage_transitions = self
                    .predictive_o1_invalid_stage_transitions
                    .saturating_add(1);
                self.terminalize_predictive_frame(identity, PredictiveReadyTerminal::InvalidStage);
                return false;
            }
        }
        match stage {
            PredictiveO1Stage::Rendering => {}
            PredictiveO1Stage::RenderReady => {
                self.predictive_o1_render_ready = self.predictive_o1_render_ready.saturating_add(1);
            }
            PredictiveO1Stage::ReadyUnbound => {
                self.predictive_o1_ready_unbound =
                    self.predictive_o1_ready_unbound.saturating_add(1);
            }
            PredictiveO1Stage::Bound => {
                self.predictive_o1_bound = self.predictive_o1_bound.saturating_add(1);
            }
            PredictiveO1Stage::WorkerQueued => {
                self.predictive_o1_worker_queued =
                    self.predictive_o1_worker_queued.saturating_add(1);
            }
            PredictiveO1Stage::Submitted => {
                self.predictive_o1_submitted = self.predictive_o1_submitted.saturating_add(1);
            }
            PredictiveO1Stage::Presented => {
                self.predictive_o1_presented = self.predictive_o1_presented.saturating_add(1);
            }
        }
        true
    }

    pub(crate) fn note_render_ready(&mut self) {
        if !self.enabled {
            return;
        }
        let Some(identity) =
            lifecycle_identity(self.active_predictive_attempt, self.active_physical_key)
        else {
            return;
        };
        if self.note_predictive_stage(identity, PredictiveO1Stage::RenderReady) {
            self.predictive_render_ahead_ready =
                self.predictive_render_ahead_ready.saturating_add(1);
            self.render_ahead_successes = self.render_ahead_successes.saturating_add(1);
        }
    }

    fn terminalize_predictive_frame(
        &mut self,
        identity: PredictiveO1LifecycleIdentity,
        terminal: PredictiveReadyTerminal,
    ) -> bool {
        if !self.predictive_o1_lifecycle.remove(identity) {
            return false;
        }
        match terminal {
            PredictiveReadyTerminal::Presented => {
                // Presented is counted at the physical pageflip stage.
            }
            PredictiveReadyTerminal::AbandonedIdentity => {
                self.predictive_unbound_abandoned_identity =
                    self.predictive_unbound_abandoned_identity.saturating_add(1);
                self.predictive_o1_abandoned_identity =
                    self.predictive_o1_abandoned_identity.saturating_add(1);
            }
            PredictiveReadyTerminal::AbandonedGeneration => {
                self.predictive_unbound_abandoned_generation = self
                    .predictive_unbound_abandoned_generation
                    .saturating_add(1);
                self.predictive_o1_abandoned_generation =
                    self.predictive_o1_abandoned_generation.saturating_add(1);
            }
            PredictiveReadyTerminal::OvertakenReady => {
                self.predictive_ready_overtaken_ready =
                    self.predictive_ready_overtaken_ready.saturating_add(1);
                self.predictive_o1_other_safe_abandonment =
                    self.predictive_o1_other_safe_abandonment.saturating_add(1);
            }
            PredictiveReadyTerminal::OvertakenWorkerQueued => {
                self.predictive_ready_overtaken_worker_queued = self
                    .predictive_ready_overtaken_worker_queued
                    .saturating_add(1);
                self.predictive_o1_other_safe_abandonment =
                    self.predictive_o1_other_safe_abandonment.saturating_add(1);
            }
            PredictiveReadyTerminal::OtherSafeAbandonment => {
                self.predictive_ready_other_safe_abandonment = self
                    .predictive_ready_other_safe_abandonment
                    .saturating_add(1);
                self.predictive_o1_other_safe_abandonment =
                    self.predictive_o1_other_safe_abandonment.saturating_add(1);
            }
            PredictiveReadyTerminal::Failed => {
                self.predictive_ready_failed = self.predictive_ready_failed.saturating_add(1);
                self.predictive_o1_failed = self.predictive_o1_failed.saturating_add(1);
            }
            PredictiveReadyTerminal::InvalidStage => {
                self.predictive_o1_invalid_stage_terminalized = self
                    .predictive_o1_invalid_stage_terminalized
                    .saturating_add(1);
            }
        }
        self.log(
            "predictive_ready_terminal",
            vec![
                PacingField::str(
                    "identity",
                    match identity {
                        PredictiveO1LifecycleIdentity::Attempt(_) => "attempt",
                        PredictiveO1LifecycleIdentity::Physical(_) => "physical",
                    },
                ),
                PacingField::u64(
                    "predictive_attempt_id",
                    match identity {
                        PredictiveO1LifecycleIdentity::Attempt(attempt_id) => attempt_id.get(),
                        PredictiveO1LifecycleIdentity::Physical(_) => 0,
                    },
                ),
                PacingField::u64(
                    "output_frame_id",
                    match identity {
                        PredictiveO1LifecycleIdentity::Attempt(_) => 0,
                        PredictiveO1LifecycleIdentity::Physical(key) => key.frame_id,
                    },
                ),
                PacingField::str(
                    "terminal",
                    match terminal {
                        PredictiveReadyTerminal::Presented => "presented",
                        PredictiveReadyTerminal::AbandonedIdentity => "abandoned_identity",
                        PredictiveReadyTerminal::AbandonedGeneration => "abandoned_generation",
                        PredictiveReadyTerminal::OvertakenReady => "overtaken_ready",
                        PredictiveReadyTerminal::OvertakenWorkerQueued => "overtaken_worker_queued",
                        PredictiveReadyTerminal::OtherSafeAbandonment => "other_safe_abandonment",
                        PredictiveReadyTerminal::Failed => "failed",
                        PredictiveReadyTerminal::InvalidStage => "invalid_stage",
                    },
                ),
            ],
        );
        true
    }

    #[cfg(test)]
    fn note_predictive_terminal_exact(
        &mut self,
        frame_id: Option<u64>,
        terminal: PredictiveReadyTerminal,
    ) -> bool {
        let Some(frame_id) = frame_id else {
            return false;
        };
        self.terminalize_predictive_frame(
            PredictiveO1LifecycleIdentity::Attempt(PredictiveO1AttemptId::new(frame_id)),
            terminal,
        )
    }

    fn note_predictive_terminal_physical(
        &mut self,
        identity: Option<OutputFrameIdentitySnapshot>,
        terminal: PredictiveReadyTerminal,
    ) -> bool {
        identity.is_some_and(|identity| {
            self.terminalize_predictive_frame(
                PredictiveO1LifecycleIdentity::Physical(OutputFrameKey::from(&identity)),
                terminal,
            )
        })
    }

    #[cfg(test)]
    pub(crate) fn note_predictive_ready_overtaken_worker_queued(&mut self, frame_id: Option<u64>) {
        if self.enabled {
            self.note_predictive_terminal_exact(
                frame_id,
                PredictiveReadyTerminal::OvertakenWorkerQueued,
            );
        }
    }

    pub(crate) fn note_predictive_ready_overtaken_worker_queued_physical(
        &mut self,
        identity: Option<OutputFrameIdentitySnapshot>,
    ) {
        if self.enabled {
            self.note_predictive_terminal_physical(
                identity,
                PredictiveReadyTerminal::OvertakenWorkerQueued,
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn note_predictive_ready_other_safe_abandonment(&mut self, frame_id: Option<u64>) {
        if self.enabled {
            self.note_predictive_terminal_exact(
                frame_id,
                PredictiveReadyTerminal::OtherSafeAbandonment,
            );
        }
    }

    pub(crate) fn note_predictive_ready_other_safe_abandonment_physical(
        &mut self,
        identity: Option<OutputFrameIdentitySnapshot>,
    ) {
        if self.enabled {
            self.note_predictive_terminal_physical(
                identity,
                PredictiveReadyTerminal::OtherSafeAbandonment,
            );
        }
    }

    pub(crate) fn note_predictive_o1_failed(&mut self) {
        if !self.enabled {
            return;
        }
        if let Some(identity) =
            lifecycle_identity(self.active_predictive_attempt, self.active_physical_key)
        {
            self.terminalize_predictive_frame(identity, PredictiveReadyTerminal::Failed);
        }
    }

    pub(crate) fn note_predictive_o1_other_safe_abandonment(&mut self) {
        if !self.enabled {
            return;
        }
        if let Some(identity) =
            lifecycle_identity(self.active_predictive_attempt, self.active_physical_key)
        {
            self.terminalize_predictive_frame(
                identity,
                PredictiveReadyTerminal::OtherSafeAbandonment,
            );
        }
    }

    pub(crate) fn note_predictive_o1_current_at_shutdown(&mut self) {
        if !self.enabled {
            return;
        }
        let remaining = self.predictive_o1_lifecycle.drain();
        self.predictive_o1_current_at_shutdown = self
            .predictive_o1_current_at_shutdown
            .saturating_add(remaining);
        self.predictive_ready_current_at_shutdown = self
            .predictive_ready_current_at_shutdown
            .saturating_add(remaining);
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn note_worker_submit(
        &mut self,
        token: u64,
        now_ns: u64,
        ready_submit: bool,
        pacing_mode: NativeOutputPacingMode,
    ) {
        self.note_submit(token, now_ns, ready_submit, pacing_mode);
    }

    pub(crate) fn reserve_worker_submission(
        &mut self,
        ready_submit: bool,
    ) -> Result<Option<WorkerPacingTicket>, &'static str> {
        if !self.enabled {
            return Ok(None);
        }
        if self.worker_reservation.is_some() {
            return Err("worker pacing reservation is already queued");
        }
        let Some(frame_id) = self.worker_submission_frame(ready_submit) else {
            return Ok(None);
        };
        let (physical_identity, physical_key, predictive_attempt_id) = if ready_submit {
            (
                self.ready_physical_identity,
                self.ready_physical_key,
                self.ready_predictive_attempt,
            )
        } else {
            (
                self.active_physical_identity,
                self.active_physical_key,
                self.active_predictive_attempt,
            )
        };
        let ticket = WorkerPacingTicket {
            reservation_id: self.worker_reservation_ids.next(),
            frame_id,
            ready_submit,
            physical_identity,
            physical_key,
            predictive_attempt_id,
        };
        self.worker_reservation = Some(ticket);
        if ready_submit {
            self.ready_worker_reservation_id = Some(ticket.reservation_id());
        } else {
            self.active_worker_reservation_id = Some(ticket.reservation_id());
        }
        if let Some(identity) = lifecycle_identity(predictive_attempt_id, physical_key) {
            self.note_predictive_stage(identity, PredictiveO1Stage::WorkerQueued);
        }
        self.log("worker_pacing_reserved", Self::worker_ticket_fields(ticket));
        Ok(Some(ticket))
    }

    fn worker_submission_frame(&self, ready_submit: bool) -> Option<NativeOutputFrameId> {
        if ready_submit {
            self.ready
        } else {
            self.active
        }
    }

    fn clear_active_worker_timing_for_frame(&mut self, frame_id: Option<NativeOutputFrameId>) {
        if self.active_queued_frame_id == frame_id {
            self.active_queued_frame_id = None;
            self.active_queued_ns = None;
        }
    }

    fn clear_ready_waiting_timing(&mut self, frame_id: Option<NativeOutputFrameId>) {
        if self.ready_waiting_frame_id == frame_id {
            self.ready_waiting_frame_id = None;
            self.ready_waiting_started_ns = None;
        }
    }

    fn note_ready_submit_timing(
        &mut self,
        frame_id: Option<NativeOutputFrameId>,
        now_ns: u64,
        reservation_id: Option<WorkerPacingReservationId>,
    ) {
        let owns_timing = match reservation_id {
            Some(reservation_id) => self.ready_worker_reservation_id == Some(reservation_id),
            None => {
                self.ready_worker_reservation_id.is_none()
                    && self.ready_waiting_frame_id == frame_id
            }
        };
        if owns_timing {
            if let Some(started_at) = self.ready_waiting_started_ns.take() {
                self.ready_waiting_for_target
                    .record(now_ns.saturating_sub(started_at) / 1_000);
            }
            self.ready_waiting_frame_id = None;
        }
    }

    fn take_worker_submission(
        &mut self,
        ticket: WorkerPacingTicket,
    ) -> Result<WorkerPacingTicket, &'static str> {
        let Some(reservation) = self.worker_reservation else {
            self.log(
                "worker_pacing_reservation_stale",
                vec![
                    PacingField::u64(
                        "returned_worker_pacing_reservation_id",
                        ticket.reservation_id().get(),
                    ),
                    PacingField::none("current_worker_pacing_reservation_id"),
                    frame_id_field(Some(ticket.frame_id())),
                ],
            );
            return Err("worker pacing reservation does not match queued state");
        };
        if reservation.reservation_id() != ticket.reservation_id() {
            self.log(
                "worker_pacing_reservation_stale",
                vec![
                    PacingField::u64(
                        "returned_worker_pacing_reservation_id",
                        ticket.reservation_id().get(),
                    ),
                    PacingField::u64(
                        "current_worker_pacing_reservation_id",
                        reservation.reservation_id().get(),
                    ),
                    frame_id_field(Some(ticket.frame_id())),
                ],
            );
            return Err("worker pacing reservation does not match queued state");
        }

        let reservation_id = reservation.reservation_id();
        let active_owned = self.active_worker_reservation_id == Some(reservation_id);
        let ready_owned = self.ready_worker_reservation_id == Some(reservation_id);
        self.worker_reservation = None;
        if active_owned {
            self.active_worker_reservation_id = None;
            self.active = None;
            self.active_predictive_attempt = None;
            self.active_physical_identity = None;
            self.active_physical_key = None;
            self.active_queued_frame_id = None;
            self.active_queued_ns = None;
        }
        if ready_owned {
            self.ready_worker_reservation_id = None;
            self.ready = None;
            self.ready_predictive_attempt = None;
            self.ready_physical_identity = None;
            self.ready_physical_key = None;
            self.ready_waiting_frame_id = None;
            self.ready_waiting_started_ns = None;
        }
        Ok(reservation)
    }

    pub(crate) fn cancel_worker_submission(&mut self, ticket: Option<WorkerPacingTicket>) -> bool {
        if !self.enabled {
            return true;
        }
        let Some(ticket) = ticket else {
            return true;
        };
        let reservation = match self.take_worker_submission(ticket) {
            Ok(reservation) => reservation,
            Err(_) => return false,
        };
        if let Some(identity) = lifecycle_identity(
            reservation.predictive_attempt_id(),
            reservation.physical_key(),
        ) {
            self.terminalize_predictive_frame(identity, PredictiveReadyTerminal::Failed);
        }
        self.log(
            "worker_submit_cancelled",
            Self::worker_ticket_fields(reservation),
        );
        true
    }

    pub(crate) fn abandon_worker_submission(&mut self, ticket: Option<WorkerPacingTicket>) -> bool {
        if !self.enabled {
            return true;
        }
        let Some(ticket) = ticket else {
            return true;
        };
        let reservation = match self.take_worker_submission(ticket) {
            Ok(reservation) => reservation,
            Err(_) => return false,
        };
        if let Some(identity) = lifecycle_identity(
            reservation.predictive_attempt_id(),
            reservation.physical_key(),
        ) {
            self.terminalize_predictive_frame(
                identity,
                PredictiveReadyTerminal::OtherSafeAbandonment,
            );
        }
        self.log(
            "worker_submit_uncertain_abandoned",
            Self::worker_ticket_fields(reservation),
        );
        true
    }

    pub(crate) fn note_worker_submit_exact(
        &mut self,
        ticket: Option<WorkerPacingTicket>,
        token: u64,
        now_ns: u64,
        pacing_mode: NativeOutputPacingMode,
    ) -> Result<(), &'static str> {
        if !self.enabled {
            return Ok(());
        }
        let Some(ticket) = ticket else {
            return Ok(());
        };
        if ticket.ready_submit() {
            self.note_ready_submit_timing(
                Some(ticket.frame_id()),
                now_ns,
                Some(ticket.reservation_id()),
            );
        }
        let reservation = self.take_worker_submission(ticket)?;
        self.log(
            "worker_pacing_submitted",
            Self::worker_ticket_fields(reservation),
        );
        self.note_submit_frame(
            Some(reservation.frame_id()),
            reservation.physical_identity(),
            reservation.predictive_attempt_id(),
            reservation.physical_key(),
            token,
            now_ns,
            reservation.ready_submit(),
            pacing_mode,
        );
        Ok(())
    }

    fn worker_ticket_fields(ticket: WorkerPacingTicket) -> Vec<PacingField> {
        vec![
            PacingField::u64(
                "worker_pacing_reservation_id",
                ticket.reservation_id().get(),
            ),
            frame_id_field(Some(ticket.frame_id())),
            PacingField::bool("ready_submit", ticket.ready_submit()),
            PacingField::option_u64(
                "predictive_attempt_id",
                ticket.predictive_attempt_id().map(|attempt| attempt.get()),
            ),
            PacingField::option_u64(
                "output_frame_id",
                ticket.physical_identity().map(|identity| identity.frame_id),
            ),
        ]
    }

    pub(crate) fn abandon_pending_submission(&mut self, token: u64) -> bool {
        if self.pending_token != Some(token) {
            return false;
        }
        if let Some(identity) =
            lifecycle_identity(self.pending_predictive_attempt, self.pending_physical_key)
        {
            self.terminalize_predictive_frame(
                identity,
                PredictiveReadyTerminal::OtherSafeAbandonment,
            );
        }
        self.pending = None;
        self.pending_predictive_attempt = None;
        self.pending_physical_identity = None;
        self.pending_physical_key = None;
        self.pending_token = None;
        self.log(
            "worker_submit_abandoned",
            vec![PacingField::u64("pageflip_token", token)],
        );
        true
    }
    pub(crate) fn note_render_ahead_ready(&mut self, now_ns: u64) {
        self.note_ready_frame(now_ns, true);
    }

    #[allow(clippy::collapsible_if)]
    pub(crate) fn note_predictive_binding_after_predecessor_pageflip(
        &mut self,
        advanced_intervals: u64,
    ) {
        if !self.enabled {
            return;
        }
        self.predictive_bound_after_predecessor_pageflip = self
            .predictive_bound_after_predecessor_pageflip
            .saturating_add(1);
        self.predictive_binding_advanced_intervals = self
            .predictive_binding_advanced_intervals
            .saturating_add(advanced_intervals);
        if let Some(identity) =
            lifecycle_identity(self.ready_predictive_attempt, self.ready_physical_key)
        {
            if self.predictive_o1_lifecycle.contains_identity(identity) {
                self.note_predictive_stage(identity, PredictiveO1Stage::Bound);
            }
        }
    }

    pub(crate) fn note_predictive_unbound_created(&mut self) {
        if self.enabled {
            self.predictive_unbound_created = self.predictive_unbound_created.saturating_add(1);
        }
    }

    #[allow(clippy::collapsible_if)]
    pub(crate) fn note_predictive_unbound_ready(&mut self) {
        if self.enabled {
            self.predictive_unbound_ready = self.predictive_unbound_ready.saturating_add(1);
            if let Some(identity) =
                lifecycle_identity(self.ready_predictive_attempt, self.ready_physical_key)
            {
                if self.predictive_o1_lifecycle.contains_identity(identity) {
                    self.note_predictive_stage(identity, PredictiveO1Stage::ReadyUnbound);
                }
            }
        }
    }

    #[allow(clippy::collapsible_if)]
    pub(crate) fn note_predictive_binding_after_render_completion(
        &mut self,
        advanced_intervals: u64,
    ) {
        if !self.enabled {
            return;
        }
        self.predictive_bound_after_render_completion = self
            .predictive_bound_after_render_completion
            .saturating_add(1);
        self.predictive_binding_advanced_intervals = self
            .predictive_binding_advanced_intervals
            .saturating_add(advanced_intervals);
        if let Some(identity) =
            lifecycle_identity(self.ready_predictive_attempt, self.ready_physical_key)
        {
            if self.predictive_o1_lifecycle.contains_identity(identity) {
                self.note_predictive_stage(identity, PredictiveO1Stage::Bound);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn note_predictive_unbound_abandoned_identity(&mut self, frame_id: Option<u64>) {
        if self.enabled {
            self.note_predictive_terminal_exact(
                frame_id,
                PredictiveReadyTerminal::AbandonedIdentity,
            );
        }
    }

    pub(crate) fn note_predictive_unbound_abandoned_identity_physical(
        &mut self,
        identity: Option<OutputFrameIdentitySnapshot>,
    ) {
        if self.enabled {
            self.note_predictive_terminal_physical(
                identity,
                PredictiveReadyTerminal::AbandonedIdentity,
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn note_predictive_unbound_abandoned_generation(&mut self, frame_id: Option<u64>) {
        if self.enabled {
            self.note_predictive_terminal_exact(
                frame_id,
                PredictiveReadyTerminal::AbandonedGeneration,
            );
        }
    }

    pub(crate) fn note_predictive_unbound_abandoned_generation_physical(
        &mut self,
        identity: Option<OutputFrameIdentitySnapshot>,
    ) {
        if self.enabled {
            self.note_predictive_terminal_physical(
                identity,
                PredictiveReadyTerminal::AbandonedGeneration,
            );
        }
    }

    pub(crate) fn note_ready_frame(&mut self, now_ns: u64, _waits_for_target: bool) {
        if !self.enabled {
            return;
        }
        if let Some(identity) =
            lifecycle_identity(self.active_predictive_attempt, self.active_physical_key)
            && !self
                .predictive_o1_lifecycle
                .has_observed_stage(identity, PredictiveO1Stage::RenderReady)
        {
            self.note_render_ready();
        }
        let ready = self.active.take();
        let predictive_attempt = self.active_predictive_attempt.take();
        let origin = std::mem::take(&mut self.active_origin);
        let physical_identity = self.active_physical_identity.take();
        let physical_key = self.active_physical_key.take();
        self.ready_worker_reservation_id = self.active_worker_reservation_id.take();
        if origin == PreparedFrameOrigin::PredictiveO1 && predictive_attempt.is_some() {
            self.predictive_ready_created = self.predictive_ready_created.saturating_add(1);
            self.ready_waiting_started_ns = None;
            self.ready_waiting_frame_id = None;
        } else {
            self.normal_ready_wait_count += 1;
            self.ready_waiting_for_target_count += 1;
            self.ready_waiting_started_ns = Some(now_ns);
            self.ready_waiting_frame_id = ready;
        }
        self.ready = ready;
        self.ready_predictive_attempt = predictive_attempt;
        self.ready_physical_identity = physical_identity;
        self.ready_physical_key = physical_key;
        self.active_queued_frame_id = None;
        self.active_queued_ns = None;
        self.log(
            "ready_queued",
            vec![
                frame_id_field(self.ready),
                PacingField::option_u64(
                    "predictive_attempt_id",
                    self.ready_predictive_attempt.map(|attempt| attempt.get()),
                ),
                PacingField::option_u64(
                    "output_frame_id",
                    self.ready_physical_identity
                        .map(|identity| identity.frame_id),
                ),
                PacingField::u64("render_end_ns", now_ns),
            ],
        );
    }

    pub(crate) fn note_ready_pull_in_attempt(&mut self) {
        if self.enabled {
            self.ready_pull_in_attempts = self.ready_pull_in_attempts.saturating_add(1);
        }
    }

    pub(crate) fn note_ready_pull_in_rejected_too_late(&mut self) {
        if self.enabled {
            self.ready_pull_in_rejected_too_late =
                self.ready_pull_in_rejected_too_late.saturating_add(1);
        }
    }

    pub(crate) fn note_ready_pull_in_rejected_owned(&mut self) {
        if self.enabled {
            self.ready_pull_in_rejected_owned = self.ready_pull_in_rejected_owned.saturating_add(1);
        }
    }

    pub(crate) fn note_ready_pull_in_rejected_identity(&mut self) {
        if self.enabled {
            self.ready_pull_in_rejected_identity =
                self.ready_pull_in_rejected_identity.saturating_add(1);
        }
    }

    pub(crate) fn note_ready_pull_in_success(
        &mut self,
        old_target: PresentationTarget,
        new_target: PresentationTarget,
    ) {
        if !self.enabled {
            return;
        }
        let advanced_intervals = old_target
            .physical_claim()
            .sequence
            .saturating_sub(new_target.physical_claim().sequence);
        self.ready_pull_in_successes = self.ready_pull_in_successes.saturating_add(1);
        self.ready_waiting_for_target_count = self.ready_waiting_for_target_count.saturating_sub(1);
        self.ready_pull_in_advanced_intervals = self
            .ready_pull_in_advanced_intervals
            .saturating_add(advanced_intervals);
        self.log(
            "ready_target_pull_in",
            vec![
                PacingField::u64("old_target_sequence", old_target.sequence),
                PacingField::u64("new_target_sequence", new_target.sequence),
                PacingField::u64("advanced_intervals", advanced_intervals),
                PacingField::u64(
                    "old_target_presentation_ns",
                    old_target.presentation_time.get(),
                ),
                PacingField::u64(
                    "new_target_presentation_ns",
                    new_target.presentation_time.get(),
                ),
            ],
        );
    }

    pub(crate) fn abandon_ready_frame(&mut self) -> bool {
        if !self.enabled {
            return true;
        }
        let Some(frame_id) = self.ready.take() else {
            return false;
        };
        let _physical_identity = self.ready_physical_identity.take();
        let predictive_attempt = self.ready_predictive_attempt.take();
        let physical_key = self.ready_physical_key.take();
        if let Some(identity) = lifecycle_identity(predictive_attempt, physical_key) {
            self.terminalize_predictive_frame(identity, PredictiveReadyTerminal::OvertakenReady);
        }
        self.clear_ready_waiting_timing(Some(frame_id));
        self.log(
            "ready_frame_abandoned",
            vec![frame_id_field(Some(frame_id))],
        );
        true
    }

    #[cfg(test)]
    pub(crate) fn note_pageflip(
        &mut self,
        now_ns: u64,
        submitted_at_ns: u64,
        token: u64,
        refresh_interval_us: u64,
    ) {
        self.note_pageflip_exact(None, now_ns, submitted_at_ns, token, refresh_interval_us);
    }

    pub(crate) fn note_pageflip_exact(
        &mut self,
        physical_identity: Option<OutputFrameIdentitySnapshot>,
        now_ns: u64,
        submitted_at_ns: u64,
        token: u64,
        refresh_interval_us: u64,
    ) {
        if !self.enabled {
            return;
        }
        if let Some(last) = self.last_pageflip_ns {
            let us = now_ns.saturating_sub(last) / 1_000;
            self.pageflip_intervals.record(us);
            if is_active_refresh_interval(us, refresh_interval_us) {
                self.active_pageflip_intervals.record(us);
                self.misses.record(us, refresh_interval_us);
            } else {
                self.idle_intervals_excluded = self.idle_intervals_excluded.saturating_add(1);
            }
        }
        self.last_pageflip_ns = Some(now_ns);
        let commit_us = now_ns.saturating_sub(submitted_at_ns) / 1_000;
        self.commit_to_present.record(commit_us);
        let id = self.pending.take();
        self.pending_physical_identity.take();
        let pending_physical_key = self.pending_physical_key.take();
        let pending_predictive_attempt = self.pending_predictive_attempt.take();
        self.pending_token = None;
        let predictive_attempt_id = pending_predictive_attempt.map(|attempt| attempt.get());
        if let Some(attempt_id) = pending_predictive_attempt {
            let actual_key = physical_identity.as_ref().map(OutputFrameKey::from);
            let identity = match (pending_physical_key, actual_key) {
                (Some(expected), Some(actual)) if expected == actual => {
                    Some(PredictiveO1LifecycleIdentity::Physical(actual))
                }
                (Some(expected), Some(actual)) => {
                    self.predictive_o1_invalid_stage_transitions = self
                        .predictive_o1_invalid_stage_transitions
                        .saturating_add(1);
                    self.terminalize_predictive_frame(
                        PredictiveO1LifecycleIdentity::Physical(expected),
                        PredictiveReadyTerminal::InvalidStage,
                    );
                    self.log(
                        "predictive_physical_identity_mismatch",
                        vec![
                            PacingField::u64("predictive_attempt_id", attempt_id.get()),
                            PacingField::u64("expected_output_frame_id", expected.frame_id),
                            PacingField::u64("actual_output_frame_id", actual.frame_id),
                        ],
                    );
                    None
                }
                (Some(expected), None) => {
                    self.predictive_o1_invalid_stage_transitions = self
                        .predictive_o1_invalid_stage_transitions
                        .saturating_add(1);
                    self.terminalize_predictive_frame(
                        PredictiveO1LifecycleIdentity::Physical(expected),
                        PredictiveReadyTerminal::InvalidStage,
                    );
                    self.log(
                        "predictive_physical_identity_missing",
                        vec![
                            PacingField::u64("predictive_attempt_id", attempt_id.get()),
                            PacingField::u64("expected_output_frame_id", expected.frame_id),
                        ],
                    );
                    None
                }
                (None, Some(actual)) => {
                    self.predictive_o1_invalid_stage_transitions = self
                        .predictive_o1_invalid_stage_transitions
                        .saturating_add(1);
                    self.terminalize_predictive_frame(
                        PredictiveO1LifecycleIdentity::Attempt(attempt_id),
                        PredictiveReadyTerminal::InvalidStage,
                    );
                    self.log(
                        "predictive_physical_identity_unbound",
                        vec![
                            PacingField::u64("predictive_attempt_id", attempt_id.get()),
                            PacingField::u64("actual_output_frame_id", actual.frame_id),
                        ],
                    );
                    None
                }
                (None, None) => Some(PredictiveO1LifecycleIdentity::Attempt(attempt_id)),
            };
            if let Some(identity) = identity
                && self.note_predictive_stage(identity, PredictiveO1Stage::Presented)
            {
                self.terminalize_predictive_frame(identity, PredictiveReadyTerminal::Presented);
            }
        }
        self.log(
            "pageflip_complete",
            vec![
                frame_id_field(id),
                PacingField::option_u64("predictive_attempt_id", predictive_attempt_id),
                PacingField::option_u64(
                    "output_frame_id",
                    physical_identity.map(|identity| identity.frame_id),
                ),
                PacingField::u64("pageflip_token", token),
                PacingField::u64("pageflip_complete_ns", now_ns),
                PacingField::u64("commit_to_present_us", commit_us),
            ],
        );
    }
    pub(crate) fn last_pageflip_ns(&self) -> Option<u64> {
        self.last_pageflip_ns
    }
    pub(crate) fn note_wake_lateness(&mut self, lateness_ns: u64) {
        if self.enabled {
            self.wake_lateness.record(lateness_ns / 1_000);
        }
    }
    pub(crate) fn note_deadline_state(
        &mut self,
        decision: SchedulerDecision,
        now_ns: u64,
        scheduler_deadline: Option<u64>,
        visual_deadline: Option<u64>,
        ready_frame_present: bool,
        timer_wake: bool,
    ) {
        if !self.enabled {
            return;
        }
        if visual_deadline.is_some() && ready_frame_present {
            self.multiple_deadline_owner_violation_count += 1;
        }
        let deadline = match (scheduler_deadline, visual_deadline) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
            (None, None) => None,
        };
        if decision == SchedulerDecision::WaitForRefresh
            && deadline.is_some_and(|deadline| deadline <= now_ns)
        {
            self.expired_deadline_wait_count += 1;
        }
        if timer_wake && deadline.is_some_and(|deadline| deadline <= now_ns) {
            if self.last_immediate_timer_deadline == deadline {
                self.repeated_immediate_timer_wake_count += 1;
            }
            self.last_immediate_timer_deadline = deadline;
        } else {
            self.last_immediate_timer_deadline = None;
        }
    }

    fn classify_fast_client_candidate(
        &mut self,
        observation: ExplicitPresentationObservation,
        fast_client_threshold_ns: u64,
        callback_handoff_limited: bool,
    ) -> FastClientCandidateOutcome {
        let Some(reaction_ns) = observation.callback_reaction_ns else {
            return FastClientCandidateOutcome::NotSeen;
        };
        if reaction_ns > fast_client_threshold_ns {
            return FastClientCandidateOutcome::NotSeen;
        }
        self.fast_candidate_seen = self.fast_candidate_seen.saturating_add(1);
        let outcome = if observation.callback_surface_id.is_none() {
            FastClientCandidateOutcome::MissingSurface
        } else if !observation.callback_surface_is_exclusive {
            FastClientCandidateOutcome::NonExclusiveSurface
        } else if observation.client_commit_ns.is_none() {
            FastClientCandidateOutcome::MissingCommit
        } else if observation.callback_admission_ns.is_none() {
            FastClientCandidateOutcome::MissingAdmission
        } else if callback_handoff_limited {
            FastClientCandidateOutcome::CallbackHandoff
        } else {
            FastClientCandidateOutcome::Qualified
        };
        match outcome {
            FastClientCandidateOutcome::MissingSurface => {
                self.fast_candidate_rejected_missing_surface = self
                    .fast_candidate_rejected_missing_surface
                    .saturating_add(1);
            }
            FastClientCandidateOutcome::NonExclusiveSurface => {
                self.fast_candidate_rejected_nonexclusive_surface = self
                    .fast_candidate_rejected_nonexclusive_surface
                    .saturating_add(1);
            }
            FastClientCandidateOutcome::MissingCommit => {
                self.fast_candidate_rejected_missing_commit = self
                    .fast_candidate_rejected_missing_commit
                    .saturating_add(1);
            }
            FastClientCandidateOutcome::MissingAdmission => {
                self.fast_candidate_rejected_missing_admission = self
                    .fast_candidate_rejected_missing_admission
                    .saturating_add(1);
            }
            FastClientCandidateOutcome::CallbackHandoff => {
                self.fast_candidate_rejected_callback_handoff = self
                    .fast_candidate_rejected_callback_handoff
                    .saturating_add(1);
            }
            FastClientCandidateOutcome::Qualified => {
                self.fast_candidate_qualified = self.fast_candidate_qualified.saturating_add(1);
            }
            FastClientCandidateOutcome::NotSeen => {}
        }
        outcome
    }

    fn note_fast_client_continuity(
        &mut self,
        observation: ExplicitPresentationObservation,
        candidate: FastClientCandidateOutcome,
    ) {
        if candidate != FastClientCandidateOutcome::Qualified {
            return;
        }
        let previous_surface = self.last_fast_client_surface_id;
        let previous_commit = self.last_fast_client_commit_ns;
        let previous_present = self.last_fast_client_presented_ns;
        if previous_present.is_none() {
            if previous_surface.is_none() && previous_commit.is_none() {
                self.fast_continuity_seeded = self.fast_continuity_seeded.saturating_add(1);
            } else {
                self.fast_continuity_broken_missing_previous_present = self
                    .fast_continuity_broken_missing_previous_present
                    .saturating_add(1);
            }
        } else if observation.callback_surface_id != previous_surface {
            self.fast_continuity_broken_surface_change =
                self.fast_continuity_broken_surface_change.saturating_add(1);
        } else if observation
            .client_commit_ns
            .zip(previous_commit)
            .is_none_or(|(current, previous)| current <= previous)
        {
            self.fast_continuity_broken_nonmonotonic_commit = self
                .fast_continuity_broken_nonmonotonic_commit
                .saturating_add(1);
        } else {
            self.fast_continuity_sampled = self.fast_continuity_sampled.saturating_add(1);
        }
    }

    pub(crate) fn note_explicit_present(&mut self, observation: ExplicitPresentationObservation) {
        if !self.enabled {
            return;
        }
        let refresh_interval_ns = observation.refresh_interval_ns.max(1);
        let previous_primary_sequence = observation
            .previous_primary_sequence
            .or(self.last_primary_sequence);
        let selected_target_distance = previous_primary_sequence
            .map(|previous| observation.planned_sequence.saturating_sub(previous))
            .unwrap_or_default();
        let actual_primary_distance = previous_primary_sequence
            .map(|previous| observation.actual_sequence.saturating_sub(previous))
            .unwrap_or_default();
        if previous_primary_sequence.is_some() {
            self.selected_target_distance_intervals
                .record(selected_target_distance);
            self.actual_primary_distance_intervals
                .record(actual_primary_distance);
        }
        self.last_primary_sequence = Some(observation.actual_sequence);
        if let Some(client_commit_ns) = observation.client_commit_ns
            && observation.composite_started_ns >= client_commit_ns
        {
            self.client_commit_to_render_start.record(
                observation
                    .composite_started_ns
                    .saturating_sub(client_commit_ns)
                    / 1_000,
            );
        }
        self.render_start_to_ready.record(
            observation
                .rendered_ns
                .saturating_sub(observation.composite_started_ns)
                / 1_000,
        );
        self.ready_to_submit.record(
            observation
                .submit_started_ns
                .saturating_sub(observation.rendered_ns)
                / 1_000,
        );
        self.submit_to_pageflip.record(
            observation
                .presented_ns
                .saturating_sub(observation.submit_returned_ns)
                / 1_000,
        );
        let target_distance = observation
            .target_ns
            .abs_diff(observation.presented_ns)
            .div_ceil(refresh_interval_ns);
        let target_is_early = observation.target_ns <= observation.presented_ns;
        match (observation.target_reason, target_is_early) {
            (PresentationTargetReason::ReactiveDouble, true) => {
                self.reactive_target_early_by_intervals = self
                    .reactive_target_early_by_intervals
                    .saturating_add(target_distance);
            }
            (PresentationTargetReason::ReactiveDouble, false) => {
                self.reactive_target_late_by_intervals = self
                    .reactive_target_late_by_intervals
                    .saturating_add(target_distance);
            }
            (_, true) => {
                self.predictive_target_early_by_intervals = self
                    .predictive_target_early_by_intervals
                    .saturating_add(target_distance);
            }
            (_, false) => {
                self.predictive_target_late_by_intervals = self
                    .predictive_target_late_by_intervals
                    .saturating_add(target_distance);
            }
        }
        let earliest_feasible_distance = previous_primary_sequence
            .map(|previous| {
                observation
                    .target_selection
                    .earliest_feasible_sequence
                    .saturating_sub(previous)
            })
            .unwrap_or(selected_target_distance);
        let fast_client_threshold_ns = (refresh_interval_ns / 2).min(2_000_000);
        if let Some(reaction_ns) = observation.callback_reaction_ns {
            self.callback_admission_to_next_commit
                .record(reaction_ns / 1_000);
            if reaction_ns <= fast_client_threshold_ns {
                self.fast_client_samples = self.fast_client_samples.saturating_add(1);
            } else {
                self.slow_client_samples = self.slow_client_samples.saturating_add(1);
            }
        }
        let callback_handoff_limited = observation
            .callback_admission_ns
            .zip(observation.client_commit_ns)
            .zip(self.last_pageflip_ns)
            .is_some_and(|((admission_ns, commit_ns), previous_pageflip_ns)| {
                admission_ns >= previous_pageflip_ns.saturating_add(refresh_interval_ns)
                    && commit_ns > previous_pageflip_ns.saturating_add(refresh_interval_ns)
                    && observation
                        .callback_reaction_ns
                        .is_some_and(|reaction| reaction <= fast_client_threshold_ns)
            });
        let attribution = classify_content_frame(
            callback_handoff_limited,
            observation.callback_reaction_ns,
            fast_client_threshold_ns,
            selected_target_distance,
            actual_primary_distance,
            observation.target_selection,
            earliest_feasible_distance,
            observation.render_missed,
            observation.submit_missed,
            observation.kms_slipped,
        );
        let fast_client_candidate = self.classify_fast_client_candidate(
            observation,
            fast_client_threshold_ns,
            callback_handoff_limited,
        );
        self.note_fast_client_continuity(observation, fast_client_candidate);
        let fast_interval_us = self
            .last_fast_client_presented_ns
            .map(|previous| observation.presented_ns.saturating_sub(previous) / 1_000);
        let continuous_fast_client = fast_client_candidate == FastClientCandidateOutcome::Qualified
            && observation
                .callback_surface_id
                .is_some_and(|surface_id| self.last_fast_client_surface_id == Some(surface_id))
            && observation
                .client_commit_ns
                .zip(self.last_fast_client_commit_ns)
                .is_some_and(|(current, previous)| current > previous)
            // The exact callback admission and next visual commit prove that
            // useful demand remained outstanding. A long physical interval
            // is therefore a compositor tail, not evidence of idle time.
            && fast_interval_us.is_some();
        if continuous_fast_client {
            self.fast_client_continuous_samples =
                self.fast_client_continuous_samples.saturating_add(1);
            if let Some(interval_us) = fast_interval_us {
                self.fast_client_primary_present_intervals
                    .record(interval_us);
                self.fast_client_misses
                    .record(interval_us, refresh_interval_ns / 1_000);
            }
            self.fast_client_actual_primary_distance_intervals
                .record(actual_primary_distance);
            match attribution {
                ContentCadenceAttribution::TargetHit => {
                    self.fast_client_target_hit = self.fast_client_target_hit.saturating_add(1);
                }
                ContentCadenceAttribution::TargetLimited => {
                    self.fast_client_target_limited =
                        self.fast_client_target_limited.saturating_add(1);
                }
                ContentCadenceAttribution::RenderLimited => {
                    self.fast_client_render_limited =
                        self.fast_client_render_limited.saturating_add(1);
                }
                ContentCadenceAttribution::SubmitLimited => {
                    self.fast_client_submit_limited =
                        self.fast_client_submit_limited.saturating_add(1);
                }
                ContentCadenceAttribution::KmsLimited => {
                    self.fast_client_kms_limited = self.fast_client_kms_limited.saturating_add(1);
                }
                ContentCadenceAttribution::CallbackHandoffLimited
                | ContentCadenceAttribution::ClientLimited => {}
            }
        }
        if fast_client_candidate == FastClientCandidateOutcome::Qualified {
            self.last_fast_client_surface_id = observation.callback_surface_id;
            self.last_fast_client_commit_ns = observation.client_commit_ns;
            self.last_fast_client_presented_ns = Some(observation.presented_ns);
        } else {
            self.last_fast_client_surface_id = None;
            self.last_fast_client_commit_ns = None;
            self.last_fast_client_presented_ns = None;
        }
        let attribution_index = attribution.index();
        self.content_attribution[attribution_index] =
            self.content_attribution[attribution_index].saturating_add(1);
        let signed_error_ns = if observation.presented_ns >= observation.target_ns {
            i64::try_from(
                observation
                    .presented_ns
                    .saturating_sub(observation.target_ns),
            )
            .unwrap_or(i64::MAX)
        } else {
            -i64::try_from(
                observation
                    .target_ns
                    .saturating_sub(observation.presented_ns),
            )
            .unwrap_or(i64::MAX)
        };
        self.target_error
            .record(signed_error_ns.unsigned_abs() / 1_000);
        self.target_error_signed.record(signed_error_ns / 1_000);
        self.target_interval_distance.record(
            observation
                .planned_sequence
                .abs_diff(observation.actual_sequence),
        );
        if observation.reactive_double && observation.actual_sequence > observation.planned_sequence
        {
            self.reactive_double_actual_misses = self
                .reactive_double_actual_misses
                .saturating_add(observation.actual_sequence - observation.planned_sequence);
        }
        if signed_error_ns < -(TARGET_TIMESTAMP_TOLERANCE_NS as i64) {
            self.early_presentation_count += 1;
        } else if signed_error_ns > TARGET_TIMESTAMP_TOLERANCE_NS as i64 {
            self.late_presentation_count += 1;
        }
        self.slot_hold.record(
            observation
                .submit_returned_ns
                .saturating_sub(observation.composite_started_ns)
                / 1_000,
        );
        self.ready_age.record(
            observation
                .submit_started_ns
                .saturating_sub(observation.rendered_ns)
                / 1_000,
        );
        self.atomic_submit.record(
            observation
                .submit_returned_ns
                .saturating_sub(observation.submit_started_ns)
                / 1_000,
        );
    }
    pub(crate) fn note_adaptive_transition(
        &mut self,
        before: AdaptiveBufferingMode,
        after: AdaptiveBufferingMode,
        miss: Option<ProvenDeadlineMiss>,
    ) {
        if !self.enabled || before == after {
            return;
        }
        match (before, after, miss) {
            (AdaptiveBufferingMode::Double, AdaptiveBufferingMode::Triple, None) => {
                self.adaptive_triple_entries_predicted += 1;
            }
            (
                AdaptiveBufferingMode::Double,
                AdaptiveBufferingMode::Triple,
                Some(ProvenDeadlineMiss::KmsDispatch),
            ) => self.adaptive_triple_entries_proven_submit_miss += 1,
            (
                AdaptiveBufferingMode::Double,
                AdaptiveBufferingMode::Triple,
                Some(ProvenDeadlineMiss::KmsApplyGuard),
            ) => self.adaptive_triple_entries_proven_presentation_miss += 1,
            (AdaptiveBufferingMode::Double, AdaptiveBufferingMode::Triple, Some(_)) => {
                self.adaptive_triple_entries_proven_render_miss += 1;
            }
            (AdaptiveBufferingMode::Triple, AdaptiveBufferingMode::Double, _) => {
                self.adaptive_triple_exits += 1;
            }
            _ => {}
        }
    }
    pub(crate) fn note_pipeline_wait(&mut self, reason: PipelineWaitReason) {
        if !self.enabled {
            return;
        }
        let index = match reason {
            PipelineWaitReason::RefreshDeadline => 0,
            PipelineWaitReason::NoFreeSlot => 1,
            PipelineWaitReason::PreparedFrameExists => 2,
            PipelineWaitReason::FuturePrimaryDepthFull => 3,
            PipelineWaitReason::WorkerQueueOccupied => 4,
            PipelineWaitReason::KernelCommitPending => 5,
            PipelineWaitReason::RenderFence => 6,
            PipelineWaitReason::DirectSteadyState => 7,
            PipelineWaitReason::CompatibilityPath => 8,
            PipelineWaitReason::TripleCapabilityUnavailable => 9,
        };
        self.pipeline_waits[index] = self.pipeline_waits[index].saturating_add(1);
    }

    pub(crate) const fn buffering_metrics(&self) -> NativeBufferingMetrics {
        NativeBufferingMetrics {
            reactive_double_frames: self.reactive_double_frames,
            predictive_triple_frames: self.predictive_triple_frames,
            render_ahead_attempts: self.render_ahead_attempts,
            render_ahead_ready: self.predictive_render_ahead_ready,
            ready_submits: self.ready_submit_count,
            triple_entries_predicted: self.adaptive_triple_entries_predicted,
            triple_entries_render_miss: self.adaptive_triple_entries_proven_render_miss,
            triple_entries_submit_miss: self.adaptive_triple_entries_proven_submit_miss,
            triple_entries_presentation_miss: self.adaptive_triple_entries_proven_presentation_miss,
            triple_exits: self.adaptive_triple_exits,
            o1_credit2_useful_hits: self.o1_credit2_useful_hits,
            o1_credit2_unnecessary_hits: self.o1_credit2_unnecessary_hits,
            o1_credit2_ineffective_misses: self.o1_credit2_ineffective_misses,
            o1_credit2_granted_not_consumed: self.o1_credit2_granted_not_consumed,
            o1_credit2_drain_events: self.o1_credit2_drain_events,
            o1_credit2_refill_suppressed_while_draining: self
                .o1_credit2_refill_suppressed_while_draining,
            ready_pull_in_attempts: self.ready_pull_in_attempts,
            ready_pull_in_successes: self.ready_pull_in_successes,
            ready_pull_in_rejected_too_late: self.ready_pull_in_rejected_too_late,
            ready_pull_in_rejected_owned: self.ready_pull_in_rejected_owned,
            ready_pull_in_rejected_identity: self.ready_pull_in_rejected_identity,
            ready_pull_in_advanced_intervals: self.ready_pull_in_advanced_intervals,
        }
    }

    pub(crate) fn timing_metrics(&self) -> NativePacingTimingMetrics {
        NativePacingTimingMetrics {
            wake_lateness: self.wake_lateness.percentiles(),
            target_error: self.target_error.percentiles(),
            pageflip_interval: self.pageflip_intervals.percentiles(),
            active_pageflip_interval: self.active_pageflip_intervals.percentiles(),
            commit_to_present: self.commit_to_present.percentiles(),
            missed_refresh_1x: self.misses.missed_1x,
            missed_refresh_2x: self.misses.missed_2x,
            missed_refresh_3x_or_more: self.misses.missed_3x_or_more,
        }
    }

    pub(crate) fn content_summary_line(&self) -> String {
        let (primary50, primary95, primary99) = self.active_pageflip_intervals.percentiles();
        let (callback50, callback95, callback99) =
            self.callback_admission_to_next_commit.percentiles();
        let (client50, client95, client99) = self.client_commit_to_render_start.percentiles();
        let (render50, render95, render99) = self.render_start_to_ready.percentiles();
        let (ready50, ready95, ready99) = self.ready_to_submit.percentiles();
        let (submit50, submit95, submit99) = self.submit_to_pageflip.percentiles();
        let (selected50, selected95, selected99) =
            self.selected_target_distance_intervals.percentiles();
        let (actual50, actual95, actual99) = self.actual_primary_distance_intervals.percentiles();
        let (fast_primary50, fast_primary95, fast_primary99) =
            self.fast_client_primary_present_intervals.percentiles();
        let (fast_actual50, fast_actual95, fast_actual99) = self
            .fast_client_actual_primary_distance_intervals
            .percentiles();
        let prediction = self.last_prediction;
        pacing_line(
            "native_content_frame_clock_summary",
            &[
                PacingField::u64("primary_present_interval_p50_us", primary50),
                PacingField::u64("primary_present_interval_p95_us", primary95),
                PacingField::u64("primary_present_interval_p99_us", primary99),
                PacingField::u64("callback_admission_to_next_commit_p50_us", callback50),
                PacingField::u64("callback_admission_to_next_commit_p95_us", callback95),
                PacingField::u64("callback_admission_to_next_commit_p99_us", callback99),
                PacingField::u64("client_commit_to_render_start_p50_us", client50),
                PacingField::u64("client_commit_to_render_start_p95_us", client95),
                PacingField::u64("client_commit_to_render_start_p99_us", client99),
                PacingField::u64("render_start_to_ready_p50_us", render50),
                PacingField::u64("render_start_to_ready_p95_us", render95),
                PacingField::u64("render_start_to_ready_p99_us", render99),
                PacingField::u64("ready_to_submit_p50_us", ready50),
                PacingField::u64("ready_to_submit_p95_us", ready95),
                PacingField::u64("ready_to_submit_p99_us", ready99),
                PacingField::u64("submit_to_pageflip_p50_us", submit50),
                PacingField::u64("submit_to_pageflip_p95_us", submit95),
                PacingField::u64("submit_to_pageflip_p99_us", submit99),
                PacingField::u64("selected_target_distance_intervals_p50", selected50),
                PacingField::u64("selected_target_distance_intervals_p95", selected95),
                PacingField::u64("selected_target_distance_intervals_p99", selected99),
                PacingField::u64("actual_primary_distance_intervals_p50", actual50),
                PacingField::u64("actual_primary_distance_intervals_p95", actual95),
                PacingField::u64("actual_primary_distance_intervals_p99", actual99),
                PacingField::u64(
                    "reactive_target_early_by_intervals",
                    self.reactive_target_early_by_intervals,
                ),
                PacingField::u64(
                    "predictive_target_early_by_intervals",
                    self.predictive_target_early_by_intervals,
                ),
                PacingField::u64(
                    "reactive_target_late_by_intervals",
                    self.reactive_target_late_by_intervals,
                ),
                PacingField::u64(
                    "predictive_target_late_by_intervals",
                    self.predictive_target_late_by_intervals,
                ),
                PacingField::u64("fast_client_samples", self.fast_client_samples),
                PacingField::u64("slow_client_samples", self.slow_client_samples),
                PacingField::u64("fast_candidate_seen", self.fast_candidate_seen),
                PacingField::u64("fast_candidate_qualified", self.fast_candidate_qualified),
                PacingField::u64(
                    "fast_candidate_rejected_missing_surface",
                    self.fast_candidate_rejected_missing_surface,
                ),
                PacingField::u64(
                    "fast_candidate_rejected_nonexclusive_surface",
                    self.fast_candidate_rejected_nonexclusive_surface,
                ),
                PacingField::u64(
                    "fast_candidate_rejected_missing_commit",
                    self.fast_candidate_rejected_missing_commit,
                ),
                PacingField::u64(
                    "fast_candidate_rejected_missing_admission",
                    self.fast_candidate_rejected_missing_admission,
                ),
                PacingField::u64(
                    "fast_candidate_rejected_callback_handoff",
                    self.fast_candidate_rejected_callback_handoff,
                ),
                PacingField::u64("fast_continuity_seeded", self.fast_continuity_seeded),
                PacingField::u64("fast_continuity_sampled", self.fast_continuity_sampled),
                PacingField::u64(
                    "fast_continuity_broken_surface_change",
                    self.fast_continuity_broken_surface_change,
                ),
                PacingField::u64(
                    "fast_continuity_broken_nonmonotonic_commit",
                    self.fast_continuity_broken_nonmonotonic_commit,
                ),
                PacingField::u64(
                    "fast_continuity_broken_missing_previous_present",
                    self.fast_continuity_broken_missing_previous_present,
                ),
                PacingField::u64(
                    "fast_client_continuous_samples",
                    self.fast_client_continuous_samples,
                ),
                PacingField::u64(
                    "fast_client_primary_present_interval_p50_us",
                    fast_primary50,
                ),
                PacingField::u64(
                    "fast_client_primary_present_interval_p95_us",
                    fast_primary95,
                ),
                PacingField::u64(
                    "fast_client_primary_present_interval_p99_us",
                    fast_primary99,
                ),
                PacingField::u64("fast_client_actual_primary_distance_p50", fast_actual50),
                PacingField::u64("fast_client_actual_primary_distance_p95", fast_actual95),
                PacingField::u64("fast_client_actual_primary_distance_p99", fast_actual99),
                PacingField::u64(
                    "fast_client_missed_refresh_1x",
                    self.fast_client_misses.missed_1x,
                ),
                PacingField::u64(
                    "fast_client_missed_refresh_2x",
                    self.fast_client_misses.missed_2x,
                ),
                PacingField::u64(
                    "fast_client_missed_refresh_3x_or_more",
                    self.fast_client_misses.missed_3x_or_more,
                ),
                PacingField::u64("fast_client_target_hit", self.fast_client_target_hit),
                PacingField::u64(
                    "fast_client_target_limited",
                    self.fast_client_target_limited,
                ),
                PacingField::u64(
                    "fast_client_render_limited",
                    self.fast_client_render_limited,
                ),
                PacingField::u64(
                    "fast_client_submit_limited",
                    self.fast_client_submit_limited,
                ),
                PacingField::u64("fast_client_kms_limited", self.fast_client_kms_limited),
                PacingField::u64(
                    "physical_claim_overtake_ready",
                    self.physical_claim_overtake_ready,
                ),
                PacingField::u64(
                    "physical_claim_overtake_worker_queued",
                    self.physical_claim_overtake_worker_queued,
                ),
                PacingField::u64(
                    "physical_claim_overtake_recoveries",
                    self.physical_claim_overtake_recoveries,
                ),
                PacingField::u64(
                    "physical_claim_overtake_recovery_failures",
                    self.physical_claim_overtake_recovery_failures,
                ),
                PacingField::u64(
                    "physical_claim_fatal_violations",
                    self.physical_claim_fatal_violations,
                ),
                PacingField::u64(
                    "content_attribution_callback_handoff_limited",
                    self.content_attribution
                        [ContentCadenceAttribution::CallbackHandoffLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_client_limited",
                    self.content_attribution[ContentCadenceAttribution::ClientLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_target_limited",
                    self.content_attribution[ContentCadenceAttribution::TargetLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_render_limited",
                    self.content_attribution[ContentCadenceAttribution::RenderLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_submit_limited",
                    self.content_attribution[ContentCadenceAttribution::SubmitLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_kms_limited",
                    self.content_attribution[ContentCadenceAttribution::KmsLimited.index()],
                ),
                PacingField::u64(
                    "content_attribution_target_hit",
                    self.content_attribution[ContentCadenceAttribution::TargetHit.index()],
                ),
                PacingField::u64(
                    "prediction_ewma_render_ns",
                    prediction.map_or(0, |value| value.ewma_render_ns),
                ),
                PacingField::u64(
                    "prediction_upper_render_deviation_ns",
                    prediction.map_or(0, |value| value.upper_render_deviation_ns),
                ),
                PacingField::u64(
                    "prediction_p90_recent_render_ns",
                    prediction.map_or(0, |value| value.p90_recent_render_ns),
                ),
                PacingField::u64(
                    "prediction_render_risk_ns",
                    prediction.map_or(0, |value| value.render_risk_ns),
                ),
                PacingField::u64(
                    "prediction_independent_total_cost_ns",
                    prediction.map_or(0, |value| value.independent_total_cost_ns),
                ),
                PacingField::u64(
                    "prediction_warm_paired_total_cost_ns",
                    prediction.map_or(0, |value| value.warm_paired_total_cost_ns),
                ),
                PacingField::u64(
                    "prediction_independent_p90_floor_ns",
                    prediction.map_or(0, |value| value.independent_p90_floor_ns),
                ),
                PacingField::u64(
                    "prediction_worker_non_ioctl_lead_ns",
                    prediction.map_or(0, |value| value.worker_non_ioctl_lead_ns),
                ),
                PacingField::usize(
                    "prediction_miss_recovery_remaining",
                    prediction.map_or(0, |value| value.miss_recovery_remaining),
                ),
                PacingField::u64(
                    "prediction_p95_wake_lateness_ns",
                    prediction.map_or(0, |value| value.p95_wake_lateness_ns),
                ),
                PacingField::u64(
                    "prediction_p95_worker_queue_residency_ns",
                    prediction.map_or(0, |value| value.p95_worker_queue_residency_ns),
                ),
                PacingField::u64(
                    "prediction_p95_worker_pre_submit_ns",
                    prediction.map_or(0, |value| value.p95_worker_pre_submit_ns),
                ),
                PacingField::u64(
                    "prediction_p95_worker_dispatch_ns",
                    prediction.map_or(0, |value| value.p95_worker_dispatch_ns),
                ),
                PacingField::u64(
                    "prediction_p95_atomic_ioctl_ns",
                    prediction.map_or(0, |value| value.p95_atomic_ioctl_ns),
                ),
                PacingField::u64(
                    "prediction_p95_atomic_submit_ns",
                    prediction.map_or(0, |value| value.p95_atomic_submit_ns),
                ),
                PacingField::u64(
                    "prediction_p95_target_slip_ns",
                    prediction.map_or(0, |value| value.p95_target_slip_ns),
                ),
                PacingField::u64(
                    "prediction_paired_service_p95_ns",
                    prediction.map_or(0, |value| value.paired_service_p95_ns),
                ),
                PacingField::u64(
                    "prediction_paired_service_samples",
                    prediction.map_or(0, |value| value.paired_service_samples as u64),
                ),
                PacingField::str(
                    "prediction_estimator_mode",
                    prediction.map_or("cold_start", |value| value.estimator_mode.as_str()),
                ),
                PacingField::u64(
                    "prediction_kms_dispatch_budget_ns",
                    prediction.map_or(0, |value| value.kms_dispatch_budget_ns),
                ),
                PacingField::u64(
                    "prediction_kms_apply_guard_ns",
                    prediction.map_or(0, |value| value.kms_apply_guard_ns),
                ),
                PacingField::u64(
                    "prediction_kms_total_lead_ns",
                    prediction.map_or(0, |value| value.kms_total_lead_ns),
                ),
                PacingField::u64(
                    "prediction_total_cost_ns",
                    prediction.map_or(0, |value| value.total_cost_ns),
                ),
                PacingField::bool(
                    "prediction_idle_wake_guard",
                    prediction.is_some_and(|value| value.idle_wake_guard),
                ),
            ],
        )
    }
    pub(crate) fn note_fence_timestamp_quality(&mut self, quality: FenceTimestampQuality) {
        if !self.enabled {
            return;
        }
        match quality {
            FenceTimestampQuality::ExactSyncFile => self.sync_file_info_exact += 1,
            FenceTimestampQuality::ObservedApproximate => self.sync_file_info_approximate += 1,
        }
    }

    pub(crate) fn note_advisory_dispatch_slip(&mut self) {
        if self.enabled {
            self.advisory_dispatch_slips = self.advisory_dispatch_slips.saturating_add(1);
        }
    }
    pub(crate) fn summary_line(
        &self,
        compositor_trace_dropped_entries: u64,
        target_identity_reuse_after_abandonment: u64,
    ) -> String {
        let (pf50, pf95, pf99) = self.pageflip_intervals.percentiles();
        let (active_pf50, active_pf95, active_pf99) = self.active_pageflip_intervals.percentiles();
        let (cp50, cp95, cp99) = self.commit_to_present.percentiles();
        let (wake50, wake95, wake99) = self.wake_lateness.percentiles();
        let (slot50, slot95, slot99) = self.slot_hold.percentiles();
        let (ready50, ready95, ready99) = self.ready_age.percentiles();
        let (target50, target95, target99) = self.target_error.percentiles();
        let (target_signed50, target_signed95, target_signed99) =
            self.target_error_signed.percentiles();
        let (target_distance50, target_distance95, target_distance99) =
            self.target_interval_distance.percentiles();
        let (ready_wait50, ready_wait95, ready_wait99) =
            self.ready_waiting_for_target.percentiles();
        let (submit50, submit95, submit99) = self.atomic_submit.percentiles();
        let predictive_o1_terminal_total = self
            .predictive_o1_presented
            .saturating_add(self.predictive_o1_abandoned_identity)
            .saturating_add(self.predictive_o1_abandoned_generation)
            .saturating_add(self.predictive_o1_other_safe_abandonment)
            .saturating_add(self.predictive_o1_failed)
            .saturating_add(self.predictive_o1_invalid_stage_terminalized)
            .saturating_add(self.predictive_o1_current_at_shutdown);
        let predictive_o1_remainder = self
            .predictive_o1_created
            .saturating_sub(predictive_o1_terminal_total);
        pacing_line(
            "summary",
            &[
                PacingField::u64("render_ahead_attempts", self.render_ahead_attempts),
                PacingField::u64("render_ahead_successes", self.render_ahead_successes),
                PacingField::u64("wait_for_buffer_count", self.wait_for_buffer_count),
                PacingField::u64("ready_submit_count", self.ready_submit_count),
                PacingField::u64("reactive_double_frames", self.reactive_double_frames),
                PacingField::u64(
                    "reactive_double_immediate_submits",
                    self.reactive_double_immediate_submits,
                ),
                PacingField::u64(
                    "reactive_double_actual_misses",
                    self.reactive_double_actual_misses,
                ),
                PacingField::u64("advisory_dispatch_slips", self.advisory_dispatch_slips),
                PacingField::u64(
                    "predictive_render_ahead_attempts",
                    self.predictive_render_ahead_attempts,
                ),
                PacingField::u64(
                    "predictive_render_ahead_ready",
                    self.predictive_render_ahead_ready,
                ),
                PacingField::u64("predictive_ready_submits", self.predictive_ready_submits),
                PacingField::u64("predictive_ready_created", self.predictive_ready_created),
                PacingField::u64("predictive_o1_created", self.predictive_o1_created),
                PacingField::u64(
                    "predictive_o1_render_ready",
                    self.predictive_o1_render_ready,
                ),
                PacingField::u64(
                    "predictive_o1_ready_unbound",
                    self.predictive_o1_ready_unbound,
                ),
                PacingField::u64("predictive_o1_bound", self.predictive_o1_bound),
                PacingField::u64(
                    "predictive_o1_worker_queued",
                    self.predictive_o1_worker_queued,
                ),
                PacingField::u64("predictive_o1_submitted", self.predictive_o1_submitted),
                PacingField::u64("predictive_o1_presented", self.predictive_o1_presented),
                PacingField::u64(
                    "predictive_o1_invalid_stage_transitions",
                    self.predictive_o1_invalid_stage_transitions,
                ),
                PacingField::u64(
                    "predictive_o1_stale_stage_events",
                    self.predictive_o1_stale_stage_events,
                ),
                PacingField::u64(
                    "predictive_o1_abandoned_identity",
                    self.predictive_o1_abandoned_identity,
                ),
                PacingField::u64(
                    "predictive_o1_abandoned_generation",
                    self.predictive_o1_abandoned_generation,
                ),
                PacingField::u64(
                    "predictive_o1_other_safe_abandonment",
                    self.predictive_o1_other_safe_abandonment,
                ),
                PacingField::u64("predictive_o1_failed", self.predictive_o1_failed),
                PacingField::u64(
                    "predictive_o1_invalid_stage_terminalized",
                    self.predictive_o1_invalid_stage_terminalized,
                ),
                PacingField::u64(
                    "predictive_o1_current_at_shutdown",
                    self.predictive_o1_current_at_shutdown,
                ),
                PacingField::u64(
                    "predictive_o1_active_entries",
                    self.predictive_o1_lifecycle.active_entries(),
                ),
                PacingField::u64(
                    "predictive_o1_peak_entries",
                    self.predictive_o1_lifecycle.peak_entries,
                ),
                PacingField::u64("predictive_o1_terminal_total", predictive_o1_terminal_total),
                PacingField::u64("predictive_o1_terminal_remainder", predictive_o1_remainder),
                PacingField::bool(
                    "predictive_o1_terminal_reconciled",
                    self.predictive_o1_created == predictive_o1_terminal_total,
                ),
                PacingField::u64(
                    "predictive_unbound_created",
                    self.predictive_unbound_created,
                ),
                PacingField::u64("predictive_unbound_ready", self.predictive_unbound_ready),
                PacingField::u64(
                    "predictive_bound_after_predecessor_pageflip",
                    self.predictive_bound_after_predecessor_pageflip,
                ),
                PacingField::u64(
                    "predictive_bound_after_render_completion",
                    self.predictive_bound_after_render_completion,
                ),
                PacingField::u64(
                    "predictive_binding_advanced_intervals",
                    self.predictive_binding_advanced_intervals,
                ),
                PacingField::u64(
                    "predictive_unbound_abandoned_identity",
                    self.predictive_unbound_abandoned_identity,
                ),
                PacingField::u64(
                    "predictive_unbound_abandoned_generation",
                    self.predictive_unbound_abandoned_generation,
                ),
                PacingField::u64(
                    "predictive_ready_submitted",
                    self.predictive_ready_submitted,
                ),
                PacingField::u64(
                    "predictive_ready_overtaken_ready",
                    self.predictive_ready_overtaken_ready,
                ),
                PacingField::u64(
                    "predictive_ready_overtaken_worker_queued",
                    self.predictive_ready_overtaken_worker_queued,
                ),
                PacingField::u64(
                    "predictive_ready_other_safe_abandonment",
                    self.predictive_ready_other_safe_abandonment,
                ),
                PacingField::u64("predictive_ready_failed", self.predictive_ready_failed),
                PacingField::u64(
                    "predictive_ready_current_at_shutdown",
                    self.predictive_ready_current_at_shutdown,
                ),
                PacingField::u64("normal_ready_wait_count", self.normal_ready_wait_count),
                PacingField::u64("ready_pull_in_attempts", self.ready_pull_in_attempts),
                PacingField::u64("ready_pull_in_successes", self.ready_pull_in_successes),
                PacingField::u64(
                    "ready_pull_in_rejected_too_late",
                    self.ready_pull_in_rejected_too_late,
                ),
                PacingField::u64(
                    "ready_pull_in_rejected_owned",
                    self.ready_pull_in_rejected_owned,
                ),
                PacingField::u64(
                    "ready_pull_in_rejected_identity",
                    self.ready_pull_in_rejected_identity,
                ),
                PacingField::u64(
                    "ready_pull_in_advanced_intervals",
                    self.ready_pull_in_advanced_intervals,
                ),
                PacingField::u64(
                    "scheduled_normal_target_count",
                    self.scheduled_normal_target_count,
                ),
                PacingField::u64(
                    "expired_deadline_wait_count",
                    self.expired_deadline_wait_count,
                ),
                PacingField::u64(
                    "repeated_immediate_timer_wake_count",
                    self.repeated_immediate_timer_wake_count,
                ),
                PacingField::u64(
                    "multiple_deadline_owner_violation_count",
                    self.multiple_deadline_owner_violation_count,
                ),
                PacingField::u64(
                    "adaptive_triple_entries_predicted",
                    self.adaptive_triple_entries_predicted,
                ),
                PacingField::u64(
                    "adaptive_triple_entries_proven_render_miss",
                    self.adaptive_triple_entries_proven_render_miss,
                ),
                PacingField::u64(
                    "adaptive_triple_entries_proven_submit_miss",
                    self.adaptive_triple_entries_proven_submit_miss,
                ),
                PacingField::u64(
                    "adaptive_triple_entries_proven_presentation_miss",
                    self.adaptive_triple_entries_proven_presentation_miss,
                ),
                PacingField::u64("adaptive_triple_exits", self.adaptive_triple_exits),
                PacingField::u64("pipeline_wait_refresh_deadline", self.pipeline_waits[0]),
                PacingField::u64("pipeline_wait_no_free_slot", self.pipeline_waits[1]),
                PacingField::u64(
                    "pipeline_wait_prepared_frame_exists",
                    self.pipeline_waits[2],
                ),
                PacingField::u64(
                    "pipeline_wait_future_primary_depth_full",
                    self.pipeline_waits[3],
                ),
                PacingField::u64(
                    "pipeline_wait_worker_queue_occupied",
                    self.pipeline_waits[4],
                ),
                PacingField::u64(
                    "pipeline_wait_kernel_commit_pending",
                    self.pipeline_waits[5],
                ),
                PacingField::u64("pipeline_wait_render_fence", self.pipeline_waits[6]),
                PacingField::u64("pipeline_wait_direct_steady_state", self.pipeline_waits[7]),
                PacingField::u64("pipeline_wait_compatibility_path", self.pipeline_waits[8]),
                PacingField::u64(
                    "pipeline_wait_triple_capability_unavailable",
                    self.pipeline_waits[9],
                ),
                PacingField::u64("sync_file_info_exact", self.sync_file_info_exact),
                PacingField::u64(
                    "sync_file_info_approximate",
                    self.sync_file_info_approximate,
                ),
                PacingField::u64(
                    "target_identity_reuse_after_abandonment",
                    target_identity_reuse_after_abandonment,
                ),
                PacingField::u64(
                    "physical_claim_overtake_ready",
                    self.physical_claim_overtake_ready,
                ),
                PacingField::u64(
                    "physical_claim_overtake_worker_queued",
                    self.physical_claim_overtake_worker_queued,
                ),
                PacingField::u64(
                    "physical_claim_overtake_recoveries",
                    self.physical_claim_overtake_recoveries,
                ),
                PacingField::u64(
                    "physical_claim_overtake_recovery_failures",
                    self.physical_claim_overtake_recovery_failures,
                ),
                PacingField::u64(
                    "physical_claim_fatal_violations",
                    self.physical_claim_fatal_violations,
                ),
                PacingField::u64("scheduler_wakeup_lateness_p50_us", wake50),
                PacingField::u64("scheduler_wakeup_lateness_p95_us", wake95),
                PacingField::u64("scheduler_wakeup_lateness_p99_us", wake99),
                PacingField::u64("slot_hold_p50_us", slot50),
                PacingField::u64("slot_hold_p95_us", slot95),
                PacingField::u64("slot_hold_p99_us", slot99),
                PacingField::u64("ready_age_p50_us", ready50),
                PacingField::u64("ready_age_p95_us", ready95),
                PacingField::u64("ready_age_p99_us", ready99),
                PacingField::u64("target_error_p50_us", target50),
                PacingField::u64("target_error_p95_us", target95),
                PacingField::u64("target_error_p99_us", target99),
                PacingField::i64("target_error_signed_p50_us", target_signed50),
                PacingField::i64("target_error_signed_p95_us", target_signed95),
                PacingField::i64("target_error_signed_p99_us", target_signed99),
                PacingField::u64("target_interval_distance_p50", target_distance50),
                PacingField::u64("target_interval_distance_p95", target_distance95),
                PacingField::u64("target_interval_distance_p99", target_distance99),
                PacingField::u64("early_presentation_count", self.early_presentation_count),
                PacingField::u64("late_presentation_count", self.late_presentation_count),
                PacingField::u64(
                    "ready_waiting_for_target_count",
                    self.ready_waiting_for_target_count,
                ),
                PacingField::u64("ready_waiting_for_target_us_p50", ready_wait50),
                PacingField::u64("ready_waiting_for_target_us_p95", ready_wait95),
                PacingField::u64("ready_waiting_for_target_us_p99", ready_wait99),
                PacingField::u64(
                    "verbose_trace_dropped_entries",
                    self.trace
                        .as_ref()
                        .map_or(0, NativeTraceSink::dropped_entries)
                        .saturating_add(compositor_trace_dropped_entries),
                ),
                PacingField::u64("atomic_submit_p50_us", submit50),
                PacingField::u64("atomic_submit_p95_us", submit95),
                PacingField::u64("atomic_submit_p99_us", submit99),
                PacingField::u64("pageflip_interval_p50_us", pf50),
                PacingField::u64("pageflip_interval_p95_us", pf95),
                PacingField::u64("pageflip_interval_p99_us", pf99),
                PacingField::u64("active_pageflip_interval_p50_us", active_pf50),
                PacingField::u64("active_pageflip_interval_p95_us", active_pf95),
                PacingField::u64("active_pageflip_interval_p99_us", active_pf99),
                PacingField::u64("commit_to_present_p50_us", cp50),
                PacingField::u64("commit_to_present_p95_us", cp95),
                PacingField::u64("commit_to_present_p99_us", cp99),
                PacingField::u64("missed_refresh_1x", self.misses.missed_1x),
                PacingField::u64("missed_refresh_2x", self.misses.missed_2x),
                PacingField::u64("missed_refresh_3x_or_more", self.misses.missed_3x_or_more),
                PacingField::u64("idle_intervals_excluded", self.idle_intervals_excluded),
            ],
        )
    }
}
