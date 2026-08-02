use super::super::cursor_cycle::{complete_plane_delta_pageflip, complete_primary_cursor_pageflip};
use super::super::presentation_transactions::{
    commit_prepared_presented_output_transaction, complete_presented_output_transaction,
    prepare_presented_output_transaction,
};
use super::super::*;
use super::cycle_direct;
use crate::native_output::kms_worker::{KmsCursorUpdate, KmsPrimaryCursorPresentation};
use crate::native_output::presentation::plane::{
    CursorCoupling, CursorRevision, PresentedCursorState,
};

fn pageflip_identity(
    token: PageFlipToken,
    output_generation: u64,
    crtc_id: u32,
) -> crate::native_output::presentation::plane::PlanePageflipIdentity {
    crate::native_output::presentation::plane::PlanePageflipIdentity {
        bundle_id:
            crate::native_output::presentation::plane::KmsCommitBundleId::from_pageflip_token(token),
        token,
        output_generation,
        crtc_id,
    }
}

fn confirmed_primary_from_worker_job(
    job: &crate::native_output::kms_worker::KmsCommitJob,
) -> Option<ConfirmedPrimaryAssignment> {
    let transaction = job.owners.primary()?.transaction.as_ref();
    match job.kind {
        AtomicCommitKind::CompositedPrimary { .. } => match transaction.planes().primary() {
            PrimaryPlaneAssignment::CompositorFramebuffer { slot, .. } => {
                Some(ConfirmedPrimaryAssignment::Composed {
                    transaction_id: job.transaction_id,
                    token: job.token,
                    slot,
                })
            }
            PrimaryPlaneAssignment::CompatibilityFramebuffer { .. }
            | PrimaryPlaneAssignment::ClientFramebuffer { .. }
            | PrimaryPlaneAssignment::Unchanged
            | PrimaryPlaneAssignment::Disabled => None,
        },
        AtomicCommitKind::DirectPrimary { .. } => {
            let OutputTransactionContent::Direct { key, .. } = transaction.content() else {
                return None;
            };
            let PrimaryPlaneAssignment::ClientFramebuffer { framebuffer_id, .. } =
                transaction.planes().primary()
            else {
                return None;
            };
            Some(ConfirmedPrimaryAssignment::Direct {
                transaction_id: job.transaction_id,
                token: job.token,
                surface_id: transaction.obligations().direct_surface_id()?,
                key,
                framebuffer_id,
            })
        }
        AtomicCommitKind::PlaneDelta { .. } => None,
    }
}

fn presented_cursor_from_worker_update(
    update: &KmsCursorUpdate,
    revision: CursorRevision,
    coupling: CursorCoupling,
    delivery: crate::native_output::presentation::plane::PresentedCursorDelivery,
    fallback: &PresentedCursorState,
) -> Option<PresentedCursorState> {
    let state = match update {
        KmsCursorUpdate::Set(state) => state.clone(),
        KmsCursorUpdate::Disable => {
            let mut hidden = *fallback;
            hidden.revision = revision;
            hidden.coupling = coupling;
            hidden.delivery = delivery;
            hidden.visible = false;
            hidden.framebuffer_id = None;
            return Some(hidden);
        }
        KmsCursorUpdate::Unchanged => {
            let mut preserved = *fallback;
            preserved.revision = revision;
            preserved.coupling = coupling;
            preserved.delivery = delivery;
            if delivery
                != crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware
            {
                preserved.visible = false;
                preserved.framebuffer_id = None;
            }
            return Some(preserved);
        }
    };
    Some(PresentedCursorState::from_atomic_with_delivery(
        revision, coupling, delivery, &state,
    ))
}

fn frozen_primary_cursor_presentation(
    presentation: KmsPrimaryCursorPresentation,
) -> Option<PresentedCursorState> {
    match presentation {
        KmsPrimaryCursorPresentation::Preserve => None,
        KmsPrimaryCursorPresentation::Promote(state) => Some(state),
    }
}

fn select_cursor_promotion(
    primary_presentation: KmsPrimaryCursorPresentation,
    cursor_plane_promotion: Option<PresentedCursorState>,
) -> Option<PresentedCursorState> {
    match primary_presentation {
        KmsPrimaryCursorPresentation::Promote(state) => Some(state),
        KmsPrimaryCursorPresentation::Preserve => cursor_plane_promotion,
    }
}

