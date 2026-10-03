use super::*;
use crate::native_output::kms_worker::KmsTestOnlyPolicy;
use crate::native_output::presentation::plane::CursorRevision;
use crate::native_output::presentation::transaction::{
    DirectContentDisposition, DirectPresentationStateDisposition, OutputPresentationStateKey,
    OutputProtocolObligations, OutputTransactionBuildError, classify_direct_content,
    classify_direct_presentation_state,
};
use crate::native_output::runtime::{
    discard_presentation_feedback_obligation, restore_presentation_feedback_obligation,
    take_client_cursor_presentation_feedback_batch,
};
use crate::native_output::scanout::direct_validation::first_qualified_direct_presentation_state;

struct DirectCandidatePresentationFlow {
    same_visual_assignment: bool,
    confirmed_state: OutputPresentationStateKey,
    requested_state: OutputPresentationStateKey,
    initial_state_disposition: Option<DirectPresentationStateDisposition>,
}

enum DirectPresentationQualification<F> {
    DeferUntilPageflip,
    PresentationRejected,
    AlreadyRepresented {
        state: OutputPresentationStateKey,
    },
    TransactionRequired {
        framebuffer: F,
        state: OutputPresentationStateKey,
        state_disposition: DirectPresentationStateDisposition,
    },
}

impl DirectCandidatePresentationFlow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        candidate_key: DirectScanoutCandidateKey,
        presented_key: Option<DirectScanoutCandidateKey>,
        pending_key: Option<DirectScanoutCandidateKey>,
        confirmed_state: OutputPresentationStateKey,
        requested_state: OutputPresentationStateKey,
        pending_state: Option<OutputPresentationStateKey>,
        has_submitted_direct_assignment: bool,
    ) -> Self {
        let content_disposition =
            classify_direct_content(candidate_key, presented_key, pending_key);
        let same_visual_assignment = content_disposition != DirectContentDisposition::NewContent;
        let initial_state_disposition = same_visual_assignment.then(|| {
            if (content_disposition == DirectContentDisposition::MatchesQueuedOrSubmitted
                || has_submitted_direct_assignment)
                && pending_state.is_none()
            {
                DirectPresentationStateDisposition::DeferUntilPageflip
            } else {
                classify_direct_presentation_state(requested_state, confirmed_state, pending_state)
            }
        });

        Self {
            same_visual_assignment,
            confirmed_state,
            requested_state,
            initial_state_disposition,
        }
    }

    fn same_visual_assignment(&self) -> bool {
        self.same_visual_assignment
    }

    fn initial_state_disposition(&self) -> Option<DirectPresentationStateDisposition> {
        self.initial_state_disposition
    }

    fn qualify<F, E>(
        &self,
        import_framebuffer: impl FnOnce() -> Result<F, E>,
        mut qualifies: impl FnMut(&F, OutputPresentationMode) -> bool,
    ) -> Result<DirectPresentationQualification<F>, E> {
        if self.initial_state_disposition
            == Some(DirectPresentationStateDisposition::DeferUntilPageflip)
        {
            return Ok(DirectPresentationQualification::DeferUntilPageflip);
        }

        let framebuffer = import_framebuffer()?;
        let Some((state, state_disposition)) = first_qualified_direct_presentation_state(
            self.requested_state.mode,
            self.requested_state.content_type,
            self.requested_state.output_generation,
            self.confirmed_state,
            |mode| qualifies(&framebuffer, mode),
        ) else {
            return Ok(DirectPresentationQualification::PresentationRejected);
        };

        if self.same_visual_assignment
            && state_disposition == DirectPresentationStateDisposition::AlreadyRepresented
        {
            Ok(DirectPresentationQualification::AlreadyRepresented { state })
        } else {
            Ok(DirectPresentationQualification::TransactionRequired {
                framebuffer,
                state,
                state_disposition,
            })
        }
    }
}

#[derive(Debug)]
enum DirectTransactionInsertError {
    Insert(OutputTransactionError),
}

fn insert_direct_transaction_and_reserve<A>(
    output_transactions: &mut OutputTransactionLedger,
    transaction: OutputTransaction,
    candidate_key: DirectScanoutCandidateKey,
    reserve: impl FnOnce(
        DirectScanoutCandidateKey,
    ) -> Result<A, crate::native_output::kms_worker::KmsWorkerAdmissionError>,
) -> Result<
    (
        Result<A, crate::native_output::kms_worker::KmsWorkerAdmissionError>,
        OutputProtocolObligations,
    ),
    DirectTransactionInsertError,
