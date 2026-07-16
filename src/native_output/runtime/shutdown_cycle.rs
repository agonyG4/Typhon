use super::*;

impl NativeRuntime {
    pub(super) fn request_native_shutdown(&mut self) -> NativeResult<()> {
        let now_ns = monotonic_now_ns()?;
        let pending_pageflip_token = self
            .atomic_commit_arbiter
            .pending_atomic_token()
            .map(PageFlipToken::get)
            .or_else(|| self.scanout.pending_page_flip_token())
            .or_else(|| self.frame_scheduler.pending_page_flip_token())
            .or_else(|| {
                self.atomic_cursor
                    .as_ref()
                    .and_then(|cursor| cursor.pending_token().map(PageFlipToken::get))
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
                ready_sources: 0,
                blocked_ns: 0,
                timer_lateness_ns: None,
                explicit_sync_acquire_tokens: Vec::new(),
                xwayland_events: Vec::new(),
            },
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
            if let Some(transition) = self.shutdown.note_kms_restore_complete() {
                self.log_shutdown_transition(transition);
            }
            return Ok(());
        }
        if let Some(cursor) = self.legacy_cursor.as_mut() {
            let _ = cursor.disable();
        }
        native_shutdown_debug_log("kms_restore_begin");
        NativeSessionIo::observe(self, NativeIoOperation::KmsRestore);
        let restoration = self.kms_backend.restore()?;
        native_shutdown_debug_log("kms_restore_end");
        self.perf.log("native.kms_restore", || {
            vec![
                NativePerfField::str("backend", self.kms_backend.effective_kind().as_str()),
                NativePerfField::str("outcome", restoration.as_str()),
                NativePerfField::str("shutdown_pageflip", reason.as_str()),
                NativePerfField::bool("pageflip_pending", self.scanout.page_flip_pending()),
            ]
        });
        native_shutdown_debug_log("drm_release");
        native_shutdown_debug_log("vt_restore");
        if let Some(transition) = self.shutdown.note_kms_restore_complete() {
            self.log_shutdown_transition(transition);
        }
        self.event_loop.arm_deadline(None)?;
        Ok(())
    }
}
