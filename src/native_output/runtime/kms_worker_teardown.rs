use super::super::kms_worker::{
    KmsCommitJob, KmsCommitWorkerHandle, KmsPrimaryUpdate, KmsSubmittedOwnership, KmsWorkerEvent,
    KmsWorkerFatalJob,
};
use super::kms_worker::{FatalWorkerJobHandler, UncertainJobRetention};
use super::*;
use oblivion_one::native::kms::RestorationOutcome;

pub(super) fn retain_complete_submitted_ownership(
    ownership: KmsSubmittedOwnership,
    emergency_ownership: &mut Vec<KmsSubmittedOwnership>,
) {
    emergency_ownership.push(ownership);
}

pub(super) fn retain_uncertain_job_with_suspension(
    job: KmsCommitJob,
    suspended_jobs: &mut Vec<KmsCommitJob>,
    emergency_jobs: &mut Vec<KmsCommitJob>,
) -> NativeResult<UncertainJobRetention> {
    if matches!(job.kind, AtomicCommitKind::DirectPrimary { .. }) {
        emergency_jobs.push(job);
        return Ok(UncertainJobRetention::EmergencyQuarantined);
    }
    suspended_jobs.push(job);
    Ok(UncertainJobRetention::Suspended)
}

impl FatalWorkerJobHandler for NativeRuntime {
    fn retain_uncertain_worker_job(
        &mut self,
        job: KmsCommitJob,
    ) -> NativeResult<UncertainJobRetention> {
        NativeRuntime::retain_uncertain_worker_job(self, job)
    }

    fn fail_known_worker_job(&mut self, job: KmsCommitJob) -> NativeResult<()> {
        self.fail_known_worker_job_impl(job)
    }

    fn drop_known_worker_job(&mut self, job: KmsCommitJob) -> NativeResult<()> {
        NativeRuntime::drop_queued_worker_job(self, job)
    }
}

pub(super) const fn classify_kms_teardown_safety(
    proof: Option<KmsSafeBoundary>,
) -> KmsTeardownSafety {
    KmsTeardownSafety::from_proof(proof)
}

pub(super) const fn proof_from_restoration(outcome: RestorationOutcome) -> Option<KmsSafeBoundary> {
    match outcome {
        RestorationOutcome::Exact | RestorationOutcome::AlreadyRestored => {
            Some(KmsSafeBoundary::Restored)
        }
        RestorationOutcome::SafeDisable => Some(KmsSafeBoundary::TargetDestroyed),
        RestorationOutcome::Unavailable => None,
    }
}

pub(super) fn record_forced_shutdown_inflight(
    runtime: &mut NativeRuntime,
    inflight: super::super::kms_worker::WorkerInFlight,
) -> bool {
    let pacing_cleared = if let Some(ticket) = inflight.pacing_ticket {
        runtime.frame_pacing.cancel_worker_submission(Some(ticket))
            || runtime
                .frame_pacing
                .abandon_pending_submission(inflight.token.get())
    } else {
        runtime
            .frame_pacing
            .abandon_pending_submission(inflight.token.get())
    };
    runtime.forced_shutdown_inflight = Some(inflight);
    runtime.forced_shutdown_pacing_settled = pacing_cleared
        .then_some(inflight.pacing_ticket)
        .flatten()
        .map(|ticket| (inflight.token, ticket));
    pacing_cleared
}

pub(crate) fn settle_returned_worker_pacing(
    frame_pacing: &mut NativeFramePacing,
    ticket: Option<WorkerPacingTicket>,
) -> NativeResult<()> {
    if frame_pacing.cancel_worker_submission(ticket) {
        Ok(())
    } else {
        Err(io::Error::other("worker pacing reservation does not match queued state").into())
    }
}

pub(crate) fn settle_submitted_worker_pacing(
    frame_pacing: &mut NativeFramePacing,
    ticket: Option<WorkerPacingTicket>,
    token: u64,
    now_ns: u64,
    pacing_mode: NativeOutputPacingMode,
) -> NativeResult<()> {
    frame_pacing
        .note_worker_submit_exact(ticket, token, now_ns, pacing_mode)
        .map_err(io::Error::other)?;
    if ticket.is_some() && !frame_pacing.abandon_pending_submission(token) {
        return Err(
            io::Error::other("worker pacing submission does not match pending token").into(),
        );
    }
    Ok(())
}

impl NativeRuntime {
    fn restore_pre_submit_worker_fence(&mut self, job: &mut KmsCommitJob) -> NativeResult<()> {
        if !matches!(job.kind, AtomicCommitKind::CompositedPrimary { .. }) {
            return Ok(());
        }
        match &mut job.primary {
            KmsPrimaryUpdate::Framebuffer { in_fence, .. } => self
                .scanout
                .restore_worker_queued_submission_fence(job.token, in_fence)
                .map_err(Into::into),
            KmsPrimaryUpdate::Unchanged => Ok(()),
        }
    }