> {
    let transaction_id = transaction.id();
    output_transactions
        .insert(transaction)
        .map_err(DirectTransactionInsertError::Insert)?;
    let obligations = output_transactions
        .transaction(transaction_id)
        .expect("direct transaction was just inserted")
        .descriptor()
        .obligations();
    Ok((reserve(candidate_key), obligations))
}

#[allow(clippy::too_many_arguments)]
fn build_direct_output_transaction(
    output_id: OutputId,
    transaction_id: OutputTransactionId,
    output_generation: u64,
    created_at: MonotonicTimestampNs,
    target: PresentationTarget,
    pacing_mode: NativeOutputPacingMode,
    frame_id: u64,
    candidate_key: DirectScanoutCandidateKey,
    framebuffer_id: u32,
    cursor_assignment: Option<CursorPlaneAssignment>,
    protocol_batch_id: oblivion_one::compositor::CompositorFrameBatchId,
    direct_surface_id: u32,
    release: OutputReleasePlan,
    presentation_mode: OutputPresentationMode,
    content_type: DrmContentType,
) -> Result<OutputTransaction, OutputTransactionBuildError> {
    OutputTransaction::direct(
        output_id,
        transaction_id,
        output_generation,
        created_at,
        target,
        pacing_mode,
        frame_id,
        candidate_key,
        framebuffer_id,
        cursor_assignment,
        protocol_batch_id,
        direct_surface_id,
        release,
    )
    .and_then(|transaction| transaction.with_presentation_state(presentation_mode, content_type))
}

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
    presentation_mode: OutputPresentationMode,
    content_type: DrmContentType,
) -> io::Result<bool> {
    let Some(frame_id) = server.prepared_frame_id() else {
        return Ok(true);
    };
    let frame_batch_id =
        server.take_frame_batch_for_render_with_presentation_samples(frame_id, std::iter::empty());
    if !server.commit_timing_submission_is_safe_for_batch(
        frame_batch_id,
        target.presentation_time,
        target.clock_generation,
    ) {
        server.restore_frame_batch_after_render_failure(frame_batch_id);
        return Ok(false);
    }
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
        scanout.direct.output_id,
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
            state: Some(state.clone()),
        }),
        frame_batch_id,
        direct_surface_id,
        release,
    )
    .and_then(|transaction| transaction.with_presentation_state(presentation_mode, content_type))
    {
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
    Ok(true)
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
        cursor_source_key: Option<NativeCursorImageKey>,
        cursor_revision: Option<CursorRevision>,
        cursor_epoch: u64,
        pacing_mode: NativeOutputPacingMode,
        confirmed_presentation_state: OutputPresentationStateKey,
        worker: Option<&crate::native_output::kms_worker::KmsCommitWorkerHandle>,
        vrr_policy: VrrPolicy,
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
        let effective_presentation = EffectivePresentation::decide(
            TearingPolicy::from_environment(std::env::var("OBLIVION_ONE_TEARING").ok().as_deref()),
            vrr_policy,
            candidate.presentation,
            AsyncEligibility {
                solitary_fullscreen: server
                    .direct_scanout_solitary_fullscreen(candidate.root_surface_id),
                async_hint: candidate.presentation.hint.is_async(),
                backend_capable: kms
                    .atomic()
                    .is_some_and(|atomic| atomic.discovery().optional.async_page_flip),
                async_format_supported: candidate
                    .buffer
                    .planes()
                    .first()
                    .zip(kms.atomic())
                    .is_some_and(|(plane, atomic)| {
                        atomic.discovery().plane_async_scanout_formats.contains(
                            &oblivion_one::native::kms::DrmFormatModifierPair {
                                fourcc: candidate.buffer.format().as_fourcc(),
                                modifier: plane.descriptor().modifier.0,
                            },
                        )
                    }),
                output_generation_qualified: candidate.generation == self.direct.drm_generation,
                explicit_sync_ready: matches!(
                    &sync_readiness,
                    DirectSyncReadiness::Qualified { .. }
                ),
                commit_timing_safe: self.direct.ownership.submitted.is_none(),
                kms_lane_free: self.direct.ownership.submitted.is_none(),
                async_test_only_accepted: kms.async_page_flip_capable(),
                cursor_visible: cursor.is_some_and(|state| state.visible),
                modeset_required: kms
                    .resolved_content_type(candidate.presentation.content_type.drm_value())
                    != confirmed_presentation_state.content_type,
                ..AsyncEligibility::default()
            },
            VrrEligibility {
                auto_candidate: server
                    .direct_scanout_solitary_fullscreen(candidate.root_surface_id),
                backend_capable: kms.effective_kind()
                    == oblivion_one::native::kms::KmsBackendKind::Atomic,
                connector_capable: kms.atomic_connector_vrr_capable(),
                crtc_property_available: kms.atomic_crtc_vrr_property_available(),
                output_generation_qualified: candidate.generation == self.direct.drm_generation,
                exact_kms_qualified: true,
                transition_supported: true,
            },
        );
        let mut presentation_mode = effective_presentation.mode;
        let content_type =
            kms.resolved_content_type(effective_presentation.content_type.drm_value());
        self.direct.counters.candidates_accepted += 1;
        let Some(candidate_key) = direct_candidate_key(
            &candidate,
            self.direct.output_id,
            self.direct.drm_generation,
            cursor,
        ) else {
            return Ok(DirectScanoutAttempt::Fallback("candidate_key_invalid"));
        };
        let release = match &sync_readiness {
            DirectSyncReadiness::Qualified { release_mode, .. } => match release_mode {
                DirectReleaseMode::Pageflip => OutputReleasePlan::Pageflip,
                DirectReleaseMode::OutFence => OutputReleasePlan::OutFenceThenPageflip,
            },
            DirectSyncReadiness::Unsupported(_) => unreachable!("checked above"),
        };
        if output_transactions.has_queued_direct_candidate(candidate_key) {
            return Ok(DirectScanoutAttempt::TimingDeferred);
        }
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
        let pending_direct_state = output_transactions
            .pending_direct_presentation_state(candidate_key)
            .or_else(|| {
                self.direct
                    .ownership
                    .submitted
                    .as_ref()
                    .filter(|frame| frame.lease.key() == candidate_key)
                    .map(|frame| {
                        let validation = frame.lease.validation_key();
                        OutputPresentationStateKey {
                            mode: validation.presentation_mode,
                            content_type: validation.content_type,
                            output_generation: validation.output_generation,
                        }
                    })
            });
        let has_submitted_direct_assignment = output_transactions
            .submitted_direct_presentation_state()
            .is_some()
            || self.direct.ownership.submitted.is_some();
        let pending_key = pending_direct_state
            .map(|_| candidate_key)
            .or(submitted_key);
        let requested_state = OutputPresentationStateKey {
            mode: presentation_mode,
            content_type,
            output_generation: self.direct.drm_generation,
        };
        let presentation_flow = DirectCandidatePresentationFlow::new(
            candidate_key,
            presented_key,
            pending_key,
            confirmed_presentation_state,
            requested_state,
            pending_direct_state,
            has_submitted_direct_assignment,
        );
        if presentation_flow.initial_state_disposition()
            == Some(DirectPresentationStateDisposition::DeferUntilPageflip)
        {
            return Ok(DirectScanoutAttempt::TimingDeferred);
        }
        if candidate.buffer.planes().is_empty() {
            return Ok(DirectScanoutAttempt::Fallback("candidate_plane_missing"));
        }
        let candidate_format = candidate.buffer.format().as_fourcc();
        let candidate_modifier = candidate.buffer.planes()[0].descriptor().modifier.0;
        if !self
            .dmabuf_scanout_capabilities
            .supports(candidate_format, candidate_modifier)
        {
            server.activate_surface_scanout_hint(candidate.surface_id);
            direct_scanout_debug(format_args!(
                "candidate rejected before import: primary plane {} does not support format={:#x} modifier={:#x}",
                self.dmabuf_scanout_capabilities.primary_plane_id,
                candidate_format,
                candidate_modifier,
            ));
            return Ok(DirectScanoutAttempt::Fallback(
                "primary_plane_format_modifier_unsupported",
            ));
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
            output_id: self.direct.output_id,
            output_generation: self.direct.drm_generation,
            crtc_id: atomic.discovery().pipeline.crtc.get(),
            primary_plane_id: atomic.discovery().pipeline.plane.get(),
            mode_width: self.width,
            mode_height: self.height,
            format: candidate_format,
            modifier: candidate_modifier,
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
            presentation_mode,
            content_type,
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
        let requested_presentation_mode = presentation_mode;
        let test_token = PageFlipToken::new(allocate_native_page_flip_token())
            .expect("allocated native TEST_ONLY pageflip token is nonzero");
        let mut qualified_validation_key = None;
        let import_attempts = &mut self.direct.counters.import_attempts;
        let import_failures = &mut self.direct.counters.import_failures;
        let import_cache_hits = &mut self.direct.counters.import_cache_hits;
        let framebuffer_cache = &mut self.direct.framebuffer_cache;
        let validation_cache_hits = &mut self.direct.counters.validation_cache_hits;
        let validation_cache_misses = &mut self.direct.counters.validation_cache_misses;
        let validation_cache = &mut self.direct.validation_cache;
        let qualification = presentation_flow.qualify(
            || {
                *import_attempts = import_attempts.saturating_add(1);
                match framebuffer_cache.get_or_import(&candidate.buffer_identity, &candidate.buffer)
                {
                    Ok((framebuffer, cache_hit)) => {
                        if cache_hit {
                            *import_cache_hits = import_cache_hits.saturating_add(1);
                        }
                        direct_scanout_debug(if cache_hit {
                            "import cache hit".to_string()
                        } else {
                            "imported dma-buf framebuffer".to_string()
                        });
                        Ok(framebuffer)
                    }
                    Err(error) => {
                        *import_failures = import_failures.saturating_add(1);
                        Err(error)
                    }
                }
            },
            |framebuffer, mode| {
                let key = validation_key.with_presentation_state(mode, content_type);
                let qualified = if validation_cache.contains(key) {
                    *validation_cache_hits = validation_cache_hits.saturating_add(1);
                    true
                } else {
                    *validation_cache_misses = validation_cache_misses.saturating_add(1);
                    let result = if let Some(cursor) = cursor {
                        kms.test_flip_with_presentation(
                            framebuffer.framebuffer,
                            test_token,
                            Some(cursor),
                            mode,
                            content_type,
                        )
                    } else {
                        kms.test_flip_without_cursor_with_presentation(
                            framebuffer.framebuffer,
                            test_token,
                            mode,
                            content_type,
                        )
                    };
                    if result.is_ok() {
                        validation_cache.record_success(key);
                    } else {
                        direct_scanout_debug(format_args!(
                            "exact presentation TEST_ONLY rejected mode={}",
                            mode.as_str()
                        ));
                    }
                    result.is_ok()
                };
                if qualified {
                    qualified_validation_key = Some(key);
                }
                qualified
            },
        );
        let (framebuffer, qualified_state, qualified_state_disposition) = match qualification {
            Ok(DirectPresentationQualification::DeferUntilPageflip) => {
                return Ok(DirectScanoutAttempt::TimingDeferred);
            }
            Ok(DirectPresentationQualification::PresentationRejected) => {
                self.note_direct_rejection(true, cursor.is_some());
                return Ok(DirectScanoutAttempt::Fallback(
                    "presentation_test_only_rejected",
                ));
            }
            Ok(DirectPresentationQualification::AlreadyRepresented { state }) => {
                presentation_mode = state.mode;
                if presentation_mode != requested_presentation_mode {
                    direct_scanout_debug(format_args!(
                        "kept direct assignment with weaker presentation mode={}",
                        presentation_mode.as_str()
                    ));
                }
                self.direct.counters.same_buffer_suppressed = self
                    .direct
                    .counters
                    .same_buffer_suppressed
                    .saturating_add(1);
                if !settle_no_visual_change_transaction(
                    self,
                    server,
                    output_transactions,
                    state.output_generation,
                    target,
                    pacing_mode,
                    candidate_key,
                    0,
                    cursor,
                    cursor_epoch,
                    candidate.surface_id,
                    release,
                    state.mode,
                    state.content_type,
                )? {
                    return Ok(DirectScanoutAttempt::TimingDeferred);
                }
                return Ok(DirectScanoutAttempt::Unchanged);
            }
            Ok(DirectPresentationQualification::TransactionRequired {
                framebuffer,
                state,
                state_disposition,
            }) => (framebuffer, state, state_disposition),
            Err(error) => {
                eprintln!("direct scanout: dma-buf import rejected: {error}");
                return Ok(DirectScanoutAttempt::Fallback("import_failed"));
            }
        };
        presentation_mode = qualified_state.mode;
        let validation_key = qualified_validation_key
            .expect("qualified direct presentation mode retains its exact validation key");
        if presentation_mode != requested_presentation_mode {
            direct_scanout_debug(format_args!(
                "kept direct assignment with weaker presentation mode={}",
                presentation_mode.as_str()
            ));
        }
        // Every chosen exact direct state was either tested here or has a
        // matching successful mode-specific validation-cache entry.
        let test_only = KmsTestOnlyPolicy::Skip;

        let frame_id = self.swapchain()?.next_frame_id();
        let presentation_samples = server
            .presentation_commit_key_for_surface_commit_with_generation(
                candidate.surface_id,
                candidate.surface_presentation_generation,
                candidate.commit_sequence,
            )
            .into_iter()
            .collect::<Vec<_>>();
        let client_cursor_presentation_key = cursor_source_key.and_then(|cursor_source_key| {
            server.presentation_commit_key_for_surface_commit(
                cursor_source_key.surface_id,
                oblivion_one::compositor::SurfaceCommitSequence(cursor_source_key.commit_sequence),
            )
        });
        let protocol_batch_id = server
            .take_frame_batch_for_render_with_presentation_samples(frame_id, presentation_samples);
        let mut sampled_surface_ids = vec![candidate.surface_id];
        if let Some(cursor_source_key) = cursor_source_key {
            sampled_surface_ids.push(cursor_source_key.surface_id);
        }
        let surface_damage = server.capture_surface_damage_presentation_for_surface_ids_and_commit(
            sampled_surface_ids,
            cursor_source_key.map(|source_key| {
                (
                    source_key.surface_id,
                    oblivion_one::compositor::SurfaceCommitSequence(source_key.commit_sequence),
                )
            }),
        );
        if let Some(source_key) = cursor_source_key
            && surface_damage.contains_surface_id(source_key.surface_id)
        {
            server.note_client_cursor_surface_sample(true);
        }
        if !server.commit_timing_submission_is_safe_for_batch(
            protocol_batch_id,
            target.presentation_time,
            target.clock_generation,
        ) {
            server.restore_frame_batch_after_render_failure(protocol_batch_id);
            drop(surface_damage);
            return Ok(DirectScanoutAttempt::TimingDeferred);
        }
        let transaction_created_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let transaction_id = match output_transactions.allocate_id() {
            Ok(transaction_id) => transaction_id,
            Err(error) => {
                server.restore_frame_batch_after_render_failure(protocol_batch_id);
                drop(surface_damage);
                return Err(io::Error::other(error));
            }
        };
        let transaction = match build_direct_output_transaction(
            self.direct.output_id,
            transaction_id,
            self.direct.drm_generation,
            transaction_created_at,
            target,
            pacing_mode,
            frame_id,
            candidate_key,
            framebuffer.framebuffer.get(),
            cursor.map(|state| CursorPlaneAssignment::Atomic {
                desired_epoch: cursor_epoch,
                state: Some(state.clone()),
            }),
            protocol_batch_id,
            candidate.surface_id,
            release,
            presentation_mode,
            content_type,
        )
        .map(|transaction| {
            transaction.with_client_cursor_presentation_key(client_cursor_presentation_key)
        }) {
            Ok(transaction) => transaction,
            Err(error) => {
                server.restore_frame_batch_after_render_failure(protocol_batch_id);
                drop(surface_damage);
                return Err(io::Error::other(error));
            }
        };
        let cursor_presentation_batch_id = take_client_cursor_presentation_feedback_batch(
            server,
            client_cursor_presentation_key,
            crate::native_output::presentation::plane::PresentedCursorDelivery::Hardware,
            cursor.is_some_and(|state| state.visible),
        );
        let transaction = match cursor_presentation_batch_id {
            Some(batch_id) => match transaction.with_presentation_feedback_batch(batch_id) {
                Ok(transaction) => transaction,
                Err(error) => {
                    server.restore_frame_batch_after_render_failure(protocol_batch_id);
                    server.restore_presentation_feedback_batch_after_failure(batch_id);
                    drop(surface_damage);
                    return Err(io::Error::other(error));
                }
            },
            None => transaction,
        };
        let (admission, obligations) = match insert_direct_transaction_and_reserve(
            output_transactions,
            transaction,
            candidate_key,
            |key| worker.try_reserve_direct_admission(key),
        ) {
            Ok(admitted) => admitted,
            Err(DirectTransactionInsertError::Insert(error)) => {
                server.restore_frame_batch_after_render_failure(protocol_batch_id);
                if let Some(batch_id) = cursor_presentation_batch_id {
                    server.restore_presentation_feedback_batch_after_failure(batch_id);
                }
                drop(surface_damage);
                return Err(io::Error::other(error));
            }
        };
        let admission = match admission {
            Ok(admission) => admission,
            Err(crate::native_output::kms_worker::KmsWorkerAdmissionError::DuplicateCandidate) => {
                self.note_direct_worker_admission_rejected(false);
                self.note_direct_same_buffer_resubmission();
                if qualified_state_disposition
                    == DirectPresentationStateDisposition::TransitionRequired
                {
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
                            restore_presentation_feedback_obligation(server, obligations);
                            let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                                io::Error::other("deferred direct transaction has no frame batch")
                            })?;
                            server.restore_frame_batch_after_render_failure(batch_id);
                            Ok(())
                        },
                    )
                    .map_err(|error| io::Error::other(error.to_string()))?;
                    self.note_direct_callback_owner_leaks(callback_owner_leaks);
                    return Ok(DirectScanoutAttempt::TimingDeferred);
                }
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
                        restore_presentation_feedback_obligation(server, obligations);
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
                        discard_presentation_feedback_obligation(server, obligations);
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
            cursor_revision,
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
            root_surface_id,
            presented_window_rect,
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
                presented.lease.root_surface_id(),
                presented.lease.presented_window_rect(),
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
            root_surface_id,
            presented_window_rect,
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
        record_direct_blocker(&mut self.direct.counters, reason);
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

