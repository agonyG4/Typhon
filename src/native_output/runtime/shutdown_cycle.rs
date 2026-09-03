use super::shutdown::KmsRestoreReason;
use super::*;

impl NativeRuntime {
    pub(super) fn request_native_shutdown(&mut self) -> NativeResult<()> {
        let now_ns = monotonic_now_ns()?;
        let first_request = self.shutdown.is_running();
        if first_request {
            self.abandon_direct_fallback();
        }
        let worker_inflight = if first_request {
            NativeSessionIo::observe(self, NativeIoOperation::KmsWorkerStopAdmission);
            self.stop_kms_worker_admission_for_shutdown()?
        } else {
            None
        };
        if first_request {
            self.forced_shutdown_inflight = worker_inflight.or_else(|| {
                let pending_token = self
                    .atomic_commit_arbiter
                    .pending_atomic_commit()
                    .filter(|pending| {
                        matches!(pending.phase, AtomicCommitPhase::KernelSubmitted { .. })
                    })
                    .map(|pending| pending.token)?;
                self.submitted_worker_ownership
                    .iter()
                    .find(|ownership| ownership.job.token == pending_token)
                    .map(|ownership| super::super::kms_worker::WorkerInFlight {
                        bundle: ownership.job.identity(),
                        token: ownership.job.token,
                        transaction_id: ownership.job.transaction_id,
                        output_generation: ownership.job.output_generation,
                        kind: ownership.job.kind,
                        direct_content_key: None,
                        submit_returned_at_ns: ownership.submit_returned_at.get(),
                    })
            });
        }
        let worker_transport = self.kms_commit_worker_transport
            == crate::native_output::kms_worker::KmsCommitWorkerTransport::Worker;
        let pending_pageflip_token = worker_inflight
            .map(|inflight| inflight.token.get())
            .or_else(|| {
                if worker_transport {
                    self.atomic_commit_arbiter
                        .kernel_submitted_token()
                        .map(PageFlipToken::get)
                        .or_else(|| {
                            self.atomic_commit_arbiter
                                .deferred_pageflip()
                                .and_then(|_| self.atomic_commit_arbiter.worker_queued_token())
                                .map(PageFlipToken::get)
                        })
                        .or_else(|| {
                            self.atomic_cursor
                                .as_ref()
                                .and_then(|cursor| cursor.pending_token().map(PageFlipToken::get))
                        })
                } else {
                    self.atomic_commit_arbiter
                        .pending_atomic_token()
                        .map(PageFlipToken::get)
                        .or_else(|| self.scanout.pending_page_flip_token())
                        .or_else(|| {
                            self.atomic_cursor
                                .as_ref()
                                .and_then(|cursor| cursor.pending_token().map(PageFlipToken::get))
                        })
                }
            });
        match self
            .shutdown
            .request_shutdown(now_ns, pending_pageflip_token)
        {
            Some(transition) => {
                native_shutdown_debug_log("shortcut_exit_requested");
                native_shutdown_debug_log("shutdown_begin");
                println!("native input exit requested; shutting down cleanly");
                self.process_supervisor.begin_quiesce();
                self.log_shutdown_transition(transition);
            }
            None => native_shutdown_debug_log("shortcut_exit_requested_duplicate"),
        }
        self.advance_shutdown_lifecycle_without_cycle()
    }

    pub(super) fn advance_shutdown_lifecycle_without_cycle(&mut self) -> NativeResult<()> {
        let cycle = NativeCycleState {
            wakeup: NativeWakeup {
                reasons: Default::default(),
                continuation: Default::default(),
                ready_sources: 0,
                blocked_ns: 0,
                timer_lateness_ns: None,
                explicit_sync_acquire_tokens: Vec::new(),
                dmabuf_gpu_release_tokens: Vec::new(),
                xwayland_events: Vec::new(),
                control_events: Vec::new(),
                cursor_io_events: Vec::new(),
            },
            work_class: NativeWorkClass::NoOutputWork,
            fast_path_completed: false,
            pageflip_drain_us: 0,
            pageflip_completed: false,
            completed_pageflip_token: None,
            frame_completed: false,
            frame_rendered: false,
            frame_submitted: false,
            present_us: 0,
            pageflip_pending_at_tick: self.scanout.page_flip_pending(),
            tick_us: 0,
            accepted: 0,
            redraw_requested: false,
            skipped_input_repaints: 0,
            input_drain_us: 0,
            raw_input_events: 0,
            coalesced_input_events: 0,
            shutdown_requested: false,
        };
        self.advance_shutdown_lifecycle(&cycle)
    }

