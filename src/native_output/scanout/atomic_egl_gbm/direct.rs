use super::*;

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
        worker_admission: Option<crate::native_output::kms_worker::KmsCommitAdmissionPermit>,
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
                self.direct.counters.composited_fallbacks += 1;
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
                self.direct.counters.composited_fallbacks += 1;
                direct_scanout_debug(format_args!("candidate rejected={}", rejection.as_str()));
                return Ok(DirectScanoutAttempt::Rejected(rejection));
            }
        };
        self.direct.counters.candidates_accepted += 1;
        let Some(candidate_key) =
            direct_candidate_key(&candidate, self.direct.drm_generation, cursor)
        else {
            self.direct.counters.composited_fallbacks += 1;
            return Ok(DirectScanoutAttempt::Fallback("candidate_key_invalid"));
        };
        let Some(worker_admission) = worker_admission else {
            self.direct.counters.composited_fallbacks += 1;
            return Ok(DirectScanoutAttempt::Fallback("worker_unavailable"));
        };
        if candidate.buffer.planes().is_empty() {
            self.direct.counters.composited_fallbacks += 1;
            return Ok(DirectScanoutAttempt::Fallback("candidate_plane_missing"));
        }
        let unchanged = self
            .direct
            .pending
            .as_ref()
            .is_some_and(|frame| frame.prepared.key == candidate_key)
            || self
                .direct
                .current
                .as_ref()
                .is_some_and(|frame| frame.prepared.key == candidate_key);
        if unchanged {
            self.direct.counters.same_buffer_suppressed = self
                .direct
                .counters
                .same_buffer_suppressed
                .saturating_add(1);
            return Ok(DirectScanoutAttempt::Unchanged);
        }
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
            .cache
            .get_or_import(&candidate.buffer_identity, &candidate.buffer)
        {
            Ok(imported) => imported,
            Err(error) => {
                self.direct.counters.import_failures += 1;
                self.direct.counters.composited_fallbacks += 1;
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
        let release = match &sync_readiness {
            DirectSyncReadiness::Qualified { release_mode, .. } => match release_mode {
                DirectReleaseMode::Pageflip => OutputReleasePlan::Pageflip,
                DirectReleaseMode::OutFence => OutputReleasePlan::OutFenceThenPageflip,
            },
            DirectSyncReadiness::Unsupported(_) => unreachable!("checked above"),
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
        let token = PageFlipToken::new(allocate_native_page_flip_token())
            .expect("allocated native pageflip token is nonzero");
        let framebuffer_id = framebuffer.framebuffer.get();
        let was_direct = self.direct.current.is_some();
        let direct_lease =
            DirectPrimaryLease::new(candidate, candidate_key, framebuffer, surface_damage);
        self.swapchain_mut()?.advance_external_frame_id(frame_id)?;
        if !was_direct {
            self.direct.counters.entries += 1;
        }
        self.scene.invalidate_presented_damage_history();
        Ok(DirectScanoutAttempt::WorkerQueued {
            transaction_id,
            token: token.get(),
            framebuffer_id,
            lease: direct_lease,
            admission: worker_admission,
        })
    }

    pub(crate) fn complete_direct_pageflip(
        &mut self,
        token: PageFlipToken,
    ) -> io::Result<DirectPageflipCompletion> {
        let pending =
            self.direct.pending.as_ref().ok_or_else(|| {
                io::Error::other("direct pageflip completed with no pending frame")
            })?;
        if self.swapchain()?.pool_generation() != self.direct.drm_generation {
            return Err(io::Error::other(
                "direct pageflip belongs to an old DRM generation",
            ));
        }
        if pending.token != token {
            return Err(io::Error::other(
                "direct pageflip token does not match pending frame",
            ));
        }
        let mut pending = self.direct.pending.take().expect("pending direct frame");
        let out_fence = pending.out_fence.take();
        drop(out_fence);
        let presented_at = MonotonicTimestampNs::new(monotonic_now_ns()?);
        let protocol_batch_id = pending.protocol_batch_id;
        let surface_damage = pending.surface_damage;
        let current = PresentedDirectFrame {
            prepared: pending.prepared,
            token,
            presented_at,
            submit_started_at: pending.submit_started_at,
            submit_returned_at: pending.submit_returned_at,
        };
        let old = self.direct.current.replace(current);
        drop(old);
        self.direct.counters.presentations += 1;
        direct_scanout_debug("direct pageflip presented");
        Ok(DirectPageflipCompletion {
            presented: self
                .direct
                .current
                .as_ref()
                .expect("direct frame was promoted")
                .clone(),
            protocol_batch_id,
            surface_damage,
        })
    }

    pub(crate) fn mark_composited_submission(&mut self) {
        if self.direct.current.is_some() {
            self.direct.inhibit_until_composited_present = true;
        }
    }

    pub(crate) fn complete_composited_transition(&mut self) {
        if self.direct.current.take().is_some() {
            self.direct.counters.exits += 1;
            direct_scanout_debug("exited direct scanout to composition");
            self.scene.invalidate_presented_damage_history();
        }
        self.direct.inhibit_until_composited_present = false;
    }

    pub(crate) fn direct_scanout_active(&self) -> bool {
        self.direct.current.is_some()
    }

    pub(crate) fn direct_scanout_pending(&self) -> bool {
        self.direct.page_flip_pending()
    }

    pub(crate) fn direct_scanout_pending_token(&self) -> Option<PageFlipToken> {
        self.direct.pending_token()
    }

    pub(crate) fn direct_scanout_pending_transaction_id(&self) -> Option<OutputTransactionId> {
        self.direct.pending_transaction_id()
    }

    pub(crate) fn direct_scanout_surface(&self) -> Option<u32> {
        self.direct.active_surface()
    }

    pub(crate) fn direct_scanout_info(&self) -> Option<(u64, u32, u32, u64)> {
        self.direct
            .worker_queued
            .as_ref()
            .map(|frame| {
                (
                    frame.key.content.buffer_id.get(),
                    frame.framebuffer_id,
                    frame.key.content.format,
                    frame.key.content.modifier,
                )
            })
            .or_else(|| {
                self.direct.pending.as_ref().map(|frame| {
                    (
                        frame.prepared.key.content.buffer_id.get(),
                        frame.prepared.framebuffer.framebuffer.get(),
                        frame.prepared.framebuffer.format,
                        frame.prepared.framebuffer.modifier,
                    )
                })
            })
            .or_else(|| {
                self.direct.current.as_ref().map(|frame| {
                    (
                        frame.prepared.key.content.buffer_id.get(),
                        frame.prepared.framebuffer.framebuffer.get(),
                        frame.prepared.framebuffer.format,
                        frame.prepared.framebuffer.modifier,
                    )
                })
            })
    }

    pub(crate) fn direct_scanout_counters(&self) -> DirectScanoutCounters {
        let mut counters = self.direct.counters;
        counters.cleanup_failures = self.direct.cache.cleanup_failures();
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

    pub(crate) fn direct_scanout_suspend(&mut self) -> io::Result<()> {
        self.direct.suspend();
        self.scene.invalidate_presented_damage_history();
        Ok(())
    }

    pub(crate) fn discard_ready_frame_before_direct(
        &mut self,
        server: &mut OwnCompositorServer,
        output_transactions: &mut OutputTransactionLedger,
    ) -> io::Result<bool> {
        let Some(frame) = self.swapchain_mut()?.take_ready_for_submission().ok() else {
            return Ok(false);
        };
        settle_superseded_output_transaction(
            output_transactions,
            frame.transaction_id,
            None,
            OutputTransactionSupersedeReason::DirectTransition,
            MonotonicTimestampNs::new(monotonic_now_ns()?),
            |obligations| {
                let batch_id = obligations.frame_batch_id().ok_or_else(|| {
                    io::Error::other("direct transition transaction has no frame batch")
                })?;
                server.restore_frame_batch_after_render_failure(batch_id);
                Ok(())
            },
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
        self.scene.discard_rendered(frame.scene_commit);
        drop(frame.surface_damage);
        Ok(true)
    }
}