fn record_direct_blocker(counters: &mut DirectScanoutCounters, reason: &str) {
    let (name, bit) = direct_blocker(reason);
    counters.blocker_set |= bit;
    if counters.first_blocker.is_none() {
        counters.first_blocker = Some(name);
    }
    counters.last_blocker = Some(name);
}

#[cfg(test)]
mod tests {
    use super::{
        DirectCandidatePresentationFlow, DirectPresentationQualification, DirectScanoutCounters,
        insert_direct_transaction_and_reserve, record_direct_blocker,
    };
    use crate::native_output::kms_worker::KmsWorkerAdmissionError;
    use crate::native_output::presentation::ledger::{
        OutputTransactionLedger, OutputTransactionState,
    };
    use crate::native_output::presentation::transaction::{
        ContentEpochId, DirectPresentationStateDisposition, DirectScanoutCandidateKey,
        OutputContentKey, OutputPresentationStateKey, OutputTransaction, OutputTransactionContent,
        OutputTransactionId,
    };
    use crate::native_output::scanout::OutputReleasePlan;
    use oblivion_one::compositor::{DrmContentType, OutputPresentationMode};
    use oblivion_one::core::OutputId;
    use oblivion_one::native::presentation_deadline::{
        MonotonicTimestampNs, PresentationTarget, PresentationTargetReason, PrimaryRefreshClaim,
    };
    use oblivion_one::native::scheduler::NativeOutputPacingMode;
    use std::{num::NonZeroU64, time::Duration};

