use super::super::kms_worker::{KmsCommitWorkerHandle, KmsWorkerEvent, KmsWorkerFatalJob};
use super::*;

impl NativeRuntime {
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
