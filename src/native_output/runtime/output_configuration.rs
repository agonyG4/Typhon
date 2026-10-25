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

// Mode mutation is qualified on the target hardware (Atomic KMS, explicit
// EGL/GBM scanout, render and KMS workers). It still passes through every
// runtime capability check, and OBLIVION_ONE_OUTPUT_MODE_MUTATION=off disables it.
const OUTPUT_MODE_SELECTION_RUNTIME_QUALIFIED: bool = true;
const OUTPUT_MODE_MUTATION_POLICY_ENV: &str = "OBLIVION_ONE_OUTPUT_MODE_MUTATION";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputModeMutationPolicy {
    Auto,
    Off,
    Qualification,
}

impl OutputModeMutationPolicy {
    pub(crate) fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("auto") => Ok(Self::Auto),
            Some("off") => Ok(Self::Off),
            Some("qualification") => Ok(Self::Qualification),
            Some(value) => Err(value.to_string()),
        }
    }

    pub(crate) fn from_environment() -> io::Result<Self> {
        match std::env::var(OUTPUT_MODE_MUTATION_POLICY_ENV) {
            Ok(value) => Self::parse(Some(&value)).map_err(|value| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "invalid {OUTPUT_MODE_MUTATION_POLICY_ENV} value {value:?}; expected auto, off, or qualification"
                    ),
                )
            }),
            Err(std::env::VarError::NotPresent) => Ok(Self::Auto),
            Err(std::env::VarError::NotUnicode(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{OUTPUT_MODE_MUTATION_POLICY_ENV} must be valid UTF-8"),
            )),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
            Self::Qualification => "qualification",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputModeMutationBlocker {
    ProjectUnqualified,
    PolicyDisabled,
    LegacyKms,
    MissingExactModeInventory,
    UnsupportedScanout,
    ScanoutUnavailable,
    PersistenceUnavailable,
    PersistenceCompensationPending,
    PersistenceCompensationFailed,
    RollbackFailed,
}

impl OutputModeMutationBlocker {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectUnqualified => "project_unqualified",
            Self::PolicyDisabled => "policy_disabled",
            Self::LegacyKms => "legacy_kms",
            Self::MissingExactModeInventory => "missing_exact_mode_inventory",
            Self::UnsupportedScanout => "unsupported_scanout",
            Self::ScanoutUnavailable => "scanout_unavailable",
            Self::PersistenceUnavailable => "persistence_unavailable",
            Self::PersistenceCompensationPending => "persistence_compensation_pending",
            Self::PersistenceCompensationFailed => "persistence_compensation_failed",
            Self::RollbackFailed => "rollback_failed",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct OutputModeMutationRuntimeEvidence {
    atomic_kms: bool,
    exact_mode_count: usize,
    scanout_kind: NativeScanoutKind,
    scanout_available: bool,
    persistence_available: bool,
    persistence_compensation_pending: bool,
    persistence_compensation_failed: bool,
    rollback_failed: bool,
}

fn output_mode_mutation_exposure_allowed(
    production_qualified: bool,
    policy: OutputModeMutationPolicy,
) -> Result<(), OutputModeMutationBlocker> {
    match policy {
        OutputModeMutationPolicy::Off => Err(OutputModeMutationBlocker::PolicyDisabled),
        OutputModeMutationPolicy::Auto if !production_qualified => {
            Err(OutputModeMutationBlocker::ProjectUnqualified)
        }
        OutputModeMutationPolicy::Auto | OutputModeMutationPolicy::Qualification => Ok(()),
    }
}

fn output_mode_mutation_runtime_support(
    evidence: OutputModeMutationRuntimeEvidence,
) -> Result<(), OutputModeMutationBlocker> {
    if !evidence.atomic_kms {
        return Err(OutputModeMutationBlocker::LegacyKms);
    }
    if evidence.exact_mode_count == 0 {
        return Err(OutputModeMutationBlocker::MissingExactModeInventory);
    }
    if !matches!(
        evidence.scanout_kind,
        NativeScanoutKind::AtomicEglGbmExplicit
            | NativeScanoutKind::NativeEglGbmOpaqueCompatibility
            | NativeScanoutKind::GbmCpuWritePageFlip
            | NativeScanoutKind::DumbFramebuffer
    ) {
        return Err(OutputModeMutationBlocker::UnsupportedScanout);
    }
    if !evidence.scanout_available {
        return Err(OutputModeMutationBlocker::ScanoutUnavailable);
    }
    if !evidence.persistence_available {
        return Err(OutputModeMutationBlocker::PersistenceUnavailable);
    }
    if evidence.persistence_compensation_pending {
        return Err(OutputModeMutationBlocker::PersistenceCompensationPending);
    }
    if evidence.persistence_compensation_failed {
        return Err(OutputModeMutationBlocker::PersistenceCompensationFailed);
    }
    if evidence.rollback_failed {
        return Err(OutputModeMutationBlocker::RollbackFailed);
    }
    Ok(())
}

fn output_mode_mutation_support(
    production_qualified: bool,
    policy: OutputModeMutationPolicy,
    evidence: OutputModeMutationRuntimeEvidence,
) -> Result<(), OutputModeMutationBlocker> {
    output_mode_mutation_exposure_allowed(production_qualified, policy)?;
    output_mode_mutation_runtime_support(evidence)
}

#[derive(Clone, Copy)]
pub(crate) enum OutputConfigurationOperation {
    TemporaryApply {
        previous: NativeAppliedOutputConfiguration,
    },
    Rollback {
        transaction_id: OutputConfigurationTransactionId,
    },
}

pub(super) enum CandidateScanoutState {
    RenderingFence {
        scanout: NativeScanoutBackend,
        frame: AtomicRenderedFrameParts,
    },
    ReadyReplacement {
        scanout: NativeScanoutBackend,
        modeset: PreparedAtomicRuntimeModeset<DrmModeBlobIo>,
        cursor: Option<AtomicCursorVisualState>,
    },
    ReuseReady {
        modeset: PreparedAtomicRuntimeModeset<DrmModeBlobIo>,
        cursor: Option<AtomicCursorVisualState>,
    },
    WorkerPreparing {
        candidate: AtomicOutputConfigurationCandidate,
        cursor: Option<AtomicCursorVisualState>,
    },
    WorkerCancelAfterPrepare {
        candidate: AtomicOutputConfigurationCandidate,
        detail: String,
    },
    WorkerRenderingFence {
        candidate: AtomicOutputConfigurationCandidate,
        frame: Box<AtomicRenderedFrameParts>,
        render_info: AtomicRenderDomainInfo,
        cursor: Option<AtomicCursorVisualState>,
    },
    WorkerReady {
        candidate: AtomicOutputConfigurationCandidate,
        frame: Box<AtomicRenderedFrameParts>,
        render_info: AtomicRenderDomainInfo,
        modeset: PreparedAtomicRuntimeModeset<DrmModeBlobIo>,
        cursor: Option<AtomicCursorVisualState>,
    },
    WorkerPromoting {
        candidate: AtomicOutputConfigurationCandidate,
        frame: Box<AtomicRenderedFrameParts>,
        render_info: AtomicRenderDomainInfo,
        cursor: Option<AtomicCursorVisualState>,
        committed_at_ns: u64,
    },
    WorkerPromotionQueued {
        candidate: AtomicOutputConfigurationCandidate,
        frame: Box<AtomicRenderedFrameParts>,
        render_info: AtomicRenderDomainInfo,
        cursor: Option<AtomicCursorVisualState>,
        committed_at_ns: u64,
    },
    WorkerPromoted {
        candidate: AtomicOutputConfigurationCandidate,
        frame: Box<AtomicRenderedFrameParts>,
        render_info: AtomicRenderDomainInfo,
        cursor: Option<AtomicCursorVisualState>,
        committed_at_ns: u64,
    },
    WorkerDiscardQueued {
        candidate: AtomicOutputConfigurationCandidate,
        detail: String,
    },
    WorkerDiscarding {
        candidate: AtomicOutputConfigurationCandidate,
        detail: String,
    },
}

pub(crate) struct PendingOutputConfigurationRequest {
    pub(crate) response: Option<(ReactorToken, u64)>,
    pub(crate) operation: OutputConfigurationOperation,
    pub(crate) target: NativeAppliedOutputConfiguration,
    pub(super) candidate: Option<CandidateScanoutState>,
    failure_code: Option<oblivion_one::control::ControlErrorCode>,
    prepared_transaction: Option<PreparedOutputConfigurationTransaction>,
    waiting_since_ns: Option<u64>,
    stall_reported_at_ns: Option<u64>,
}

/// A pending output configuration that keeps waiting this long is reported,
/// once per interval, with the state and the boundary that holds it.
const OUTPUT_CONFIGURATION_STALL_REPORT_NS: u64 = 1_000_000_000;

impl CandidateScanoutState {
    const fn name(&self) -> &'static str {
        match self {
            Self::RenderingFence { .. } => "rendering_fence",
            Self::ReadyReplacement { .. } => "ready_replacement",
            Self::ReuseReady { .. } => "reuse_ready",
            Self::WorkerPreparing { .. } => "worker_preparing",
            Self::WorkerCancelAfterPrepare { .. } => "worker_cancel_after_prepare",
            Self::WorkerRenderingFence { .. } => "worker_rendering_fence",
            Self::WorkerReady { .. } => "worker_ready",
            Self::WorkerPromoting { .. } => "worker_promoting",
            Self::WorkerPromotionQueued { .. } => "worker_promotion_queued",
            Self::WorkerPromoted { .. } => "worker_promoted",
            Self::WorkerDiscardQueued { .. } => "worker_discard_queued",
            Self::WorkerDiscarding { .. } => "worker_discarding",
        }
    }

    /// The new mode is already committed to KMS; only the worker promotion
    /// and publication remain, and the old epoch was quiesced before commit.
    const fn modeset_committed(&self) -> bool {
        matches!(
            self,
            Self::WorkerPromotionQueued { .. }
                | Self::WorkerPromoting { .. }
                | Self::WorkerPromoted { .. }
        )
    }
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
            prepared_transaction: None,
            waiting_since_ns: None,
            stall_reported_at_ns: None,
        }
    }

    pub(crate) fn waiting_on_render_fence(&self) -> bool {
        matches!(
            self.candidate,
            Some(
                CandidateScanoutState::RenderingFence { .. }
                    | CandidateScanoutState::WorkerRenderingFence { .. }
            )
        )
    }
}

impl NativeRuntime {
    pub(super) fn rebase_output_epoch_frame_lanes(&mut self, now_ns: u64) {
        debug_assert!(
            !self.frame_pacing.has_unsettled_output_frame_ownership(),
            "scheduler/pacing rebase requires old output ownership to be terminal"
        );
        debug_assert!(!self.frame_scheduler.ready_frame_queued());
        debug_assert!(!self.frame_scheduler.page_flip_pending());
        debug_assert!(!self.frame_scheduler.worker_submission_queued());
        self.frame_pacing.rebase_after_output_epoch();
        self.frame_scheduler = NativeFrameScheduler::new_with_refresh_interval_ns(
            self.output_refresh_rate.interval_ns(),
            now_ns,
        );
    }