    fn candidate_key() -> DirectScanoutCandidateKey {
        DirectScanoutCandidateKey {
            output_id: OutputId::from_raw(1).expect("nonzero output id"),
            content: OutputContentKey::new(
                7,
                NonZeroU64::new(1).expect("buffer id"),
                ContentEpochId::new(NonZeroU64::new(1).expect("content epoch")),
                1920,
                1080,
                0x3432_5241,
                0,
                0,
                1_000,
                0,
            ),
            output_generation: 1,
            cursor_content_key: None,
            color_epoch: 0,
        }
    }

    fn target() -> PresentationTarget {
        let now = MonotonicTimestampNs::new(10);
        PresentationTarget {
            sequence: 1,
            presentation_time: now,
            submit_not_before: now,
            render_start_deadline: now,
            refresh_interval: Duration::from_millis(16),
            reason: PresentationTargetReason::ReactiveDouble,
            clock_generation: 1,
            estimated: false,
            predicted_unreachable: false,
            physical_claim: PrimaryRefreshClaim {
                sequence: 1,
                presentation_time: now,
                clock_generation: 1,
            },
            selection_evidence: Default::default(),
        }
    }

    fn direct_transaction(
        id: OutputTransactionId,
        key: DirectScanoutCandidateKey,
        framebuffer_id: u32,
        state: OutputPresentationStateKey,
    ) -> OutputTransaction {
        super::build_direct_output_transaction(
            key.output_id,
            id,
            state.output_generation,
            MonotonicTimestampNs::new(11),
            target(),
            NativeOutputPacingMode::ReactiveDouble,
            id.get(),
            key,
            framebuffer_id,
            None,
            oblivion_one::compositor::CompositorFrameBatchId::new(
                NonZeroU64::new(id.get()).expect("frame batch id"),
            ),
            key.content.surface_id,
            OutputReleasePlan::Pageflip,
            state.mode,
            state.content_type,
        )
        .map(|transaction| transaction.with_client_cursor_presentation_key(None))
        .expect("direct transaction")
    }

