//! Runtime switching of the triple-buffering and KMS commit worker policies.
//!
//! Both policies are normally fixed by the environment at startup. Switching
//! them on a live session lets one session compare configurations without a
//! restart, so differences come from the policy and not from session state.

use super::*;
use crate::native_output::kms_worker::{
    KmsCommitWorkerHandle, KmsCommitWorkerPolicy, KmsCommitWorkerStartup, KmsCommitWorkerTransport,
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
    InvalidHeldCommits,
    InvalidCreditGrant,
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
            Self::InvalidHeldCommits => "invalid_held_commits",
            Self::InvalidCreditGrant => "invalid_credit_grant",
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
            triple_buffering: self.head.triple_buffer_policy.as_str().to_string(),
            buffering_mode: self.head.adaptive_buffering.mode().as_str().to_string(),
            kms_worker: self.head.kms_commit_worker_policy.as_str().to_string(),
            kms_worker_transport: self.head.kms_commit_worker_transport.as_str().to_string(),
            held_commits: if crate::native_output::kms_worker::held_commits_late_latch() {
                "late_latch"
            } else {
                "immediate"
            }
            .to_string(),
            credit_grant: if self.head.adaptive_buffering.sustained_overlap_grants() {
                "sustained"
            } else {
                "ceiling"
            }
            .to_string(),
        }
    }

    /// Apply the requested policies. Both values are validated before either
    /// is applied, and the worker switch (the only one that can fail) runs
    /// first, so a failed request leaves both policies unchanged.
    pub(super) fn set_presentation_policy(
        &mut self,
        triple_buffering: Option<&str>,
        kms_worker: Option<&str>,
        held_commits: Option<&str>,
        credit_grant: Option<&str>,
    ) -> Result<(), PresentationPolicyError> {
        let sustained_grants = credit_grant
            .map(|value| match value {
                "sustained" => Ok(true),
                "ceiling" => Ok(false),
                _ => Err(PresentationPolicyError::InvalidCreditGrant),
            })
            .transpose()?;
        let held_late_latch = held_commits
            .map(|value| match value {
                "late_latch" => Ok(true),
                "immediate" => Ok(false),
                _ => Err(PresentationPolicyError::InvalidHeldCommits),
            })
            .transpose()?;
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
        if let Some(sustained) = sustained_grants {
            self.head
                .adaptive_buffering
                .set_sustained_overlap_grants(sustained);
        }
        if let Some(late_latch) = held_late_latch {
            crate::native_output::kms_worker::set_held_commits_late_latch(late_latch);
        }
        if let Some(triple) = triple
            && triple != self.head.triple_buffer_policy
        {
            self.head.triple_buffer_policy = triple;
            self.head.adaptive_buffering.set_policy(triple);
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
        if transport != self.head.kms_commit_worker_transport {
            if !self.kms_transport_quiescent() {
                return Err(PresentationPolicyError::KmsWorkerBusy);
            }
            match transport {
                KmsCommitWorkerTransport::Synchronous => self.stop_kms_commit_worker()?,
                KmsCommitWorkerTransport::Worker => self.start_kms_commit_worker_at_runtime()?,
            }
            self.head.kms_commit_worker_transport = transport;
            // Timing evidence gathered on the other transport no longer
            // describes the submit path.
            self.head.adaptive_buffering.reset();
            self.queued_redraw_requested = true;
        }
        self.head.kms_commit_worker_policy = policy;
        Ok(())
    }

    /// No commit, pageflip or worker hand-off is outstanding, so the submit
    /// transport can change without orphaning a completion.
    fn kms_transport_quiescent(&self) -> bool {
        !self.head.atomic_commit_arbiter.atomic_commit_pending()
            && !self.head.scanout.page_flip_pending()
            && self.head.deferred_worker_pageflip.is_none()
            && self.head.deferred_worker_completion.is_none()
            && self.head.worker_timeout_pending.is_none()
            && self
                .head
                .atomic_cursor
                .as_ref()
                .is_none_or(|cursor| cursor.pending_token().is_none())
            && self
                .head
                .kms_commit_worker
                .as_ref()
                .is_none_or(|worker| !worker.submission_active() && worker.queue_depth() == 0)
    }

    fn stop_kms_commit_worker(&mut self) -> Result<(), PresentationPolicyError> {
        let Some(worker) = self.head.kms_commit_worker.as_ref() else {
            return Ok(());
        };
        worker.request_quiesce();
        // Same join as a session suspend: unregisters the eventfd, drops the
        // seat's quiesce authority and settles any events left behind.
        session_io::NativeSessionIo::join_kms_worker(self)
            .map_err(|_| PresentationPolicyError::KmsWorkerJoinFailed)?;
        self.head.kms_commit_worker_startup = KmsCommitWorkerStartup::IntentionallySynchronous;
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
            self.head.output_id,
            self.head.presented_planes.revision,
            self.head.output_epoch.get(),
            self.head.target.crtc_id,
        );
        let token = self
            .event_loop
            .register(worker.event_fd(), NativeEventSource::KmsCommitWorker)
            .map_err(|_| PresentationPolicyError::KmsWorkerStartFailed)?;
        if let Some(seat) = self.seat_session.as_ref() {
            seat.install_pre_disable_quiesce_authority(Some(worker.quiesce_authority()));
        }
        self.head.kms_commit_worker_reactor_token = Some(token);
        self.head.kms_commit_worker = Some(worker);
        self.head.kms_commit_worker_startup = KmsCommitWorkerStartup::WorkerStarted;
        Ok(())
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PresentationPolicySetArgs {
    triple_buffering: Option<String>,
    kms_worker: Option<String>,
    held_commits: Option<String>,
    credit_grant: Option<String>,
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
                    || (args.triple_buffering.is_none()
                        && args.kms_worker.is_none()
                        && args.held_commits.is_none()
                        && args.credit_grant.is_none()) =>
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
            args.held_commits.as_deref(),
            args.credit_grant.as_deref(),
        ) {
            let code = match error {
                PresentationPolicyError::InvalidTripleBuffering
                | PresentationPolicyError::InvalidKmsWorker
                | PresentationPolicyError::InvalidHeldCommits
                | PresentationPolicyError::InvalidCreditGrant
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

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PresentationTraceArgs {
    path: Option<std::path::PathBuf>,
}

impl NativeRuntime {
    /// Start or stop the presentation trace on a live session, so a capture
    /// does not need a restart with OBLIVION_ONE_PRESENTATION_TRACE set.
    pub(super) fn dispatch_presentation_trace_command(
        &mut self,
        command: ControlCommand,
        request: ControlRequest,
    ) -> ControlResponse {
        let invalid = |message| {
            ControlResponse::failure(
                request.id,
                ControlError::new(ControlErrorCode::InvalidArgument, message),
            )
        };
        let Ok(args) = serde_json::from_value::<PresentationTraceArgs>(request.args) else {
            return invalid("invalid presentation trace arguments");
        };
        let path = match (command, args.path) {
            (ControlCommand::PresentationTraceStart, Some(path)) if path.is_absolute() => {
                if self.presentation_trace_writer.is_some() {
                    return invalid("presentation trace is already being written");
                }
                let trace = PresentationTransactionTraceRing::new(65_536);
                match crate::native_output::presentation::trace_writer::PresentationTraceWriter::start_capture(
                    path.clone(),
                    &trace,
                ) {
                    Ok(writer) => {
                        self.presentation_trace = trace;
                        self.presentation_trace_writer = Some(writer);
                        Some(path)
                    }
                    Err(error) => {
                        return ControlResponse::failure(
                            request.id,
                            ControlError::new(
                                ControlErrorCode::Internal,
                                "presentation trace writer did not start",
                            )
                            .with_detail(error.to_string()),
                        );
                    }
                }
            }
            (ControlCommand::PresentationTraceStop, None) => {
                if let Some(mut writer) = self.presentation_trace_writer.take() {
                    writer.shutdown(&self.presentation_trace);
                }
                self.presentation_trace = PresentationTransactionTraceRing::disabled(65_536);
                None
            }
            _ => {
                return invalid("presentation trace start needs an absolute path; stop takes none");
            }
        };
        let snapshot = oblivion_one::control_snapshots::PresentationTraceSnapshot {
            active: path.is_some(),
            path: path.map(|path| path.to_string_lossy().into_owned()),
        };
        match serde_json::to_value(snapshot) {
            Ok(result) => ControlResponse::success(request.id, result),
            Err(_) => ControlResponse::failure(
                request.id,
                ControlError::new(
                    ControlErrorCode::Internal,
                    "presentation trace snapshot failed",
                ),
            ),
        }
    }
}