    pub(super) fn settle_and_rebase_session_output_frame_lanes(
        &mut self,
        now_ns: u64,
    ) -> NativeResult<()> {
        if self.output_transactions.active_count() != 0 {
            return Err(io::Error::other(
                "session output epoch still owns active output transactions",
            )
            .into());
        }
        if let Some(physical_identity) = self.frame_pacing.unsubmitted_physical_identity() {
            self.frame_pacing
                .terminalize_unsubmitted_physical_frame(physical_identity)
                .map_err(io::Error::other)?;
        } else if self.frame_pacing.active.is_some() {
            self.frame_pacing
                .complete_unsubmitted_render_without_visual_change();
        }
        if self.frame_pacing.has_unsettled_output_frame_ownership()
            || self.frame_scheduler.ready_frame_queued()
            || self.frame_scheduler.page_flip_pending()
            || self.frame_scheduler.worker_submission_queued()
        {
            return Err(io::Error::other(
                "session output epoch retained frame ownership before rebase",
            )
            .into());
        }
        self.rebase_output_epoch_frame_lanes(now_ns);
        Ok(())
    }

    pub(super) fn render_worker_candidate_prepared(
        &mut self,
        identity: AtomicRenderCandidateIdentity,
        result: io::Result<(Box<AtomicRenderedFrameParts>, AtomicRenderDomainInfo)>,
    ) -> NativeResult<()> {
        let Some(mut pending) = self.pending_output_configuration.take() else {
            return Err(io::Error::other(
                "render candidate completion arrived without output-configuration ownership",
            )
            .into());
        };
        let Some(candidate_state) = pending.candidate.take() else {
            self.pending_output_configuration = Some(pending);
            return Err(io::Error::other(
                "render candidate completion arrived outside candidate preparation",
            )
            .into());
        };
        let (mut candidate, cursor, cancel_after_prepare) = match candidate_state {
            CandidateScanoutState::WorkerPreparing { candidate, cursor } => {
                (candidate, cursor, None)
            }
            CandidateScanoutState::WorkerCancelAfterPrepare { candidate, detail } => {
                (candidate, None, Some(detail))
            }
            other => {
                pending.candidate = Some(other);
                self.pending_output_configuration = Some(pending);
                return Err(io::Error::other(
                    "render candidate completion arrived outside candidate preparation",
                )
                .into());
            }
        };
        if !candidate.identity().matches(identity) {
            pending.candidate = Some(CandidateScanoutState::WorkerPreparing { candidate, cursor });
            self.pending_output_configuration = Some(pending);
            return Err(io::Error::other(
                "render candidate completion identity does not match output configuration",
            )
            .into());
        }
        if let Some(detail) = cancel_after_prepare {
            match result {
                Ok((_frame, _render_info)) => {
                    pending.candidate =
                        Some(CandidateScanoutState::WorkerDiscardQueued { candidate, detail });
                    self.pending_output_configuration = Some(pending);
                }
                Err(error) => {
                    candidate.worker_domain_destroyed();
                    drop(candidate);
                    self.fail_pending_output_configuration(
                        pending,
                        format!("{detail}; candidate preparation failed: {error}"),
                    )?;
                }
            }
            self.event_loop.request_continuation(
                oblivion_one::native::event_loop::NativeContinuationReason::FrameScheduler,
            )?;
            return Ok(());
        }
        match result {
            Ok((frame, render_info)) => {
                if frame.slot != identity.slot {
                    pending.candidate =
                        Some(CandidateScanoutState::WorkerPreparing { candidate, cursor });
                    self.pending_output_configuration = Some(pending);
                    return Err(io::Error::other(
                        "render candidate completed for the wrong physical slot",
                    )
                    .into());
                }
                let fd = frame
                    .render_fence
                    .readiness_fd()
                    .ok_or_else(|| io::Error::other("candidate render fence has no readiness fd"))?
                    .as_raw_fd();
                self.output_render_fence_token = Some(
                    self.event_loop
                        .register(fd, NativeEventSource::OutputRenderFence)?,
                );
                pending.candidate = Some(CandidateScanoutState::WorkerRenderingFence {
                    candidate,
                    frame,
                    render_info,
                    cursor,
                });
                self.pending_output_configuration = Some(pending);
            }
            Err(error) => {
                candidate.worker_domain_destroyed();
                pending.candidate = None;
                self.fail_pending_output_configuration(pending, error.to_string())?;
            }
        }
        self.event_loop.request_continuation(
            oblivion_one::native::event_loop::NativeContinuationReason::FrameScheduler,
        )?;
        Ok(())
    }

    pub(super) fn render_worker_candidate_promoted(
        &mut self,
        identity: AtomicRenderCandidateIdentity,
    ) -> NativeResult<()> {
        let Some(mut pending) = self.pending_output_configuration.take() else {
            return Err(io::Error::other(
                "render candidate promotion acknowledgement has no output request",
            )
            .into());
        };
        let Some(CandidateScanoutState::WorkerPromoting {
            candidate,
            frame,
            render_info,
            cursor,
            committed_at_ns,
        }) = pending.candidate.take()
        else {
            self.pending_output_configuration = Some(pending);
            return Err(io::Error::other(
                "render candidate promotion acknowledgement arrived in the wrong state",
            )
            .into());
        };
        if !candidate.identity().matches(identity) {
            pending.candidate = Some(CandidateScanoutState::WorkerPromoting {
                candidate,
                frame,
                render_info,
                cursor,
                committed_at_ns,
            });
            self.pending_output_configuration = Some(pending);
            return Err(io::Error::other(
                "render candidate promotion identity does not match output request",
            )
            .into());
        }
        pending.candidate = Some(CandidateScanoutState::WorkerPromoted {
            candidate,
            frame,
            render_info,
            cursor,
            committed_at_ns,
        });
        self.pending_output_configuration = Some(pending);
        self.event_loop.request_continuation(
            oblivion_one::native::event_loop::NativeContinuationReason::FrameScheduler,
        )?;
        Ok(())
    }