    pub(super) fn restore_kms_for_shutdown(&mut self) -> NativeResult<()> {
        let Some(reason) = self.shutdown.begin_kms_restore() else {
            return Ok(());
        };
        native_shutdown_debug_log("input_backend_stop");
        let forced_timeout = matches!(reason, KmsRestoreReason::ForcedPageflipTimeout { .. });
        if forced_timeout {
            NativeSessionIo::observe(self, NativeIoOperation::KmsWorkerForceShutdownAbandon);
            self.force_kms_worker_shutdown_abandon()?;
        }
        NativeSessionIo::observe(self, NativeIoOperation::KmsWorkerQuiesce);
        NativeSessionIo::quiesce_kms_worker(self)?;
        NativeSessionIo::observe(self, NativeIoOperation::KmsWorkerJoin);
        NativeSessionIo::join_kms_worker(self)?;
        let forced_identity = self.forced_shutdown_inflight.take();
        if let Some(identity) = forced_identity {
            let _ = self
                .frame_pacing
                .abandon_pending_submission(identity.token.get());
        }
        if forced_timeout {
            // The forced path deliberately abandons unresolved kernel
            // ownership. Clear the compositor-side pending state only after
            // the worker has stopped, then restore KMS.
            NativeSessionIo::observe(self, NativeIoOperation::PageflipQuarantine);
            NativeSessionIo::quarantine_pageflip(self)?;
        }
        self.acquire_watches.shutdown(&mut self.event_loop)?;
        if !self.session.permits_output() {
            teardown_without_drm_io(self);
            self.parked_acquire_watches.clear();
            self.perf.log("native.shutdown_session", || {
                vec![NativePerfField::str(
                    "action",
                    "skip_kms_restore_while_seat_inactive",
                )]
            });
        } else {
            if let Some(cursor) = self.legacy_cursor.as_mut() {
                let _ = cursor.disable();
            }
            native_shutdown_debug_log("kms_restore_begin");
            NativeSessionIo::observe(self, NativeIoOperation::KmsRestore);
            native_shutdown_debug_log("kms_restore_end");
        }
        let pageflip_pending = if self.scanout_destroyed {
            false
        } else {
            self.scanout.page_flip_pending()
        };
        let safety = self.establish_kms_teardown_safety();
        if let Some(identity) = forced_identity {
            if safety.permits_release() {
                self.settle_forced_shutdown_inflight(identity)?;
                self.retire_settled_output_terminals();
            } else {
                self.forced_shutdown_inflight = Some(identity);
            }
        }
        self.perf.log("native.kms_restore", || {
            vec![
                NativePerfField::str("backend", self.kms_backend.effective_kind().as_str()),
                NativePerfField::str("outcome", safety.as_str()),
                NativePerfField::str("shutdown_pageflip", reason.as_str()),
                NativePerfField::bool("pageflip_pending", pageflip_pending),
            ]
        });
        if safety.permits_release() {
            native_shutdown_debug_log("drm_release");
            native_shutdown_debug_log("vt_restore");
        } else {
            native_shutdown_debug_log("drm_release_deferred_unproven");
        }
        if let Some(transition) = self.shutdown.note_kms_teardown_complete(safety) {
            self.log_shutdown_transition(transition);
        }
        self.retire_settled_output_terminals();
        self.install_native_wake_plan(NativeWakePlan::default(), monotonic_now_ns()?)?;
        Ok(())
    }
}
