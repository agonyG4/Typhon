//! Runtime switching of the triple-buffering and KMS commit worker policies.
//!
//! Both policies are normally fixed by the environment at startup. Switching
//! them on a live session lets one session compare configurations without a
//! restart, so differences come from the policy and not from session state.

use super::*;
use crate::native_output::kms_worker::{
    KmsCommitWorkerHandle, KmsCommitWorkerPolicy, KmsCommitWorkerStartup,
    KmsCommitWorkerTransport,
};
use oblivion_one::control::{
    ControlCommand, ControlError, ControlErrorCode, ControlRequest, ControlResponse,
};
use oblivion_one::control_snapshots::PresentationPolicySnapshot;
use oblivion_one::native::kms::KmsBackendKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PresentationPolicyError {
    InvalidTripleBuffering,
    InvalidKmsWorker,
    /// The worker cannot be started or stopped while KMS work is in flight.
    KmsWorkerBusy,
    KmsWorkerUnsupported,
    KmsWorkerStartFailed,
    KmsWorkerJoinFailed,
}

impl PresentationPolicyError {
    pub(super) const fn detail(self) -> &'static str {
        match self {
            Self::InvalidTripleBuffering => "invalid_triple_buffering",
            Self::InvalidKmsWorker => "invalid_kms_worker",
            Self::KmsWorkerBusy => "kms_worker_busy",
            Self::KmsWorkerUnsupported => "kms_worker_unsupported",
            Self::KmsWorkerStartFailed => "kms_worker_start_failed",
            Self::KmsWorkerJoinFailed => "kms_worker_join_failed",
        }
    }
}

impl NativeRuntime {
    pub(super) fn presentation_policy_snapshot(&self) -> PresentationPolicySnapshot {
        PresentationPolicySnapshot {
            triple_buffering: self.triple_buffer_policy.as_str().to_string(),
            buffering_mode: self.adaptive_buffering.mode().as_str().to_string(),
            kms_worker: self.kms_commit_worker_policy.as_str().to_string(),
            kms_worker_transport: self.kms_commit_worker_transport.as_str().to_string(),
        }
    }

    /// Apply the requested policies. Both values are validated before either
    /// is applied, and the worker switch (the only one that can fail) runs
    /// first, so a failed request leaves both policies unchanged.
    pub(super) fn set_presentation_policy(
        &mut self,
        triple_buffering: Option<&str>,
        kms_worker: Option<&str>,
    ) -> Result<(), PresentationPolicyError> {
        let triple = triple_buffering
            .map(AdaptiveTripleBufferPolicy::parse)
            .transpose()
            .map_err(|_| PresentationPolicyError::InvalidTripleBuffering)?;
        let worker = kms_worker
            .map(|value| KmsCommitWorkerPolicy::parse(Some(value)))
            .transpose()
            .map_err(|_| PresentationPolicyError::InvalidKmsWorker)?;
        if let Some(worker) = worker {
            self.set_kms_commit_worker_policy(worker)?;
        }
        if let Some(triple) = triple
            && triple != self.triple_buffer_policy
        {
            self.triple_buffer_policy = triple;
            self.adaptive_buffering.set_policy(triple);
            self.queued_redraw_requested = true;
        }
        Ok(())
    }

    fn set_kms_commit_worker_policy(
        &mut self,
        policy: KmsCommitWorkerPolicy,
    ) -> Result<(), PresentationPolicyError> {
        let atomic = self.kms_backend.effective_kind() == KmsBackendKind::Atomic;
        let transport = match (policy, atomic) {
            (KmsCommitWorkerPolicy::Off, _) | (KmsCommitWorkerPolicy::Auto, false) => {
                KmsCommitWorkerTransport::Synchronous
            }
            (KmsCommitWorkerPolicy::Force, false) => {
                return Err(PresentationPolicyError::KmsWorkerUnsupported);
            }
            (KmsCommitWorkerPolicy::Auto | KmsCommitWorkerPolicy::Force, true) => {
                KmsCommitWorkerTransport::Worker
            }
        };
        if transport != self.kms_commit_worker_transport {
            if !self.kms_transport_quiescent() {
                return Err(PresentationPolicyError::KmsWorkerBusy);
            }
            match transport {
                KmsCommitWorkerTransport::Synchronous => self.stop_kms_commit_worker()?,
                KmsCommitWorkerTransport::Worker => self.start_kms_commit_worker_at_runtime()?,
            }
            self.kms_commit_worker_transport = transport;
            // Timing evidence gathered on the other transport no longer
            // describes the submit path.
            self.adaptive_buffering.reset();
            self.queued_redraw_requested = true;
        }
        self.kms_commit_worker_policy = policy;
        Ok(())
    }