    fn same_candidate_flow(
        key: DirectScanoutCandidateKey,
        confirmed: OutputPresentationMode,
        requested: OutputPresentationMode,
    ) -> DirectCandidatePresentationFlow {
        DirectCandidatePresentationFlow::new(
            key,
            Some(key),
            None,
            OutputPresentationStateKey {
                mode: confirmed,
                content_type: DrmContentType::Graphics,
                output_generation: key.output_generation,
            },
            OutputPresentationStateKey {
                mode: requested,
                content_type: DrmContentType::Graphics,
                output_generation: key.output_generation,
            },
            None,
            false,
        )
    }

    #[test]
    fn presented_adaptive_direct_candidate_requalifies_falls_back_and_is_admitted_zero_copy() {
        let key = candidate_key();
        let flow = same_candidate_flow(
            key,
            OutputPresentationMode::AdaptiveSync,
            OutputPresentationMode::AdaptiveSync,
        );
        assert!(flow.same_visual_assignment());
        assert_eq!(
            flow.initial_state_disposition(),
            Some(DirectPresentationStateDisposition::AlreadyRepresented)
        );

        let framebuffer_id = 73;
        let mut imported_framebuffer = None;
        let mut tested_modes = Vec::new();
        let qualification = flow
            .qualify(
                || {
                    let reused_framebuffer = framebuffer_id;
                    imported_framebuffer = Some(reused_framebuffer);
                    Ok::<_, ()>(reused_framebuffer)
                },
                |reused_framebuffer, mode| {
                    assert_eq!(*reused_framebuffer, framebuffer_id);
                    tested_modes.push(mode);
                    mode == OutputPresentationMode::Vsync
                },
            )
            .expect("framebuffer cache reuse");
        let DirectPresentationQualification::TransactionRequired {
            framebuffer,
            state,
            state_disposition,
        } = qualification
        else {
            panic!("Vsync fallback must require a direct KMS transaction");
        };
        assert_eq!(imported_framebuffer, Some(framebuffer_id));
        assert_eq!(framebuffer, framebuffer_id);
        assert_eq!(
            tested_modes,
            [
                OutputPresentationMode::AdaptiveSync,
                OutputPresentationMode::Vsync
            ]
        );
        assert_eq!(state.mode, OutputPresentationMode::Vsync);
        assert_eq!(
            state_disposition,
            DirectPresentationStateDisposition::TransitionRequired
        );

        let transaction_id = OutputTransactionId::new(NonZeroU64::new(9).unwrap());
        let transaction = direct_transaction(transaction_id, key, framebuffer, state);
        let mut ledger = OutputTransactionLedger::new();
        let mut worker_admissions = 0;
        let (admission, _obligations) =
            insert_direct_transaction_and_reserve(&mut ledger, transaction, key, |admitted_key| {
                assert_eq!(admitted_key, key);
                worker_admissions += 1;
                Ok::<_, KmsWorkerAdmissionError>(())
            })
            .expect("worker admission");

        assert_eq!(
            admission.expect("worker accepted the Direct assignment"),
            ()
        );
        assert_eq!(worker_admissions, 1);
        let record = ledger
            .transaction(transaction_id)
            .expect("active Direct transaction");
        assert_eq!(record.state(), OutputTransactionState::Built);
        assert_eq!(
            record.descriptor().presentation_mode(),
            OutputPresentationMode::Vsync
        );
        assert!(matches!(
            record.descriptor().content(),
            OutputTransactionContent::Direct {
                key: transaction_key,
                ..
            } if transaction_key == key
        ));
    }

