use super::*;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
};

use crate::native_output::KmsModeTiming;
use oblivion_one::native::kms::{
    AtomicCursorVisualState, DrmModeBlobIo, FramebufferId, PreparedAtomicRuntimeModeset,
};
use oblivion_one::native::presentation_deadline::MonotonicTimestampNs;
use oblivion_one::native::scheduler::NativeFrameScheduler;
use oblivion_one::private_config::{PrivateConfigError, PrivateConfigFile};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

#[derive(Clone, Copy)]
pub(crate) enum OutputConfigurationOperation {
    TemporaryApply {
        previous: NativeAppliedOutputConfiguration,
    },
    Rollback {
        transaction_id: OutputConfigurationTransactionId,
    },
}

enum CandidateScanoutState {
    RenderingFence {
        scanout: NativeScanoutBackend,
        frame: AtomicRenderedFrameParts,
    },
    Ready {
        scanout: NativeScanoutBackend,
        modeset: PreparedAtomicRuntimeModeset<DrmModeBlobIo>,
        cursor: Option<AtomicCursorVisualState>,
    },
}

pub(crate) struct PendingOutputConfigurationRequest {
    pub(crate) response: Option<(ReactorToken, u64)>,
    pub(crate) operation: OutputConfigurationOperation,
    pub(crate) target: NativeAppliedOutputConfiguration,
    candidate: Option<CandidateScanoutState>,
    failure_code: Option<oblivion_one::control::ControlErrorCode>,
}

impl PendingOutputConfigurationRequest {
    pub(crate) fn new(
        response: Option<(ReactorToken, u64)>,
        operation: OutputConfigurationOperation,
        target: NativeAppliedOutputConfiguration,
    ) -> Self {
        Self {
            response,
            operation,
            target,
            candidate: None,
            failure_code: None,
        }
    }

    pub(crate) fn waiting_on_render_fence(&self) -> bool {
        matches!(
            self.candidate,
            Some(CandidateScanoutState::RenderingFence { .. })
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputPersistenceSubmitError {
    Busy,
    Unavailable,
}

#[derive(Debug)]
pub(crate) struct OutputPersistenceCompletion {
    pub(crate) job_id: u64,
    pub(crate) result: Result<(), String>,
}

struct OutputPersistenceJob {
    job_id: u64,
    bytes: Vec<u8>,
}

pub(crate) struct OutputConfigurationPersistenceWorker {
    jobs: SyncSender<OutputPersistenceJob>,
    completions: Receiver<OutputPersistenceCompletion>,
    notification: Arc<OwnedFd>,
    busy: Arc<AtomicBool>,
    available: Arc<AtomicBool>,
    _thread: std::thread::JoinHandle<()>,
}

impl OutputConfigurationPersistenceWorker {
    pub(crate) fn from_environment() -> io::Result<Self> {
        let file = PrivateConfigFile::from_environment(OUTPUT_CONFIGURATION_FILE_NAME)
            .map_err(|error| io::Error::other(format!("output configuration store: {error:?}")))?;
        Self::new(file)
    }

    fn new(file: PrivateConfigFile) -> io::Result<Self> {
        let notification = Arc::new(create_output_persistence_event_fd()?);
        let worker_notification = Arc::clone(&notification);
        let (jobs, job_receiver) = mpsc::sync_channel::<OutputPersistenceJob>(1);
        let (completion_sender, completions) = mpsc::sync_channel::<OutputPersistenceCompletion>(1);
        let busy = Arc::new(AtomicBool::new(false));
        let worker_busy = Arc::clone(&busy);
        let available = Arc::new(AtomicBool::new(true));
        let worker_available = Arc::clone(&available);
        let thread = std::thread::Builder::new()
            .name("typhon-output-persistence".to_string())
            .spawn(move || {
                while let Ok(job) = job_receiver.recv() {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        file.write_bytes(&job.bytes, MAX_OUTPUT_CONFIGURATION_BYTES)
                    }))
                    .unwrap_or(Err(PrivateConfigError::WriteFailed))
                    .map_err(|error| format!("{error:?}"));
                    let delivered = completion_sender
                        .send(OutputPersistenceCompletion {
                            job_id: job.job_id,
                            result,
                        })
                        .is_ok();
                    worker_busy.store(false, Ordering::Release);
                    if !delivered
                        || notify_output_persistence_event_fd(&worker_notification).is_err()
                    {
                        worker_available.store(false, Ordering::Release);
                        break;
                    }
                }
                worker_available.store(false, Ordering::Release);
            })?;
        Ok(Self {
            jobs,
            completions,
            notification,
            busy,
            available,
            _thread: thread,
        })
    }

    pub(crate) fn event_fd(&self) -> i32 {
        self.notification.as_raw_fd()
    }

    pub(crate) fn submit(
        &self,
        job_id: u64,
        bytes: Vec<u8>,
    ) -> Result<(), OutputPersistenceSubmitError> {
        if !self.available.load(Ordering::Acquire) {
            return Err(OutputPersistenceSubmitError::Unavailable);
        }
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err(OutputPersistenceSubmitError::Busy);
        }
        match self.jobs.try_send(OutputPersistenceJob { job_id, bytes }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.busy.store(false, Ordering::Release);
                Err(OutputPersistenceSubmitError::Busy)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.busy.store(false, Ordering::Release);
                self.available.store(false, Ordering::Release);
                Err(OutputPersistenceSubmitError::Unavailable)
            }
        }
    }

    pub(crate) fn drain_notification(&self) -> io::Result<()> {
        let mut value = 0u64;
        loop {
            let result = unsafe {
                libc::read(
                    self.notification.as_raw_fd(),
                    (&mut value as *mut u64).cast(),
                    std::mem::size_of::<u64>(),
                )
            };
            if result == std::mem::size_of::<u64>() as isize {
                continue;
            }
            if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
                return Ok(());
            }
            if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "short eventfd read",
                ))
            };
        }
    }

    pub(crate) fn try_completion(&self) -> Option<OutputPersistenceCompletion> {
        match self.completions.try_recv() {
            Ok(completion) => Some(completion),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    pub(crate) fn is_available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }
}

