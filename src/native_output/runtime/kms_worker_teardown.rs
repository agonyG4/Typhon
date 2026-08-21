use super::super::kms_worker::{
    KmsCommitJob, KmsCommitWorkerHandle, KmsSubmittedOwnership, KmsWorkerEvent, KmsWorkerFatalJob,
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

impl NativeRuntime {
    pub(super) fn process_kms_worker_event_after_join_safely(
        &mut self,
        event: KmsWorkerEvent,
    ) -> NativeResult<()> {
        match event {
            KmsWorkerEvent::Submitted { ownership } => {
                self.quarantine_submitted_ownership(ownership)
            }
            KmsWorkerEvent::TestRejected { job, .. }
            | KmsWorkerEvent::SubmitRejected { job, .. }
            | KmsWorkerEvent::BusyExhausted { job, .. } => {
                self.worker_quarantine.jobs.push(job);
                Ok(())
            }
            KmsWorkerEvent::Quiesced {
                returned_jobs,
                returned_sidecar,
            } => {
                self.worker_quarantine.jobs.extend(returned_jobs);
                self.worker_quarantine
                    .cursor_sidecars
                    .extend(returned_sidecar);
                Ok(())
            }
            KmsWorkerEvent::Fatal { .. }
            | KmsWorkerEvent::BusyDeferred { .. }
            | KmsWorkerEvent::PageflipTimeout { .. }
            | KmsWorkerEvent::CursorSidecarReturned { .. }
            | KmsWorkerEvent::ValidationBaseInvalidated { .. } => Ok(()),
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
        self.scanout.disarm_drm_cleanup();
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
            self.scanout.retain_direct_for_unproven_teardown();
            self.server.disarm_shutdown_releases();
            self.scanout.disarm_drm_cleanup();
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
        self.scanout.disarm_drm_cleanup();
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
            } else {
                self.worker_quarantine.jobs.push(fatal_job.job);
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
                    self.quarantine_submitted_ownership(ownership)
                }
                KmsWorkerEvent::TestRejected { job, .. }
                | KmsWorkerEvent::SubmitRejected { job, .. }
                | KmsWorkerEvent::BusyExhausted { job, .. } => {
                    self.worker_quarantine.jobs.push(job);
                    Ok(())
                }
                KmsWorkerEvent::Quiesced {
                    returned_jobs,
                    returned_sidecar,
                } => {
                    self.worker_quarantine.jobs.extend(returned_jobs);
                    self.worker_quarantine
                        .cursor_sidecars
                        .extend(returned_sidecar);
                    Ok(())
                }
                KmsWorkerEvent::Fatal { .. }
                | KmsWorkerEvent::BusyDeferred { .. }
                | KmsWorkerEvent::PageflipTimeout { .. }
                | KmsWorkerEvent::CursorSidecarReturned { .. }
                | KmsWorkerEvent::ValidationBaseInvalidated { .. } => Ok(()),
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