    #[test]
    fn same_direct_state_is_no_visual_only_after_exact_qualification() {
        let flow = same_candidate_flow(
            candidate_key(),
            OutputPresentationMode::AdaptiveSync,
            OutputPresentationMode::AdaptiveSync,
        );
        assert_eq!(
            flow.initial_state_disposition(),
            Some(DirectPresentationStateDisposition::AlreadyRepresented)
        );

        let mut tested_modes = Vec::new();
        let qualification = flow
            .qualify(
                || Ok::<_, ()>(73_u32),
                |framebuffer_id, mode| {
                    assert_eq!(*framebuffer_id, 73);
                    tested_modes.push(mode);
                    true
                },
            )
            .expect("framebuffer cache reuse");

        assert_eq!(tested_modes, [OutputPresentationMode::AdaptiveSync]);
        assert!(matches!(
            qualification,
            DirectPresentationQualification::AlreadyRepresented { state }
                if state.mode == OutputPresentationMode::AdaptiveSync
        ));
    }

    #[test]
    fn vsync_direct_candidate_qualifies_adaptive_state_only_transition() {
        let key = candidate_key();
        let flow = same_candidate_flow(
            key,
            OutputPresentationMode::Vsync,
            OutputPresentationMode::AdaptiveSync,
        );
        let mut reused = 0;
        let qualification = flow
            .qualify(
                || {
                    reused += 1;
                    Ok::<_, ()>(73_u32)
                },
                |framebuffer_id, mode| {
                    *framebuffer_id == 73 && mode == OutputPresentationMode::AdaptiveSync
                },
            )
            .expect("framebuffer cache reuse");
        let DirectPresentationQualification::TransactionRequired {
            framebuffer,
            state,
            state_disposition,
        } = qualification
        else {
            panic!("accepted Adaptive Sync must create a direct transaction");
        };
        assert_eq!(reused, 1);
        assert_eq!(framebuffer, 73);
        assert!(flow.same_visual_assignment());
        assert_eq!(state.mode, OutputPresentationMode::AdaptiveSync);
        assert_eq!(
            state_disposition,
            DirectPresentationStateDisposition::TransitionRequired
        );

        let transaction_id = OutputTransactionId::new(NonZeroU64::new(10).unwrap());
        let transaction = direct_transaction(transaction_id, key, framebuffer, state);
        let mut ledger = OutputTransactionLedger::new();
        let mut worker_admissions = 0;
        let (admission, _obligations) =
            insert_direct_transaction_and_reserve(&mut ledger, transaction, key, |admitted_key| {
                assert_eq!(admitted_key, key);
                worker_admissions += 1;
                Ok::<_, KmsWorkerAdmissionError>(())
            })
            .expect("worker admission");

        admission.expect("worker accepted the state-only Direct transition");
        assert_eq!(worker_admissions, 1);
        let record = ledger
            .transaction(transaction_id)
            .expect("active Direct state-transition transaction");
        assert_eq!(record.state(), OutputTransactionState::Built);
        assert_eq!(
            record.descriptor().presentation_mode(),
            OutputPresentationMode::AdaptiveSync
        );
        assert!(matches!(
            record.descriptor().content(),
            OutputTransactionContent::Direct {
                key: transaction_key,
                ..
            } if transaction_key == key
        ));
    }

    #[test]
    fn note_direct_blocker_preserves_first_blocker() {
        let mut counters = DirectScanoutCounters::default();

        record_direct_blocker(&mut counters, "import_failed");
        record_direct_blocker(&mut counters, "test_only_rejected");

        assert_eq!(counters.first_blocker, Some("import_failed"));
    }

    #[test]
    fn later_direct_blocker_updates_latest_without_replacing_first() {
        let mut counters = DirectScanoutCounters::default();

        record_direct_blocker(&mut counters, "import_failed");
        record_direct_blocker(&mut counters, "test_only_rejected");

        assert_eq!(counters.first_blocker, Some("import_failed"));
        assert_eq!(counters.last_blocker, Some("test_only_rejected"));
        assert_eq!(counters.blocker_set, (1 << 6) | (1 << 8));
    }
}