fn create_output_persistence_event_fd() -> io::Result<OwnedFd> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn notify_output_persistence_event_fd(fd: &OwnedFd) -> io::Result<()> {
    let value = 1u64;
    loop {
        let written = unsafe {
            libc::write(
                fd.as_raw_fd(),
                (&value as *const u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        if written == std::mem::size_of::<u64>() as isize {
            return Ok(());
        }
        if written < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(io::Error::last_os_error());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputConfigurationAdvance {
    Waiting,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputPersistencePurpose {
    Confirm {
        transaction_id: OutputConfigurationTransactionId,
    },
    Compensate,
}

pub(crate) struct PendingOutputPersistence {
    pub(crate) job_id: u64,
    pub(crate) purpose: OutputPersistencePurpose,
    pub(crate) response: Option<(ReactorToken, u64)>,
}

impl NativeRuntime {
    pub(crate) fn mode_selection_supported(&self) -> bool {
        let rollback_failed =
            self.output_configuration_transactions
                .active()
                .is_some_and(|transaction| {
                    matches!(
                        transaction.phase,
                        OutputConfigurationTransactionPhase::RollbackFailed { .. }
                    )
                });
        output_mode_mutation_is_supported(
            self.kms_backend.effective_kind() == oblivion_one::native::kms::KmsBackendKind::Atomic,
            !self.output_capabilities.mode_inventory.entries().is_empty(),
            self.scanout.kind(),
            !self.scanout_destroyed,
            self.output_configuration_persistence_worker
                .as_ref()
                .is_some_and(OutputConfigurationPersistenceWorker::is_available),
            self.output_persistence_compensation_required,
            self.output_persistence_compensation_failed,
            rollback_failed,
        )
    }

    pub(crate) fn enqueue_temporary_output_configuration(
        &mut self,
        token: ReactorToken,
        request_id: u64,
        target: NativeAppliedOutputConfiguration,
    ) {
        let previous = self.target;
        self.pending_output_configuration = Some(PendingOutputConfigurationRequest::new(
            Some((token, request_id)),
            OutputConfigurationOperation::TemporaryApply { previous },
            target,
        ));
    }

    pub(crate) fn dispatch_output_transaction_command(
        &mut self,
        token: ReactorToken,
        request_id: u64,
        command: oblivion_one::control::ControlCommand,
        value: serde_json::Value,
    ) -> Option<oblivion_one::control::ControlResponse> {
        use oblivion_one::control::{
            ControlCommand, ControlError, ControlErrorCode, ControlResponse,
        };
        let args = match serde_json::from_value::<OutputTransactionArgs>(value) {
            Ok(args) if args.version == 1 && args.transaction_id > 0 => args,
            _ => {
                return Some(ControlResponse::failure(
                    request_id,
                    ControlError::new(
                        ControlErrorCode::InvalidArgument,
                        "invalid output transaction request",
                    ),
                ));
            }
        };
        let Some(transaction_id) = OutputConfigurationTransactionId::new(args.transaction_id)
        else {
            return Some(ControlResponse::failure(
                request_id,
                ControlError::new(
                    ControlErrorCode::InvalidArgument,
                    "invalid output transaction ID",
                ),
            ));
        };
        let now_ns = monotonic_now_ns().unwrap_or(0);
        match command {
            ControlCommand::OutputsConfigureConfirm => {
                let config = match self
                    .output_configuration_transactions
                    .begin_confirm(transaction_id, now_ns)
                {
                    Ok(config) => config,
                    Err(_) => {
                        return Some(ControlResponse::failure(
                            request_id,
                            ControlError::new(
                                ControlErrorCode::OutputTransactionNotFound,
                                "output transaction is stale or cannot be confirmed",
                            ),
                        ));
                    }
                };
                let bytes = match self
                    .persisted_output_configuration(config)
                    .and_then(|document| {
                        document
                            .encode()
                            .map_err(|error| io::Error::other(format!("{error:?}")))
                    }) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        let _ = self.output_configuration_transactions.complete_confirm(
                            transaction_id,
                            false,
                            now_ns,
                        );
                        let mut response = ControlResponse::failure(
                            request_id,
                            ControlError::new(
                                ControlErrorCode::OutputPersistFailed,
                                "output configuration could not be persisted",
                            ),
                        );
                        if let Some(detail) = response.error.as_mut() {
                            detail.detail = Some(error.to_string());
                        }
                        return Some(response);
                    }
                };
                match self.submit_output_persistence(
                    OutputPersistencePurpose::Confirm { transaction_id },
                    Some((token, request_id)),
                    bytes,
                ) {
                    Ok(()) => None,
                    Err(error) => {
                        let _ = self.output_configuration_transactions.complete_confirm(
                            transaction_id,
                            false,
                            now_ns,
                        );
                        Some(ControlResponse::failure(
                            request_id,
                            ControlError::new(
                                ControlErrorCode::OutputPersistFailed,
                                "output persistence worker is unavailable",
                            )
                            .with_detail(format!("{error:?}")),
                        ))
                    }
                }
            }
            ControlCommand::OutputsConfigureRevert => {
                let target = match self
                    .output_configuration_transactions
                    .begin_revert(transaction_id)
                {
                    Ok(target) => target,
                    Err(_) => {
                        return Some(ControlResponse::failure(
                            request_id,
                            ControlError::new(
                                ControlErrorCode::OutputTransactionNotFound,
                                "output transaction is stale or cannot be reverted",
                            ),
                        ));
                    }
                };
                self.pending_output_configuration = Some(PendingOutputConfigurationRequest::new(
                    Some((token, request_id)),
                    OutputConfigurationOperation::Rollback { transaction_id },
                    target,
                ));
                None
            }
            _ => Some(ControlResponse::failure(
                request_id,
                ControlError::new(
                    ControlErrorCode::InvalidCommand,
                    "not an output transaction command",
                ),
            )),
        }
    }

    fn persisted_output_configuration(
        &self,
        target: NativeAppliedOutputConfiguration,
    ) -> io::Result<PersistedOutputConfiguration> {
        let physical_size_mm =
            self.output_capabilities
                .physical_size_mm
                .as_ref()
                .and_then(|size| {
                    Some(PersistedOutputPhysicalSize {
                        width_mm: size.width_mm,
                        height_mm: size.height_mm,
                    })
                });
        Ok(PersistedOutputConfiguration::from_native_mode(
            self.output_capabilities.connector_name.clone(),
            physical_size_mm,
            &target.mode,
        ))
    }

    fn submit_output_persistence(
        &mut self,
        purpose: OutputPersistencePurpose,
        response: Option<(ReactorToken, u64)>,
        bytes: Vec<u8>,
    ) -> Result<(), OutputPersistenceSubmitError> {
        let worker = self
            .output_configuration_persistence_worker
            .as_ref()
            .ok_or(OutputPersistenceSubmitError::Unavailable)?;
        let job_id = self.next_output_persistence_job_id;
        let Some(next_job_id) = job_id.checked_add(1) else {
            return Err(OutputPersistenceSubmitError::Unavailable);
        };
        worker.submit(job_id, bytes)?;
        self.next_output_persistence_job_id = next_job_id;
        self.pending_output_persistence = Some(PendingOutputPersistence {
            job_id,
            purpose,
            response,
        });
        Ok(())
    }

    pub(crate) fn service_output_persistence_completions(
        &mut self,
        wakeup: &NativeWakeup,
    ) -> NativeResult<()> {
        let unavailable = self
            .output_configuration_persistence_worker
            .as_ref()
            .is_some_and(|worker| !worker.is_available());
        if wakeup.reasons.output_configuration_persistence_worker() || unavailable {
            let completion = self
                .output_configuration_persistence_worker
                .as_ref()
                .and_then(|worker| {
                    let _ = worker.drain_notification();
                    worker.try_completion()
                });
            let Some(completion) = completion else {
                if unavailable && self.pending_output_persistence.is_some() {
                    self.finish_output_persistence_failure("worker_unavailable")?;
                }
                return Ok(());
            };
            let Some(pending) = self.pending_output_persistence.take() else {
                return Ok(());
            };
            if pending.job_id != completion.job_id {
                self.pending_output_persistence = Some(pending);
                return Ok(());
            }
            match pending.purpose {
                OutputPersistencePurpose::Confirm { transaction_id } => {
                    match self.output_configuration_transactions.complete_confirm(
                        transaction_id,
                        completion.result.is_ok(),
                        monotonic_now_ns()?,
                    ) {
                        OutputConfigurationConfirmCompletion::Confirmed => {
                            self.queue_output_snapshot_response(pending.response)?;
                        }
                        OutputConfigurationConfirmCompletion::PersistenceFailed => {
                            self.queue_output_error_response(
                                pending.response,
                                oblivion_one::control::ControlErrorCode::OutputPersistFailed,
                                "output_persist_failed",
                            )?;
                        }
                        OutputConfigurationConfirmCompletion::RollbackOwns
                        | OutputConfigurationConfirmCompletion::Stale => {
                            if completion.result.is_ok() {
                                self.output_persistence_compensation_required = true;
                            }
                            self.queue_output_error_response(
                                pending.response,
                                oblivion_one::control::ControlErrorCode::OutputTransactionNotFound,
                                "output configuration rollback took ownership before persistence completed",
                            )?;
                        }
                    }
                }
                OutputPersistencePurpose::Compensate => match completion.result {
                    Ok(()) => {
                        self.output_persistence_compensation_required = false;
                        self.output_persistence_compensation_failed = false;
                    }
                    Err(error) => {
                        self.output_persistence_compensation_required = false;
                        self.output_persistence_compensation_failed = true;
                        self.perf
                            .log("native.output_persistence_compensation_failed", || {
                                vec![NativePerfField::str("error", error)]
                            });
                    }
                },
            }
        }
        Ok(())
    }

    fn finish_output_persistence_failure(&mut self, detail: &'static str) -> NativeResult<()> {
        let Some(pending) = self.pending_output_persistence.take() else {
            return Ok(());
        };
        if let OutputPersistencePurpose::Confirm { transaction_id } = pending.purpose {
            let _ = self.output_configuration_transactions.complete_confirm(
                transaction_id,
                false,
                monotonic_now_ns()?,
            );
            self.queue_output_error_response(
                pending.response,
                oblivion_one::control::ControlErrorCode::OutputPersistFailed,
                detail,
            )?;
        }
        Ok(())
    }

    fn queue_output_snapshot_response(
        &mut self,
        response: Option<(ReactorToken, u64)>,
    ) -> NativeResult<()> {
        let Some((token, request_id)) = response else {
            return Ok(());
        };
        let result = serde_json::to_value(self.control_output_list_snapshot())
            .map_err(|_| io::Error::other("output snapshot serialization failed"))?;
        self.control_server.queue_response(
            &mut self.event_loop,
            token,
            oblivion_one::control::ControlResponse::success(request_id, result),
        )?;
        Ok(())
    }

    fn queue_output_error_response(
        &mut self,
        response: Option<(ReactorToken, u64)>,
        code: oblivion_one::control::ControlErrorCode,
        detail: &'static str,
    ) -> NativeResult<()> {
        let Some((token, request_id)) = response else {
            return Ok(());
        };
        self.control_server.queue_response(
            &mut self.event_loop,
            token,
            oblivion_one::control::ControlResponse::failure(
                request_id,
                oblivion_one::control::ControlError::new(code, detail).with_detail(detail),
            ),
        )?;
        Ok(())
    }

    pub(crate) fn advance_output_configuration(
        &mut self,
        now_ns: u64,
    ) -> NativeResult<OutputConfigurationAdvance> {
        let advance_started = Instant::now();
        if !self.parked_acquire_watches.is_empty() {
            self.rearm_output_acquire_watches(now_ns);
        }
        if self.pending_output_configuration.is_none()
            && let Some((transaction_id, target)) = self
                .output_configuration_transactions
                .begin_expired_rollback(now_ns)
        {
            self.pending_output_configuration = Some(PendingOutputConfigurationRequest::new(
                None,
                OutputConfigurationOperation::Rollback { transaction_id },
                target,
            ));
        }

        if self.pending_output_configuration.is_none()
            && self.output_persistence_compensation_required
            && self.pending_output_persistence.is_none()
            && self.output_configuration_transactions.active().is_none()
        {
            let current = self.persisted_output_configuration(self.target)?;
            if let Ok(bytes) = current.encode() {
                if let Err(error) = self.submit_output_persistence(
                    OutputPersistencePurpose::Compensate,
                    None,
                    bytes,
                ) {
                    self.output_persistence_compensation_required = false;
                    self.output_persistence_compensation_failed = true;
                    self.perf
                        .log("native.output_persistence_compensation_failed", || {
                            vec![NativePerfField::str("error", format!("{error:?}"))]
                        });
                }
            }
        }

        let Some(mut pending) = self.pending_output_configuration.take() else {
            return Ok(OutputConfigurationAdvance::Completed);
        };
        let candidate_render_fence_owned = pending.waiting_on_render_fence();
        let boundary = self.output_configuration_safe_boundary();
        if self.scanout.ready_frame_queued() && boundary.is_safe_to_retire_unsubmitted_ready_frame()
        {
            self.abandon_unsubmitted_output_frame()?;
        }
        if !self
            .output_configuration_safe_boundary()
            .is_safe_with_candidate_render_fence(candidate_render_fence_owned)
        {
            self.pending_output_configuration = Some(pending);
            return Ok(OutputConfigurationAdvance::Waiting);
        }

        if pending.candidate.is_none() {
            match self.prepare_output_configuration_candidate(&pending.target) {
                Ok(candidate) => pending.candidate = Some(candidate),
                Err(error) => {
                    self.fail_pending_output_configuration(pending, error.to_string())?;
                    return Ok(OutputConfigurationAdvance::Completed);
                }
            }
        }

        if matches!(
            pending.candidate.as_ref(),
            Some(CandidateScanoutState::RenderingFence { .. })
        ) {
            let ready = match pending.candidate.as_ref() {
                Some(CandidateScanoutState::RenderingFence { frame, .. }) => {
                    frame.render_fence.is_signaled_nonblocking()?
                }
                _ => false,
            };
            if !ready {
                self.pending_output_configuration = Some(pending);
                return Ok(OutputConfigurationAdvance::Waiting);
            }
            if let Some(token) = self.output_render_fence_token.take() {
                self.event_loop.unregister(token)?;
            }
            let CandidateScanoutState::RenderingFence { mut scanout, frame } = pending
                .candidate
                .take()
                .expect("render fence candidate checked")
            else {
                unreachable!()
            };
            let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut scanout else {
                self.fail_pending_output_configuration(
                    pending,
                    "explicit candidate backend changed".to_string(),
                )?;
                return Ok(OutputConfigurationAdvance::Completed);
            };
            let framebuffer = explicit.framebuffer(frame.slot)?;
            explicit.promote_initial_presented(frame.slot, frame.scene_commit)?;
            let cursor = self.candidate_cursor_state(&pending.target);
            let modeset = match self.kms_backend.prepare_runtime_modeset_candidate(
                pending.target.mode,
                pending.target.width,
                pending.target.height,
                framebuffer,
                cursor,
            ) {
                Ok(modeset) => modeset,
                Err(error) => {
                    pending.candidate = None;
                    self.fail_pending_output_configuration(pending, error.to_string())?;
                    return Ok(OutputConfigurationAdvance::Completed);
                }
            };
            pending.candidate = Some(CandidateScanoutState::Ready {
                scanout,
                modeset,
                cursor,
            });
        }

        let CandidateScanoutState::Ready {
            mut scanout,
            mut modeset,
            cursor,
        } = pending.candidate.take().expect("candidate prepared")
        else {
            self.pending_output_configuration = Some(pending);
            return Ok(OutputConfigurationAdvance::Waiting);
        };
        if let Err(error) = self
            .kms_backend
            .test_runtime_modeset_candidate(&mut modeset)
        {
            self.fail_pending_output_configuration(pending, format!("TEST_ONLY: {error}"))?;
            return Ok(OutputConfigurationAdvance::Completed);
        }

        if !self.scanout.can_retire_direct_after_synchronous_modeset() {
            self.fail_pending_output_configuration(
                pending,
                "direct scanout ownership did not drain before synchronous modeset".to_string(),
            )?;
            return Ok(OutputConfigurationAdvance::Completed);
        }

        let prepared_transaction = match pending.operation {
            OutputConfigurationOperation::TemporaryApply { previous } => {
                let next_generation = self
                    .output_configuration_generation
                    .get()
                    .wrapping_add(1)
                    .max(1);
                match self
                    .output_configuration_transactions
                    .prepare_temporary_apply(
                        format!("output-{}", self.output_id.get()),
                        previous,
                        pending.target,
                        next_generation,
                    ) {
                    Ok(prepared) => Some(prepared),
                    Err(error) => {
                        self.fail_pending_output_configuration(
                            pending,
                            format!("confirmation transaction reservation failed: {error:?}"),
                        )?;
                        return Ok(OutputConfigurationAdvance::Completed);
                    }
                }
            }
            OutputConfigurationOperation::Rollback { .. } => None,
        };

        // Pause watch admission by not servicing explicit-sync changes while
        // this pending request owns the cycle. Drain actual KMS ownership above,
        // then park every remaining acquire obligation before the real commit.
        match self
            .acquire_watches
            .park_for_session_suspend(&mut self.event_loop)
        {
            Ok(parked) => self.parked_acquire_watches.extend(parked),
            Err(error) => {
                if let Some(prepared) = prepared_transaction {
                    self.output_configuration_transactions
                        .cancel_prepared_temporary_apply(prepared);
                }
                self.rearm_output_acquire_watches(now_ns);
                self.fail_pending_output_configuration(pending, error.to_string())?;
                return Ok(OutputConfigurationAdvance::Completed);
            }
        }

        let commit_started_at_ns = now_ns.saturating_add(
            u64::try_from(advance_started.elapsed().as_nanos()).unwrap_or(u64::MAX),
        );
        let commit_started = Instant::now();
        if let Err(error) = self
            .kms_backend
            .commit_runtime_modeset_candidate(&mut modeset)
        {
            if let Some(prepared) = prepared_transaction {
                self.output_configuration_transactions
                    .cancel_prepared_temporary_apply(prepared);
            }
            self.rearm_output_acquire_watches(commit_started_at_ns);
            self.fail_pending_output_configuration(pending, format!("real commit: {error}"))?;
            return Ok(OutputConfigurationAdvance::Completed);
        }
        self.kms_backend
            .adopt_committed_runtime_modeset_candidate(modeset);
        let committed_at_ns = commit_started_at_ns
            .saturating_add(u64::try_from(commit_started.elapsed().as_nanos()).unwrap_or(u64::MAX));

        if let Some(prepared) = prepared_transaction {
            let transaction_id = self
                .output_configuration_transactions
                .finalize_temporary_apply(prepared, committed_at_ns);
            self.publish_output_configuration(
                pending.target,
                &mut scanout,
                cursor,
                committed_at_ns,
            );
            self.perf.log("native.output_configuration_applied", || {
                vec![
                    NativePerfField::u64("transaction_id", transaction_id.get()),
                    NativePerfField::u64(
                        "configuration_generation",
                        self.output_configuration_generation.get(),
                    ),
                    NativePerfField::u64("width", u64::from(self.target.width)),
                    NativePerfField::u64("height", u64::from(self.target.height)),
                ]
            });
            self.queue_output_snapshot_response_after_commit(pending.response);
        } else {
            self.publish_output_configuration(
                pending.target,
                &mut scanout,
                cursor,
                committed_at_ns,
            );
        }

        match pending.operation {
            OutputConfigurationOperation::TemporaryApply { .. } => {}
            OutputConfigurationOperation::Rollback { transaction_id } => {
                self.output_configuration_transactions
                    .finalize_committed_rollback(transaction_id);
                if self.pending_output_persistence.as_ref().is_some_and(|job| {
                    matches!(job.purpose, OutputPersistencePurpose::Confirm { .. })
                }) {
                    // A late successful write must be repaired after the rollback owns the mode.
                    self.output_persistence_compensation_required = true;
                }
                self.perf
                    .log("native.output_configuration_rolled_back", || {
                        vec![
                            NativePerfField::u64(
                                "configuration_generation",
                                self.output_configuration_generation.get(),
                            ),
                            NativePerfField::u64("width", u64::from(self.target.width)),
                            NativePerfField::u64("height", u64::from(self.target.height)),
                        ]
                    });
                self.queue_output_snapshot_response_after_commit(pending.response);
            }
        }
        self.rearm_output_acquire_watches(committed_at_ns);
        Ok(OutputConfigurationAdvance::Completed)
    }

    fn rearm_output_acquire_watches(&mut self, now_ns: u64) {
        if self.parked_acquire_watches.is_empty() {
            self.output_reconfiguration_rearm_retry_deadline_ns = None;
            return;
        }
        match self.acquire_watches.rearm_parked_requests(
            std::mem::take(&mut self.parked_acquire_watches),
            &mut self.event_loop,
            now_ns,
            &self.acquire_notifier,
        ) {
            Ok(already_ready) => {
                for request in already_ready {
                    let _ = self.server.mark_acquire_commit_ready(
                        request.commit_id,
                        request.surface_id,
                        &request.acquire,
                    );
                }
                self.output_reconfiguration_rearm_retry_deadline_ns = None;
            }
            Err(failure) => {
                let (error, parked) = failure.into_parts();
                self.parked_acquire_watches = parked;
                self.output_reconfiguration_rearm_retry_deadline_ns =
                    Some(now_ns.saturating_add(10_000_000));
                self.perf.log("native.output_acquire_rearm_deferred", || {
                    vec![NativePerfField::str("error", error.to_string())]
                });
            }
        }
    }

    fn queue_output_snapshot_response_after_commit(
        &mut self,
        response: Option<(ReactorToken, u64)>,
    ) {
        if let Err(error) = self.queue_output_snapshot_response(response) {
            self.perf
                .log("native.output_configuration_response_deferred", || {
                    vec![NativePerfField::str("error", error.to_string())]
                });
        }
    }

    fn output_configuration_safe_boundary(&self) -> OutputConfigurationSafeBoundary {
        let worker_in_flight_owned = self
            .kms_commit_worker
            .as_ref()
            .is_some_and(|worker| worker.submission_active() || worker.inflight());
        let worker_queued_next_owned = self
            .kms_commit_worker
            .as_ref()
            .is_some_and(|worker| worker.queue_depth() > 0)
            || !self.submitted_worker_ownership.is_empty()
            || !self.emergency_quarantined_worker_jobs.is_empty()
            || !self.emergency_quarantined_submitted_ownership.is_empty();
        let explicit_sync_obligation_owned = self.acquire_watches.metrics().active_eventfd_watches
            > 0
            || self.acquire_watches.metrics().active_fallback_watches > 0
            || !self.parked_acquire_watches.is_empty();
        OutputConfigurationSafeBoundary {
            main_thread_pageflip_owned: self.scanout.page_flip_pending(),
            worker_in_flight_owned,
            worker_queued_next_owned,
            atomic_commit_arbiter_owned: self.atomic_commit_arbiter.atomic_commit_pending(),
            presentation_transaction_owned: self.output_transactions.active_count() > 0,
            direct_scanout_pageflip_owned: self.scanout.direct_scanout_pending(),
            cursor_plane_work_owned: self.atomic_cursor.as_ref().is_some_and(|cursor| {
                cursor.pending_token().is_some() || cursor.worker_queued_submission().is_some()
            }) || self.cursor_output_arbitration.pending()
                || self.pending_cursor_job.is_some(),
            pacing_worker_reservation_owned: self.frame_pacing.worker_reservation_present(),
            output_render_fence_owned: self.output_render_fence_token.is_some(),
            deferred_worker_event_owned: self.deferred_worker_pageflip.is_some()
                || self.deferred_worker_completion.is_some()
                || self.worker_timeout_pending.is_some(),
            explicit_sync_obligation_owned,
        }
    }

    fn abandon_unsubmitted_output_frame(&mut self) -> NativeResult<()> {
        if !self.scanout.ready_frame_queued() {
            return Ok(());
        }
        if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut *self.scanout {
            let Some(identity) = explicit.swapchain()?.ready_identity() else {
                return Err(
                    io::Error::other("explicit READY scanout has no frame identity").into(),
                );
            };
            super::cycle::abandon_overtaken_ready(
                explicit,
                identity,
                &mut self.scene_history,
                &mut self.frame_pacing,
                &mut self.frame_scheduler,
                &mut self.server,
                &mut self.output_transactions,
                MonotonicTimestampNs::new(monotonic_now_ns()?),
            )?;
        } else {
            if !self.scanout.abandon_unsubmitted_compatibility_ready()? {
                return Err(io::Error::other("compatibility READY ownership disappeared").into());
            }
            self.scene_history.discard_ready();
            if !self.frame_pacing.abandon_ready_frame() {
                self.frame_pacing.cancel_unsubmitted_render();
            }
            self.frame_scheduler.discard_ready_frame();
        }
        Ok(())
    }

    fn candidate_cursor_state(
        &self,
        target: &NativeAppliedOutputConfiguration,
    ) -> Option<AtomicCursorVisualState> {
        let mut state = self.atomic_cursor.as_ref()?.current().clone();
        let (x, y) = self.input_state.cursor_position();
        state.x = x.clamp(0, target.width.saturating_sub(1) as i32);
        state.y = y.clamp(0, target.height.saturating_sub(1) as i32);
        Some(state)
    }

    fn prepare_output_configuration_candidate(
        &mut self,
        target: &NativeAppliedOutputConfiguration,
    ) -> io::Result<CandidateScanoutState> {
        let generation = self.drm_file_generation.wrapping_add(1).max(1);
        let output_id = self.output_id;
        let mut scanout = match self.scanout.kind() {
            NativeScanoutKind::AtomicEglGbmExplicit => {
                let connector = ConnectorId::new(target.connector_id)
                    .ok_or_else(|| io::Error::other("connector ID is zero"))?;
                let crtc = CrtcId::new(target.crtc_id)
                    .ok_or_else(|| io::Error::other("CRTC ID is zero"))?;
                let format = self.scanout.scanout_format();
                let discovery = KmsBackendSelection::discover_atomic_pipeline(
                    self.kms.file().as_fd(),
                    connector,
                    crtc,
                    format,
                )
                .map_err(io::Error::other)?;
                let explicit = AtomicEglGbmScanout::create_unattached_pool(
                    self.kms.file(),
                    &discovery,
                    target.width,
                    target.height,
                    generation,
                    output_id,
                )?;
                NativeScanoutBackend::from_atomic_explicit(explicit)
            }
            kind => NativeScanoutBackend::open_kind(
                kind,
                self.kms.file(),
                target.width,
                target.height,
                generation,
            )?,
        };
        let material_generation = self.server.trusted_effect_registry().current();
        scanout.publish_material_effect_generation(&material_generation);
        let effect_generation = self.server.trusted_effect_registry().current();
        scanout
            .publish_effect_registry_generation((*effect_generation).clone())
            .map_err(io::Error::other)?;
        scanout.set_cursor_image(self.cursor_image.clone());
        // The candidate framebuffer has target pixel dimensions while server
        // geometry is deliberately still the old authoritative projection.
        // Render a full-damage empty transition frame: no old logical
        // coordinates or live client surface can be sampled, and protocol and
        // lifecycle obligations stay pending for the first ordinary frame
        // after publication.
        let scene = ResolvedNativeFrameScene::for_synchronous_output_reconfiguration(&self.server);
        let mut candidate_input_state = self.input_state.clone();
        candidate_input_state.reconfigure_output_bounds(target.width, target.height);
        let cursor = self.candidate_cursor_state(target);
        let candidate_cursor_mode = match self.cursor_render_mode {
            NativeCursorRenderMode::SoftwareClient => NativeCursorRenderMode::Software,
            mode => mode,
        };
        match &mut scanout {
            NativeScanoutBackend::AtomicEglGbm(explicit) => {
                let slot = explicit.initial_slot();
                let mut gpu_sampling_started = false;
                match explicit.render_to_slot(
                    slot,
                    self.frame_index.wrapping_add(1).max(1),
                    &mut self.frame_renderer,
                    &scene,
                    &self.server,
                    &candidate_input_state,
                    candidate_cursor_mode,
                    &NativeOutputDamage::full_output(target.width, target.height),
                    &mut gpu_sampling_started,
                )? {
                    AtomicSlotRenderOutcome::Rendered(frame) => {
                        let frame = *frame;
                        let fd = frame
                            .render_fence
                            .readiness_fd()
                            .ok_or_else(|| {
                                io::Error::other("candidate render fence has no readiness fd")
                            })?
                            .as_raw_fd();
                        self.output_render_fence_token = Some(
                            self.event_loop
                                .register(fd, NativeEventSource::OutputRenderFence)?,
                        );
                        Ok(CandidateScanoutState::RenderingFence { scanout, frame })
                    }
                    AtomicSlotRenderOutcome::Skipped { reason, .. } => Err(io::Error::other(
                        format!("candidate full repaint skipped: {reason:?}"),
                    )),
                    AtomicSlotRenderOutcome::LifecycleFallback { fallbacks, .. } => {
                        Err(io::Error::other(format!(
                            "candidate full repaint lifecycle fallback: {fallbacks:?}"
                        )))
                    }
                }
            }
            _ => match scanout.paint_server_frame(
                &mut self.frame_renderer,
                &scene,
                &self.server,
                &candidate_input_state,
                candidate_cursor_mode,
                &NativeOutputDamage::full_output(target.width, target.height),
            )? {
                NativePaintOutcome::Rendered { .. } => {
                    let framebuffer = FramebufferId::new(scanout.fb_id())
                        .ok_or_else(|| io::Error::other("candidate framebuffer ID is zero"))?;
                    let modeset = self
                        .kms_backend
                        .prepare_runtime_modeset_candidate(
                            target.mode,
                            target.width,
                            target.height,
                            framebuffer,
                            cursor,
                        )
                        .map_err(io::Error::other)?;
                    Ok(CandidateScanoutState::Ready {
                        scanout,
                        modeset,
                        cursor,
                    })
                }
                NativePaintOutcome::Skipped(_) => {
                    Err(io::Error::other("candidate full repaint did not render"))
                }
                NativePaintOutcome::LifecycleFallback { fallbacks, .. } => Err(io::Error::other(
                    format!("candidate repaint lifecycle fallback: {fallbacks:?}"),
                )),
            },
        }
    }

    fn publish_output_configuration(
        &mut self,
        target: NativeAppliedOutputConfiguration,
        candidate_scanout: &mut NativeScanoutBackend,
        cursor: Option<AtomicCursorVisualState>,
        now_ns: u64,
    ) {
        candidate_scanout.finish_initial_scanout();
        self.scanout.retire_direct_after_synchronous_modeset();
        std::mem::swap(&mut *self.scanout, candidate_scanout);

        self.target = target;
        self.drm_file_generation = self.drm_file_generation.wrapping_add(1).max(1);
        self.acquire_watches
            .set_drm_file_generation(self.drm_file_generation);
        let generation = self.output_configuration_generation.advance();
        self.output_capabilities
            .mode_inventory
            .requalify(generation);
        self.output_refresh_rate = super::super::output::output_refresh_rate_for_mode(&target.mode);
        self.mode_label = format!(
            "{}x{}@{}",
            target.width,
            target.height,
            self.output_refresh_rate.rounded_hz()
        );
        self.server.set_output_size(target.width, target.height);
        self.server
            .set_output_refresh_rate(self.output_refresh_rate);
        let input_effect = self
            .input_state
            .reconfigure_output_bounds(target.width, target.height);
        self.input_devices
            .reconfigure_output_dimensions(target.width, target.height);
        if let Some((x, y)) = input_effect.cursor_position {
            self.server.send_pointer_motion(f64::from(x), f64::from(y));
        }
        if let Some(runtime_cursor) = self.atomic_cursor.as_mut() {
            runtime_cursor.reconfigure_output_identity(
                self.drm_file_generation,
                target.crtc_id,
                target.width,
                target.height,
                cursor.as_ref(),
            );
            let (x, y) = self.input_state.cursor_position();
            runtime_cursor.set_position(x, y);
            runtime_cursor.set_visible(
                resolve_native_cursor_for_server(&self.server, &self.input_state).visible,
            );
        }
        let cursor_plane_state = self.atomic_cursor.as_ref().map_or_else(
            crate::native_output::presentation::plane::PresentedCursorState::hidden,
            |cursor| cursor.presented_plane_state(),
        );
        self.presented_planes
            .rebase_after_synchronous_modeset(cursor_plane_state);
        self.confirmed_kms_presentation = ConfirmedKmsPresentationState::default();
        // Validation keys are tied to the output's KMS geometry and plane
        // state. Drop any qualification carried by the replacement scanout
        // before allowing direct promotion on the next ordinary frame.
        self.scanout.invalidate_direct_validation_cache();
        self.last_direct_candidate_key = None;
        self.scene_history.invalidate_presented_damage_history();
        self.last_client_cursor_damage = None;
        self.last_software_cursor_damage = None;
        self.last_client_cursor_path = None;
        self.last_submitted_cursor_epoch = self
            .atomic_cursor
            .as_ref()
            .map_or(0, |cursor| cursor.desired_epoch());
        self.presentation_timing.reconfigure(
            KmsModeTiming::from_mode(&target.mode, self.output_refresh_rate.interval_ns()),
            self.drm_file_generation,
        );
        let interval = Duration::from_nanos(self.output_refresh_rate.interval_ns());
        self.presentation_deadline.invalidate(interval);
        self.scheduled_presentation_target = None;
        self.server.invalidate_commit_timing_targets();
        self.frame_scheduler = NativeFrameScheduler::new_with_refresh_interval_ns(
            self.output_refresh_rate.interval_ns(),
            now_ns,
        );
        self.render_journal.reset();
        self.adaptive_buffering.reset();
        self.pending_proven_deadline_miss = None;
        self.direct_fallback_tracker = None;
        self.queued_redraw_requested = true;
        self.frame_pacing.cancel_unsubmitted_render();
        super::queue_visual_work_after_synchronous_modeset(
            &mut self.frame_pacing,
            &mut self.frame_scheduler,
            now_ns,
            self.server.scene_render_generation(),
        );
    }

    fn fail_pending_output_configuration(
        &mut self,
        pending: PendingOutputConfigurationRequest,
        detail: String,
    ) -> NativeResult<()> {
        match pending.operation {
            OutputConfigurationOperation::TemporaryApply { .. } => {
                let code = pending
                    .failure_code
                    .unwrap_or(oblivion_one::control::ControlErrorCode::OutputApplyFailed);
                if let Some((token, request_id)) = pending.response {
                    let response = oblivion_one::control::ControlResponse::failure(
                        request_id,
                        oblivion_one::control::ControlError::new(
                            code,
                            "output mode application failed",
                        )
                        .with_detail(detail),
                    );
                    self.control_server
                        .queue_response(&mut self.event_loop, token, response)?;
                }
            }
            OutputConfigurationOperation::Rollback { transaction_id } => {
                let _ = self
                    .output_configuration_transactions
                    .fail_rollback(transaction_id, "output_rollback_failed");
                self.perf
                    .log("native.output_configuration_rollback_failed", || {
                        vec![
                            NativePerfField::str("detail", detail.clone()),
                            NativePerfField::u64("transaction_id", transaction_id.get()),
                        ]
                    });
                self.queue_output_error_response(
                    pending.response,
                    oblivion_one::control::ControlErrorCode::OutputRollbackFailed,
                    "output_rollback_failed",
                )?;
            }
        }
        Ok(())
    }
}

fn output_mode_mutation_is_supported(
    atomic_kms: bool,
    has_exact_mode_inventory: bool,
    scanout_kind: NativeScanoutKind,
    scanout_available: bool,
    persistence_available: bool,
    persistence_compensation_pending: bool,
    persistence_compensation_failed: bool,
    rollback_failed: bool,
) -> bool {
    atomic_kms
        && has_exact_mode_inventory
        && scanout_available
        && persistence_available
        && !persistence_compensation_pending
        && !persistence_compensation_failed
        && !rollback_failed
        && matches!(
            scanout_kind,
            NativeScanoutKind::AtomicEglGbmExplicit
                | NativeScanoutKind::NativeEglGbmOpaqueCompatibility
                | NativeScanoutKind::GbmCpuWritePageFlip
                | NativeScanoutKind::DumbFramebuffer
        )
}

pub(crate) const fn output_configuration_cycle_is_due(
    pending_request: bool,
    rollback_deadline_expired: bool,
    persistence_compensation_required: bool,
    acquire_rearm_retry_due: bool,
) -> bool {
    pending_request
        || rollback_deadline_expired
        || persistence_compensation_required
        || acquire_rearm_retry_due
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OutputTransactionArgs {
    version: u8,
    transaction_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_requires_atomic_exact_inventory_replacement_and_persistence() {
        let supported = |atomic_kms, scanout_kind| {
            output_mode_mutation_is_supported(
                atomic_kms,
                true,
                scanout_kind,
                true,
                true,
                false,
                false,
                false,
            )
        };
        assert!(!supported(false, NativeScanoutKind::AtomicEglGbmExplicit));
        assert!(!output_mode_mutation_is_supported(
            true,
            false,
            NativeScanoutKind::AtomicEglGbmExplicit,
            true,
            true,
            false,
            false,
            false,
        ));
        assert!(!supported(true, NativeScanoutKind::Unavailable));
        assert!(supported(true, NativeScanoutKind::AtomicEglGbmExplicit));
        assert!(supported(true, NativeScanoutKind::GbmCpuWritePageFlip));
        assert!(!output_mode_mutation_is_supported(
            true,
            true,
            NativeScanoutKind::AtomicEglGbmExplicit,
            true,
            false,
            false,
            false,
            false,
        ));
        assert!(!output_mode_mutation_is_supported(
            true,
            true,
            NativeScanoutKind::AtomicEglGbmExplicit,
            true,
            true,
            false,
            false,
            true,
        ));
    }

    #[test]
    fn persistence_worker_reports_only_bounded_write_completion() {
        // Filesystem behavior is covered by PrivateConfigFile tests; runtime
        // queue ownership is verified with the transaction model below.
        assert_eq!(MAX_OUTPUT_CONFIGURATION_BYTES, 4096);
    }

    #[test]
    fn pending_persistence_compensation_keeps_the_runtime_cycle_armed() {
        assert!(output_configuration_cycle_is_due(false, false, true, false));
        assert!(output_configuration_cycle_is_due(true, false, false, false));
        assert!(output_configuration_cycle_is_due(false, true, false, false));
        assert!(output_configuration_cycle_is_due(false, false, false, true));
        assert!(!output_configuration_cycle_is_due(
            false, false, false, false
        ));
    }

    #[test]
    fn applied_refresh_and_interval_follow_one_native_timing_authority() {
        for (clock, expected_millihz, expected_interval_ns) in [
            (148_352, 59_940, 16_683_293),
            (148_500, 60_000, 16_666_666),
            (297_000, 120_000, 8_333_333),
            (408_375, 165_000, 6_060_606),
        ] {
            let mode = drm_sys::drm_mode_modeinfo {
                clock,
                hdisplay: 1920,
                hsync_start: 2008,
                hsync_end: 2052,
                htotal: 2200,
                vdisplay: 1080,
                vsync_start: 1084,
                vsync_end: 1089,
                vtotal: 1125,
                ..Default::default()
            };
            let refresh = super::super::super::output::output_refresh_rate_for_mode(&mode);
            let scheduler =
                NativeFrameScheduler::new_with_refresh_interval_ns(refresh.interval_ns(), 0);
            let mut deadline =
                oblivion_one::native::presentation_deadline::PresentationDeadlinePlanner::new(
                    Duration::from_nanos(refresh.interval_ns()),
                );
            let deadline_target = deadline
                .plan_normal(MonotonicTimestampNs::new(0), Duration::ZERO)
                .expect("native refresh interval produces a deadline target");
            let kms_timing = KmsModeTiming::from_mode(&mode, refresh.interval_ns());

            assert_eq!(refresh.refresh_millihz(), expected_millihz);
            assert_eq!(refresh.interval_ns(), expected_interval_ns);
            assert_eq!(refresh.wl_output_millihertz(), expected_millihz as i32);
            assert_eq!(
                refresh.presentation_refresh_nsec(),
                expected_interval_ns as u32
            );
            assert_eq!(scheduler.refresh_interval_ns(), expected_interval_ns);
            assert_eq!(
                kms_timing.refresh_interval_ns(),
                expected_interval_ns,
                "KMS presentation timing uses the native mode timing",
            );
            assert_eq!(
                deadline_target.refresh_interval,
                Duration::from_nanos(expected_interval_ns),
                "presentation deadline uses the same native mode interval",
            );
        }
    }
}
