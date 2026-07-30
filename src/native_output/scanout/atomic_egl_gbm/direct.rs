use super::*;
use crate::native_output::kms_worker::KmsTestOnlyPolicy;

#[allow(clippy::too_many_arguments)]
fn settle_no_visual_change_transaction(
    scanout: &mut AtomicEglGbmScanout,
    server: &mut OwnCompositorServer,
    output_transactions: &mut OutputTransactionLedger,
    output_generation: u64,
    target: PresentationTarget,
    pacing_mode: NativeOutputPacingMode,
    key: DirectScanoutCandidateKey,
    framebuffer_id: u32,
    cursor: Option<&AtomicCursorVisualState>,
    cursor_epoch: u64,
    direct_surface_id: u32,
    release: OutputReleasePlan,
) -> io::Result<()> {
    let Some(frame_id) = server.prepared_frame_id() else {
        return Ok(());
    };
    let frame_batch_id = server.take_frame_batch_for_render(frame_id);
    let created_at = match monotonic_now_ns() {
        Ok(now) => MonotonicTimestampNs::new(now),
        Err(error) => {
            server.restore_frame_batch_after_render_failure(frame_batch_id);
            return Err(error);
        }
    };
    let transaction_id = match output_transactions.allocate_id() {
        Ok(transaction_id) => transaction_id,
        Err(error) => {
            server.restore_frame_batch_after_render_failure(frame_batch_id);
            return Err(io::Error::other(error));
        }
    };
    let transaction = match OutputTransaction::direct(
        transaction_id,
        output_generation,
        created_at,
        target,
        pacing_mode,
        frame_id,
        key,
        framebuffer_id,
        cursor.map(|state| CursorPlaneAssignment::Atomic {
            desired_epoch: cursor_epoch,
            framebuffer_id: state.framebuffer_id,
            visible: state.visible,
        }),
        frame_batch_id,
        direct_surface_id,
        release,
    ) {
        Ok(transaction) => transaction,
        Err(error) => {
            server.restore_frame_batch_after_render_failure(frame_batch_id);
            return Err(io::Error::other(error));
        }
    };
    if let Err(error) = output_transactions.insert(transaction) {
        server.restore_frame_batch_after_render_failure(frame_batch_id);
        return Err(io::Error::other(error));
    }
    let obligations = output_transactions
        .transaction(transaction_id)
        .expect("direct no-visual-change transaction was just inserted")
        .descriptor()
        .obligations();
    let callback_owner_leaks = direct_terminal_callback_owner_leaks(
        server,
        transaction_id,
        obligations,
        DirectTerminalCallbackDisposition::NoVisualChange,
    );
    settle_no_visual_change_output_transaction(
        output_transactions,
        transaction_id,
        created_at,
        |obligations| {
            let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                io::Error::other("no-visual-change transaction has no frame batch")
            })?;
            debug_assert_eq!(batch_id, frame_batch_id);
            server.complete_no_visual_change_frame_batch(batch_id);
            Ok(())
        },
    )
    .map_err(|error| io::Error::other(error.to_string()))?;
    scanout.note_direct_callback_owner_leaks(callback_owner_leaks);
    if callback_owner_leaks.leak_events > 0 {
        direct_scanout_debug(format_args!(
            "direct no-visual-change callback-owner leak transaction={} events={} callbacks={}",
            transaction_id.get(),
            callback_owner_leaks.leak_events,
            callback_owner_leaks.leaked_callbacks,
        ));
    }
    Ok(())
}