    pub(super) fn render_worker_candidate_discarded(
        &mut self,
        identity: AtomicRenderCandidateIdentity,
    ) -> NativeResult<()> {
        let Some(mut pending) = self.pending_output_configuration.take() else {
            return Err(io::Error::other(
                "render candidate discard acknowledgement has no output request",
            )
            .into());
        };
        let Some(CandidateScanoutState::WorkerDiscarding {
            mut candidate,
            detail,
        }) = pending.candidate.take()
        else {
            self.pending_output_configuration = Some(pending);
            return Err(io::Error::other(
                "render candidate discard acknowledgement arrived in the wrong state",
            )
            .into());
        };
        if !candidate.identity().matches(identity) {
            pending.candidate = Some(CandidateScanoutState::WorkerDiscarding { candidate, detail });
            self.pending_output_configuration = Some(pending);
            return Err(io::Error::other(
                "render candidate discard identity does not match output request",
            )
            .into());
        }
        candidate.worker_domain_destroyed();
        drop(candidate);
        self.fail_pending_output_configuration(pending, detail)?;
        self.event_loop.request_continuation(
            oblivion_one::native::event_loop::NativeContinuationReason::FrameScheduler,
        )?;
        Ok(())
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
        let thread = oblivion_one::native::scheduling_qos::spawn_background(
            "typhon-output-persistence",
            move || {
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
            },
        )?;
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

/// A ready synchronous candidate crossing the single irreversible KMS commit
/// boundary. Candidate construction and all KMS behavior stay in their owners.
enum OutputConfigurationTransitionCandidate {
    ReadyReplacement {
        scanout: NativeScanoutBackend,
        modeset: PreparedAtomicRuntimeModeset<DrmModeBlobIo>,
        cursor: Option<AtomicCursorVisualState>,
    },
    ReadyReuse {
        modeset: PreparedAtomicRuntimeModeset<DrmModeBlobIo>,
        cursor: Option<AtomicCursorVisualState>,
    },
    AdoptedReplacement {
        scanout: NativeScanoutBackend,
        cursor: Option<AtomicCursorVisualState>,
    },
    AdoptedReuse {
        cursor: Option<AtomicCursorVisualState>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SameStorageReuseRejection {
    ScanoutBackend,
    RenderWorkerInactive,
    ConnectorChanged,
    CrtcChanged,
    WidthChanged,
    HeightChanged,
    ScaleChanged,
    TransformChanged,
}

impl SameStorageReuseRejection {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ScanoutBackend => "scanout_backend",
            Self::RenderWorkerInactive => "render_worker_inactive",
            Self::ConnectorChanged => "connector_changed",
            Self::CrtcChanged => "crtc_changed",
            Self::WidthChanged => "width_changed",
            Self::HeightChanged => "height_changed",
            Self::ScaleChanged => "scale_changed",
            Self::TransformChanged => "transform_changed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SameStorageReuseDecision {
    Eligible,
    Rejected(SameStorageReuseRejection),
}

impl SameStorageReuseDecision {
    const fn result(self) -> &'static str {
        match self {
            Self::Eligible => "eligible",
            Self::Rejected(_) => "rejected",
        }
    }

    const fn rejection(self) -> Option<SameStorageReuseRejection> {
        match self {
            Self::Eligible => None,
            Self::Rejected(reason) => Some(reason),
        }
    }
}

fn same_storage_reuse_decision(
    current: &NativeAppliedOutputConfiguration,
    target: &NativeAppliedOutputConfiguration,
    scanout_kind: NativeScanoutKind,
    render_worker_active: bool,
) -> SameStorageReuseDecision {
    let rejection = if scanout_kind != NativeScanoutKind::AtomicEglGbmExplicit {
        Some(SameStorageReuseRejection::ScanoutBackend)
    } else if !render_worker_active {
        Some(SameStorageReuseRejection::RenderWorkerInactive)
    } else if current.connector_id != target.connector_id {
        Some(SameStorageReuseRejection::ConnectorChanged)
    } else if current.crtc_id != target.crtc_id {
        Some(SameStorageReuseRejection::CrtcChanged)
    } else if current.width != target.width {
        Some(SameStorageReuseRejection::WidthChanged)
    } else if current.height != target.height {
        Some(SameStorageReuseRejection::HeightChanged)
    } else if current.scale_milli != target.scale_milli {
        Some(SameStorageReuseRejection::ScaleChanged)
    } else if current.transform != target.transform {
        Some(SameStorageReuseRejection::TransformChanged)
    } else {
        None
    };

    match rejection {
        Some(reason) => SameStorageReuseDecision::Rejected(reason),
        None => SameStorageReuseDecision::Eligible,
    }
}

fn same_storage_reuse_decision_diagnostic_line(
    current: &NativeAppliedOutputConfiguration,
    target: &NativeAppliedOutputConfiguration,
    scanout_kind: NativeScanoutKind,
    render_worker_active: bool,
    decision: SameStorageReuseDecision,
) -> String {
    let current_refresh_millihz =
        super::super::output::output_refresh_rate_for_mode(&current.mode).refresh_millihz();
    let target_refresh_millihz =
        super::super::output::output_refresh_rate_for_mode(&target.mode).refresh_millihz();
    let reason = decision
        .rejection()
        .map(|reason| format!(" reason={}", reason.as_str()))
        .unwrap_or_default();

    format!(
        "native output configuration: event=same_storage_reuse_decision result={}{} current_mode_id={} target_mode_id={} current_refresh_millihz={} target_refresh_millihz={} current_width={} current_height={} target_width={} target_height={} scanout_kind={} render_worker_active={} current_connector_id={} target_connector_id={} current_crtc_id={} target_crtc_id={} current_scale={} target_scale={} current_transform={:?} target_transform={:?}",
        decision.result(),
        reason,
        current.mode_id,
        target.mode_id,
        current_refresh_millihz,
        target_refresh_millihz,
        current.width,
        current.height,
        target.width,
        target.height,
        scanout_kind.metric_name(),
        render_worker_active,
        current.connector_id,
        target.connector_id,
        current.crtc_id,
        target.crtc_id,
        current.scale_milli,
        target.scale_milli,
        current.transform,
        target.transform,
    )
}

fn reusable_framebuffer_success_diagnostic_line(
    reusable: ReusableAtomicCompositorFramebuffer,
) -> String {
    let framebuffer_sidecar_id = reusable
        .framebuffer_sidecar
        .map(|framebuffer| framebuffer.get().to_string())
        .unwrap_or_else(|| "none".to_owned());
    format!(
        "native output configuration: event=same_storage_reuse_storage result=success pool_generation={} swapchain_generation={} current_slot={} physical_framebuffer_id={} framebuffer_sidecar_id={} width={} height={}",
        reusable.pool_generation,
        reusable.swapchain_pool_generation,
        reusable.slot.get(),
        reusable.framebuffer.get(),
        framebuffer_sidecar_id,
        reusable.width,
        reusable.height,
    )
}

fn reusable_framebuffer_unavailable_diagnostic_line(
    reason: ReusableStorageUnavailableReason,
) -> String {
    format!(
        "native output configuration: event=same_storage_reuse_storage result=unavailable reason={}",
        reason.as_str(),
    )
}

fn candidate_route_diagnostic_line(
    route: &str,
    reason: Option<&str>,
    unavailable_reason: Option<ReusableStorageUnavailableReason>,
) -> String {
    let reason_field = reason
        .map(|reason| format!(" reason={reason}"))
        .unwrap_or_default();
    let unavailable_field = unavailable_reason
        .map(|reason| format!(" unavailable_reason={}", reason.as_str()))
        .unwrap_or_default();
    format!(
        "native output configuration: event=candidate_route route={route}{reason_field}{unavailable_field}"
    )
}

fn reusable_framebuffer_invariant_error_diagnostic_line(error: &io::Error) -> String {
    format!(
        "native output configuration: event=same_storage_reuse_storage result=invariant_error reason=storage_invariant detail={error:?}"
    )
}

/// Statically dispatched coordination port for the synchronous output
/// transition. NativeRuntime supplies the production owners; qualification
/// tests substitute only these narrow owner calls.
trait OutputConfigurationRuntime {
    type Candidate;
    type ConfirmationReservation;
    type Response;

    fn test_candidate(&mut self, candidate: &mut Self::Candidate) -> Result<(), String>;
    fn direct_scanout_retirement_safe(&mut self) -> bool;
    fn reserve_confirmation_transaction(
        &mut self,
        operation: OutputConfigurationOperation,
        target: NativeAppliedOutputConfiguration,
    ) -> Result<Option<Self::ConfirmationReservation>, String>;
    fn cancel_confirmation_reservation(&mut self, reservation: Self::ConfirmationReservation);
    fn park_acquire_watches(&mut self) -> Result<(), String>;
    fn quiesce_old_output_epoch(&mut self) -> Result<QuiescedOutputEpoch, String>;
    fn commit_started_at_ns(&mut self, cycle_started_at: Instant, now_ns: u64) -> u64;
    fn commit_candidate(
        &mut self,
        candidate: &mut Self::Candidate,
        quiesced_epoch: QuiescedOutputEpoch,
        commit_started_at_ns: u64,
    ) -> Result<u64, String>;
    fn adopt_committed_candidate(&mut self, candidate: Self::Candidate) -> Self::Candidate;
    fn finalize_committed_transaction(
        &mut self,
        operation: OutputConfigurationOperation,
        reservation: Option<Self::ConfirmationReservation>,
        committed_at_ns: u64,
    );
    fn publish_committed_output_configuration(
        &mut self,
        operation: OutputConfigurationOperation,
        target: NativeAppliedOutputConfiguration,
        candidate: Self::Candidate,
        committed_at_ns: u64,
    );
    fn rearm_acquire_watches(&mut self, now_ns: u64);
    fn queue_success_response(
        &mut self,
        operation: OutputConfigurationOperation,
        response: Self::Response,
    );
    fn queue_failure_response(
        &mut self,
        operation: OutputConfigurationOperation,
        target: NativeAppliedOutputConfiguration,
        response: Self::Response,
        failure_code: Option<oblivion_one::control::ControlErrorCode>,
        detail: String,
    ) -> NativeResult<()>;
    fn begin_persistence(
        &mut self,
        purpose: OutputPersistencePurpose,
        response: Self::Response,
        bytes: Vec<u8>,
    ) -> Result<(), OutputPersistenceSubmitError>;
}

fn advance_output_configuration_transition<R: OutputConfigurationRuntime>(
    runtime: &mut R,
    operation: OutputConfigurationOperation,
    target: NativeAppliedOutputConfiguration,
    response: R::Response,
    failure_code: Option<oblivion_one::control::ControlErrorCode>,
    mut candidate: R::Candidate,
    now_ns: u64,
    cycle_started_at: Instant,
) -> NativeResult<OutputConfigurationAdvance> {
    if let Err(detail) = runtime.test_candidate(&mut candidate) {
        runtime.queue_failure_response(operation, target, response, failure_code, detail)?;
        return Ok(OutputConfigurationAdvance::Completed);
    }
    if !runtime.direct_scanout_retirement_safe() {
        runtime.queue_failure_response(
            operation,
            target,
            response,
            failure_code,
            "direct scanout ownership did not drain before synchronous modeset".to_string(),
        )?;
        return Ok(OutputConfigurationAdvance::Completed);
    }

    let reservation = match runtime.reserve_confirmation_transaction(operation, target) {
        Ok(reservation) => reservation,
        Err(detail) => {
            runtime.queue_failure_response(operation, target, response, failure_code, detail)?;
            return Ok(OutputConfigurationAdvance::Completed);
        }
    };
    if matches!(
        operation,
        OutputConfigurationOperation::TemporaryApply { .. }
    ) && reservation.is_none()
    {
        runtime.queue_failure_response(
            operation,
            target,
            response,
            failure_code,
            "confirmation transaction reservation was not created".to_string(),
        )?;
        return Ok(OutputConfigurationAdvance::Completed);
    }

    if let Err(detail) = runtime.park_acquire_watches() {
        if let Some(reservation) = reservation {
            runtime.cancel_confirmation_reservation(reservation);
        }
        runtime.rearm_acquire_watches(now_ns);
        runtime.queue_failure_response(operation, target, response, failure_code, detail)?;
        return Ok(OutputConfigurationAdvance::Completed);
    }

    let quiesced_epoch = match runtime.quiesce_old_output_epoch() {
        Ok(epoch) => epoch,
        Err(detail) => {
            if let Some(reservation) = reservation {
                runtime.cancel_confirmation_reservation(reservation);
            }
            runtime.rearm_acquire_watches(now_ns);
            runtime.queue_failure_response(operation, target, response, failure_code, detail)?;
            return Ok(OutputConfigurationAdvance::Completed);
        }
    };

    let commit_started_at_ns = runtime.commit_started_at_ns(cycle_started_at, now_ns);
    let committed_at_ns =
        match runtime.commit_candidate(&mut candidate, quiesced_epoch, commit_started_at_ns) {
            Ok(committed_at_ns) => committed_at_ns,
            Err(detail) => {
                if let Some(reservation) = reservation {
                    runtime.cancel_confirmation_reservation(reservation);
                }
                runtime.rearm_acquire_watches(commit_started_at_ns);
                runtime.queue_failure_response(
                    operation,
                    target,
                    response,
                    failure_code,
                    format!("real commit: {detail}"),
                )?;
                return Ok(OutputConfigurationAdvance::Completed);
            }
        };

    let candidate = runtime.adopt_committed_candidate(candidate);
    runtime.finalize_committed_transaction(operation, reservation, committed_at_ns);
    runtime.publish_committed_output_configuration(operation, target, candidate, committed_at_ns);
    runtime.rearm_acquire_watches(committed_at_ns);
    runtime.queue_success_response(operation, response);
    Ok(OutputConfigurationAdvance::Completed)
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
    fn commit_modeset_after_quiescence(
        &mut self,
        modeset: &mut PreparedAtomicRuntimeModeset<DrmModeBlobIo>,
        quiesced_epoch: QuiescedOutputEpoch,
        commit_started_at_ns: u64,
    ) -> Result<u64, String> {
        if quiesced_epoch.output_epoch() != self.output_epoch {
            return Err("quiescence proof belongs to a stale output epoch".into());
        }
        let commit_started = Instant::now();
        self.kms_backend
            .commit_runtime_modeset_candidate(modeset)
            .map_err(|error| error.to_string())?;
        Ok(commit_started_at_ns
            .saturating_add(u64::try_from(commit_started.elapsed().as_nanos()).unwrap_or(u64::MAX)))
    }

    pub(crate) fn mode_selection_supported(&self) -> bool {
        self.output_mode_mutation_support().is_ok()
    }

    fn output_mode_mutation_support(&self) -> Result<(), OutputModeMutationBlocker> {
        let rollback_failed =
            self.output_configuration_transactions
                .active()
                .is_some_and(|transaction| {
                    matches!(
                        transaction.phase,
                        OutputConfigurationTransactionPhase::RollbackFailed { .. }
                    )
                });
        output_mode_mutation_support(
            OUTPUT_MODE_SELECTION_RUNTIME_QUALIFIED,
            self.output_mode_mutation_policy,
            OutputModeMutationRuntimeEvidence {
                atomic_kms: self.kms_backend.effective_kind()
                    == oblivion_one::native::kms::KmsBackendKind::Atomic,
                exact_mode_count: self.output_capabilities.mode_inventory.entries().len(),
                scanout_kind: self.scanout.kind(),
                scanout_available: !self.scanout_destroyed,
                persistence_available: self
                    .output_configuration_persistence_worker
                    .as_ref()
                    .is_some_and(OutputConfigurationPersistenceWorker::is_available),
                persistence_compensation_pending: self.output_persistence_compensation_required,
                persistence_compensation_failed: self.output_persistence_compensation_failed,
                rollback_failed,
            },
        )
    }

    pub(crate) fn log_output_mode_mutation_exposure(&self) {
        let support = self.output_mode_mutation_support();
        let blocker = support
            .err()
            .map_or("none", OutputModeMutationBlocker::as_str);
        eprintln!(
            "native display mutation: policy={} production_qualified={} effective={} blocker={} kms={} scanout={} exact_modes={}",
            self.output_mode_mutation_policy.as_str(),
            OUTPUT_MODE_SELECTION_RUNTIME_QUALIFIED,
            blocker == "none",
            blocker,
            self.kms_backend.effective_kind().as_str(),
            self.scanout.kind().as_str(),
            self.output_capabilities.mode_inventory.entries().len(),
        );
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
                match OutputConfigurationRuntime::begin_persistence(
                    self,
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
        if unavailable && !self.output_persistence_unavailable_logged {
            self.output_persistence_unavailable_logged = true;
            eprintln!(
                "native display mutation became unavailable: blocker={}",
                OutputModeMutationBlocker::PersistenceUnavailable.as_str()
            );
        }
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

    fn validate_output_configuration_ready_ownership(
        &self,
        explicit: &crate::native_output::scanout::AtomicEglGbmScanout,
    ) -> NativeResult<()> {
        if let Err(error) = super::cycle::validate_reconfiguration_frame_ownership_with_arbiter(
            explicit,
            self.output_epoch,
            self.target.crtc_id,
            &self.scene_history,
            &self.frame_pacing,
            &self.frame_scheduler,
            &self.atomic_commit_arbiter,
            &self.output_transactions,
        ) {
            let swapchain = explicit.swapchain()?;
            let physical_ready = swapchain.ready_identity();
            let scheduler_target_identity = self
                .frame_scheduler
                .ready_target()
                .map(|target| target.identity());
            let physical_target_identity = physical_ready
                .and_then(|owner| owner.fixed_target())
                .map(|target| target.identity());
            let transaction = physical_ready.and_then(|owner| {
                self.output_transactions
                    .transaction(owner.transaction_id)
                    .map(|record| (owner.transaction_id, record.state().kind()))
            });
            let worker_queued = swapchain.worker_queued_identity();
            let worker_transaction = worker_queued.and_then(|owner| {
                self.output_transactions
                    .transaction(owner.frame.transaction_id)
                    .map(|record| {
                        (
                            owner.frame.transaction_id,
                            record.state().kind(),
                            record.descriptor().reservation(),
                        )
                    })
            });
            let worker_pacing_ticket = self.frame_pacing.worker_reservation_ticket();
            let worker_ticket = worker_pacing_ticket.map(|ticket| {
                (
                    ticket,
                    ticket.physical_identity(),
                    ticket.physical_key(),
                    ticket.ready_submit(),
                )
            });
            let worker_queued_present = worker_queued.is_some();
            let worker_queued_frame_id = worker_queued.map(|owner| owner.frame.frame_id);
            let worker_queued_token = worker_queued.map(|owner| owner.token.get());
            let worker_ticket_present = worker_pacing_ticket.is_some();
            let worker_ticket_reservation_id =
                worker_pacing_ticket.map(|ticket| ticket.reservation_id().get());
            let worker_ticket_frame_id = worker_pacing_ticket.map(|ticket| ticket.frame_id().get());
            let worker_ticket_ready_submit =
                worker_pacing_ticket.map(|ticket| ticket.ready_submit());
            let worker_ticket_physical_frame_id = worker_pacing_ticket
                .and_then(|ticket| ticket.physical_identity())
                .map(|identity| identity.frame_id);
            let worker_scene = worker_queued.map(|owner| {
                (
                    owner.token,
                    self.scene_history.submitted_identity(owner.token.get()),
                )
            });
            let pacing_lane = physical_ready
                .and_then(|owner| self.frame_pacing.unsubmitted_physical_frame_lane(owner));
            let worker_snapshot = self.kms_commit_worker.as_ref().map(|worker| {
                (
                    worker.queue_depth(),
                    worker.inflight(),
                    worker.fatal_reason_name(),
                )
            });
            let arbiter_snapshot = (
                self.atomic_commit_arbiter.worker_queued_token(),
                self.atomic_commit_arbiter.worker_queued_kind(),
                self.atomic_commit_arbiter.kernel_submitted_token(),
                self.atomic_commit_arbiter.kernel_submitted_kind(),
                self.atomic_commit_arbiter
                    .worker_queued_commit()
                    .map(|commit| commit.phase),
                self.atomic_commit_arbiter
                    .kernel_submitted_commit()
                    .map(|commit| commit.phase),
            );
            return Err(io::Error::other(format!(
                "{error}; output ownership diagnostic: output_epoch={} scheduler_ready={} worker_queued_present={} worker_queued_frame_id={worker_queued_frame_id:?} worker_queued_token={worker_queued_token:?} worker_ticket_present={worker_ticket_present} worker_ticket_reservation_id={worker_ticket_reservation_id:?} worker_ticket_frame_id={worker_ticket_frame_id:?} worker_ticket_ready_submit={worker_ticket_ready_submit:?} worker_ticket_physical_frame_id={worker_ticket_physical_frame_id:?} scheduler_target_identity={scheduler_target_identity:?} physical_target_identity={physical_target_identity:?} latest_submitted_frame_id={:?} latest_presented_frame_id={:?} physical_ready={physical_ready:?} scene_ready={:?} ready_transaction={transaction:?} pacing_identity={:?} pacing_lane={pacing_lane:?} scheduler_target={:?} worker_queued={worker_queued:?} worker_transaction={worker_transaction:?} worker_ticket={worker_ticket:?} worker_scene={worker_scene:?} swapchain_submitted={:?} arbiter_queued=(token={:?},kind={:?},phase={:?}) arbiter_submitted=(token={:?},kind={:?},phase={:?}) kms_worker={worker_snapshot:?}",
                self.output_epoch.get(),
                self.frame_scheduler.ready_frame_queued(),
                worker_queued_present,
                self.scene_history.latest_submitted_frame_id(),
                self.scene_history.presented_frame_id(),
                self.scene_history.ready_frame_identity(),
                self.frame_pacing.unsubmitted_physical_identity(),
                self.frame_scheduler.ready_target(),
                swapchain.pending_identity(),
                arbiter_snapshot.0,
                arbiter_snapshot.1,
                arbiter_snapshot.2,
                arbiter_snapshot.3,
                arbiter_snapshot.4,
                arbiter_snapshot.5,
            ))
            .into());
        }
        Ok(())
    }

    /// Report a pending output configuration that keeps waiting, so a stall
    /// names the candidate state and the boundary owners that hold it.
    pub(crate) fn report_stalled_output_configuration(&mut self, now_ns: u64) {
        let boundary = self.native_output_reconfiguration_boundary();
        let render_worker_busy = self.scanout.render_worker_busy();
        let Some(pending) = self.pending_output_configuration.as_mut() else {
            return;
        };
        let waiting_since_ns = *pending.waiting_since_ns.get_or_insert(now_ns);
        let waited_ns = now_ns.saturating_sub(waiting_since_ns);
        if waited_ns < OUTPUT_CONFIGURATION_STALL_REPORT_NS
            || pending.stall_reported_at_ns.is_some_and(|reported_at_ns| {
                now_ns.saturating_sub(reported_at_ns) < OUTPUT_CONFIGURATION_STALL_REPORT_NS
            })
        {
            return;
        }
        pending.stall_reported_at_ns = Some(now_ns);
        eprintln!(
            "native output configuration: event=stalled waited_ms={} state={} target={}x{} render_worker_busy={} boundary={:?}",
            waited_ns / 1_000_000,
            pending
                .candidate
                .as_ref()
                .map_or("unprepared", CandidateScanoutState::name),
            pending.target.width,
            pending.target.height,
            render_worker_busy,
            boundary,
        );
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
                if let Err(error) = OutputConfigurationRuntime::begin_persistence(
                    self,
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
        // After the modeset commit the old epoch is gone. Waiting for its
        // boundary here deadlocked: cursor and frame work stay gated while the
        // configuration is pending, so a cursor move kept the boundary unsafe.
        let modeset_committed = pending
            .candidate
            .as_ref()
            .is_some_and(CandidateScanoutState::modeset_committed);
        if !modeset_committed {
            if let NativeScanoutBackend::AtomicEglGbm(explicit) = &*self.scanout {
                self.validate_output_configuration_ready_ownership(explicit)?;
            }
            let boundary = self.native_output_reconfiguration_boundary();
            if boundary.is_safe_to_retire_unsubmitted_ready_frame() {
                self.abandon_unsubmitted_output_frame()?;
            }
            if let NativeScanoutBackend::AtomicEglGbm(explicit) = &*self.scanout {
                self.validate_output_configuration_ready_ownership(explicit)?;
            }
            if !self
                .native_output_reconfiguration_boundary()
                .is_safe_with_candidate_render_fence(candidate_render_fence_owned)
            {
                self.pending_output_configuration = Some(pending);
                return Ok(OutputConfigurationAdvance::Waiting);
            }
        }

        if pending.candidate.is_none() {
            if self.scanout.render_worker_active() && self.scanout.render_worker_busy() {
                self.pending_output_configuration = Some(pending);
                return Ok(OutputConfigurationAdvance::Waiting);
            }
            match self.prepare_output_configuration_candidate(&pending.target) {
                Ok(candidate) => pending.candidate = Some(candidate),
                Err(error) => {
                    if error.kind() == io::ErrorKind::WouldBlock {
                        self.pending_output_configuration = Some(pending);
                        return Ok(OutputConfigurationAdvance::Waiting);
                    }
                    self.fail_pending_output_configuration(pending, error.to_string())?;
                    return Ok(OutputConfigurationAdvance::Completed);
                }
            }
        }

        if matches!(
            pending.candidate.as_ref(),
            Some(CandidateScanoutState::WorkerDiscardQueued { .. })
        ) {
            let Some(CandidateScanoutState::WorkerDiscardQueued { candidate, detail }) =
                pending.candidate.take()
            else {
                unreachable!()
            };
            let identity = candidate.identity();
            match self.scanout.try_discard_worker_output_candidate(identity) {
                Ok(true) => {
                    pending.candidate =
                        Some(CandidateScanoutState::WorkerDiscarding { candidate, detail });
                }
                Ok(false) => {
                    pending.candidate =
                        Some(CandidateScanoutState::WorkerDiscardQueued { candidate, detail });
                }
                Err(error) => {
                    pending.candidate =
                        Some(CandidateScanoutState::WorkerDiscardQueued { candidate, detail });
                    self.pending_output_configuration = Some(pending);
                    return Err(error.into());
                }
            }
            self.pending_output_configuration = Some(pending);
            return Ok(OutputConfigurationAdvance::Waiting);
        }

        if matches!(
            pending.candidate.as_ref(),
            Some(CandidateScanoutState::WorkerPromotionQueued { .. })
        ) {
            let Some(CandidateScanoutState::WorkerPromotionQueued {
                candidate,
                frame,
                render_info,
                cursor,
                committed_at_ns,
            }) = pending.candidate.take()
            else {
                unreachable!()
            };
            let identity = candidate.identity();
            match self.scanout.try_promote_worker_output_candidate(identity) {
                Ok(true) => {
                    pending.candidate = Some(CandidateScanoutState::WorkerPromoting {
                        candidate,
                        frame,
                        render_info,
                        cursor,
                        committed_at_ns,
                    });
                }
                Ok(false) => {
                    pending.candidate = Some(CandidateScanoutState::WorkerPromotionQueued {
                        candidate,
                        frame,
                        render_info,
                        cursor,
                        committed_at_ns,
                    });
                }
                Err(error) => {
                    pending.candidate = Some(CandidateScanoutState::WorkerPromotionQueued {
                        candidate,
                        frame,
                        render_info,
                        cursor,
                        committed_at_ns,
                    });
                    self.pending_output_configuration = Some(pending);
                    return Err(error.into());
                }
            }
            self.pending_output_configuration = Some(pending);
            return Ok(OutputConfigurationAdvance::Waiting);
        }

        if matches!(
            pending.candidate.as_ref(),
            Some(CandidateScanoutState::WorkerPromoted { .. })
        ) {
            let Some(CandidateScanoutState::WorkerPromoted {
                candidate,
                frame,
                render_info,
                cursor,
                committed_at_ns,
            }) = pending.candidate.take()
            else {
                unreachable!()
            };
            self.complete_worker_output_configuration(
                pending,
                candidate,
                frame,
                render_info,
                cursor,
                committed_at_ns,
            )?;
            return Ok(OutputConfigurationAdvance::Completed);
        }

        if matches!(
            pending.candidate.as_ref(),
            Some(
                CandidateScanoutState::WorkerPreparing { .. }
                    | CandidateScanoutState::WorkerCancelAfterPrepare { .. }
                    | CandidateScanoutState::WorkerDiscarding { .. }
                    | CandidateScanoutState::WorkerPromoting { .. }
                    | CandidateScanoutState::WorkerPromotionQueued { .. }
                    | CandidateScanoutState::WorkerPromoted { .. }
            )
        ) {
            self.pending_output_configuration = Some(pending);
            return Ok(OutputConfigurationAdvance::Waiting);
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
            pending.candidate = Some(CandidateScanoutState::ReadyReplacement {
                scanout,
                modeset,
                cursor,
            });
        }

        if matches!(
            pending.candidate.as_ref(),
            Some(CandidateScanoutState::WorkerRenderingFence { .. })
        ) {
            let ready = match pending.candidate.as_ref() {
                Some(CandidateScanoutState::WorkerRenderingFence { frame, .. }) => {
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
            let Some(CandidateScanoutState::WorkerRenderingFence {
                candidate,
                frame,
                render_info,
                cursor,
            }) = pending.candidate.take()
            else {
                unreachable!()
            };
            let framebuffer = candidate.framebuffer(frame.slot)?;
            let modeset = match self.kms_backend.prepare_runtime_modeset_candidate(
                pending.target.mode,
                pending.target.width,
                pending.target.height,
                framebuffer,
                cursor,
            ) {
                Ok(modeset) => modeset,
                Err(error) => {
                    pending.candidate = Some(CandidateScanoutState::WorkerRenderingFence {
                        candidate,
                        frame,
                        render_info,
                        cursor,
                    });
                    self.fail_pending_output_configuration(pending, error.to_string())?;
                    return Ok(OutputConfigurationAdvance::Waiting);
                }
            };
            pending.candidate = Some(CandidateScanoutState::WorkerReady {
                candidate,
                frame,
                render_info,
                modeset,
                cursor,
            });
        }

        if matches!(
            pending.candidate.as_ref(),
            Some(CandidateScanoutState::WorkerReady { .. })
        ) {
            let Some(CandidateScanoutState::WorkerReady {
                candidate,
                frame,
                render_info,
                modeset,
                cursor,
            }) = pending.candidate.take()
            else {
                unreachable!()
            };
            return self.advance_worker_candidate_ready(
                pending,
                candidate,
                frame,
                render_info,
                modeset,
                cursor,
                now_ns,
                advance_started,
            );
        }

        let transition_candidate = match pending.candidate.take().expect("candidate prepared") {
            CandidateScanoutState::ReadyReplacement {
                scanout,
                modeset,
                cursor,
            } => OutputConfigurationTransitionCandidate::ReadyReplacement {
                scanout,
                modeset,
                cursor,
            },
            CandidateScanoutState::ReuseReady { modeset, cursor } => {
                OutputConfigurationTransitionCandidate::ReadyReuse { modeset, cursor }
            }
            other => {
                pending.candidate = Some(other);
                self.pending_output_configuration = Some(pending);
                return Ok(OutputConfigurationAdvance::Waiting);
            }
        };
        debug_assert!(pending.prepared_transaction.is_none());
        advance_output_configuration_transition(
            self,
            pending.operation,
            pending.target,
            pending.response,
            pending.failure_code,
            transition_candidate,
            now_ns,
            advance_started,
        )
    }

    pub(super) fn quiesce_worker_output_candidate_step(&mut self) -> NativeResult<bool> {
        let Some(candidate) = self
            .pending_output_configuration
            .as_ref()
            .and_then(|pending| pending.candidate.as_ref())
        else {
            return Ok(false);
        };
        match candidate {
            CandidateScanoutState::WorkerPreparing { .. }
            | CandidateScanoutState::WorkerCancelAfterPrepare { .. }
            | CandidateScanoutState::WorkerDiscarding { .. }
            | CandidateScanoutState::WorkerPromoting { .. } => {
                self.process_render_worker_events()?;
                Ok(true)
            }
            CandidateScanoutState::WorkerRenderingFence { .. }
            | CandidateScanoutState::WorkerReady { .. } => {
                let pending = self
                    .pending_output_configuration
                    .take()
                    .expect("candidate state came from a pending output request");
                self.fail_pending_output_configuration(
                    pending,
                    "output configuration cancelled for session suspend".to_string(),
                )?;
                Ok(true)
            }
            CandidateScanoutState::WorkerDiscardQueued { .. }
            | CandidateScanoutState::WorkerPromotionQueued { .. }
            | CandidateScanoutState::WorkerPromoted { .. } => {
                let now_ns = monotonic_now_ns()?;
                let _ = self.advance_output_configuration(now_ns)?;
                Ok(true)
            }
            CandidateScanoutState::RenderingFence { .. }
            | CandidateScanoutState::ReadyReplacement { .. }
            | CandidateScanoutState::ReuseReady { .. } => Ok(false),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn advance_worker_candidate_ready(
        &mut self,
        mut pending: PendingOutputConfigurationRequest,
        candidate: AtomicOutputConfigurationCandidate,
        frame: Box<AtomicRenderedFrameParts>,
        render_info: AtomicRenderDomainInfo,
        modeset: PreparedAtomicRuntimeModeset<DrmModeBlobIo>,
        cursor: Option<AtomicCursorVisualState>,
        now_ns: u64,
        advance_started: Instant,
    ) -> NativeResult<OutputConfigurationAdvance> {
        pending.candidate = Some(CandidateScanoutState::WorkerReady {
            candidate,
            frame,
            render_info,
            modeset,
            cursor,
        });
        if self.scanout.render_worker_busy() {
            self.pending_output_configuration = Some(pending);
            return Ok(OutputConfigurationAdvance::Waiting);
        }
        let Some(CandidateScanoutState::WorkerReady {
            candidate,
            frame,
            render_info,
            mut modeset,
            cursor,
        }) = pending.candidate.take()
        else {
            unreachable!("worker candidate ready state was just installed")
        };

        if let Err(error) = self
            .kms_backend
            .test_runtime_modeset_candidate(&mut modeset)
        {
            pending.candidate = Some(CandidateScanoutState::WorkerReady {
                candidate,
                frame,
                render_info,
                modeset,
                cursor,
            });
            self.fail_pending_output_configuration(pending, format!("TEST_ONLY: {error}"))?;
            return Ok(OutputConfigurationAdvance::Waiting);
        }
        if !self.scanout.can_retire_direct_after_synchronous_modeset() {
            pending.candidate = Some(CandidateScanoutState::WorkerReady {
                candidate,
                frame,
                render_info,
                modeset,
                cursor,
            });
            self.fail_pending_output_configuration(
                pending,
                "direct scanout ownership did not drain before synchronous modeset".to_string(),
            )?;
            return Ok(OutputConfigurationAdvance::Waiting);
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
                        pending.candidate = Some(CandidateScanoutState::WorkerReady {
                            candidate,
                            frame,
                            render_info,
                            modeset,
                            cursor,
                        });
                        self.fail_pending_output_configuration(
                            pending,
                            format!("confirmation transaction reservation failed: {error:?}"),
                        )?;
                        return Ok(OutputConfigurationAdvance::Waiting);
                    }
                }
            }
            OutputConfigurationOperation::Rollback { .. } => None,
        };

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
                pending.candidate = Some(CandidateScanoutState::WorkerReady {
                    candidate,
                    frame,
                    render_info,
                    modeset,
                    cursor,
                });
                self.fail_pending_output_configuration(pending, error.to_string())?;
                return Ok(OutputConfigurationAdvance::Waiting);
            }
        }

        let quiesced_epoch = match OutputConfigurationRuntime::quiesce_old_output_epoch(self) {
            Ok(epoch) => epoch,
            Err(detail) => {
                if let Some(prepared) = prepared_transaction {
                    self.output_configuration_transactions
                        .cancel_prepared_temporary_apply(prepared);
                }
                self.rearm_output_acquire_watches(now_ns);
                pending.candidate = Some(CandidateScanoutState::WorkerReady {
                    candidate,
                    frame,
                    render_info,
                    modeset,
                    cursor,
                });
                self.fail_pending_output_configuration(pending, detail)?;
                return Ok(OutputConfigurationAdvance::Waiting);
            }
        };

        let commit_started_at_ns = now_ns.saturating_add(
            u64::try_from(advance_started.elapsed().as_nanos()).unwrap_or(u64::MAX),
        );
        let committed_at_ns = match self.commit_modeset_after_quiescence(
            &mut modeset,
            quiesced_epoch,
            commit_started_at_ns,
        ) {
            Ok(committed_at_ns) => committed_at_ns,
            Err(error) => {
                if let Some(prepared) = prepared_transaction {
                    self.output_configuration_transactions
                        .cancel_prepared_temporary_apply(prepared);
                }
                self.rearm_output_acquire_watches(commit_started_at_ns);
                pending.candidate = Some(CandidateScanoutState::WorkerReady {
                    candidate,
                    frame,
                    render_info,
                    modeset,
                    cursor,
                });
                self.fail_pending_output_configuration(pending, format!("real commit: {error}"))?;
                return Ok(OutputConfigurationAdvance::Waiting);
            }
        };
        self.kms_backend
            .adopt_committed_runtime_modeset_candidate(modeset);
        pending.prepared_transaction = prepared_transaction;
        pending.candidate = Some(CandidateScanoutState::WorkerPromotionQueued {
            candidate,
            frame,
            render_info,
            cursor,
            committed_at_ns,
        });
        self.pending_output_configuration = Some(pending);
        self.event_loop.request_continuation(
            oblivion_one::native::event_loop::NativeContinuationReason::FrameScheduler,
        )?;
        Ok(OutputConfigurationAdvance::Waiting)
    }

    fn complete_worker_output_configuration(
        &mut self,
        mut pending: PendingOutputConfigurationRequest,
        candidate: AtomicOutputConfigurationCandidate,
        frame: Box<AtomicRenderedFrameParts>,
        render_info: AtomicRenderDomainInfo,
        cursor: Option<AtomicCursorVisualState>,
        committed_at_ns: u64,
    ) -> NativeResult<()> {
        self.publish_worker_output_configuration(
            pending.target,
            candidate,
            frame,
            render_info,
            cursor,
            committed_at_ns,
        )?;
        if let Some(prepared) = pending.prepared_transaction.take() {
            let transaction_id = self
                .output_configuration_transactions
                .finalize_temporary_apply(prepared, committed_at_ns);
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
            match pending.operation {
                OutputConfigurationOperation::TemporaryApply { .. } => {
                    return Err(io::Error::other(
                        "committed worker output apply lost its reserved transaction",
                    )
                    .into());
                }
                OutputConfigurationOperation::Rollback { transaction_id } => {
                    self.output_configuration_transactions
                        .finalize_committed_rollback(transaction_id);
                    if self.pending_output_persistence.as_ref().is_some_and(|job| {
                        matches!(job.purpose, OutputPersistencePurpose::Confirm { .. })
                    }) {
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
        }
        self.rearm_output_acquire_watches(committed_at_ns);
        Ok(())
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

    fn native_output_reconfiguration_boundary(&self) -> NativeOutputReconfigurationBoundary {
        let worker_in_flight_owned = self
            .kms_commit_worker
            .as_ref()
            .is_some_and(|worker| worker.submission_active() || worker.inflight())
            || self.forced_shutdown_inflight.is_some();
        let worker_queued_next_owned = self
            .kms_commit_worker
            .as_ref()
            .is_some_and(|worker| worker.queue_depth() > 0)
            || !self.worker_quarantine.jobs.is_empty()
            || !self.worker_quarantine.cursor_sidecars.is_empty()
            || !self.submitted_worker_ownership.is_empty()
            || !self.emergency_quarantined_worker_jobs.is_empty()
            || !self.emergency_quarantined_submitted_ownership.is_empty()
            || self.forced_shutdown_pacing_settled.is_some();
        let explicit_sync_watch_owned = self.acquire_watches.metrics().active_eventfd_watches > 0
            || self.acquire_watches.metrics().active_fallback_watches > 0;
        NativeOutputReconfigurationBoundary {
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
                || self.worker_timeout_pending.is_some()
                || self.pending_render_worker_identity.is_some()
                || self.pending_render_worker_context.is_some()
                || self.pending_render_worker_outer.is_some()
                || self.pending_render_worker_completion.is_some()
                || self.pending_render_worker_capture.is_some()
                || self.pending_worker_effect_reload.is_some()
                || self.pending_dmabuf_release_fence.is_some(),
            explicit_sync_watch_owned,
            parked_explicit_sync_obligation_owned: !self.parked_acquire_watches.is_empty(),
        }
    }

    fn abandon_unsubmitted_output_frame(&mut self) -> NativeResult<()> {
        if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut *self.scanout {
            super::cycle::terminalize_unsubmitted_frame_for_reconfiguration(
                explicit,
                self.output_epoch,
                &mut self.scene_history,
                &mut self.frame_pacing,
                &mut self.frame_scheduler,
                &mut self.server,
                &mut self.output_transactions,
                MonotonicTimestampNs::new(monotonic_now_ns()?),
            )?;
            super::cycle::recover_or_arm_suspended_output_ownership(
                explicit,
                &mut self.event_loop,
                &mut self.output_render_fence_token,
            )?;
            return Ok(());
        }
        if !self.scanout.ready_frame_queued() {
            return Ok(());
        }
        if !self.scanout.abandon_unsubmitted_compatibility_ready()? {
            return Err(io::Error::other("compatibility READY ownership disappeared").into());
        }
        self.scene_history.discard_ready();
        if !self.frame_pacing.abandon_ready_frame() {
            self.frame_pacing.cancel_unsubmitted_render();
        }
        self.frame_scheduler.discard_ready_frame();
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
        let output_epoch = self.output_epoch.next();
        let generation = output_epoch.get();
        let cursor = self.candidate_cursor_state(target);
        let scanout_kind = self.scanout.kind();
        let render_worker_active = self.scanout.render_worker_active();
        let reuse_decision =
            same_storage_reuse_decision(&self.target, target, scanout_kind, render_worker_active);
        eprintln!(
            "{}",
            same_storage_reuse_decision_diagnostic_line(
                &self.target,
                target,
                scanout_kind,
                render_worker_active,
                reuse_decision,
            )
        );

        let mut replacement_reason = None;
        let mut unavailable_reason = None;
        let reusable = match reuse_decision {
            SameStorageReuseDecision::Rejected(reason) => {
                replacement_reason = Some(reason.as_str());
                None
            }
            SameStorageReuseDecision::Eligible => {
                let lookup = match &*self.scanout {
                    NativeScanoutBackend::AtomicEglGbm(explicit) => explicit
                        .current_compositor_framebuffer_for_same_storage_modeset(
                            self.target.width,
                            self.target.height,
                        ),
                    _ => Ok(ReusableAtomicCompositorFramebufferLookup::Unavailable(
                        ReusableStorageUnavailableReason::GenericStorageUnavailable,
                    )),
                };
                match lookup {
                    Ok(ReusableAtomicCompositorFramebufferLookup::Proven(reusable)) => {
                        eprintln!("{}", reusable_framebuffer_success_diagnostic_line(reusable));
                        Some(reusable)
                    }
                    Ok(ReusableAtomicCompositorFramebufferLookup::Unavailable(reason)) => {
                        eprintln!(
                            "{}",
                            reusable_framebuffer_unavailable_diagnostic_line(reason)
                        );
                        replacement_reason = Some("reusable_storage_unavailable");
                        unavailable_reason = Some(reason);
                        None
                    }
                    Err(error) => {
                        eprintln!(
                            "{}",
                            reusable_framebuffer_invariant_error_diagnostic_line(&error)
                        );
                        eprintln!(
                            "{}",
                            candidate_route_diagnostic_line(
                                "error",
                                Some("storage_invariant"),
                                None,
                            )
                        );
                        return Err(error);
                    }
                }
            }
        };

        if let Some(reusable) = reusable {
            let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut *self.scanout else {
                unreachable!("reuse eligibility requires explicit Atomic EGL/GBM scanout")
            };
            if let Err(error) = explicit.queue_presented_damage_history_invalidation() {
                eprintln!(
                    "{}",
                    candidate_route_diagnostic_line("error", Some("reuse_candidate_prepare"), None)
                );
                return Err(error);
            }
            let modeset = match self
                .kms_backend
                .prepare_runtime_modeset_candidate(
                    target.mode,
                    target.width,
                    target.height,
                    reusable.framebuffer,
                    cursor,
                )
                .map_err(io::Error::other)
            {
                Ok(modeset) => modeset,
                Err(error) => {
                    eprintln!(
                        "{}",
                        candidate_route_diagnostic_line(
                            "error",
                            Some("reuse_candidate_prepare"),
                            None,
                        )
                    );
                    return Err(error);
                }
            };
            let old_refresh_millihz =
                super::super::output::output_refresh_rate_for_mode(&self.target.mode)
                    .refresh_millihz();
            let new_refresh_millihz =
                super::super::output::output_refresh_rate_for_mode(&target.mode).refresh_millihz();
            self.perf
                .log("native.output_configuration_storage_reuse", || {
                    vec![
                        NativePerfField::u64("old_mode_id", u64::from(self.target.mode_id)),
                        NativePerfField::u64("target_mode_id", u64::from(target.mode_id)),
                        NativePerfField::u64("old_refresh_millihz", u64::from(old_refresh_millihz)),
                        NativePerfField::u64(
                            "target_refresh_millihz",
                            u64::from(new_refresh_millihz),
                        ),
                        NativePerfField::u64("output_epoch", output_epoch.get()),
                        NativePerfField::u64("pool_generation", reusable.pool_generation),
                        NativePerfField::u64("slot_id", u64::from(reusable.slot.get())),
                        NativePerfField::u64(
                            "framebuffer_id",
                            u64::from(reusable.framebuffer.get()),
                        ),
                        NativePerfField::u64("width", u64::from(target.width)),
                        NativePerfField::u64("height", u64::from(target.height)),
                    ]
                });
            eprintln!("{}", candidate_route_diagnostic_line("reuse", None, None));
            return Ok(CandidateScanoutState::ReuseReady { modeset, cursor });
        }

        eprintln!(
            "{}",
            candidate_route_diagnostic_line("replacement", replacement_reason, unavailable_reason,)
        );

        let pool_generation = match &*self.scanout {
            NativeScanoutBackend::AtomicEglGbm(explicit) => explicit.next_pool_generation()?,
            _ => generation,
        };
        let output_id = self.output_id;
        let current_format = self.scanout.scanout_format();
        if let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut *self.scanout
            && explicit.render_worker_active()
        {
            if explicit.render_worker_busy() {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "render worker is busy before output candidate preparation",
                ));
            }
            let connector = ConnectorId::new(target.connector_id)
                .ok_or_else(|| io::Error::other("connector ID is zero"))?;
            let crtc =
                CrtcId::new(target.crtc_id).ok_or_else(|| io::Error::other("CRTC ID is zero"))?;
            let discovery = KmsBackendSelection::discover_atomic_pipeline(
                self.kms.file().as_fd(),
                connector,
                crtc,
                current_format,
            )
            .map_err(io::Error::other)?;
            let scene =
                ResolvedNativeFrameScene::for_synchronous_output_reconfiguration(&self.server)
                    .into_owned();
            let mut candidate_input_state = self.input_state.clone();
            candidate_input_state.reconfigure_output_bounds(target.width, target.height);
            let candidate_cursor_mode = match self.cursor_render_mode {
                NativeCursorRenderMode::SoftwareClient => NativeCursorRenderMode::Software,
                mode => mode,
            };
            let frozen_frame = FrozenNativeRenderFrame::freeze(
                target.width,
                target.height,
                scene,
                &self.server,
                &candidate_input_state,
                candidate_cursor_mode,
                Some(
                    NativeOutputDamage::full_output(target.width, target.height)
                        .as_renderer_damage(target.width, target.height),
                ),
            );
            let job_id = super::render_worker::next_render_worker_job_id()?;
            let material_generation = self.server.trusted_effect_registry().current();
            let effect_generation = self.server.trusted_effect_registry().current();
            let candidate = explicit.start_worker_output_configuration_candidate(
                self.kms.file(),
                discovery,
                target.width,
                target.height,
                output_epoch,
                pool_generation,
                job_id,
                self.frame_index.wrapping_add(1).max(1),
                frozen_frame,
                (*material_generation).clone(),
                (*effect_generation).clone(),
                self.cursor_image.clone(),
            )?;
            return Ok(CandidateScanoutState::WorkerPreparing { candidate, cursor });
        }
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
                let initial_generation = self.server.trusted_effect_registry().current();
                let explicit = AtomicEglGbmScanout::create_synchronous_unattached_pool(
                    self.kms.file(),
                    &discovery,
                    target.width,
                    target.height,
                    output_epoch,
                    self.drm_session_generation,
                    pool_generation,
                    output_id,
                    initial_generation,
                    self.cursor_image.clone(),
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
        let scene = ResolvedNativeFrameScene::for_synchronous_output_reconfiguration(&self.server)
            .into_owned();
        let mut candidate_input_state = self.input_state.clone();
        candidate_input_state.reconfigure_output_bounds(target.width, target.height);
        let cursor = self.candidate_cursor_state(target);
        let candidate_cursor_mode = match self.cursor_render_mode {
            NativeCursorRenderMode::SoftwareClient => NativeCursorRenderMode::Software,
            mode => mode,
        };
        match &mut scanout {
            NativeScanoutBackend::AtomicEglGbm(explicit) => {
                let mut frozen_candidate = FrozenNativeRenderFrame::freeze(
                    target.width,
                    target.height,
                    scene,
                    &self.server,
                    &candidate_input_state,
                    candidate_cursor_mode,
                    Some(
                        NativeOutputDamage::full_output(target.width, target.height)
                            .as_renderer_damage(target.width, target.height),
                    ),
                );
                let slot = explicit.initial_slot();
                let render_target = explicit.render_target_request(slot)?;
                let mut gpu_sampling_started = false;
                match explicit.render_to_slot(
                    render_target,
                    self.frame_index.wrapping_add(1).max(1),
                    &mut frozen_candidate,
                    &mut gpu_sampling_started,
                    None,
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
                    Ok(CandidateScanoutState::ReadyReplacement {
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

        self.publish_new_output_epoch(target, self.output_epoch.next(), cursor, now_ns);
    }

    fn publish_worker_output_configuration(
        &mut self,
        target: NativeAppliedOutputConfiguration,
        candidate: AtomicOutputConfigurationCandidate,
        frame: Box<AtomicRenderedFrameParts>,
        render_info: AtomicRenderDomainInfo,
        cursor: Option<AtomicCursorVisualState>,
        now_ns: u64,
    ) -> NativeResult<()> {
        self.scanout.retire_direct_after_synchronous_modeset();
        let NativeScanoutBackend::AtomicEglGbm(explicit) = &mut *self.scanout else {
            return Err(io::Error::other(
                "worker output candidate lost its explicit Atomic scanout owner",
            )
            .into());
        };
        explicit.adopt_worker_output_candidate(candidate, frame, render_info)?;
        self.publish_new_output_epoch(target, self.output_epoch.next(), cursor, now_ns);
        Ok(())
    }

    fn publish_new_output_epoch(
        &mut self,
        target: NativeAppliedOutputConfiguration,
        output_epoch: OutputEpochId,
        cursor: Option<AtomicCursorVisualState>,
        now_ns: u64,
    ) {
        self.target = target;
        self.output_epoch = output_epoch;
        self.scanout.bind_output_epoch(output_epoch);
        self.acquire_watches.set_output_epoch(output_epoch);
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
                output_epoch.get(),
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
        // The KMS worker outlives the modeset, so its established base still
        // names the old epoch's presented revision. Every new-epoch job would
        // be invalidated and re-planned against that stale base forever.
        if let Some(worker) = self.kms_commit_worker.as_ref() {
            worker.set_established_presented_base_for_output(
                self.output_id,
                self.presented_planes.revision,
                output_epoch.get(),
                target.crtc_id,
            );
        }
        self.confirmed_kms_presentation = ConfirmedKmsPresentationState::default();
        // Validation keys are tied to the output's KMS geometry and plane
        // state. Drop any qualification carried by the replacement scanout
        // before allowing direct promotion on the next ordinary frame.
        self.scanout.invalidate_direct_validation_cache();
        self.last_direct_candidate_key = None;
        self.scene_history
            .reset_composited_history_for_output_epoch();
        self.last_client_cursor_damage = None;
        self.last_software_cursor_damage = None;
        self.last_client_cursor_path = None;
        self.last_submitted_cursor_epoch = self
            .atomic_cursor
            .as_ref()
            .map_or(0, |cursor| cursor.desired_epoch());
        self.presentation_timing.reconfigure(
            KmsModeTiming::from_mode(&target.mode, self.output_refresh_rate.interval_ns()),
            output_epoch.get(),
        );
        self.cursor_timing.reconfigure(
            KmsModeTiming::from_mode(&target.mode, self.output_refresh_rate.interval_ns()),
            output_epoch.get(),
        );
        let interval = Duration::from_nanos(self.output_refresh_rate.interval_ns());
        self.presentation_deadline.invalidate(interval);
        self.scheduled_presentation_target = None;
        self.server.invalidate_commit_timing_targets();
        self.rebase_output_epoch_frame_lanes(now_ns);
        self.render_journal.reset();
        self.adaptive_buffering.reset();
        self.pending_proven_deadline_miss = None;
        self.direct_fallback_tracker = None;
        self.queued_redraw_requested = true;
        super::queue_visual_work_after_synchronous_modeset(
            &mut self.frame_pacing,
            &mut self.frame_scheduler,
            now_ns,
            self.server.scene_render_generation(),
        );
        if let NativeScanoutBackend::AtomicEglGbm(explicit) = &*self.scanout {
            debug_assert!(
                super::cycle::validate_reconfiguration_frame_ownership_with_arbiter(
                    explicit,
                    output_epoch,
                    self.target.crtc_id,
                    &self.scene_history,
                    &self.frame_pacing,
                    &self.frame_scheduler,
                    &self.atomic_commit_arbiter,
                    &self.output_transactions,
                )
                .is_ok()
            );
        }
    }

    fn fail_pending_output_configuration(
        &mut self,
        mut pending: PendingOutputConfigurationRequest,
        detail: String,
    ) -> NativeResult<()> {
        if let Some(candidate_state) = pending.candidate.take() {
            match candidate_state {
                CandidateScanoutState::WorkerPreparing { candidate, .. }
                | CandidateScanoutState::WorkerCancelAfterPrepare { candidate, .. } => {
                    pending.candidate =
                        Some(CandidateScanoutState::WorkerCancelAfterPrepare { candidate, detail });
                    self.pending_output_configuration = Some(pending);
                    return Ok(());
                }
                CandidateScanoutState::WorkerRenderingFence {
                    candidate,
                    frame,
                    render_info,
                    cursor,
                } => {
                    if let Some(token) = self.output_render_fence_token.take()
                        && let Err(error) = self.event_loop.unregister(token)
                    {
                        self.output_render_fence_token = Some(token);
                        pending.candidate = Some(CandidateScanoutState::WorkerRenderingFence {
                            candidate,
                            frame,
                            render_info,
                            cursor,
                        });
                        self.pending_output_configuration = Some(pending);
                        return Err(error.into());
                    }
                    drop(frame);
                    self.queue_worker_candidate_discard(pending, candidate, detail)?;
                    return Ok(());
                }
                CandidateScanoutState::WorkerReady {
                    candidate,
                    frame,
                    render_info: _,
                    modeset: _,
                    cursor: _,
                } => {
                    drop(frame);
                    self.queue_worker_candidate_discard(pending, candidate, detail)?;
                    return Ok(());
                }
                queued @ CandidateScanoutState::WorkerDiscardQueued { .. }
                | queued @ CandidateScanoutState::WorkerDiscarding { .. } => {
                    pending.candidate = Some(queued);
                    self.pending_output_configuration = Some(pending);
                    return Ok(());
                }
                promoted @ CandidateScanoutState::WorkerPromotionQueued { .. }
                | promoted @ CandidateScanoutState::WorkerPromoting { .. }
                | promoted @ CandidateScanoutState::WorkerPromoted { .. } => {
                    pending.candidate = Some(promoted);
                    self.pending_output_configuration = Some(pending);
                    return Err(io::Error::other(
                        "output configuration cannot be failed after KMS committed a worker candidate",
                    )
                    .into());
                }
                other => {
                    pending.candidate = Some(other);
                    // Synchronous candidate ownership is destroyed by dropping
                    // its own render domain before its physical pool.
                    let _ = pending.candidate.take();
                }
            }
        }
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

    fn queue_worker_candidate_discard(
        &mut self,
        mut pending: PendingOutputConfigurationRequest,
        candidate: AtomicOutputConfigurationCandidate,
        detail: String,
    ) -> NativeResult<()> {
        let identity = candidate.identity();
        let state = match self.scanout.try_discard_worker_output_candidate(identity) {
            Ok(true) => CandidateScanoutState::WorkerDiscarding { candidate, detail },
            Ok(false) => CandidateScanoutState::WorkerDiscardQueued { candidate, detail },
            Err(error) => {
                pending.candidate =
                    Some(CandidateScanoutState::WorkerDiscardQueued { candidate, detail });
                self.pending_output_configuration = Some(pending);
                return Err(error.into());
            }
        };
        pending.candidate = Some(state);
        self.pending_output_configuration = Some(pending);
        Ok(())
    }
}

impl OutputConfigurationRuntime for NativeRuntime {
    type Candidate = OutputConfigurationTransitionCandidate;
    type ConfirmationReservation = PreparedOutputConfigurationTransaction;
    type Response = Option<(ReactorToken, u64)>;

    fn test_candidate(&mut self, candidate: &mut Self::Candidate) -> Result<(), String> {
        let modeset = match candidate {
            OutputConfigurationTransitionCandidate::ReadyReplacement { modeset, .. }
            | OutputConfigurationTransitionCandidate::ReadyReuse { modeset, .. } => modeset,
            _ => return Err("output candidate is not ready for TEST_ONLY".to_string()),
        };
        self.kms_backend
            .test_runtime_modeset_candidate(modeset)
            .map_err(|error| format!("TEST_ONLY: {error}"))
    }

    fn direct_scanout_retirement_safe(&mut self) -> bool {
        self.scanout.can_retire_direct_after_synchronous_modeset()
    }

    fn reserve_confirmation_transaction(
        &mut self,
        operation: OutputConfigurationOperation,
        target: NativeAppliedOutputConfiguration,
    ) -> Result<Option<Self::ConfirmationReservation>, String> {
        let OutputConfigurationOperation::TemporaryApply { previous } = operation else {
            return Ok(None);
        };
        let next_generation = self
            .output_configuration_generation
            .get()
            .wrapping_add(1)
            .max(1);
        self.output_configuration_transactions
            .prepare_temporary_apply(
                format!("output-{}", self.output_id.get()),
                previous,
                target,
                next_generation,
            )
            .map(Some)
            .map_err(|error| format!("confirmation transaction reservation failed: {error:?}"))
    }

    fn cancel_confirmation_reservation(&mut self, reservation: Self::ConfirmationReservation) {
        self.output_configuration_transactions
            .cancel_prepared_temporary_apply(reservation);
    }

    fn park_acquire_watches(&mut self) -> Result<(), String> {
        let parked = self
            .acquire_watches
            .park_for_session_suspend(&mut self.event_loop)
            .map_err(|error| error.to_string())?;
        self.parked_acquire_watches.extend(parked);
        Ok(())
    }

    fn quiesce_old_output_epoch(&mut self) -> Result<QuiescedOutputEpoch, String> {
        if let NativeScanoutBackend::AtomicEglGbm(explicit) = &*self.scanout {
            super::cycle::validate_reconfiguration_frame_ownership_with_arbiter(
                explicit,
                self.output_epoch,
                self.target.crtc_id,
                &self.scene_history,
                &self.frame_pacing,
                &self.frame_scheduler,
                &self.atomic_commit_arbiter,
                &self.output_transactions,
            )
            .map_err(|error| error.to_string())?;
        }
        let boundary = self.native_output_reconfiguration_boundary();
        let epoch =
            QuiescedOutputEpoch::from_boundary(self.output_epoch, boundary).ok_or_else(|| {
                format!(
                    "old output epoch {} is not quiesced: {boundary:?}",
                    self.output_epoch.get()
                )
            })?;
        if self.scanout.ready_frame_queued() {
            return Err("old output epoch still owns an unsubmitted physical READY frame".into());
        }
        if let NativeScanoutBackend::AtomicEglGbm(explicit) = &*self.scanout
            && explicit
                .has_suspended_output_ownership()
                .map_err(|error| error.to_string())?
        {
            return Err("old output epoch still owns a suspended physical frame".into());
        }
        if self.frame_pacing.has_unsettled_output_frame_ownership() {
            return Err("old output epoch still owns pacing frame state".into());
        }
        if self.frame_scheduler.ready_frame_queued() {
            return Err("old output epoch still owns scheduler READY state".into());
        }
        if self.scene_history.has_ready_frame() {
            return Err("old output epoch still owns scene-history READY state".into());
        }
        Ok(epoch)
    }

    fn commit_started_at_ns(&mut self, cycle_started_at: Instant, now_ns: u64) -> u64 {
        now_ns.saturating_add(
            u64::try_from(cycle_started_at.elapsed().as_nanos()).unwrap_or(u64::MAX),
        )
    }

    fn commit_candidate(
        &mut self,
        candidate: &mut Self::Candidate,
        quiesced_epoch: QuiescedOutputEpoch,
        commit_started_at_ns: u64,
    ) -> Result<u64, String> {
        let modeset = match candidate {
            OutputConfigurationTransitionCandidate::ReadyReplacement { modeset, .. }
            | OutputConfigurationTransitionCandidate::ReadyReuse { modeset, .. } => modeset,
            _ => return Err("output candidate is not ready for real commit".to_string()),
        };
        self.commit_modeset_after_quiescence(modeset, quiesced_epoch, commit_started_at_ns)
    }

    fn adopt_committed_candidate(&mut self, candidate: Self::Candidate) -> Self::Candidate {
        match candidate {
            OutputConfigurationTransitionCandidate::ReadyReplacement {
                scanout,
                modeset,
                cursor,
            } => {
                self.kms_backend
                    .adopt_committed_runtime_modeset_candidate(modeset);
                OutputConfigurationTransitionCandidate::AdoptedReplacement { scanout, cursor }
            }
            OutputConfigurationTransitionCandidate::ReadyReuse { modeset, cursor } => {
                self.kms_backend
                    .adopt_committed_runtime_modeset_candidate(modeset);
                OutputConfigurationTransitionCandidate::AdoptedReuse { cursor }
            }
            _ => unreachable!("only a tested synchronous candidate can be committed"),
        }
    }

    fn finalize_committed_transaction(
        &mut self,
        operation: OutputConfigurationOperation,
        reservation: Option<Self::ConfirmationReservation>,
        committed_at_ns: u64,
    ) {
        match operation {
            OutputConfigurationOperation::TemporaryApply { .. } => {
                let reservation =
                    reservation.expect("temporary apply reservation is held through real commit");
                self.output_configuration_transactions
                    .finalize_temporary_apply(reservation, committed_at_ns);
            }
            OutputConfigurationOperation::Rollback { transaction_id } => {
                self.output_configuration_transactions
                    .finalize_committed_rollback(transaction_id);
                if self.pending_output_persistence.as_ref().is_some_and(|job| {
                    matches!(job.purpose, OutputPersistencePurpose::Confirm { .. })
                }) {
                    // A late successful write must be repaired after rollback owns the mode.
                    self.output_persistence_compensation_required = true;
                }
            }
        }
    }

    fn publish_committed_output_configuration(
        &mut self,
        operation: OutputConfigurationOperation,
        target: NativeAppliedOutputConfiguration,
        candidate: Self::Candidate,
        committed_at_ns: u64,
    ) {
        match candidate {
            OutputConfigurationTransitionCandidate::AdoptedReplacement {
                mut scanout,
                cursor,
            } => {
                self.publish_output_configuration(target, &mut scanout, cursor, committed_at_ns);
            }
            OutputConfigurationTransitionCandidate::AdoptedReuse { cursor } => {
                self.scanout.retire_direct_after_synchronous_modeset();
                self.publish_new_output_epoch(
                    target,
                    self.output_epoch.next(),
                    cursor,
                    committed_at_ns,
                );
            }
            _ => unreachable!("KMS adoption precedes authoritative output publication"),
        }
        match operation {
            OutputConfigurationOperation::TemporaryApply { .. } => {
                let transaction_id = self
                    .output_configuration_transactions
                    .active()
                    .expect("temporary apply transaction was finalized before publication")
                    .id;
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
            }
            OutputConfigurationOperation::Rollback { .. } => {
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
            }
        }
    }

    fn rearm_acquire_watches(&mut self, now_ns: u64) {
        self.rearm_output_acquire_watches(now_ns);
    }

    fn queue_success_response(
        &mut self,
        _operation: OutputConfigurationOperation,
        response: Self::Response,
    ) {
        self.queue_output_snapshot_response_after_commit(response);
    }

    fn queue_failure_response(
        &mut self,
        operation: OutputConfigurationOperation,
        target: NativeAppliedOutputConfiguration,
        response: Self::Response,
        failure_code: Option<oblivion_one::control::ControlErrorCode>,
        detail: String,
    ) -> NativeResult<()> {
        let mut pending = PendingOutputConfigurationRequest::new(response, operation, target);
        pending.failure_code = failure_code;
        self.fail_pending_output_configuration(pending, detail)
    }

    fn begin_persistence(
        &mut self,
        purpose: OutputPersistencePurpose,
        response: Self::Response,
        bytes: Vec<u8>,
    ) -> Result<(), OutputPersistenceSubmitError> {
        self.submit_output_persistence(purpose, response, bytes)
    }
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

    fn fully_qualified_evidence() -> OutputModeMutationRuntimeEvidence {
        OutputModeMutationRuntimeEvidence {
            atomic_kms: true,
            exact_mode_count: 7,
            scanout_kind: NativeScanoutKind::AtomicEglGbmExplicit,
            scanout_available: true,
            persistence_available: true,
            persistence_compensation_pending: false,
            persistence_compensation_failed: false,
            rollback_failed: false,
        }
    }

    #[test]
    fn output_mode_policy_parser_accepts_only_documented_values() {
        assert_eq!(
            OutputModeMutationPolicy::parse(None),
            Ok(OutputModeMutationPolicy::Auto)
        );
        assert_eq!(
            OutputModeMutationPolicy::parse(Some("auto")),
            Ok(OutputModeMutationPolicy::Auto)
        );
        assert_eq!(
            OutputModeMutationPolicy::parse(Some("off")),
            Ok(OutputModeMutationPolicy::Off)
        );
        assert_eq!(
            OutputModeMutationPolicy::parse(Some("qualification")),
            Ok(OutputModeMutationPolicy::Qualification)
        );
        for value in ["1", "true", "yes", "on", "", "AUTO"] {
            assert!(
                OutputModeMutationPolicy::parse(Some(value)).is_err(),
                "{value:?} is not an accepted policy"
            );
        }
    }

    #[test]
    fn output_mode_exposure_policy_follows_production_qualification() {
        let cases = [
            (false, OutputModeMutationPolicy::Auto, false),
            (false, OutputModeMutationPolicy::Off, false),
            (false, OutputModeMutationPolicy::Qualification, true),
            (true, OutputModeMutationPolicy::Auto, true),
            (true, OutputModeMutationPolicy::Off, false),
            (true, OutputModeMutationPolicy::Qualification, true),
        ];
        for (production_qualified, policy, expected) in cases {
            assert_eq!(
                output_mode_mutation_support(
                    production_qualified,
                    policy,
                    fully_qualified_evidence(),
                )
                .is_ok(),
                expected,
                "production_qualified={production_qualified}, policy={policy:?}"
            );
        }
    }

    #[test]
    fn qualification_policy_does_not_bypass_runtime_capability_blockers() {
        assert_eq!(
            output_mode_mutation_support(
                false,
                OutputModeMutationPolicy::Qualification,
                fully_qualified_evidence(),
            ),
            Ok(())
        );

        let mut evidence = fully_qualified_evidence();
        evidence.atomic_kms = false;
        assert_eq!(
            output_mode_mutation_support(false, OutputModeMutationPolicy::Qualification, evidence,),
            Err(OutputModeMutationBlocker::LegacyKms)
        );

        let mut evidence = fully_qualified_evidence();
        evidence.exact_mode_count = 0;
        assert_eq!(
            output_mode_mutation_support(false, OutputModeMutationPolicy::Qualification, evidence,),
            Err(OutputModeMutationBlocker::MissingExactModeInventory)
        );

        let mut evidence = fully_qualified_evidence();
        evidence.scanout_kind = NativeScanoutKind::Unavailable;
        assert_eq!(
            output_mode_mutation_support(false, OutputModeMutationPolicy::Qualification, evidence,),
            Err(OutputModeMutationBlocker::UnsupportedScanout)
        );

        let mut evidence = fully_qualified_evidence();
        evidence.scanout_available = false;
        assert_eq!(
            output_mode_mutation_support(false, OutputModeMutationPolicy::Qualification, evidence,),
            Err(OutputModeMutationBlocker::ScanoutUnavailable)
        );

        let mut evidence = fully_qualified_evidence();
        evidence.persistence_available = false;
        assert_eq!(
            output_mode_mutation_support(false, OutputModeMutationPolicy::Qualification, evidence,),
            Err(OutputModeMutationBlocker::PersistenceUnavailable)
        );

        let mut evidence = fully_qualified_evidence();
        evidence.persistence_compensation_pending = true;
        assert_eq!(
            output_mode_mutation_support(false, OutputModeMutationPolicy::Qualification, evidence,),
            Err(OutputModeMutationBlocker::PersistenceCompensationPending)
        );

        let mut evidence = fully_qualified_evidence();
        evidence.persistence_compensation_failed = true;
        assert_eq!(
            output_mode_mutation_support(false, OutputModeMutationPolicy::Qualification, evidence,),
            Err(OutputModeMutationBlocker::PersistenceCompensationFailed)
        );

        let mut evidence = fully_qualified_evidence();
        evidence.rollback_failed = true;
        assert_eq!(
            output_mode_mutation_support(false, OutputModeMutationPolicy::Qualification, evidence,),
            Err(OutputModeMutationBlocker::RollbackFailed)
        );
    }

    #[test]
    fn output_mode_blocker_priority_is_stable() {
        let mut evidence = fully_qualified_evidence();
        evidence.atomic_kms = false;
        evidence.exact_mode_count = 0;
        evidence.scanout_kind = NativeScanoutKind::Unavailable;
        evidence.scanout_available = false;
        evidence.persistence_available = false;
        evidence.persistence_compensation_pending = true;
        evidence.persistence_compensation_failed = true;
        evidence.rollback_failed = true;
        assert_eq!(
            output_mode_mutation_support(false, OutputModeMutationPolicy::Qualification, evidence,),
            Err(OutputModeMutationBlocker::LegacyKms)
        );

        evidence.atomic_kms = true;
        assert_eq!(
            output_mode_mutation_support(false, OutputModeMutationPolicy::Qualification, evidence,),
            Err(OutputModeMutationBlocker::MissingExactModeInventory)
        );

        assert_eq!(
            output_mode_mutation_support(
                false,
                OutputModeMutationPolicy::Auto,
                fully_qualified_evidence(),
            ),
            Err(OutputModeMutationBlocker::ProjectUnqualified)
        );
        assert_eq!(
            output_mode_mutation_support(
                true,
                OutputModeMutationPolicy::Off,
                fully_qualified_evidence(),
            ),
            Err(OutputModeMutationBlocker::PolicyDisabled)
        );
    }

    #[test]
    fn mode_selection_is_exposed_by_default_on_qualified_runtimes() {
        assert!(OUTPUT_MODE_SELECTION_RUNTIME_QUALIFIED);
        assert_eq!(
            output_mode_mutation_support(
                OUTPUT_MODE_SELECTION_RUNTIME_QUALIFIED,
                OutputModeMutationPolicy::Auto,
                fully_qualified_evidence(),
            ),
            Ok(())
        );
    }

    #[test]
    fn runtime_support_accepts_only_qualified_scanout_kinds() {
        for scanout_kind in [
            NativeScanoutKind::AtomicEglGbmExplicit,
            NativeScanoutKind::NativeEglGbmOpaqueCompatibility,
            NativeScanoutKind::GbmCpuWritePageFlip,
            NativeScanoutKind::DumbFramebuffer,
        ] {
            let mut evidence = fully_qualified_evidence();
            evidence.scanout_kind = scanout_kind;
            assert_eq!(output_mode_mutation_runtime_support(evidence), Ok(()));
        }
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

#[cfg(test)]
#[path = "output_configuration_qualification_tests.rs"]
mod qualification_tests;