    /// No commit, pageflip or worker hand-off is outstanding, so the submit
    /// transport can change without orphaning a completion.
    fn kms_transport_quiescent(&self) -> bool {
        !self.atomic_commit_arbiter.atomic_commit_pending()
            && !self.scanout.page_flip_pending()
            && self.deferred_worker_pageflip.is_none()
            && self.deferred_worker_completion.is_none()
            && self.worker_timeout_pending.is_none()
            && self
                .atomic_cursor
                .as_ref()
                .is_none_or(|cursor| cursor.pending_token().is_none())
            && self.kms_commit_worker.as_ref().is_none_or(|worker| {
                !worker.submission_active() && worker.queue_depth() == 0
            })
    }

    fn stop_kms_commit_worker(&mut self) -> Result<(), PresentationPolicyError> {
        let Some(worker) = self.kms_commit_worker.as_ref() else {
            return Ok(());
        };
        worker.request_quiesce();
        // Same join as a session suspend: unregisters the eventfd, drops the
        // seat's quiesce authority and settles any events left behind.
        session_io::NativeSessionIo::join_kms_worker(self)
            .map_err(|_| PresentationPolicyError::KmsWorkerJoinFailed)?;
        self.kms_commit_worker_startup = KmsCommitWorkerStartup::IntentionallySynchronous;
        Ok(())
    }

    fn start_kms_commit_worker_at_runtime(&mut self) -> Result<(), PresentationPolicyError> {
        let submitter = self
            .kms_backend
            .atomic()
            .ok_or(PresentationPolicyError::KmsWorkerUnsupported)?
            .commit_submitter();
        let worker = KmsCommitWorkerHandle::start_atomic(submitter)
            .map_err(|_| PresentationPolicyError::KmsWorkerStartFailed)?;
        // Seed the presented base as a runtime modeset does; the worker
        // would otherwise adopt whatever base its first job names.
        worker.set_established_presented_base_for_output(
            self.output_id,
            self.presented_planes.revision,
            self.output_epoch.get(),
            self.target.crtc_id,
        );
        let token = self
            .event_loop
            .register(worker.event_fd(), NativeEventSource::KmsCommitWorker)
            .map_err(|_| PresentationPolicyError::KmsWorkerStartFailed)?;
        if let Some(seat) = self.seat_session.as_ref() {
            seat.install_pre_disable_quiesce_authority(Some(worker.quiesce_authority()));
        }
        self.kms_commit_worker_reactor_token = Some(token);
        self.kms_commit_worker = Some(worker);
        self.kms_commit_worker_startup = KmsCommitWorkerStartup::WorkerStarted;
        Ok(())
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PresentationPolicySetArgs {
    triple_buffering: Option<String>,
    kms_worker: Option<String>,
}

impl NativeRuntime {
    pub(super) fn dispatch_presentation_policy_command(
        &mut self,
        command: ControlCommand,
        request: ControlRequest,
    ) -> ControlResponse {
        let args = match serde_json::from_value::<PresentationPolicySetArgs>(request.args) {
            Ok(args)
                if command == ControlCommand::PresentationPolicySet
                    || (args.triple_buffering.is_none() && args.kms_worker.is_none()) =>
            {
                args
            }
            _ => {
                return ControlResponse::failure(
                    request.id,
                    ControlError::new(
                        ControlErrorCode::InvalidArgument,
                        "invalid presentation policy arguments",
                    ),
                );
            }
        };
        if let Err(error) = self.set_presentation_policy(
            args.triple_buffering.as_deref(),
            args.kms_worker.as_deref(),
        ) {
            let code = match error {
                PresentationPolicyError::InvalidTripleBuffering
                | PresentationPolicyError::InvalidKmsWorker
                | PresentationPolicyError::KmsWorkerUnsupported => {
                    ControlErrorCode::InvalidArgument
                }
                PresentationPolicyError::KmsWorkerBusy
                | PresentationPolicyError::KmsWorkerStartFailed
                | PresentationPolicyError::KmsWorkerJoinFailed => ControlErrorCode::Internal,
            };
            return ControlResponse::failure(
                request.id,
                ControlError::new(code, "presentation policy was not applied")
                    .with_detail(error.detail()),
            );
        }
        match serde_json::to_value(self.presentation_policy_snapshot()) {
            Ok(result) => ControlResponse::success(request.id, result),
            Err(_) => ControlResponse::failure(
                request.id,
                ControlError::new(
                    ControlErrorCode::Internal,
                    "presentation policy snapshot failed",
                ),
            ),
        }
    }
}