impl NativeRuntime {
    pub(super) fn wait_for_events_and_pageflips(&mut self) -> NativeResult<NativeCycleState> {
        let wakeup = self.event_loop.wait()?;
        self.check_kms_commit_worker_health()?;
        if wakeup.reasons.kms_commit_worker() || wakeup.reasons.drm() {
            // Submit acknowledgments establish physical pending ownership
            // before the DRM pageflip validation below runs.
            self.process_kms_worker_events()?;
        }
        let deferred_worker_pageflip = self.deferred_worker_pageflip.take();
        let deferred_worker_completion = self.deferred_worker_completion.take();
        let worker_timeout_pending = self.worker_timeout_pending.take();
        self.dispatch_runtime_seat_events(&wakeup)?;
        if self.session.permits_output()
            && (wakeup.reasons.drm()
                || (wakeup.reasons.timer()
                    && (self.scanout.page_flip_pending()
                        || self.atomic_commit_arbiter.atomic_commit_pending())))
        {
            NativeSessionIo::observe(self, NativeIoOperation::PageflipDrain);
        }
        let perf = self.perf;
        let Self {
            server,
            perf: _,
            kms,
            kms_backend,
            target,
            mode_label: _,
            refresh_hz,
            drm_file_generation,
            drm_timestamp_clock,
            presentation_clock,
            scanout,
            frame_renderer: _,
            input_state: _,
            cursor_preference: _,
            cursor_render_mode: _,
            atomic_cursor,
            legacy_cursor: _,
            input_devices: _,
            seat_session: _,
            session: _,
            acquire_notifier: _,
            acquire_watches,
            parked_acquire_watches: _,
            event_loop,
            drm_reactor_token: _,
            output_render_fence_token,
            kms_commit_worker,
            kms_commit_worker_transport,
            submitted_worker_ownership,
            deferred_worker_pageflip: _,
            deferred_worker_completion: _,
            worker_timeout_pending: _,
            frame_scheduler,
            atomic_commit_arbiter,
            output_transactions,
            confirmed_primary_assignment,
            presentation_deadline,
            scheduled_presentation_target,
            render_journal,
            adaptive_buffering,
            pending_proven_deadline_miss,
            effective_app_gpu_policy: _,
            last_primary_presented_at_ns,
            direct_fallback_tracker,
            last_refresh_sequence,
            last_renderable_surfaces: _,
            queued_redraw_requested: _,
            frame_index,
            known_toplevels: _,
            pending_launches: _,
            mismatched_pageflip_events,
            stale_pageflip_events,
            presentation_cadence,
            frame_pacing,
            last_acquire_ready_at_ns: _,
            resize_perf: _,
            pointer_constraint_backend: _,
            process_supervisor: _,
            shutdown,
            ..
        } = self;
        let scheduler_state_before = frame_scheduler.state();
        perf.log("native.wakeup", || {
            vec![
                NativePerfField::u64("ready_mask", u64::from(wakeup.reasons.bits())),
                NativePerfField::usize("ready_sources", wakeup.ready_sources),
                NativePerfField::u64("blocked_us", wakeup.blocked_ns / 1_000),
                NativePerfField::u64(
                    "deadline_late_us",
                    wakeup.timer_lateness_ns.unwrap_or(0) / 1_000,
                ),
                NativePerfField::str("scheduler_before", format!("{scheduler_state_before:?}")),
                NativePerfField::bool("pageflip_pending", scanout.page_flip_pending()),
            ]
        });
        if wakeup.reasons.timer() {
            let wake_lateness_ns = wakeup.timer_lateness_ns.unwrap_or(0);
            render_journal.record_wake_lateness(wake_lateness_ns);
            frame_pacing.note_wake_lateness(wake_lateness_ns);
            perf.log("native.deadline", || {
                vec![
                    NativePerfField::u64("lateness_us", wake_lateness_ns / 1_000),
                    NativePerfField::u64("scheduler_wakeup_lateness_ns", wake_lateness_ns),
                    NativePerfField::str("scheduler_state", format!("{scheduler_state_before:?}")),
                    NativePerfField::bool("pageflip_watchdog", frame_scheduler.page_flip_pending()),
                ]
            });
        }
        if wakeup.reasons.output_render_fence() {
            if let Some(token) = output_render_fence_token.take() {
                event_loop.unregister(token)?;
            }
            if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut **scanout
                && let Some(timing) = explicit
                    .sample_pending_timing(MonotonicTimestampNs::new(monotonic_now_ns()?))?
            {
                frame_pacing.note_fence_timestamp_quality(timing.quality);
                render_journal.record_render_sample(
                    timing
                        .signaled_at
                        .get()
                        .saturating_sub(timing.composite_started_at.get()),
                    timing.signaled_at,
                );
                let before = render_journal.prediction(timing.target.refresh_interval);
                let observed_miss = match timing.quality {
                    FenceTimestampQuality::ExactSyncFile
                        if timing.signaled_at > timing.target.presentation_time =>
                    {
                        Some(ProvenDeadlineMiss::ExactRender)
                    }
                    FenceTimestampQuality::ObservedApproximate
                        if approximate_observation_is_late(
                            timing.signaled_at.get(),
                            timing.target.presentation_time.get(),
                            before.p95_wake_lateness_ns,
                        ) =>
                    {
                        Some(ProvenDeadlineMiss::GuardedApproximateRender)
                    }
                    _ => None,
                };
                if let Some(miss) = observed_miss {
                    pending_proven_deadline_miss.get_or_insert(miss);
                    let prepared_frame_exists = explicit.swapchain()?.ready_slot().is_some()
                        || explicit.swapchain()?.rendering_slot().is_some();
                    let future_primary_depth = u8::from(
                        atomic_commit_arbiter
                            .pending_atomic_commit()
                            .is_some_and(|commit| commit.kind.is_primary()),
                    )
                    .saturating_add(u8::from(
                        atomic_commit_arbiter
                            .worker_queued_commit()
                            .is_some_and(|commit| commit.kind.is_primary()),
                    ))
                    .saturating_add(u8::from(prepared_frame_exists));
                    let buffering_mode_before = adaptive_buffering.mode();
                    adaptive_buffering.observe_with_pipeline(
                        before.total_cost_ns,
                        timing.target.refresh_interval,
                        Some(miss),
                        *last_refresh_sequence,
                        timing.signaled_at,
                        frame_scheduler.visual_work_queued(),
                        adaptive_buffering.capability(),
                        prepared_frame_exists,
                        future_primary_depth,
                    );
                    frame_pacing.note_adaptive_transition(
                        buffering_mode_before,
                        adaptive_buffering.mode(),
                        Some(miss),
                    );
                }
                perf.log("native.render_fence", || {
                    vec![
                        NativePerfField::u64("frame_id", timing.frame_id),
                        NativePerfField::u64("signal_ns", timing.signaled_at.get()),
                        NativePerfField::u64("target_ns", timing.target.presentation_time.get()),
                        NativePerfField::u64(
                            "render_fence_signal_latency_ns",
                            timing
                                .signaled_at
                                .get()
                                .saturating_sub(timing.composite_started_at.get()),
                        ),
                        NativePerfField::str("quality", format!("{:?}", timing.quality)),
                    ]
                });
            }
        }
        if !self.session.permits_output() {
            return Ok(NativeCycleState {
                wakeup,
                pageflip_drain_us: 0,
                pageflip_completed: false,
                completed_pageflip_token: None,
                frame_completed: false,
                frame_rendered: false,
                frame_submitted: false,
                present_us: 0,
                pageflip_pending_at_tick: false,
                tick_us: 0,
                accepted: 0,
                redraw_requested: false,
                skipped_input_repaints: 0,
                input_drain_us: 0,
                raw_input_events: 0,
                coalesced_input_events: 0,
                shutdown_requested: false,
            });
        }
        let pageflip_drain_start = Instant::now();
        let should_drain_pageflips = wakeup.reasons.drm()
            || (wakeup.reasons.timer()
                && (frame_scheduler.page_flip_pending()
                    || atomic_commit_arbiter.atomic_commit_pending()
                    || shutdown.state() == ShutdownState::Draining));
        let pageflip_drain = if should_drain_pageflips {
            scanout
                .drain_page_flip_events(kms.file().as_raw_fd(), kms_backend.effective_kind())
                .map_err(|error| {
                    native_runtime_error(
                        NativeRuntimeStage::DrainPageFlipEvents,
                        scanout.kind(),
                        target.crtc_id,
                        *frame_index,
                        error,
                    )
                })?
        } else {
            NativePageFlipDrain::default()
        };
        let pageflip_drain_us = elapsed_micros(pageflip_drain_start);
        *mismatched_pageflip_events =
            mismatched_pageflip_events.saturating_add(pageflip_drain.mismatched_events);
        *stale_pageflip_events = stale_pageflip_events.saturating_add(pageflip_drain.stale_events);
        if pageflip_drain.mismatched_events > 0 || pageflip_drain.stale_events > 0 {
            perf.log("native.pageflip_event_error", || {
                vec![
                    NativePerfField::u64("mismatched", pageflip_drain.mismatched_events),
                    NativePerfField::u64("stale", pageflip_drain.stale_events),
                    NativePerfField::u64(
                        "expected_token",
                        pageflip_drain.last_mismatch.map_or(0, |value| value.0),
                    ),
                    NativePerfField::u64(
                        "received_token",
                        pageflip_drain.last_mismatch.map_or(0, |value| value.1),
                    ),
                    NativePerfField::u64(
                        "stale_token",
                        pageflip_drain.last_stale_token.unwrap_or(0),
                    ),
                    NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
                    NativePerfField::u64("backend_generation", *drm_file_generation),
                ]
            });
        }
        let wrong_crtc_pageflip = pageflip_drain
            .completion
            .is_some_and(|event| event.crtc_id != target.crtc_id);
        if wrong_crtc_pageflip {
            *mismatched_pageflip_events = mismatched_pageflip_events.saturating_add(1);
        }
        let pageflip_event = deferred_worker_pageflip.or(pageflip_drain
            .completion
            .filter(|event| event.crtc_id == target.crtc_id));
        let (pageflip_event, atomic_completion, atomic_watchdog_kind) =
            if deferred_worker_completion.is_some() {
                (pageflip_event, deferred_worker_completion, None)
            } else {
                validate_atomic_pageflip(
                    atomic_commit_arbiter,
                    kms_backend.effective_kind(),
                    pageflip_event,
                    *drm_file_generation,
                    monotonic_now_ns()?,
                    mismatched_pageflip_events,
                    stale_pageflip_events,
                    *kms_commit_worker_transport
                        == crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker,
                )?
            };
        if let Some(kind) = atomic_watchdog_kind {
            perf.log("native.atomic_commit_watchdog", || {
                vec![
                    NativePerfField::str("kind", format!("{kind:?}")),
                    NativePerfField::u64(
                        "token",
                        atomic_commit_arbiter
                            .pending_atomic_token()
                            .map_or(0, PageFlipToken::get),
                    ),
                    NativePerfField::u64("crtc", u64::from(target.crtc_id)),
                    NativePerfField::u64("generation", *drm_file_generation),
                    NativePerfField::bool("final_drain_completed", false),
                ]
            });
            acquire_watches.shutdown(event_loop)?;
            return Err(io::Error::other(
                "native Atomic commit watchdog expired; final DRM drain found no completion",
            )
            .into());
        }
        if let Some((token, detected_at)) = worker_timeout_pending {
            let handled_at = monotonic_now_ns().unwrap_or(0);
            if pageflip_event.is_some() {
                if let Some(worker) = kms_commit_worker.as_ref()
                    && handled_at.saturating_sub(detected_at) > 2_000_000
                {
                    worker.record_main_thread_stall();
                }
            } else {
                if let Some(worker) = kms_commit_worker.as_ref() {
                    worker.record_driver_timeout_suspicion();
                }
                perf.log("native.kms_commit_worker_timeout", || {
                    vec![
                        NativePerfField::u64("token", token.get()),
                        NativePerfField::u64("detected_at_ns", detected_at),
                        NativePerfField::u64("handled_at_ns", handled_at),
                    ]
                });
                acquire_watches.shutdown(event_loop)?;
                return Err(io::Error::other(
                    "native Atomic worker pageflip timeout; DRM drain found no completion",
                )
                .into());
            }
        }
        let pageflip_completed = pageflip_event.is_some();
        let mut completed_pageflip_token = None;
        let mut frame_completed = false;
        let frame_rendered = false;
        let frame_submitted = false;
        if let Some(pageflip) = pageflip_event {
            *last_refresh_sequence = u64::from(pageflip.sequence);
            if let Some(tracker) = direct_fallback_tracker.as_mut() {
                tracker.observe_refresh(*last_refresh_sequence);
                scanout.note_direct_fallback_cycles(tracker.cycles);
            }
            completed_pageflip_token = Some(pageflip.user_data);
            let compositor_receive_ns = monotonic_now_ns()?;
            let cursor_commit = atomic_completion.is_some_and(|completion| {
                matches!(
                    completion,
                    AtomicCommitCompletion::Completed {
                        kind: AtomicCommitKind::PlaneDelta { .. },
                        ..
                    }
                )
            });
            let cursor_transaction_id = match atomic_completion {
                Some(AtomicCommitCompletion::Completed {
                    kind: AtomicCommitKind::PlaneDelta { transaction_id, .. },
                    ..
                }) => Some(transaction_id),
                _ => None,
            };
            if cursor_commit {
                // Cursor-only completion is not a complete compositor cycle.
                // Continue through protocol and input dispatch so a primary
                // producer can be observed and scheduled immediately.
                if let Some(transaction_id) = cursor_transaction_id {
                    complete_presented_output_transaction(
                        output_transactions,
                        &mut self.presentation_trace,
                        transaction_id,
                        PageFlipToken::new(pageflip.user_data)
                            .ok_or_else(|| io::Error::other("pageflip token is zero"))?,
                        *drm_file_generation,
                        MonotonicTimestampNs::new(compositor_receive_ns),
                        None,
                        |obligations| {
                            debug_assert!(obligations.frame_batch_id().is_none());
                            let _ = complete_plane_delta_pageflip(
                                atomic_cursor,
                                pageflip.user_data,
                                *drm_file_generation,
                                perf,
                            )?;
                            Ok(())
                        },
                    )?;
                } else {
                    let _ = complete_plane_delta_pageflip(
                        atomic_cursor,
                        pageflip.user_data,
                        *drm_file_generation,
                        perf,
                    )?;
                }
                if *kms_commit_worker_transport
                    != crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker
                    && let Some(cursor) = atomic_cursor.as_ref()
                {
                    let token = PageFlipToken::new(pageflip.user_data)
                        .ok_or_else(|| io::Error::other("cursor pageflip token is zero"))?;
                    let identity = pageflip_identity(token, *drm_file_generation, target.crtc_id);
                    if !self.presented_planes.promote_bundle(
                        identity,
                        identity,
                        None,
                        Some(cursor.presented_plane_state()),
                    ) {
                        return Err(io::Error::other(
                            "cursor pageflip promotion identity mismatch",
                        )
                        .into());
                    }
                }
            }
            let direct_pending = matches!(
                atomic_completion,
                Some(AtomicCommitCompletion::Completed {
                    kind: AtomicCommitKind::DirectPrimary { .. },
                    ..
                })
            );
            let completion = if cursor_commit {
                // Cursor-only Atomic commits are validated and completed by
                // the Atomic arbiter, not by the primary frame scheduler.
                PageFlipCompletionResult::Stale
            } else if let Some(AtomicCommitCompletion::Completed {
                submitted_at_ns, ..
            }) = atomic_completion
            {
                PageFlipCompletionResult::Completed { submitted_at_ns }
            } else {
                frame_scheduler.complete_kernel_pageflip(pageflip.user_data, compositor_receive_ns)
            };
            if matches!(completion, PageFlipCompletionResult::Completed { .. }) {
                if let Some(token) = pageflip_drain.deferred_promotion_token {
                    scanout
                        .promote_page_flip(PageFlipToken::new(token).ok_or_else(|| {
                            io::Error::other("pageflip promotion token is zero")
                        })?)?;
                } else if deferred_worker_completion.is_some() {
                    scanout.promote_worker_early_page_flip(
                        PageFlipToken::new(pageflip.user_data)
                            .ok_or_else(|| io::Error::other("pageflip token is zero"))?,
                    )?;
                }
            }
            if let PageFlipCompletionResult::Completed { submitted_at_ns } = completion {
                let completed_frame_id = frame_pacing.pending;
                let presentation = if direct_pending {
                    FramePresentation::synchronized_zero_copy(
                        *presentation_clock,
                        pageflip.timestamp.seconds,
                        pageflip.timestamp.microseconds,
                        pageflip.sequence,
                    )?
                } else {
                    FramePresentation::synchronized(
                        *presentation_clock,
                        pageflip.timestamp.seconds,
                        pageflip.timestamp.microseconds,
                        pageflip.sequence,
                    )?
                };
                let compositor_receive_us = sample_clock_microseconds(*drm_timestamp_clock)?;
                let kernel_timestamp_us = u64::from(pageflip.timestamp.seconds)
                    .saturating_mul(1_000_000)
                    .saturating_add(u64::from(pageflip.timestamp.microseconds));
                let receive_delay_us = compositor_receive_us.saturating_sub(kernel_timestamp_us);
                let presented_at_ns =
                    compositor_receive_ns.saturating_sub(receive_delay_us.saturating_mul(1_000));
                *last_primary_presented_at_ns = Some(presented_at_ns);
                if direct_pending {
                    let presented_at = MonotonicTimestampNs::new(presented_at_ns);
                    let actual_logical_sequence =
                        presentation_deadline.note_presented(presented_at);
                    let transaction_id = match atomic_completion {
                        Some(AtomicCommitCompletion::Completed {
                            kind: AtomicCommitKind::DirectPrimary { transaction_id, .. },
                            ..
                        }) => transaction_id,
                        _ => {
                            return Err(
                                io::Error::other("direct pageflip has no transaction").into()
                            );
                        }
                    };
                    let pageflip_token = PageFlipToken::new(pageflip.user_data)
                        .ok_or_else(|| io::Error::other("direct pageflip token is zero"))?;
                    cycle_direct::settle_direct_pageflip(
                        scanout,
                        server,
                        output_transactions,
                        &mut self.presentation_trace,
                        atomic_cursor,
                        *drm_file_generation,
                        transaction_id,
                        pageflip_token,
                        pageflip.user_data,
                        pageflip.sequence,
                        presented_at,
                        presented_at_ns,
                        actual_logical_sequence,
                        presentation,
                        confirmed_primary_assignment,
                        render_journal,
                        frame_pacing,
                        scheduled_presentation_target,
                    )?;
                    if *kms_commit_worker_transport
                        != crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker
                        && let Some(cursor) = atomic_cursor.as_ref()
                    {
                        let identity =
                            pageflip_identity(pageflip_token, *drm_file_generation, target.crtc_id);
                        if !self.presented_planes.promote_bundle(
                            identity,
                            identity,
                            *confirmed_primary_assignment,
                            Some(cursor.presented_plane_state()),
                        ) {
                            return Err(io::Error::other(
                                "direct pageflip promotion identity mismatch",
                            )
                            .into());
                        }
                    }
                } else if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut **scanout {
                    if let Some(token) = output_render_fence_token.take() {
                        event_loop.unregister(token)?;
                    }
                    let presented_at = MonotonicTimestampNs::new(presented_at_ns);
                    let actual_logical_sequence =
                        presentation_deadline.note_presented(presented_at);
                    let transaction_id = match atomic_completion {
                        Some(AtomicCommitCompletion::Completed {
                            kind: AtomicCommitKind::CompositedPrimary { transaction_id, .. },
                            ..
                        }) => transaction_id,
                        _ => {
                            return Err(
                                io::Error::other("composited pageflip has no transaction").into()
                            );
                        }
                    };
                    let pageflip_token = PageFlipToken::new(pageflip.user_data)
                        .ok_or_else(|| io::Error::other("composited pageflip token is zero"))?;
                    let previous_assignment = *confirmed_primary_assignment;
                    let CompositedPageflipCompletion {
                        presented: frame,
                        protocol_batch_id,
                        surface_damage,
                    } = explicit.complete_pageflip(pageflip_token)?;
                    let prepared_logical = prepare_presented_output_transaction(
                        output_transactions,
                        transaction_id,
                        pageflip_token,
                        *drm_file_generation,
                        presented_at,
                        Some(actual_logical_sequence),
                    )?;
                    debug_assert!(prepared_logical.obligations().direct_surface_id().is_none());
                    let direct_transition = match previous_assignment {
                        Some(ConfirmedPrimaryAssignment::Direct {
                            transaction_id,
                            token,
                            surface_id,
                            key: candidate_key,
                            framebuffer_id,
                        }) => {
                            let expected = ExpectedPresentedDirectPrimary {
                                transaction_id,
                                token,
                                surface_id,
                                candidate_key,
                                framebuffer_id,
                            };
                            let worker_content_keys = kms_commit_worker
                                .as_ref()
                                .map(|worker| worker.direct_content_keys())
                                .unwrap_or((None, None, None));
                            if let Err(reason) = explicit
                                .validate_composited_transition(expected, worker_content_keys)
                            {
                                output_transactions
                                    .rollback_settlement(prepared_logical)
                                    .map_err(io::Error::other)?;
                                return cycle_direct::fail_composited_transition(
                                    kms_commit_worker.as_ref(),
                                    direct_fallback_tracker,
                                    scanout,
                                    frame_scheduler,
                                    atomic_commit_arbiter,
                                    reason,
                                );
                            }
                            Some((expected, worker_content_keys))
                        }
                        _ => None,
                    };
                    let completed_frame_id = frame.frame_id;
                    commit_prepared_presented_output_transaction(
                        output_transactions,
                        &mut self.presentation_trace,
                        prepared_logical,
                        |obligations| {
                            debug_assert!(obligations.direct_surface_id().is_none());
                            server.commit_surface_damage_presented(surface_damage);
                            server.complete_presented_frame_batch(
                                completed_frame_id,
                                protocol_batch_id,
                                presentation,
                            );
                            complete_primary_cursor_pageflip(
                                atomic_cursor,
                                pageflip.user_data,
                                *drm_file_generation,
                            )?;
                            Ok(())
                        },
                    )?;
                    if let Some((expected, worker_content_keys)) = direct_transition {
                        match explicit.complete_composited_transition(expected, worker_content_keys)
                        {
                            CompositedTransitionResult::Completed { .. } => {
                                debug_assert!(explicit.direct_scanout_presented_info().is_none());
                            }
                            CompositedTransitionResult::Fatal { reason } => {
                                return cycle_direct::fail_composited_transition(
                                    kms_commit_worker.as_ref(),
                                    direct_fallback_tracker,
                                    scanout,
                                    frame_scheduler,
                                    atomic_commit_arbiter,
                                    reason,
                                );
                            }
                        }
                    }
                    if let Some(mut tracker) = direct_fallback_tracker.take() {
                        tracker.observe_refresh(*last_refresh_sequence);
                        explicit.note_direct_composited_fallback(tracker.cycles);
                    }
                    *confirmed_primary_assignment = Some(ConfirmedPrimaryAssignment::Composed {
                        transaction_id,
                        token: pageflip_token,
                        slot: explicit.swapchain()?.current(),
                    });
                    if *kms_commit_worker_transport
                        != crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker
                        && let Some(cursor) = atomic_cursor.as_ref()
                    {
                        let identity =
                            pageflip_identity(pageflip_token, *drm_file_generation, target.crtc_id);
                        if !self.presented_planes.promote_bundle(
                            identity,
                            identity,
                            *confirmed_primary_assignment,
                            Some(cursor.presented_plane_state()),
                        ) {
                            return Err(io::Error::other(
                                "composited pageflip promotion identity mismatch",
                            )
                            .into());
                        }
                    }
                    render_journal.note_matching_presentation(presented_at);
                    render_journal.record_target_slip(
                        presented_at
                            .get()
                            .saturating_sub(frame.target.presentation_time.get()),
                    );
                    render_journal.record_atomic_submit(
                        frame
                            .submit_returned_at
                            .get()
                            .saturating_sub(frame.submit_started_at.get()),
                    );
                    let refresh = frame.target.refresh_interval;
                    let before_sample = render_journal.prediction(refresh);
                    let mut proven_miss = pending_proven_deadline_miss.take();
                    if let Some((signaled_at, quality)) = frame.fence_signal {
                        frame_pacing.note_fence_timestamp_quality(quality);
                        render_journal.record_render_sample(
                            render_sample_duration_ns(frame.composite_started_at, signaled_at),
                            signaled_at,
                        );
                        let target_ns = frame.target.presentation_time.get();
                        let fence_miss = match quality {
                            FenceTimestampQuality::ExactSyncFile
                                if signaled_at.get() > target_ns =>
                            {
                                Some(ProvenDeadlineMiss::ExactRender)
                            }
                            FenceTimestampQuality::ObservedApproximate
                                if approximate_observation_is_late(
                                    signaled_at.get(),
                                    target_ns,
                                    before_sample.p95_wake_lateness_ns,
                                ) =>
                            {
                                Some(ProvenDeadlineMiss::GuardedApproximateRender)
                            }
                            _ => None,
                        };
                        if let Some(miss) = fence_miss {
                            proven_miss = Some(miss);
                        }
                    }
                    if frame.submit_returned_at.get() > frame.target.presentation_time.get() {
                        proven_miss = Some(ProvenDeadlineMiss::AtomicSubmit);
                    }
                    proven_miss = merge_presentation_miss(
                        proven_miss,
                        frame.target.sequence,
                        actual_logical_sequence,
                    );
                    let prediction = render_journal.prediction(refresh);
                    let prepared_frame_exists = scanout.third_slot_owned();
                    let future_primary_depth = u8::from(
                        atomic_commit_arbiter
                            .pending_atomic_commit()
                            .is_some_and(|commit| commit.kind.is_primary()),
                    )
                    .saturating_add(u8::from(
                        atomic_commit_arbiter
                            .worker_queued_commit()
                            .is_some_and(|commit| commit.kind.is_primary()),
                    ))
                    .saturating_add(u8::from(prepared_frame_exists));
                    let buffering_mode_before = adaptive_buffering.mode();
                    adaptive_buffering.observe_with_pipeline(
                        prediction.total_cost_ns,
                        refresh,
                        proven_miss,
                        actual_logical_sequence,
                        presented_at,
                        server.has_unowned_frame_work() || frame_scheduler.visual_work_queued(),
                        adaptive_buffering.capability(),
                        prepared_frame_exists,
                        future_primary_depth,
                    );
                    frame_pacing.note_adaptive_transition(
                        buffering_mode_before,
                        adaptive_buffering.mode(),
                        proven_miss,
                    );
                    frame_pacing.note_explicit_present(ExplicitPresentationObservation {
                        planned_sequence: frame.target.sequence,
                        actual_sequence: actual_logical_sequence,
                        target_ns: frame.target.presentation_time.get(),
                        presented_ns: presented_at_ns,
                        composite_started_ns: frame.composite_started_at.get(),
                        rendered_ns: frame.rendered_at.get(),
                        submit_started_ns: frame.submit_started_at.get(),
                        submit_returned_ns: frame.submit_returned_at.get(),
                        reactive_double: frame.target.reason
                            == PresentationTargetReason::ReactiveDouble,
                    });
                    *scheduled_presentation_target = None;
                } else {
                    complete_primary_cursor_pageflip(
                        atomic_cursor,
                        pageflip.user_data,
                        *drm_file_generation,
                    )?;
                    let compatibility_transaction_id = output_transactions.submitted_transaction(
                        PageFlipToken::new(pageflip.user_data)
                            .ok_or_else(|| io::Error::other("pageflip token is zero"))?,
                        *drm_file_generation,
                    );
                    if let Some(transaction_id) = compatibility_transaction_id {
                        complete_presented_output_transaction(
                            output_transactions,
                            &mut self.presentation_trace,
                            transaction_id,
                            PageFlipToken::new(pageflip.user_data)
                                .ok_or_else(|| io::Error::other("pageflip token is zero"))?,
                            *drm_file_generation,
                            MonotonicTimestampNs::new(presented_at_ns),
                            None,
                            |obligations| {
                                let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                                    io::Error::other(
                                        "compatibility pageflip transaction has no frame batch",
                                    )
                                })?;
                                server.finish_presented_frame_batch(batch_id, presentation)?;
                                Ok(())
                            },
                        )?;
                    } else {
                        server.finish_frame_with_presentation(presentation);
                    }
                }
                frame_pacing.note_pageflip(
                    presented_at_ns,
                    submitted_at_ns,
                    pageflip.user_data,
                    1_000_000u64 / u64::from((*refresh_hz).max(1)),
                );
                let mut pacing_fields = vec![
                    frame_id_field(completed_frame_id),
                    PacingField::u64("render_generation", server.render_generation()),
                    PacingField::u64("pageflip_token", pageflip.user_data),
                    PacingField::u64("pageflip_complete_ns", presented_at_ns),
                ];
                pacing_fields.extend(snapshot_fields(scanout.buffer_snapshot()));
                frame_pacing.log("frame_complete", pacing_fields);
                let refresh_interval_us = 1_000_000u64 / u64::from((*refresh_hz).max(1));
                let cadence = presentation_cadence.record_with_refresh(
                    pageflip.sequence,
                    presented_at_ns / 1_000,
                    refresh_interval_us,
                );
                let finish_frame_start = Instant::now();
                if !server.has_unowned_frame_work() {
                    frame_scheduler.complete_protocol_only();
                }
                frame_completed = true;
                perf.log("native.finish_frame", || {
                    vec![
                        NativePerfField::str("reason", "pageflip_complete"),
                        NativePerfField::u64("elapsed_us", elapsed_micros(finish_frame_start)),
                        NativePerfField::usize("surfaces", server.renderable_surfaces().len()),
                        NativePerfField::u64("render_generation", server.render_generation()),
                        NativePerfField::u64("pageflip_token", pageflip.user_data),
                        NativePerfField::str("kms_backend", kms_backend.effective_kind().as_str()),
                        NativePerfField::u64("backend_generation", *drm_file_generation),
                        NativePerfField::u64("kernel_sequence", u64::from(pageflip.sequence)),
                        NativePerfField::u64("kernel_timestamp_us", kernel_timestamp_us),
                        NativePerfField::u64("presentations", cadence.presentations),
                        NativePerfField::u64(
                            "presentation_interval_us",
                            cadence.interval_us.unwrap_or(0),
                        ),
                        NativePerfField::u64(
                            "presentation_sequence_delta",
                            cadence.sequence_delta.map(u64::from).unwrap_or(0),
                        ),
                        NativePerfField::u64(
                            "logical_presentation_sequence",
                            cadence.logical_sequence,
                        ),
                        NativePerfField::u64(
                            "logical_presentation_sequence_delta",
                            cadence.logical_sequence_delta.unwrap_or(0),
                        ),
                        NativePerfField::bool(
                            "timestamp_sequence_fallback",
                            cadence.timestamp_sequence_fallback,
                        ),
                        NativePerfField::bool("presentation_sequence_gap", cadence.sequence_gap),
                        NativePerfField::u64(
                            "presented_hz_millihz",
                            cadence.estimated_hz_millihz.unwrap_or(0),
                        ),
                        NativePerfField::u64("presentation_sequence_gaps", cadence.sequence_gaps),
                        NativePerfField::u64("compositor_receive_us", compositor_receive_us),
                        NativePerfField::u64(
                            "receive_delay_us",
                            compositor_receive_us.saturating_sub(kernel_timestamp_us),
                        ),
                        NativePerfField::u64(
                            "submit_to_completion_us",
                            compositor_receive_ns.saturating_sub(submitted_at_ns) / 1_000,
                        ),
                        NativePerfField::str(
                            "completion_owner",
                            if atomic_completion.is_some() {
                                "atomic_arbiter"
                            } else {
                                "compatibility_scheduler"
                            },
                        ),
                    ]
                });
            }
            if *kms_commit_worker_transport
                == crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker
                && let Some(AtomicCommitCompletion::Completed { kind, .. }) = atomic_completion
            {
                let path_completed = match kind {
                    AtomicCommitKind::PlaneDelta { .. } => cursor_commit,
                    AtomicCommitKind::CompositedPrimary { .. }
                    | AtomicCommitKind::DirectPrimary { .. } => {
                        matches!(completion, PageFlipCompletionResult::Completed { .. })
                    }
                };
                if path_completed && let Some(worker) = kms_commit_worker.as_ref() {
                    let transaction_id = match kind {
                        AtomicCommitKind::CompositedPrimary { transaction_id, .. }
                        | AtomicCommitKind::DirectPrimary { transaction_id, .. }
                        | AtomicCommitKind::PlaneDelta { transaction_id, .. } => transaction_id,
                    };
                    let pageflip_token = PageFlipToken::new(pageflip.user_data)
                        .ok_or_else(|| io::Error::other("pageflip token is zero"))?;
                    let worker_promotion = submitted_worker_ownership
                        .iter()
                        .find(|ownership| ownership.job.token == pageflip_token)
                        .map(|ownership| {
                            let cursor_owner = ownership.job.owners.cursor();
                            (
                                confirmed_primary_from_worker_job(&ownership.job),
                                cursor_owner.map(|owner| {
                                    (
                                        owner.revision,
                                        owner.sidecar_id.is_some(),
                                        Some(owner.transaction.id()),
                                        ownership.job.cursor.clone(),
                                        ownership.job.cursor_delivery,
                                    )
                                }),
                                ownership.job.primary_cursor_presentation,
                                ownership.job.identity(),
                            )
                        });
                    let sidecar_transaction_id = worker_promotion
                        .as_ref()
                        .and_then(|(_, cursor, _, _)| cursor.as_ref())
                        .filter(|(_, sidecar, _, _, _)| *sidecar)
                        .and_then(|(_, _, transaction_id, _, _)| *transaction_id);
                    let worker_identity = worker_promotion
                        .as_ref()
                        .map(|(_, _, _, identity)| *identity)
                        .ok_or_else(|| {
                            io::Error::other(
                                "worker pageflip has no matching submitted bundle identity",
                            )
                        })?;
                    worker
                        .ack_pageflip_identity(worker_identity, transaction_id)
                        .map_err(|error| {
                            io::Error::other(format!("worker pageflip ack: {error:?}"))
                        })?;
                    if let Some(sidecar_transaction_id) = sidecar_transaction_id {
                        complete_presented_output_transaction(
                            output_transactions,
                            &mut self.presentation_trace,
                            sidecar_transaction_id,
                            pageflip_token,
                            *drm_file_generation,
                            MonotonicTimestampNs::new(compositor_receive_ns),
                            None,
                            |obligations| {
                                debug_assert!(obligations.frame_batch_id().is_none());
                                debug_assert!(obligations.direct_surface_id().is_none());
                                Ok(())
                            },
                        )?;
                    }
                    if let Some((
                        primary,
                        cursor_owner,
                        primary_cursor_presentation,
                        bundle_identity,
                    )) = worker_promotion
                    {
                        if bundle_identity.token != pageflip_token
                            || bundle_identity.output_generation != *drm_file_generation
                            || bundle_identity.crtc_id != target.crtc_id
                        {
                            return Err(io::Error::other(
                                "worker pageflip promotion bundle identity mismatch",
                            )
                            .into());
                        }
                        let identity =
                            crate::native_output::presentation::plane::PlanePageflipIdentity {
                                bundle_id: bundle_identity.id,
                                token: bundle_identity.token,
                                output_generation: bundle_identity.output_generation,
                                crtc_id: bundle_identity.crtc_id,
                            };
                        let cursor_plane_promotion = cursor_owner
                            .and_then(|(revision, sidecar, _, update, delivery)| {
                                let coupling = if delivery
                                    == crate::native_output::presentation::plane::PresentedCursorDelivery::Software
                                {
                                    CursorCoupling::EmbeddedInPrimary
                                } else if !matches!(
                                    &update,
                                    KmsCursorUpdate::Set(state) if state.visible
                                ) {
                                    CursorCoupling::Hidden
                                } else if sidecar {
                                    CursorCoupling::IndependentPlane
                                } else {
                                    CursorCoupling::EmbeddedInPrimary
                                };
                                presented_cursor_from_worker_update(
                                    &update,
                                    revision,
                                    coupling,
                                    delivery,
                                    &self.presented_planes.cursor,
                                )
                            });
                        let cursor = select_cursor_promotion(
                            primary_cursor_presentation,
                            cursor_plane_promotion.or_else(|| {
                                frozen_primary_cursor_presentation(primary_cursor_presentation)
                            }),
                        );
                        if (primary.is_some() || cursor.is_some())
                            && !self
                                .presented_planes
                                .promote_bundle(identity, identity, primary, cursor)
                        {
                            return Err(io::Error::other(
                                "worker pageflip promotion identity mismatch",
                            )
                            .into());
                        }
                        if let Some(worker) = kms_commit_worker.as_ref() {
                            worker.set_established_presented_base(
                                self.presented_planes.revision,
                                *drm_file_generation,
                                target.crtc_id,
                            );
                        }
                    }
                    submitted_worker_ownership
                        .retain(|ownership| ownership.job.token != pageflip_token);
                }
            }
        }
        Ok(NativeCycleState {
            wakeup,
            pageflip_drain_us,
            pageflip_completed,
            completed_pageflip_token,
            frame_completed,
            frame_rendered,
            frame_submitted,
            present_us: 0,
            pageflip_pending_at_tick: false,
            tick_us: 0,
            accepted: 0,
            redraw_requested: false,
            skipped_input_repaints: 0,
            input_drain_us: 0,
            raw_input_events: 0,
            coalesced_input_events: 0,
            shutdown_requested: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_output::kms_worker::KmsPrimaryCursorPresentation;
    use crate::native_output::presentation::plane::{
        CursorCoupling, CursorPlanePoint, CursorRevision, PresentedCursorDelivery,
        PresentedCursorState,
    };

    #[test]
    fn primary_software_presentation_wins_over_disabled_cursor_owner() {
        let software = PresentedCursorState {
            revision: CursorRevision::initial().advance_image(),
            coupling: CursorCoupling::EmbeddedInPrimary,
            delivery: PresentedCursorDelivery::Software,
            framebuffer_id: None,
            visible: true,
            output_position: CursorPlanePoint { x: 200, y: 300 },
            hotspot: CursorPlanePoint { x: 4, y: 5 },
        };
        let old_hardware = PresentedCursorState {
            revision: CursorRevision::initial(),
            coupling: CursorCoupling::IndependentPlane,
            delivery: PresentedCursorDelivery::Hardware,
            framebuffer_id: Some(91),
            visible: true,
            output_position: CursorPlanePoint { x: 10, y: 20 },
            hotspot: CursorPlanePoint { x: 1, y: 2 },
        };

        assert_eq!(
            select_cursor_promotion(
                KmsPrimaryCursorPresentation::Promote(software),
                Some(old_hardware),
            ),
            Some(software)
        );
    }

    #[test]
    fn primary_pageflip_uses_frozen_cursor_presentation_metadata() {
        let frozen_state = AtomicCursorVisualState::hidden(64, 64);
        let frozen = PresentedCursorState::from_atomic_with_delivery(
            CursorRevision::initial().advance_image(),
            CursorCoupling::EmbeddedInPrimary,
            crate::native_output::presentation::plane::PresentedCursorDelivery::Software,
            &frozen_state,
        );
        let expected = frozen;

        assert_eq!(
            frozen_primary_cursor_presentation(KmsPrimaryCursorPresentation::Promote(frozen)),
            Some(expected)
        );
    }

    #[test]
    fn preserved_primary_cursor_does_not_fabricate_a_new_presentation() {
        assert_eq!(
            frozen_primary_cursor_presentation(KmsPrimaryCursorPresentation::Preserve),
            None
        );
    }

    #[test]
    fn software_primary_metadata_freezes_revision_before_desired_advances() {
        let mut cursor = crate::native_output::output::test_cursor_for_worker();
        cursor.set_position(11, 22);
        let frozen_state = cursor.desired().clone();
        let frozen_revision = cursor.desired_revision();
        let metadata =
            crate::native_output::runtime::presentation_cursor::freeze_primary_cursor_presentation(
                crate::native_output::presentation::plane::PresentedCursorDelivery::Hidden,
                crate::native_output::presentation::plane::PresentedCursorDelivery::Software,
                Some(&frozen_state),
                Some(&cursor),
                7,
            );

        cursor.set_position(900, 901);
        let KmsPrimaryCursorPresentation::Promote(frozen) = metadata else {
            panic!("software primary must carry frozen cursor metadata");
        };
        assert_eq!(frozen.revision, frozen_revision);
        assert_eq!(frozen.output_position.x, 11);
        assert_eq!(frozen.output_position.y, 22);
        assert_eq!(frozen.delivery, PresentedCursorDelivery::Software);
    }
}