    fn retain_returned_worker_job(&mut self, mut job: KmsCommitJob) -> NativeResult<()> {
        let pacing_result =
            settle_returned_worker_pacing(&mut self.frame_pacing, job.pacing_ticket);
        let fence_result = self.restore_pre_submit_worker_fence(&mut job);
        if let Err(error) = pacing_result {
            self.emergency_quarantined_worker_jobs.push(job);
            return Err(error);
        }
        if let Err(error) = fence_result {
            self.emergency_quarantined_worker_jobs.push(job);
            return Err(error);
        }
        self.worker_quarantine.jobs.push(job);
        Ok(())
    }

    fn settle_submitted_worker_pacing_after_join(
        &mut self,
        ownership: &KmsSubmittedOwnership,
    ) -> NativeResult<()> {
        let Some(ticket) = ownership.job.pacing_ticket else {
            return Ok(());
        };
        if self.forced_shutdown_pacing_settled == Some((ownership.job.token, ticket)) {
            return Ok(());
        }
        let pacing_mode = ownership
            .job
            .owners
            .primary()
            .map(|owner| owner.transaction.pacing_mode())
            .ok_or_else(|| io::Error::other("submitted worker job has no primary pacing owner"))?;
        settle_submitted_worker_pacing(
            &mut self.frame_pacing,
            Some(ticket),
            ownership.job.token.get(),
            ownership.submit_returned_at.get(),
            pacing_mode,
        )
    }

    fn quarantine_submitted_worker_ownership_after_join(
        &mut self,
        ownership: KmsSubmittedOwnership,
    ) -> NativeResult<()> {
        let pacing_result = self.settle_submitted_worker_pacing_after_join(&ownership);
        let quarantine_result = self.quarantine_submitted_ownership(ownership);
        pacing_result.and(quarantine_result)
    }

    pub(super) fn process_kms_worker_event_after_join_safely(
        &mut self,
        event: KmsWorkerEvent,
    ) -> NativeResult<()> {
        match event {
            KmsWorkerEvent::Submitted { ownership } => {
                self.quarantine_submitted_worker_ownership_after_join(ownership)
            }
            KmsWorkerEvent::TestRejected { job, .. }
            | KmsWorkerEvent::SubmitRejected { job, .. }
            | KmsWorkerEvent::BusyExhausted { job, .. }
            | KmsWorkerEvent::ValidationBaseInvalidated { job, .. } => {
                self.retain_returned_worker_job(job)
            }
            KmsWorkerEvent::Quiesced {
                returned_jobs,
                returned_sidecar,
            } => {
                let mut first_error = None;
                for job in returned_jobs {
                    if let Err(error) = self.retain_returned_worker_job(job)
                        && first_error.is_none()
                    {
                        first_error = Some(error);
                    }
                }
                self.worker_quarantine
                    .cursor_sidecars
                    .extend(returned_sidecar);
                first_error.map_or(Ok(()), Err)
            }
            KmsWorkerEvent::Fatal { .. }
            | KmsWorkerEvent::BusyDeferred { .. }
            | KmsWorkerEvent::PageflipTimeout { .. }
            | KmsWorkerEvent::CursorSidecarReturned { .. } => Ok(()),
        }
    }

    fn destroy_kms_target(&mut self) -> NativeResult<Option<KmsSafeBoundary>> {
        if !self.session.permits_output() {
            return Ok(None);
        }
        if self.kms_commit_worker.is_some() {
            return Err(io::Error::other(
                "cannot destroy KMS target while commit worker is running",
            )
            .into());
        }
        if let Some(token) = self.drm_reactor_token.take() {
            self.event_loop.unregister(token)?;
        }
        if let Some(token) = self.output_render_fence_token.take() {
            self.event_loop.unregister(token)?;
        }
        if !self.scanout_destroyed {
            self.scanout.disarm_drm_cleanup();
        }
        if let Some(mut cursor) = self.atomic_cursor.take() {
            cursor.disarm_drm_cleanup();
        }
        if let Some(mut cursor) = self.legacy_cursor.take() {
            cursor.disarm_drm_cleanup();
        }
        if !self.scanout_destroyed {
            // SAFETY: worker termination has already completed. DRM cleanup
            // is disarmed before dropping scanout resources, and the target
            // fd is closed immediately afterward as the destruction proof.
            unsafe { std::mem::ManuallyDrop::drop(&mut self.scanout) };
            self.scanout_destroyed = true;
        }
        self.kms_backend.disarm_drm_io();
        Ok(self
            .kms
            .destroy_target()
            .map(|_| KmsSafeBoundary::TargetDestroyed))
    }

