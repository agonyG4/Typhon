use super::super::kms_worker::{KmsCommitWorkerHandle, KmsWorkerEvent, KmsWorkerFatalJob};
use super::*;

pub(super) const fn classify_kms_teardown_safety(
    output_permitted: bool,
    restore_succeeded: bool,
) -> KmsTeardownSafety {
    if !output_permitted {
        KmsTeardownSafety::TargetDestroyed
    } else if restore_succeeded {
        KmsTeardownSafety::Restored
    } else {
        KmsTeardownSafety::Unproven
    }
}

impl NativeRuntime {
    pub(super) fn process_kms_worker_event_after_join_safely(
        &mut self,
        event: KmsWorkerEvent,
    ) -> NativeResult<()> {
        match event {
            KmsWorkerEvent::Submitted { ownership } => {
                self.process_kms_worker_event(KmsWorkerEvent::Submitted { ownership })
            }
            KmsWorkerEvent::TestRejected { job, .. }
            | KmsWorkerEvent::SubmitRejected { job, .. }
            | KmsWorkerEvent::BusyExhausted { job, .. } => {
                self.quarantined_worker_jobs.push(job);
                Ok(())
            }
            KmsWorkerEvent::Quiesced { returned_jobs } => {
                self.quarantined_worker_jobs.extend(returned_jobs);
                Ok(())
            }
            KmsWorkerEvent::Fatal { .. }
            | KmsWorkerEvent::BusyDeferred { .. }
            | KmsWorkerEvent::SubmitLate { .. }
            | KmsWorkerEvent::PageflipTimeout { .. } => Ok(()),
        }
    }

    pub(super) fn establish_kms_teardown_safety(&mut self) -> KmsTeardownSafety {
        let output_permitted = self.session.permits_output();
        let restoration = if output_permitted {
            match self.kms_backend.restore() {
                Ok(outcome) => {
                    eprintln!("native KMS teardown reached a safe boundary: {outcome:?}");
                    true
                }
                Err(error) => {
                    eprintln!(
                        "native KMS teardown could not prove a safe boundary; retaining ownership: {error}"
                    );
                    false
                }
            }
        } else {
            false
        };
        if !output_permitted {
            self.scanout.disarm_drm_cleanup();
            self.kms_backend.disarm_drm_io();
            if let Some(cursor) = self.atomic_cursor.as_mut() {
                cursor.disarm_drm_cleanup();
            }
            if let Some(cursor) = self.legacy_cursor.as_mut() {
                cursor.disarm_drm_cleanup();
            }
        }
        let safety = classify_kms_teardown_safety(output_permitted, restoration);
        self.kms_teardown_safety = safety;
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
        std::mem::forget(std::mem::take(&mut self.quarantined_worker_jobs));
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
                self.quarantined_worker_jobs.push(fatal_job.job);
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
                    self.quarantined_worker_jobs.push(job);
                    Ok(())
                }
                KmsWorkerEvent::Quiesced { returned_jobs } => {
                    self.quarantined_worker_jobs.extend(returned_jobs);
                    Ok(())
                }
                KmsWorkerEvent::Fatal { .. }
                | KmsWorkerEvent::BusyDeferred { .. }
                | KmsWorkerEvent::SubmitLate { .. }
                | KmsWorkerEvent::PageflipTimeout { .. } => Ok(()),
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