impl AtomicEglGbmScanout {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn try_direct_scanout(
        &mut self,
        kms: &KmsBackendSelection,
        server: &mut OwnCompositorServer,
        output_transactions: &mut OutputTransactionLedger,
        target: PresentationTarget,
        cursor: Option<&AtomicCursorVisualState>,
        cursor_epoch: u64,
        pacing_mode: NativeOutputPacingMode,
        worker: Option<&crate::native_output::kms_worker::KmsCommitWorkerHandle>,
    ) -> io::Result<DirectScanoutAttempt> {
        self.direct.counters.candidate_checks += 1;
        let sync_readiness = DirectSyncReadiness::from_capabilities(
            // direct_scanout_scene_candidate() only exposes published
            // attachments, which means unresolved acquire work has already
            // been withheld by compositor publication ordering.
            true,
            true,
            kms.atomic().is_some(),
            kms.atomic()
                .is_some_and(|atomic| atomic.discovery().optional.in_fence_fd),
            kms.atomic()
                .is_some_and(|atomic| atomic.discovery().optional.out_fence_ptr),
            false,
        );
        match &sync_readiness {
            DirectSyncReadiness::Qualified {
                in_fence,
                release_mode,
            } => {
                debug_assert!(in_fence.is_none());
                direct_scanout_debug(format_args!(
                    "synchronization qualified release_mode={release_mode:?}"
                ));
            }
            DirectSyncReadiness::Unsupported(reason) => {
                direct_scanout_debug(format_args!("synchronization rejected: {reason}"));
                return Ok(DirectScanoutAttempt::Fallback(reason));
            }
        }
        let candidate = match server.direct_scanout_scene_candidate() {
            Ok(candidate) => {
                let debug_key = (
                    candidate.surface_id,
                    candidate.buffer_identity.id().get(),
                    candidate.generation,
                    candidate.commit_sequence.get(),
                );
                if self.direct.last_debug_candidate != Some(debug_key) {
                    direct_scanout_debug(format_args!(
                        "candidate surface={} buffer={} generation={} commit={}",
                        candidate.surface_id,
                        candidate.buffer_identity.id().get(),
                        candidate.generation,
                        candidate.commit_sequence.get(),
                    ));
                    self.direct.last_debug_candidate = Some(debug_key);
                }
                candidate
            }
            Err(rejection) => {
                direct_scanout_debug(format_args!("candidate rejected={}", rejection.as_str()));
                return Ok(DirectScanoutAttempt::Rejected(rejection));
            }
        };
        self.direct.counters.candidates_accepted += 1;
        let Some(candidate_key) =
            direct_candidate_key(&candidate, self.direct.drm_generation, cursor)
        else {
            return Ok(DirectScanoutAttempt::Fallback("candidate_key_invalid"));
        };
        let release = match &sync_readiness {
            DirectSyncReadiness::Qualified { release_mode, .. } => match release_mode {
                DirectReleaseMode::Pageflip => OutputReleasePlan::Pageflip,
                DirectReleaseMode::OutFence => OutputReleasePlan::OutFenceThenPageflip,
            },
            DirectSyncReadiness::Unsupported(_) => unreachable!("checked above"),
        };
        let presented_key = self
            .direct
            .ownership
            .presented
            .as_ref()
            .map(|frame| frame.lease.key());
        let submitted_key = self
            .direct
            .ownership
            .submitted
            .as_ref()
            .map(|frame| frame.lease.key());
        let disposition = classify_direct_content(candidate_key, presented_key, submitted_key);
        if disposition != DirectContentDisposition::NewContent {
            self.direct.counters.same_buffer_suppressed = self
                .direct
                .counters
                .same_buffer_suppressed
                .saturating_add(1);
            settle_no_visual_change_transaction(
                self,
                server,
                output_transactions,
                self.direct.drm_generation,
                target,
                pacing_mode,
                candidate_key,
                0,
                cursor,
                cursor_epoch,
                candidate.surface_id,
                release,
            )?;
            return Ok(DirectScanoutAttempt::Unchanged);
        }
        if candidate.buffer.planes().is_empty() {
            return Ok(DirectScanoutAttempt::Fallback("candidate_plane_missing"));
        }
        let Some(worker) = worker else {
            self.note_direct_worker_admission_rejected(false);
            return Ok(DirectScanoutAttempt::Fallback("worker_unavailable"));
        };
        let atomic = kms
            .atomic()
            .expect("qualified direct scanout requires an Atomic backend");
        let release_mode = match &sync_readiness {
            DirectSyncReadiness::Qualified { release_mode, .. } => *release_mode,
            DirectSyncReadiness::Unsupported(_) => unreachable!("checked above"),
        };
        let validation_key = DirectPlaneValidationKey {
            output_generation: self.direct.drm_generation,
            crtc_id: atomic.discovery().pipeline.crtc.get(),
            primary_plane_id: atomic.discovery().pipeline.plane.get(),
            mode_width: self.width,
            mode_height: self.height,
            format: candidate.buffer.format().as_fourcc(),
            modifier: candidate.buffer.planes()[0].descriptor().modifier.0,
            buffer_width: candidate.buffer.size().width,
            buffer_height: candidate.buffer.size().height,
            plane_layout_hash: plane_layout_hash(&candidate.buffer),
            cursor_atomic_key: direct_cursor_atomic_validation_key(
                cursor,
                true,
                atomic
                    .discovery()
                    .cursor_plane
                    .as_ref()
                    .map(|plane| plane.plane_id),
            ),
            synchronization_key: synchronization_contract_key(
                matches!(
                    &sync_readiness,
                    DirectSyncReadiness::Qualified {
                        in_fence: Some(_),
                        ..
                    }
                ),
                matches!(release_mode, DirectReleaseMode::OutFence),
                match release_mode {
                    DirectReleaseMode::Pageflip => DirectValidationReleaseMode::Pageflip,
                    DirectReleaseMode::OutFence => DirectValidationReleaseMode::OutFence,
                },
            ),
        };
        let test_only = if self.direct.validation_cache.contains(validation_key) {
            self.direct.counters.validation_cache_hits =
                self.direct.counters.validation_cache_hits.saturating_add(1);
            KmsTestOnlyPolicy::Skip
        } else {
            self.direct.counters.validation_cache_misses = self
                .direct
                .counters
                .validation_cache_misses
                .saturating_add(1);
            KmsTestOnlyPolicy::Required
        };
        if candidate.viewport_identity_metadata_present
            && !self.direct.identity_viewport_metadata_logged
        {
            direct_scanout_debug(format_args!(
                "accepted identity viewport metadata surface={} buffer={}x{} output={}x{}",
                candidate.surface_id,
                candidate.buffer_size.width,
                candidate.buffer_size.height,
                candidate.output_size.width,
                candidate.output_size.height,
            ));
            self.direct.identity_viewport_metadata_logged = true;
        }
        self.direct.counters.import_attempts += 1;
        let (framebuffer, cache_hit) = match self
            .direct
            .framebuffer_cache
            .get_or_import(&candidate.buffer_identity, &candidate.buffer)
        {
            Ok(imported) => imported,
            Err(error) => {
                self.direct.counters.import_failures += 1;
                eprintln!("direct scanout: dma-buf import rejected: {error}");
                return Ok(DirectScanoutAttempt::Fallback("import_failed"));
            }
        };
        if cache_hit {
            self.direct.counters.import_cache_hits += 1;
        }
        direct_scanout_debug(if cache_hit {
            "import cache hit".to_string()
        } else {
            "imported dma-buf framebuffer".to_string()
        });

        let frame_id = self.swapchain()?.next_frame_id();
        let protocol_batch_id = server.take_frame_batch_for_render(frame_id);
        let surface_damage =
            server.capture_surface_damage_presentation_for_surface(candidate.surface_id);
        let transaction_id = match output_transactions.allocate_id() {
            Ok(transaction_id) => transaction_id,
            Err(error) => {
                server.restore_frame_batch_after_render_failure(protocol_batch_id);
                drop(surface_damage);
                return Err(io::Error::other(error));
            }
        };
        let transaction = match OutputTransaction::direct(
            transaction_id,
            self.direct.drm_generation,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            target,
            pacing_mode,
            frame_id,
            candidate_key,
            framebuffer.framebuffer.get(),
            cursor.map(|state| CursorPlaneAssignment::Atomic {
                desired_epoch: cursor_epoch,
                framebuffer_id: state.framebuffer_id,
                visible: state.visible,
            }),
            protocol_batch_id,
            candidate.surface_id,
            release,
        ) {
            Ok(transaction) => transaction,
            Err(error) => {
                server.restore_frame_batch_after_render_failure(protocol_batch_id);
                drop(surface_damage);
                return Err(io::Error::other(error));
            }
        };
        if let Err(error) = output_transactions.insert(transaction) {
            server.restore_frame_batch_after_render_failure(protocol_batch_id);
            drop(surface_damage);
            return Err(io::Error::other(error));
        }
        let obligations = output_transactions
            .transaction(transaction_id)
            .expect("direct transaction was just inserted")
            .descriptor()
            .obligations();
        let admission = match worker.try_reserve_direct_admission(candidate_key) {
            Ok(admission) => admission,
            Err(crate::native_output::kms_worker::KmsWorkerAdmissionError::DuplicateCandidate) => {
                self.note_direct_worker_admission_rejected(false);
                self.note_direct_same_buffer_resubmission();
                let callback_owner_leaks = direct_terminal_callback_owner_leaks(
                    server,
                    transaction_id,
                    obligations,
                    DirectTerminalCallbackDisposition::NoVisualChange,
                );
                settle_no_visual_change_output_transaction(
                    output_transactions,
                    transaction_id,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("duplicate direct transaction has no frame batch")
                        })?;
                        server.complete_no_visual_change_frame_batch(batch_id);
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                self.note_direct_callback_owner_leaks(callback_owner_leaks);
                return Ok(DirectScanoutAttempt::Unchanged);
            }
            Err(error) => {
                self.note_direct_worker_admission_rejected(matches!(
                    error,
                    crate::native_output::kms_worker::KmsWorkerAdmissionError::QueueFull
                ));
                let callback_owner_leaks = direct_terminal_callback_owner_leaks(
                    server,
                    transaction_id,
                    obligations,
                    DirectTerminalCallbackDisposition::Retryable,
                );
                settle_failed_output_transaction(
                    output_transactions,
                    transaction_id,
                    OutputTransactionFailureStage::KmsSubmit,
                    MonotonicTimestampNs::new(monotonic_now_ns()?),
                    |obligations| {
                        let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                            io::Error::other("rejected direct transaction has no frame batch")
                        })?;
                        server.restore_frame_batch_after_render_failure(batch_id);
                        Ok(())
                    },
                )
                .map_err(|error| io::Error::other(error.to_string()))?;
                self.note_direct_callback_owner_leaks(callback_owner_leaks);
                return Ok(DirectScanoutAttempt::AdmissionRejected {
                    transaction_id,
                    reason: error,
                });
            }
        };
        let token = PageFlipToken::new(allocate_native_page_flip_token())
            .expect("allocated native pageflip token is nonzero");
        let framebuffer_id = framebuffer.framebuffer.get();
        let direct_lease = DirectPrimaryLease::new(
            candidate,
            candidate_key,
            validation_key,
            framebuffer,
            surface_damage,
            std::sync::Arc::clone(&self.direct.live_lease_count),
        );
        debug_assert_eq!(direct_lease.key(), candidate_key);
        debug_assert_eq!(direct_lease.surface_id(), candidate_key.content.surface_id);
        debug_assert_eq!(direct_lease.framebuffer_id(), framebuffer_id);
        debug_assert_eq!(direct_lease.validation_key(), validation_key);
        self.swapchain_mut()?.advance_external_frame_id(frame_id)?;
        Ok(DirectScanoutAttempt::WorkerQueued {
            transaction_id,
            token: token.get(),
            framebuffer_id,
            lease: Box::new(direct_lease),
            admission,
            test_only,
        })
    }

    pub(crate) fn complete_direct_pageflip(
        &mut self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        presented_at: MonotonicTimestampNs,
    ) -> io::Result<DirectPageflipCompletion> {
        let prepared = self.prepare_direct_pageflip(transaction_id, token, presented_at)?;
        Ok(self.commit_prepared_direct_pageflip(prepared))
    }

    pub(crate) fn prepare_direct_pageflip(
        &self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
        presented_at: MonotonicTimestampNs,
    ) -> io::Result<PreparedDirectPageflip> {
        self.direct
            .ownership
            .prepare_pageflip(transaction_id, token, presented_at)
            .map_err(|error| error.error)
    }

    pub(crate) fn commit_prepared_direct_pageflip(
        &mut self,
        prepared: PreparedDirectPageflip,
    ) -> DirectPageflipCompletion {
        let presented_at = prepared.presented_at;
        let (
            frame_id,
            presented_transaction_id,
            presented_token,
            surface_id,
            framebuffer_id,
            candidate_key,
            protocol_batch_id,
            target,
            submit_started_at,
            submit_returned_at,
            surface_damage,
            replaced,
        ) = {
            let (replaced, surface_damage) =
                self.direct.ownership.commit_prepared_pageflip(prepared);
            let presented = self
                .direct
                .ownership
                .presented
                .as_ref()
                .expect("presented ownership was just installed");
            (
                presented.frame_id,
                presented.transaction_id,
                presented.token,
                presented.lease.surface_id(),
                presented.lease.framebuffer_id(),
                presented.lease.key(),
                presented.protocol_batch_id,
                presented.target,
                presented.submit_started_at,
                presented.submit_returned_at,
                surface_damage,
                replaced,
            )
        };
        direct_scanout_debug("direct pageflip presented");
        DirectPageflipCompletion {
            frame_id,
            transaction_id: presented_transaction_id,
            token: presented_token,
            surface_id,
            framebuffer_id,
            candidate_key,
            protocol_batch_id,
            target,
            presented_at,
            submit_started_at,
            submit_returned_at,
            surface_damage,
            replaced,
        }
    }

    pub(crate) fn direct_pageflip_info(
        &self,
        transaction_id: OutputTransactionId,
        token: PageFlipToken,
    ) -> io::Result<DirectPageflipInfo> {
        self.direct
            .ownership
            .submitted_pageflip_info(transaction_id, token)
            .map_err(|error| error.error)
    }

    pub(crate) fn note_direct_entry(&mut self) {
        self.direct.counters.entries = self.direct.counters.entries.saturating_add(1);
    }

    pub(crate) fn note_direct_presentation(&mut self) {
        self.direct.counters.presentations = self.direct.counters.presentations.saturating_add(1);
    }

    pub(crate) fn note_direct_fallback_cycles(&mut self, cycles: u64) {
        self.direct.counters.fallback_cycles_current = cycles;
    }

    pub(crate) fn note_direct_composited_fallback(&mut self, cycles: u64) {
        self.direct.counters.fallback_cycles_current = 0;
        self.direct.counters.fallback_cycles_last = cycles;
        self.direct.counters.fallback_cycles_max =
            self.direct.counters.fallback_cycles_max.max(cycles);
        self.direct.counters.fallback_cycles = cycles;
        self.direct.counters.composited_fallbacks =
            self.direct.counters.composited_fallbacks.saturating_add(1);
    }

    pub(crate) fn note_direct_replacement(&mut self) {
        self.direct.counters.direct_replacements =
            self.direct.counters.direct_replacements.saturating_add(1);
    }

    pub(crate) fn invalidate_presented_damage_history(&mut self) {
        self.scene.invalidate_presented_damage_history();
    }

    pub(crate) fn mark_composited_submission(&mut self) {
        if self.direct.ownership.presented.is_some() {
            self.direct.inhibit_until_composited_present = true;
        }
    }

    pub(crate) fn complete_composited_transition(
        &mut self,
        expected: ExpectedPresentedDirectPrimary,
        worker_content_keys: (
            Option<DirectScanoutCandidateKey>,
            Option<DirectScanoutCandidateKey>,
            Option<DirectScanoutCandidateKey>,
        ),
    ) -> CompositedTransitionResult {
        let worker_owns_current = self.worker_owns_presented(worker_content_keys);
        let result = self
            .direct
            .complete_composited_transition(expected, worker_owns_current);
        if let CompositedTransitionResult::Completed { released: Some(_) } = &result {
            direct_scanout_debug("exited direct scanout to composition");
            self.scene.invalidate_presented_damage_history();
        }
        result
    }

    pub(crate) fn validate_composited_transition(
        &mut self,
        expected: ExpectedPresentedDirectPrimary,
        worker_content_keys: (
            Option<DirectScanoutCandidateKey>,
            Option<DirectScanoutCandidateKey>,
            Option<DirectScanoutCandidateKey>,
        ),
    ) -> Result<(), DirectReleaseViolation> {
        self.direct.validate_composited_transition(
            expected,
            self.worker_owns_presented(worker_content_keys),
        )
    }

    fn worker_owns_presented(
        &self,
        worker_content_keys: (
            Option<DirectScanoutCandidateKey>,
            Option<DirectScanoutCandidateKey>,
            Option<DirectScanoutCandidateKey>,
        ),
    ) -> bool {
        self.direct
            .ownership
            .presented
            .as_ref()
            .is_some_and(|presented| {
                worker_content_keys.0 == Some(presented.lease.key())
                    || worker_content_keys.1 == Some(presented.lease.key())
                    || worker_content_keys.2 == Some(presented.lease.key())
            })
    }

    pub(crate) fn direct_scanout_pending(&self) -> bool {
        self.direct.page_flip_pending()
    }

    pub(crate) fn direct_scanout_pending_token(&self) -> Option<PageFlipToken> {
        self.direct.pending_token()
    }

    pub(crate) fn direct_scanout_info(&self) -> Option<(u64, u32, u32, u64)> {
        self.direct
            .ownership
            .submitted
            .as_ref()
            .map(|frame| {
                (
                    frame.lease.key().content.buffer_id.get(),
                    frame.lease.framebuffer_id(),
                    frame.lease.key().content.format,
                    frame.lease.key().content.modifier,
                )
            })
            .or_else(|| {
                self.direct.ownership.presented.as_ref().map(|frame| {
                    (
                        frame.lease.key().content.buffer_id.get(),
                        frame.lease.framebuffer_id(),
                        frame.lease.key().content.format,
                        frame.lease.key().content.modifier,
                    )
                })
            })
    }

    pub(crate) fn direct_scanout_submitted_info(&self) -> Option<(u32, u64, u32, u64)> {
        self.direct.ownership.submitted.as_ref().map(|frame| {
            (
                frame.lease.surface_id(),
                frame.lease.key().content.buffer_id.get(),
                frame.lease.framebuffer_id(),
                frame.lease.key().content.content_epoch.get(),
            )
        })
    }

    pub(crate) fn direct_scanout_counters(&self) -> DirectScanoutCounters {
        let mut counters = self.direct.counters;
        counters.cleanup_failures = self.direct.framebuffer_cache.cleanup_failures();
        counters.live_leases = self
            .direct
            .live_lease_count
            .load(std::sync::atomic::Ordering::Acquire);
        counters
    }

    pub(crate) fn direct_scanout_inhibited(&self) -> bool {
        self.direct.inhibit_until_composited_present
    }

    pub(crate) fn note_composited_render_ahead_suppressed(&mut self) {
        self.direct.counters.composited_render_ahead_suppressed = self
            .direct
            .counters
            .composited_render_ahead_suppressed
            .saturating_add(1);
    }

    pub(crate) fn note_direct_rejection(&mut self, _test_only: bool, combined_cursor: bool) {
        if combined_cursor {
            self.direct.counters.combined_cursor_rejections = self
                .direct
                .counters
                .combined_cursor_rejections
                .saturating_add(1);
        }
    }

    pub(crate) fn note_direct_test_only(&mut self, duration_ns: u64, rejected: bool) {
        self.direct.counters.record_test_only(duration_ns, rejected);
    }

    pub(crate) fn note_direct_real_submit_attempt(&mut self, rejected: bool) {
        self.direct.counters.record_real_submit_attempt(rejected);
    }

    pub(crate) fn note_direct_same_buffer_resubmission(&mut self) {
        self.direct.counters.same_buffer_resubmissions = self
            .direct
            .counters
            .same_buffer_resubmissions
            .saturating_add(1);
    }

    pub(crate) fn note_direct_worker_admission_rejected(&mut self, queue_overflow: bool) {
        self.direct.counters.worker_admission_rejected = self
            .direct
            .counters
            .worker_admission_rejected
            .saturating_add(1);
        if queue_overflow {
            self.direct.counters.worker_queue_overflow =
                self.direct.counters.worker_queue_overflow.saturating_add(1);
        }
    }

    pub(crate) fn note_direct_callback_owner_leaks(&mut self, leaks: DirectCallbackLeakMetrics) {
        self.direct.counters.callback_owner_leak_events = self
            .direct
            .counters
            .callback_owner_leak_events
            .saturating_add(leaks.leak_events);
        self.direct.counters.callback_owner_leaked_callbacks = self
            .direct
            .counters
            .callback_owner_leaked_callbacks
            .saturating_add(leaks.leaked_callbacks);
    }

    pub(crate) fn note_direct_fallback_redraw(&mut self) {
        self.direct.counters.fallback_redraws =
            self.direct.counters.fallback_redraws.saturating_add(1);
    }

    pub(crate) fn note_direct_worker_submission(
        &mut self,
        test_only_was_required: bool,
        submit_started_at: u64,
        submit_returned_at: u64,
    ) {
        let elapsed_ns = submit_returned_at.saturating_sub(submit_started_at);
        let _ = test_only_was_required;
        self.direct.counters.real_submit_timing.record(elapsed_ns);
    }

    pub(crate) fn note_direct_blocker(&mut self, reason: &str) {
        let (name, bit) = direct_blocker(reason);
        self.direct.counters.blocker_set |= bit;
        if self.direct.counters.first_blocker.is_none() {
            self.direct.counters.first_blocker = Some(name);
        }
    }

    pub(crate) fn note_direct_duplicate_feedback(&mut self) {
        self.direct.counters.duplicate_feedback =
            self.direct.counters.duplicate_feedback.saturating_add(1);
    }

    pub(crate) fn note_dmabuf_feedback_unchanged_rebuild(&mut self) {
        self.direct.counters.dmabuf_feedback_unchanged_rebuilds = self
            .direct
            .counters
            .dmabuf_feedback_unchanged_rebuilds
            .saturating_add(1);
    }

    pub(crate) fn direct_scanout_presented_info(&self) -> Option<(u32, u64, u32, u64)> {
        self.direct.ownership.presented.as_ref().map(|frame| {
            (
                frame.lease.surface_id(),
                frame.lease.key().content.buffer_id.get(),
                frame.lease.framebuffer_id(),
                frame.lease.key().content.content_epoch.get(),
            )
        })
    }

    pub(crate) fn direct_scanout_suspend(&mut self) -> io::Result<()> {
        self.direct.suspend()?;
        self.scene.invalidate_presented_damage_history();
        Ok(())
    }
}