    pub(super) fn establish_kms_teardown_safety(&mut self) -> KmsTeardownSafety {
        if self.kms_teardown_safety_established {
            return self.kms_teardown_safety;
        }
        self.kms_teardown_safety_established = true;
        let proof = if self.session.permits_output() {
            match self.kms_backend.restore() {
                Ok(outcome) => {
                    eprintln!("native KMS teardown reached a safe boundary: {outcome:?}");
                    proof_from_restoration(outcome)
                }
                Err(error) => {
                    eprintln!(
                        "native KMS restore could not prove a safe boundary; attempting explicit target destruction: {error}"
                    );
                    NativeSessionIo::observe(self, NativeIoOperation::KmsTargetDestroy);
                    match self.destroy_kms_target() {
                        Ok(proof) => proof,
                        Err(error) => {
                            eprintln!(
                                "native KMS target destruction could not prove a safe boundary; retaining ownership: {error}"
                            );
                            None
                        }
                    }
                }
            }
        } else {
            None
        };
        let safety = classify_kms_teardown_safety(proof);
        self.kms_teardown_safety = safety;
        if !safety.permits_release() {
            if !self.scanout_destroyed {
                self.scanout.retain_direct_for_unproven_teardown();
            }
            self.server.disarm_shutdown_releases();
            if !self.scanout_destroyed {
                self.scanout.disarm_drm_cleanup();
            }
            self.kms_backend.disarm_drm_io();
            if let Some(cursor) = self.atomic_cursor.as_mut() {
                cursor.disarm_drm_cleanup();
            }
            if let Some(cursor) = self.legacy_cursor.as_mut() {
                cursor.disarm_drm_cleanup();
            }
        }
        safety
    }

    pub(super) fn retain_unproven_teardown_ownership(&mut self) {
        self.server.disarm_shutdown_releases();
        if !self.scanout_destroyed {
            self.scanout.disarm_drm_cleanup();
        }
        self.kms_backend.disarm_drm_io();
        if let Some(cursor) = self.atomic_cursor.as_mut() {
            cursor.disarm_drm_cleanup();
        }
        if let Some(cursor) = self.legacy_cursor.as_mut() {
            cursor.disarm_drm_cleanup();
        }
        std::mem::forget(std::mem::take(&mut self.submitted_worker_ownership));
        std::mem::forget(std::mem::take(&mut self.worker_quarantine.jobs));
        std::mem::forget(std::mem::take(&mut self.worker_quarantine.cursor_sidecars));
        std::mem::forget(std::mem::take(&mut self.emergency_quarantined_worker_jobs));
        std::mem::forget(std::mem::take(
            &mut self.emergency_quarantined_submitted_ownership,
        ));
    }

    pub(super) fn defer_fatal_worker_jobs_for_teardown(
        &mut self,
        fatal_jobs: impl IntoIterator<Item = KmsWorkerFatalJob>,
    ) -> NativeResult<()> {
        let mut uncertain_submit = false;
        let mut first_error = None;
        for fatal_job in fatal_jobs {
            if fatal_job.uncertain_submit {
                if let Err(error) = self.retain_uncertain_worker_job(fatal_job.job)
                    && first_error.is_none()
                {
                    first_error = Some(error);
                }
                uncertain_submit = true;
            } else if let Err(error) = self.retain_returned_worker_job(fatal_job.job)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if uncertain_submit
            && let Err(error) = self.quarantine_after_worker_fatal()
            && first_error.is_none()
        {
            first_error = Some(error);
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(super) fn drain_kms_worker_events_for_teardown(
        &mut self,
        worker: &KmsCommitWorkerHandle,
    ) -> NativeResult<()> {
        let mut first_error = worker
            .drain_eventfd()
            .err()
            .map(|error| Box::new(error) as Box<dyn std::error::Error>);
        for event in worker.drain_events() {
            let result = match event {
                KmsWorkerEvent::Submitted { ownership } => {
                    self.quarantine_submitted_worker_ownership_after_join(ownership)
                }
                KmsWorkerEvent::TestRejected { job, .. }
                | KmsWorkerEvent::SubmitRejected { job, .. }
                | KmsWorkerEvent::BusyExhausted { job, .. }
                | KmsWorkerEvent::ValidationBaseInvalidated { job, .. } => {
                    self.retain_returned_worker_job(job)
                }
                KmsWorkerEvent::Quiesced {
                    returned_jobs,
                    returned_sidecar,
                } => {
                    let mut returned_job_error = None;
                    for job in returned_jobs {
                        if let Err(error) = self.retain_returned_worker_job(job)
                            && returned_job_error.is_none()
                        {
                            returned_job_error = Some(error);
                        }
                    }
                    self.worker_quarantine
                        .cursor_sidecars
                        .extend(returned_sidecar);
                    returned_job_error.map_or(Ok(()), Err)
                }
                KmsWorkerEvent::Fatal { .. }
                | KmsWorkerEvent::BusyDeferred { .. }
                | KmsWorkerEvent::PageflipTimeout { .. }
                | KmsWorkerEvent::CursorSidecarReturned { .. } => Ok(()),
            };
            if let Err(error) = result
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}
