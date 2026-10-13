//! Startup policy and reactor-side admission state for the Atomic render worker.

use super::atomic_render_domain::{
    AtomicRenderDomain, AtomicRenderDomainBootstrap, AtomicRenderDomainInfo,
    AtomicRenderedFrameParts, AtomicSlotRenderOutcome,
};
use crate::native_output::{
    runtime::FrozenNativeRenderFrame,
    scanout::{AtomicRenderPhaseDurations, AtomicRenderSlotDescriptor, RenderTargetRequest},
};
use std::{
    error::Error,
    fmt, io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use crate::egl_renderer::native_fence::NativeRenderFence;
use crate::egl_renderer::{EglSceneFrameCommit, OutputDamage};
use crate::native_output::presentation::transaction::OutputTransactionId;
use oblivion_one::cursor_theme::CompositorCursorImage;
use oblivion_one::effects::{EffectRegistryGeneration, PreparedEffectRegistryReload};
use oblivion_one::native::OutputEpochId;
use oblivion_one::native::kms::DrmFormatModifierPair;

pub(crate) type AtomicRenderWorkerDiscovery =
    Vec<oblivion_one::render_backend::egl_gles::EglGlesDmabufFormat>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicRenderWorkerPolicy {
    Off,
    Auto,
    Force,
}

impl Default for AtomicRenderWorkerPolicy {
    fn default() -> Self {
        Self::Off
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AtomicRenderWorkerPolicyError(String);

impl fmt::Display for AtomicRenderWorkerPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid OBLIVION_ONE_RENDER_WORKER={:?}; expected off, auto, or force",
            self.0
        )
    }
}

impl Error for AtomicRenderWorkerPolicyError {}

impl AtomicRenderWorkerPolicy {
    pub(crate) fn parse(value: Option<&str>) -> Result<Self, AtomicRenderWorkerPolicyError> {
        match value {
            None | Some("off") => Ok(Self::Off),
            Some("auto") => Ok(Self::Auto),
            Some("force") => Ok(Self::Force),
            Some(value) => Err(AtomicRenderWorkerPolicyError(value.to_owned())),
        }
    }

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Force => "force",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicRenderWorkerStartup {
    IntentionallySynchronous,
    WorkerStarted,
    AutomaticStartupDegraded,
}

impl AtomicRenderWorkerStartup {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::IntentionallySynchronous => "intentionally_synchronous",
            Self::WorkerStarted => "worker_started",
            Self::AutomaticStartupDegraded => "automatic_startup_degraded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct RenderWorkerJobId(std::num::NonZeroU64);

impl RenderWorkerJobId {
    pub(crate) fn new(value: u64) -> Option<Self> {
        std::num::NonZeroU64::new(value).map(Self)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0.get()
    }
}

pub(crate) struct SequencedRenderControl {
    pub(crate) epoch: u64,
    pub(crate) control: AtomicRenderControl,
}

pub(crate) struct AtomicRenderWorkerJob {
    pub(crate) job_id: RenderWorkerJobId,
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) output_epoch: OutputEpochId,
    pub(crate) render_generation: u64,
    pub(crate) target: RenderTargetRequest,
    pub(crate) frozen_frame: FrozenNativeRenderFrame,
    pub(crate) need_no_visual_release_fence: bool,
    pub(crate) dispatch_at_ns: u64,
    pub(crate) required_control_epoch: u64,
    pub(crate) controls_before: Vec<SequencedRenderControl>,
}

/// The first output frame is rendered during process bootstrap, before the
/// reactor enters its event loop. It still executes on the worker thread, but
/// its completion uses a bounded startup reply instead of reactor eventfd
/// settlement because no output transaction exists yet.
pub(crate) struct AtomicRenderWorkerInitialFrame {
    pub(crate) target: RenderTargetRequest,
    pub(crate) frame_id: u64,
    pub(crate) frozen_frame: FrozenNativeRenderFrame,
    pub(crate) required_control_epoch: u64,
    pub(crate) controls_before: Vec<SequencedRenderControl>,
}

type AtomicRenderWorkerInitialFrameResult =
    io::Result<(AtomicSlotRenderOutcome, FrozenNativeRenderFrame)>;

/// A candidate output pool is imported and rendered on the existing render
/// thread. The reactor keeps the matching physical pool until a discard or
/// promotion acknowledgement proves its EGL imports have been destroyed or
/// become the active domain.
pub(crate) struct AtomicRenderCandidateJob {
    pub(crate) identity: AtomicRenderCandidateIdentity,
    pub(crate) render_device_fd: OwnedFd,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) format_modifier: DrmFormatModifierPair,
    pub(crate) descriptors: Vec<AtomicRenderSlotDescriptor>,
    pub(crate) physical_slot_identities: Vec<(super::OutputSlotId, u64)>,
    pub(crate) target: RenderTargetRequest,
    pub(crate) frame_id: u64,
    pub(crate) frozen_frame: FrozenNativeRenderFrame,
    pub(crate) material_generation: EffectRegistryGeneration,
    pub(crate) effect_generation: EffectRegistryGeneration,
    pub(crate) cursor_image: Arc<CompositorCursorImage>,
    pub(crate) required_control_epoch: u64,
    pub(crate) controls_before: Vec<SequencedRenderControl>,
}

pub(crate) enum AtomicRenderControl {
    CommitPresented {
        frame: EglSceneFrameCommit,
        presented_transition_damage: OutputDamage,
    },
    DiscardRendered {
        frame: EglSceneFrameCommit,
    },
    InvalidatePresentedDamageHistory,
    FrameSwapFailed,
    PublishMaterialEffectGeneration(EffectRegistryGeneration),
    PublishEffectRegistryGeneration(EffectRegistryGeneration),
    SetCursorImage(Arc<CompositorCursorImage>),
    RebindPoolGeneration(u64),
    FinishRenderWork,
}

pub(crate) struct AtomicRenderWorkerCompletion {
    pub(crate) job_id: RenderWorkerJobId,
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) output_epoch: OutputEpochId,
    pub(crate) render_generation: u64,
    pub(crate) target: RenderTargetRequest,
    pub(crate) control_epoch: u64,
    pub(crate) result: io::Result<AtomicSlotRenderOutcome>,
    pub(crate) frozen_frame: FrozenNativeRenderFrame,
    pub(crate) gpu_sampling_started: bool,
    pub(crate) no_visual_release_fence: Option<NativeRenderFence>,
    pub(crate) phase_durations: AtomicRenderPhaseDurations,
    pub(crate) dispatch_at_ns: u64,
    pub(crate) worker_started_at_ns: Option<u64>,
    pub(crate) worker_finished_at_ns: Option<u64>,
    pub(crate) worker_thread_cpu_ns: Option<u64>,
    pub(crate) worker_wall_ns: u64,
}

pub(crate) struct AtomicRenderWorkerCapture {
    pub(crate) job_id: RenderWorkerJobId,
    pub(crate) frozen_frame: FrozenNativeRenderFrame,
    pub(crate) required_control_epoch: u64,
    pub(crate) controls_before: Vec<SequencedRenderControl>,
}

pub(crate) struct AtomicRenderWorkerReleaseFence {
    pub(crate) job_id: RenderWorkerJobId,
    pub(crate) required_control_epoch: u64,
    pub(crate) controls_before: Vec<SequencedRenderControl>,
}

pub(crate) struct AtomicRenderWorkerEffectReload {
    pub(crate) job_id: RenderWorkerJobId,
    pub(crate) previous_generation: Arc<EffectRegistryGeneration>,
    pub(crate) manifest: Option<(String, oblivion_one::effects::EffectManifest)>,
    pub(crate) required_control_epoch: u64,
    pub(crate) controls_before: Vec<SequencedRenderControl>,
}

pub(crate) struct PreparedWorkerEffectReload {
    pub(crate) manifest_path: String,
    pub(crate) prepared: PreparedEffectRegistryReload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AtomicRenderWorkerIdentity {
    pub(crate) job_id: RenderWorkerJobId,
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) output_epoch: OutputEpochId,
    pub(crate) render_generation: u64,
    pub(crate) control_epoch: u64,
    pub(crate) target: RenderTargetRequest,
}

impl AtomicRenderWorkerIdentity {
    pub(crate) fn belongs_to_output_epoch(self, output_epoch: OutputEpochId) -> bool {
        self.output_epoch == output_epoch
    }
}

/// Exact identity for a render-domain candidate which is being prepared for
/// output reconfiguration. Candidate domains and their physical pools may
/// coexist with the active generation, so the acknowledgement must name both
/// the job and the slot-pool generation it imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AtomicRenderCandidateIdentity {
    pub(crate) job_id: RenderWorkerJobId,
    pub(crate) output_epoch: OutputEpochId,
    pub(crate) pool_generation: u64,
    pub(crate) slot: super::OutputSlotId,
}

impl AtomicRenderCandidateIdentity {
    pub(crate) const fn new(
        job_id: RenderWorkerJobId,
        output_epoch: OutputEpochId,
        pool_generation: u64,
        slot: super::OutputSlotId,
    ) -> Self {
        Self {
            job_id,
            output_epoch,
            pool_generation,
            slot,
        }
    }

    pub(crate) fn matches(self, actual: Self) -> bool {
        self.job_id.get() == actual.job_id.get()
            && self.output_epoch == actual.output_epoch
            && self.pool_generation == actual.pool_generation
            && self.slot.get() == actual.slot.get()
    }
}

impl AtomicRenderWorkerCompletion {
    pub(crate) const fn identity(&self) -> AtomicRenderWorkerIdentity {
        AtomicRenderWorkerIdentity {
            job_id: self.job_id,
            frame_id: self.frame_id,
            transaction_id: self.transaction_id,
            output_epoch: self.output_epoch,
            render_generation: self.render_generation,
            control_epoch: self.control_epoch,
            target: self.target,
        }
    }
}

pub(crate) fn completion_matches_job(
    expected: AtomicRenderWorkerIdentity,
    actual: AtomicRenderWorkerIdentity,
) -> bool {
    expected.job_id.get() == actual.job_id.get()
        && expected.frame_id == actual.frame_id
        && expected.transaction_id.get() == actual.transaction_id.get()
        && expected.output_epoch == actual.output_epoch
        && expected.render_generation == actual.render_generation
        && expected.control_epoch == actual.control_epoch
        && expected.target.slot.get() == actual.target.slot.get()
        && expected.target.pool_generation == actual.target.pool_generation
        && expected.target.buffer_age == actual.target.buffer_age
}

pub(crate) enum AtomicRenderWorkerEvent {
    Rendered(AtomicRenderWorkerCompletion),
    CandidatePrepared {
        identity: AtomicRenderCandidateIdentity,
        result: io::Result<(Box<AtomicRenderedFrameParts>, AtomicRenderDomainInfo)>,
    },
    CandidatePromoted {
        identity: AtomicRenderCandidateIdentity,
    },
    CandidateDiscarded {
        identity: AtomicRenderCandidateIdentity,
    },
    ReleaseFenceReady {
        job_id: RenderWorkerJobId,
        control_epoch: u64,
        result: io::Result<NativeRenderFence>,
    },
    Captured {
        job_id: RenderWorkerJobId,
        control_epoch: u64,
        result: io::Result<Vec<u8>>,
    },
    EffectsReloaded {
        job_id: RenderWorkerJobId,
        control_epoch: u64,
        result: Result<Option<PreparedWorkerEffectReload>, String>,
    },
    Fatal {
        job_id: Option<RenderWorkerJobId>,
        gpu_execution_uncertain: bool,
        reason: &'static str,
    },
}

struct RenderWorkerInit {
    width: u32,
    height: u32,
    pool_generation: u64,
    format_modifier: DrmFormatModifierPair,
    descriptors: Vec<AtomicRenderSlotDescriptor>,
    physical_slot_identities: Vec<(super::OutputSlotId, u64)>,
    material_generation: Arc<EffectRegistryGeneration>,
    effect_generation: Arc<EffectRegistryGeneration>,
    cursor_image: Arc<CompositorCursorImage>,
}

enum RenderWorkerCommand {
    Initialize {
        init: RenderWorkerInit,
        response: SyncSender<io::Result<AtomicRenderDomainInfo>>,
    },
    Render(AtomicRenderWorkerJob),
    RenderInitialFrame {
        initial: AtomicRenderWorkerInitialFrame,
        response: SyncSender<AtomicRenderWorkerInitialFrameResult>,
    },
    CreateReleaseFence(AtomicRenderWorkerReleaseFence),
    PrepareCandidate(AtomicRenderCandidateJob),
    PromoteCandidate {
        identity: AtomicRenderCandidateIdentity,
        controls_before: Vec<SequencedRenderControl>,
        required_control_epoch: u64,
    },
    DiscardCandidate(AtomicRenderCandidateIdentity),
    Capture(AtomicRenderWorkerCapture),
    ReloadEffects(AtomicRenderWorkerEffectReload),
    ApplyControls {
        controls: Vec<SequencedRenderControl>,
        required_control_epoch: u64,
        response: SyncSender<io::Result<u64>>,
    },
    Shutdown,
}

/// Fixed-capacity, one-command transport. `try_dispatch_render` is the only
/// normal reactor admission operation; it never waits on the worker.
pub(crate) struct AtomicRenderWorkerHandle {
    commands: SyncSender<RenderWorkerCommand>,
    events: Receiver<AtomicRenderWorkerEvent>,
    event_fd: Arc<OwnedFd>,
    join: Option<JoinHandle<()>>,
    busy: bool,
    initialized: bool,
    fatal: Arc<AtomicBool>,
    control_lane_overflowed: bool,
}

impl AtomicRenderWorkerHandle {
    pub(crate) fn start(
        render_device_fd: OwnedFd,
    ) -> io::Result<(Self, AtomicRenderWorkerDiscovery)> {
        let event_fd = Arc::new(create_eventfd()?);
        let worker_event_fd = Arc::clone(&event_fd);
        let fatal = Arc::new(AtomicBool::new(false));
        let worker_fatal = Arc::clone(&fatal);
        let (command_tx, command_rx) = mpsc::sync_channel(1);
        let (event_tx, event_rx) = mpsc::sync_channel(1);
        let (discovery_tx, discovery_rx) = mpsc::sync_channel(1);
        let join = oblivion_one::native::scheduling_qos::spawn_role_thread(
            "typhon-render",
            oblivion_one::native::scheduling_qos::SchedulingThreadRole::Render,
            move || {
                let boot = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let device = gbm::Device::new(render_device_fd)?;
                    let bootstrap = AtomicRenderDomainBootstrap::new(device)?;
                    let formats = bootstrap.renderable_dmabuf_formats().to_vec();
                    Ok::<_, io::Error>((bootstrap, formats))
                }));
                let (bootstrap, formats) = match boot {
                    Ok(Ok(result)) => result,
                    Ok(Err(error)) => {
                        let _ = discovery_tx.send(Err(error.to_string()));
                        return;
                    }
                    Err(_) => {
                        let _ = discovery_tx.send(Err(String::from(
                            "render worker panicked during EGL capability discovery",
                        )));
                        return;
                    }
                };
                if discovery_tx.send(Ok(formats)).is_err() {
                    return;
                }
                let panic_events = event_tx.clone();
                let panic_fd = Arc::clone(&worker_event_fd);
                let loop_result = super::render_worker_panic::catch_render_worker_loop(|| {
                    render_worker_loop(
                        bootstrap,
                        command_rx,
                        event_tx,
                        worker_event_fd,
                        worker_fatal.clone(),
                    );
                });
                if let Err(panic) = loop_result {
                    worker_fatal.store(true, Ordering::Release);
                    let fatal = panic.fatal_event_evidence();
                    let _ = publish_event(
                        &panic_events,
                        &panic_fd,
                        AtomicRenderWorkerEvent::Fatal {
                            job_id: None,
                            gpu_execution_uncertain: fatal.gpu_execution_uncertain,
                            reason: fatal.reason,
                        },
                    );
                }
            },
        )?;
        let formats = discovery_rx
            .recv()
            .map_err(|_| io::Error::other("render worker exited before EGL discovery"))?
            .map_err(io::Error::other)?;
        Ok((
            Self {
                commands: command_tx,
                events: event_rx,
                event_fd,
                join: Some(join),
                busy: false,
                initialized: false,
                fatal,
                control_lane_overflowed: false,
            },
            formats,
        ))
    }

    /// Stage B runs before the reactor is promoted. The renderer and GL pool
    /// are created on the worker thread, and only owned capability data comes
    /// back to the caller.
    pub(super) fn initialize(
        &mut self,
        width: u32,
        height: u32,
        pool_generation: u64,
        format_modifier: DrmFormatModifierPair,
        descriptors: Vec<AtomicRenderSlotDescriptor>,
        physical_slot_identities: Vec<(super::OutputSlotId, u64)>,
        material_generation: Arc<EffectRegistryGeneration>,
        effect_generation: Arc<EffectRegistryGeneration>,
        cursor_image: Arc<CompositorCursorImage>,
    ) -> io::Result<AtomicRenderDomainInfo> {
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.commands
            .send(RenderWorkerCommand::Initialize {
                init: RenderWorkerInit {
                    width,
                    height,
                    pool_generation,
                    format_modifier,
                    descriptors,
                    physical_slot_identities,
                    material_generation,
                    effect_generation,
                    cursor_image,
                },
                response: response_tx,
            })
            .map_err(|_| io::Error::other("render worker exited before renderer initialization"))?;
        let info = response_rx
            .recv()
            .map_err(|_| io::Error::other("render worker dropped renderer initialization"))??;
        self.initialized = true;
        Ok(info)
    }

    pub(crate) fn event_fd(&self) -> i32 {
        self.event_fd.as_raw_fd()
    }

    pub(crate) const fn busy(&self) -> bool {
        self.busy
    }

    pub(crate) fn try_dispatch_render(
        &mut self,
        job: AtomicRenderWorkerJob,
    ) -> Result<(), AtomicRenderWorkerJob> {
        if !self.initialized || self.busy || self.fatal.load(Ordering::Acquire) {
            return Err(job);
        }
        match self.commands.try_send(RenderWorkerCommand::Render(job)) {
            Ok(()) => {
                self.busy = true;
                Ok(())
            }
            Err(TrySendError::Full(RenderWorkerCommand::Render(job)))
            | Err(TrySendError::Disconnected(RenderWorkerCommand::Render(job))) => Err(job),
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                unreachable!("only a render command is offered through try_dispatch_render")
            }
        }
    }

    /// Bootstrap-only render admission. The process has not entered the
    /// realtime reactor yet, so startup may await its single bounded reply.
    pub(crate) fn render_initial_frame(
        &mut self,
        initial: AtomicRenderWorkerInitialFrame,
    ) -> AtomicRenderWorkerInitialFrameResult {
        if !self.initialized || self.busy || self.fatal.load(Ordering::Acquire) {
            return Err(io::Error::other(
                "initial render requires an idle initialized render worker",
            ));
        }
        let (response, reply) = mpsc::sync_channel(1);
        if let Err(error) = self
            .commands
            .try_send(RenderWorkerCommand::RenderInitialFrame { initial, response })
        {
            return Err(io::Error::other(match error {
                TrySendError::Full(_) => "render worker command slot is full during initial render",
                TrySendError::Disconnected(_) => "render worker disconnected during initial render",
            }));
        }
        self.busy = true;
        match reply.recv_timeout(Duration::from_secs(30)) {
            Ok(result) => {
                self.busy = false;
                result
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.fatal.store(true, Ordering::Release);
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "render worker initial frame exceeded the startup bound",
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.fatal.store(true, Ordering::Release);
                Err(io::Error::other(
                    "render worker dropped its initial-frame startup reply",
                ))
            }
        }
    }

    pub(crate) fn try_prepare_candidate(
        &mut self,
        candidate: AtomicRenderCandidateJob,
    ) -> Result<(), AtomicRenderCandidateJob> {
        if !self.initialized || self.busy || self.fatal.load(Ordering::Acquire) {
            return Err(candidate);
        }
        match self
            .commands
            .try_send(RenderWorkerCommand::PrepareCandidate(candidate))
        {
            Ok(()) => {
                self.busy = true;
                Ok(())
            }
            Err(TrySendError::Full(RenderWorkerCommand::PrepareCandidate(candidate)))
            | Err(TrySendError::Disconnected(RenderWorkerCommand::PrepareCandidate(candidate))) => {
                Err(candidate)
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                unreachable!("only candidate preparation is offered through this method")
            }
        }
    }

    pub(crate) fn try_promote_candidate(
        &mut self,
        identity: AtomicRenderCandidateIdentity,
        controls_before: Vec<SequencedRenderControl>,
        required_control_epoch: u64,
    ) -> Result<(), (io::Error, Vec<SequencedRenderControl>)> {
        if !self.initialized || self.busy || self.fatal.load(Ordering::Acquire) {
            return Err((
                io::Error::other("render candidate promotion requires an idle initialized worker"),
                controls_before,
            ));
        }
        match self
            .commands
            .try_send(RenderWorkerCommand::PromoteCandidate {
                identity,
                controls_before,
                required_control_epoch,
            }) {
            Ok(()) => {
                self.busy = true;
                Ok(())
            }
            Err(TrySendError::Full(RenderWorkerCommand::PromoteCandidate {
                controls_before,
                ..
            })) => Err((
                io::Error::other("render worker command slot is full during candidate promotion"),
                controls_before,
            )),
            Err(TrySendError::Disconnected(RenderWorkerCommand::PromoteCandidate {
                controls_before,
                ..
            })) => Err((
                io::Error::other("render worker disconnected during candidate promotion"),
                controls_before,
            )),
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                unreachable!("candidate promotion preserves its command on rejection")
            }
        }
    }

    pub(crate) fn try_discard_candidate(
        &mut self,
        identity: AtomicRenderCandidateIdentity,
    ) -> io::Result<()> {
        if !self.initialized || self.busy || self.fatal.load(Ordering::Acquire) {
            return Err(io::Error::other(
                "render candidate discard requires an idle initialized worker",
            ));
        }
        self.commands
            .try_send(RenderWorkerCommand::DiscardCandidate(identity))
            .map_err(|error| {
                let reason = match error {
                    TrySendError::Full(_) => {
                        "render worker command slot is full during candidate discard"
                    }
                    TrySendError::Disconnected(_) => {
                        "render worker disconnected during candidate discard"
                    }
                };
                io::Error::other(reason)
            })?;
        self.busy = true;
        Ok(())
    }

    pub(crate) fn try_dispatch_capture(
        &mut self,
        capture: AtomicRenderWorkerCapture,
    ) -> Result<(), AtomicRenderWorkerCapture> {
        if !self.initialized || self.busy || self.fatal.load(Ordering::Acquire) {
            return Err(capture);
        }
        match self
            .commands
            .try_send(RenderWorkerCommand::Capture(capture))
        {
            Ok(()) => {
                self.busy = true;
                Ok(())
            }
            Err(TrySendError::Full(RenderWorkerCommand::Capture(capture)))
            | Err(TrySendError::Disconnected(RenderWorkerCommand::Capture(capture))) => {
                Err(capture)
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                unreachable!("only a capture command is offered through try_dispatch_capture")
            }
        }
    }

    pub(crate) fn try_dispatch_release_fence(
        &mut self,
        request: AtomicRenderWorkerReleaseFence,
    ) -> Result<(), AtomicRenderWorkerReleaseFence> {
        if !self.initialized || self.busy || self.fatal.load(Ordering::Acquire) {
            return Err(request);
        }
        match self
            .commands
            .try_send(RenderWorkerCommand::CreateReleaseFence(request))
        {
            Ok(()) => {
                self.busy = true;
                Ok(())
            }
            Err(TrySendError::Full(RenderWorkerCommand::CreateReleaseFence(request)))
            | Err(TrySendError::Disconnected(RenderWorkerCommand::CreateReleaseFence(request))) => {
                Err(request)
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                unreachable!("only release-fence requests use this method")
            }
        }
    }

    pub(crate) fn try_dispatch_effect_reload(
        &mut self,
        reload: AtomicRenderWorkerEffectReload,
    ) -> Result<(), AtomicRenderWorkerEffectReload> {
        if !self.initialized || self.busy || self.fatal.load(Ordering::Acquire) {
            return Err(reload);
        }
        match self
            .commands
            .try_send(RenderWorkerCommand::ReloadEffects(reload))
        {
            Ok(()) => {
                self.busy = true;
                Ok(())
            }
            Err(TrySendError::Full(RenderWorkerCommand::ReloadEffects(reload)))
            | Err(TrySendError::Disconnected(RenderWorkerCommand::ReloadEffects(reload))) => {
                Err(reload)
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                unreachable!("only a reload command is offered through try_dispatch_effect_reload")
            }
        }
    }

    /// Bootstrap may await trusted shader publication before the event loop
    /// starts. The worker still performs all GL compilation; only the bounded
    /// startup reply is synchronous on the pre-reactor caller.
    pub(crate) fn reload_effects_before_event_loop(
        &mut self,
        reload: AtomicRenderWorkerEffectReload,
    ) -> Result<Option<PreparedWorkerEffectReload>, String> {
        if !self.initialized || self.busy || self.fatal.load(Ordering::Acquire) {
            return Err(String::from(
                "startup effect reload requires an idle initialized render worker",
            ));
        }
        let expected_job_id = reload.job_id;
        let expected_epoch = reload.required_control_epoch;
        self.commands
            .try_send(RenderWorkerCommand::ReloadEffects(reload))
            .map_err(|error| match error {
                TrySendError::Full(_) => {
                    String::from("render worker command slot is full during startup effect reload")
                }
                TrySendError::Disconnected(_) => {
                    String::from("render worker disconnected during startup effect reload")
                }
            })?;
        self.busy = true;
        match self.events.recv_timeout(Duration::from_secs(30)) {
            Ok(event) => {
                self.drain_eventfd().map_err(|error| {
                    self.fatal.store(true, Ordering::Release);
                    format!("failed to drain startup render-worker eventfd: {error}")
                })?;
                self.busy = false;
                match event {
                    AtomicRenderWorkerEvent::EffectsReloaded {
                        job_id,
                        control_epoch,
                        result,
                    } if job_id == expected_job_id && control_epoch == expected_epoch => result,
                    AtomicRenderWorkerEvent::Fatal { reason, .. } => {
                        self.fatal.store(true, Ordering::Release);
                        Err(format!(
                            "render worker failed during startup effect reload: {reason}"
                        ))
                    }
                    _ => {
                        self.fatal.store(true, Ordering::Release);
                        Err(String::from(
                            "render worker returned an unexpected startup effect-reload event",
                        ))
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.fatal.store(true, Ordering::Release);
                self.fail_control_lane();
                Err(String::from(
                    "render worker startup effect reload exceeded the 30 second bound",
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.fatal.store(true, Ordering::Release);
                Err(String::from(
                    "render worker dropped its startup effect-reload completion",
                ))
            }
        }
    }

    /// Applies already-ordered controls while session recovery has stopped
    /// admission and the render worker is known idle. The bounded reply wait
    /// is reserved for this non-realtime lifecycle path; normal rendering
    /// always carries controls with its next nonblocking command.
    pub(crate) fn apply_controls_while_quiesced(
        &mut self,
        controls: Vec<SequencedRenderControl>,
        required_control_epoch: u64,
    ) -> io::Result<u64> {
        if !self.initialized || self.busy || self.fatal.load(Ordering::Acquire) {
            return Err(io::Error::other(
                "renderer controls require an initialized, quiesced render worker",
            ));
        }
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        let command = RenderWorkerCommand::ApplyControls {
            controls,
            required_control_epoch,
            response: response_tx,
        };
        if let Err(error) = self.commands.try_send(command) {
            let reason = match error {
                TrySendError::Full(_) => "render worker command slot was unexpectedly full",
                TrySendError::Disconnected(_) => "render worker disconnected during control rebind",
            };
            self.fail_control_lane();
            return Err(io::Error::other(reason));
        }
        self.busy = true;
        match response_rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(applied_epoch)) if applied_epoch == required_control_epoch => {
                self.busy = false;
                Ok(applied_epoch)
            }
            Ok(Ok(applied_epoch)) => {
                self.busy = false;
                self.fail_control_lane();
                Err(io::Error::other(format!(
                    "renderer control rebind acknowledged epoch {applied_epoch}, expected {required_control_epoch}"
                )))
            }
            Ok(Err(error)) => {
                self.busy = false;
                self.fail_control_lane();
                Err(error)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // The worker may still own GL/EGL state. Keep `busy` set and
                // report fatal uncertainty so no render or physical teardown
                // can assume the rebind completed.
                self.fail_control_lane();
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "render worker did not acknowledge quiesced renderer controls",
                ))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.fail_control_lane();
                Err(io::Error::other(
                    "render worker dropped quiesced renderer control acknowledgment",
                ))
            }
        }
    }

    pub(crate) fn drain_eventfd(&self) -> io::Result<()> {
        drain_eventfd(&self.event_fd)
    }

    pub(crate) fn fail_control_lane(&mut self) {
        self.control_lane_overflowed = true;
        self.fatal.store(true, Ordering::Release);
        let _ = signal_eventfd(&self.event_fd);
    }

    pub(crate) fn try_take_event(&mut self) -> io::Result<Option<AtomicRenderWorkerEvent>> {
        self.drain_eventfd()?;
        if std::mem::take(&mut self.control_lane_overflowed) {
            return Ok(Some(AtomicRenderWorkerEvent::Fatal {
                job_id: None,
                gpu_execution_uncertain: self.busy,
                reason: "control_lane_overflow",
            }));
        }
        match self.events.try_recv() {
            Ok(event) => {
                self.busy = false;
                Ok(Some(event))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(io::Error::other(
                "render worker completion transport disconnected",
            )),
        }
    }

    pub(crate) fn shutdown_and_join(&mut self) -> io::Result<()> {
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        // Shutdown is a bounded lifecycle operation. A wedged GPU call must
        // not make teardown wait forever; on timeout the caller retains the
        // physical BOs and this JoinHandle detaches the still-owning thread.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match self.commands.try_send(RenderWorkerCommand::Shutdown) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => break,
                Err(TrySendError::Full(_)) => {
                    if Instant::now() >= deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "render worker shutdown command did not enter its bounded lane",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        while !join.is_finished() {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "render worker did not destroy its render domain before shutdown timeout",
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        join.join()
            .map_err(|_| io::Error::other("render worker panicked during teardown"))
    }
}

impl Drop for AtomicRenderWorkerHandle {
    fn drop(&mut self) {
        let _ = self.shutdown_and_join();
    }
}

fn render_worker_loop(
    bootstrap: AtomicRenderDomainBootstrap,
    commands: Receiver<RenderWorkerCommand>,
    events: SyncSender<AtomicRenderWorkerEvent>,
    event_fd: Arc<OwnedFd>,
    fatal: Arc<AtomicBool>,
) {
    let mut bootstrap = Some(bootstrap);
    let mut domain: Option<AtomicRenderDomain> = None;
    let mut candidate_domain: Option<(AtomicRenderCandidateIdentity, AtomicRenderDomain)> = None;
    let mut applied_control_epoch = 0u64;
    while let Ok(command) = commands.recv() {
        match command {
            RenderWorkerCommand::Initialize { init, response } => {
                let Some(bootstrap) = bootstrap.take() else {
                    let _ = response.send(Err(io::Error::other(
                        "render worker received duplicate initialization",
                    )));
                    return;
                };
                let result = (|| {
                    let RenderWorkerInit {
                        width,
                        height,
                        pool_generation,
                        format_modifier,
                        descriptors,
                        physical_slot_identities,
                        material_generation,
                        effect_generation,
                        cursor_image,
                    } = init;
                    let (mut initialized, info) =
                        bootstrap.initialize_renderer(width, height, format_modifier)?;
                    initialized.install_pool(
                        descriptors,
                        pool_generation,
                        &physical_slot_identities,
                    )?;
                    initialized.publish_material_effect_generation(&material_generation);
                    initialized
                        .publish_effect_registry_generation((*effect_generation).clone())
                        .map_err(|error| io::Error::other(error.to_string()))?;
                    initialized.set_cursor_image(cursor_image);
                    domain = Some(initialized);
                    Ok(info)
                })();
                if response.send(result).is_err() {
                    return;
                }
                if domain.is_none() {
                    return;
                }
            }
            RenderWorkerCommand::Render(job) => {
                let AtomicRenderWorkerJob {
                    job_id,
                    frame_id,
                    transaction_id,
                    output_epoch,
                    render_generation,
                    target,
                    frozen_frame: mut frozen_frame,
                    need_no_visual_release_fence,
                    dispatch_at_ns,
                    required_control_epoch,
                    controls_before,
                } = job;
                let worker_started = Instant::now();
                let worker_started_at_ns = clock_ns(libc::CLOCK_MONOTONIC);
                let worker_cpu_started_ns = clock_ns(libc::CLOCK_THREAD_CPUTIME_ID);
                let mut gpu_sampling_started = false;
                let mut phase_durations = AtomicRenderPhaseDurations::default();
                let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> io::Result<(io::Result<AtomicSlotRenderOutcome>, Option<NativeRenderFence>)> {
                    let domain = domain.as_mut().ok_or_else(|| {
                        io::Error::other("render worker received a job before initialization")
                    })?;
                    validate_render_control_batch(
                        applied_control_epoch,
                        &controls_before,
                        required_control_epoch,
                    )?;
                    for control in controls_before {
                        apply_worker_control(
                            domain,
                            candidate_domain.as_mut().map(|(_, domain)| domain),
                            control.control,
                        )?;
                        applied_control_epoch = control.epoch;
                    }
                    let result = domain.render_to_slot(
                        target,
                        frame_id,
                        &mut frozen_frame,
                        &mut gpu_sampling_started,
                        Some(&mut phase_durations),
                    );
                    match &result {
                        Ok(AtomicSlotRenderOutcome::Rendered(_)) => {}
                        Ok(AtomicSlotRenderOutcome::Skipped { .. })
                        | Ok(AtomicSlotRenderOutcome::LifecycleFallback { .. })
                        | Err(_) => {
                            if gpu_sampling_started {
                                domain.finish_render_work();
                            }
                        }
                    }
                    let no_visual_release_fence = if need_no_visual_release_fence
                        && matches!(&result, Ok(AtomicSlotRenderOutcome::Skipped { .. }))
                    {
                        domain.create_render_fence().ok()
                    } else {
                        None
                    };
                    Ok((result, no_visual_release_fence))
                }));
                let event = match rendered {
                    Ok(Ok((result, no_visual_release_fence))) => {
                        AtomicRenderWorkerEvent::Rendered(AtomicRenderWorkerCompletion {
                            job_id,
                            frame_id,
                            transaction_id,
                            output_epoch,
                            render_generation,
                            target,
                            control_epoch: applied_control_epoch,
                            result,
                            frozen_frame,
                            gpu_sampling_started,
                            no_visual_release_fence,
                            phase_durations,
                            dispatch_at_ns,
                            worker_started_at_ns,
                            worker_finished_at_ns: clock_ns(libc::CLOCK_MONOTONIC),
                            worker_thread_cpu_ns: worker_cpu_started_ns
                                .zip(clock_ns(libc::CLOCK_THREAD_CPUTIME_ID))
                                .map(|(started, finished)| finished.saturating_sub(started)),
                            worker_wall_ns: u64::try_from(worker_started.elapsed().as_nanos())
                                .unwrap_or(u64::MAX),
                        })
                    }
                    Ok(Err(error)) => AtomicRenderWorkerEvent::Fatal {
                        job_id: Some(job_id),
                        gpu_execution_uncertain: gpu_sampling_started,
                        reason: if error.to_string().contains("before initialization") {
                            "render_before_initialization"
                        } else {
                            "render_domain_error"
                        },
                    },
                    Err(_) => AtomicRenderWorkerEvent::Fatal {
                        job_id: Some(job_id),
                        gpu_execution_uncertain: gpu_sampling_started,
                        reason: "panic_during_render",
                    },
                };
                fatal.store(
                    matches!(&event, AtomicRenderWorkerEvent::Fatal { .. }),
                    Ordering::Release,
                );
                if publish_event(&events, &event_fd, event).is_err() {
                    fatal.store(true, Ordering::Release);
                    return;
                }
            }
            RenderWorkerCommand::RenderInitialFrame { initial, response } => {
                let AtomicRenderWorkerInitialFrame {
                    target,
                    frame_id,
                    mut frozen_frame,
                    required_control_epoch,
                    controls_before,
                } = initial;
                let mut gpu_sampling_started = false;
                let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> io::Result<AtomicSlotRenderOutcome> {
                        let domain = domain.as_mut().ok_or_else(|| {
                            io::Error::other(
                                "render worker received initial frame before initialization",
                            )
                        })?;
                        validate_render_control_batch(
                            applied_control_epoch,
                            &controls_before,
                            required_control_epoch,
                        )?;
                        for control in controls_before {
                            apply_worker_control(
                                domain,
                                candidate_domain.as_mut().map(|(_, domain)| domain),
                                control.control,
                            )?;
                            applied_control_epoch = control.epoch;
                        }
                        let result = domain.render_to_slot(
                            target,
                            frame_id,
                            &mut frozen_frame,
                            &mut gpu_sampling_started,
                            None,
                        );
                        if !matches!(&result, Ok(AtomicSlotRenderOutcome::Rendered(_)))
                            && gpu_sampling_started
                        {
                            domain.finish_render_work();
                        }
                        result
                    },
                ));
                match rendered {
                    Ok(Ok(outcome)) => {
                        if response.send(Ok((outcome, frozen_frame))).is_err() {
                            fatal.store(true, Ordering::Release);
                            let _ = publish_event(
                                &events,
                                &event_fd,
                                AtomicRenderWorkerEvent::Fatal {
                                    job_id: None,
                                    gpu_execution_uncertain: gpu_sampling_started,
                                    reason: "initial_render_reply_disconnected",
                                },
                            );
                            return;
                        }
                    }
                    Ok(Err(error)) => {
                        fatal.store(true, Ordering::Release);
                        let _ = response.send(Err(error));
                        let _ = publish_event(
                            &events,
                            &event_fd,
                            AtomicRenderWorkerEvent::Fatal {
                                job_id: None,
                                gpu_execution_uncertain: gpu_sampling_started,
                                reason: "initial_render_failed",
                            },
                        );
                        return;
                    }
                    Err(_) => {
                        fatal.store(true, Ordering::Release);
                        let _ = response.send(Err(io::Error::other(
                            "render worker panicked during initial frame rendering",
                        )));
                        let _ = publish_event(
                            &events,
                            &event_fd,
                            AtomicRenderWorkerEvent::Fatal {
                                job_id: None,
                                gpu_execution_uncertain: true,
                                reason: "panic_during_initial_render",
                            },
                        );
                        return;
                    }
                }
            }
            RenderWorkerCommand::CreateReleaseFence(request) => {
                let AtomicRenderWorkerReleaseFence {
                    job_id,
                    required_control_epoch,
                    controls_before,
                } = request;
                let fenced = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> io::Result<io::Result<NativeRenderFence>> {
                        let active = domain.as_mut().ok_or_else(|| {
                            io::Error::other(
                                "release-fence request arrived before renderer initialization",
                            )
                        })?;
                        validate_render_control_batch(
                            applied_control_epoch,
                            &controls_before,
                            required_control_epoch,
                        )?;
                        for control in controls_before {
                            apply_worker_control(
                                active,
                                candidate_domain.as_mut().map(|(_, domain)| domain),
                                control.control,
                            )?;
                            applied_control_epoch = control.epoch;
                        }
                        Ok(active.create_render_fence())
                    },
                ));
                let event = match fenced {
                    Ok(Ok(result)) => AtomicRenderWorkerEvent::ReleaseFenceReady {
                        job_id,
                        control_epoch: applied_control_epoch,
                        result,
                    },
                    Ok(Err(_)) => {
                        fatal.store(true, Ordering::Release);
                        AtomicRenderWorkerEvent::Fatal {
                            job_id: Some(job_id),
                            gpu_execution_uncertain: true,
                            reason: "release_fence_control_application_failed",
                        }
                    }
                    Err(_) => AtomicRenderWorkerEvent::Fatal {
                        job_id: Some(job_id),
                        gpu_execution_uncertain: true,
                        reason: "panic_during_release_fence_export",
                    },
                };
                fatal.store(
                    matches!(&event, AtomicRenderWorkerEvent::Fatal { .. }),
                    Ordering::Release,
                );
                if publish_event(&events, &event_fd, event).is_err() {
                    fatal.store(true, Ordering::Release);
                    return;
                }
            }
            RenderWorkerCommand::PrepareCandidate(candidate) => {
                let AtomicRenderCandidateJob {
                    identity,
                    render_device_fd,
                    width,
                    height,
                    format_modifier,
                    descriptors,
                    physical_slot_identities,
                    target,
                    frame_id,
                    mut frozen_frame,
                    material_generation,
                    effect_generation,
                    cursor_image,
                    required_control_epoch,
                    controls_before,
                } = candidate;
                let mut gpu_sampling_started = false;
                let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> io::Result<(
                        AtomicRenderDomain,
                        Box<AtomicRenderedFrameParts>,
                        AtomicRenderDomainInfo,
                    )> {
                        let active = domain.as_mut().ok_or_else(|| {
                            io::Error::other("candidate preparation arrived before renderer initialization")
                        })?;
                        if candidate_domain.is_some() {
                            return Err(io::Error::other(
                                "render worker already owns an output candidate domain",
                            ));
                        }
                        validate_render_control_batch(
                            applied_control_epoch,
                            &controls_before,
                            required_control_epoch,
                        )?;
                        for control in controls_before {
                            apply_worker_control(active, None, control.control)?;
                            applied_control_epoch = control.epoch;
                        }

                        let device = gbm::Device::new(render_device_fd)?;
                        let bootstrap = AtomicRenderDomainBootstrap::new(device)?;
                        let (mut candidate, info) = bootstrap.initialize_renderer(
                            width,
                            height,
                            format_modifier,
                        )?;
                        candidate.install_pool(
                            descriptors,
                            identity.pool_generation,
                            &physical_slot_identities,
                        )?;
                        candidate.publish_material_effect_generation(&material_generation);
                        candidate
                            .publish_effect_registry_generation(effect_generation)
                            .map_err(|error| io::Error::other(error.to_string()))?;
                        candidate.set_cursor_image(cursor_image);
                        match candidate.render_to_slot(
                            target,
                            frame_id,
                            &mut frozen_frame,
                            &mut gpu_sampling_started,
                            None,
                        )? {
                            AtomicSlotRenderOutcome::Rendered(frame) => {
                                Ok((candidate, frame, info))
                            }
                            AtomicSlotRenderOutcome::Skipped { reason, .. } => {
                                if gpu_sampling_started {
                                    candidate.finish_render_work();
                                }
                                Err(io::Error::other(format!(
                                    "candidate full repaint skipped: {reason:?}"
                                )))
                            }
                            AtomicSlotRenderOutcome::LifecycleFallback { fallbacks, .. } => {
                                if gpu_sampling_started {
                                    candidate.finish_render_work();
                                }
                                Err(io::Error::other(format!(
                                    "candidate full repaint lifecycle fallback: {fallbacks:?}"
                                )))
                            }
                        }
                    },
                ));
                let event = match prepared {
                    Ok(Ok((candidate, frame, info))) => {
                        if candidate_domain.is_some() {
                            drop(candidate);
                            AtomicRenderWorkerEvent::Fatal {
                                job_id: Some(identity.job_id),
                                gpu_execution_uncertain: gpu_sampling_started,
                                reason: "duplicate_candidate_domain",
                            }
                        } else {
                            candidate_domain = Some((identity, candidate));
                            AtomicRenderWorkerEvent::CandidatePrepared {
                                identity,
                                result: Ok((frame, info)),
                            }
                        }
                    }
                    Ok(Err(error)) => AtomicRenderWorkerEvent::CandidatePrepared {
                        identity,
                        result: Err(error),
                    },
                    Err(_) => AtomicRenderWorkerEvent::Fatal {
                        job_id: Some(identity.job_id),
                        gpu_execution_uncertain: gpu_sampling_started,
                        reason: "panic_during_candidate_preparation",
                    },
                };
                fatal.store(
                    matches!(&event, AtomicRenderWorkerEvent::Fatal { .. }),
                    Ordering::Release,
                );
                if publish_event(&events, &event_fd, event).is_err() {
                    fatal.store(true, Ordering::Release);
                    return;
                }
            }
            RenderWorkerCommand::PromoteCandidate {
                identity,
                controls_before,
                required_control_epoch,
            } => {
                let promoted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> io::Result<()> {
                        let active = domain.as_mut().ok_or_else(|| {
                            io::Error::other(
                                "candidate promotion arrived before renderer initialization",
                            )
                        })?;
                        validate_render_control_batch(
                            applied_control_epoch,
                            &controls_before,
                            required_control_epoch,
                        )?;
                        for control in controls_before {
                            apply_worker_control(
                                active,
                                candidate_domain.as_mut().map(|(_, domain)| domain),
                                control.control,
                            )?;
                            applied_control_epoch = control.epoch;
                        }
                        let (actual, _) = candidate_domain.as_ref().ok_or_else(|| {
                            io::Error::other("candidate promotion has no prepared render domain")
                        })?;
                        if !identity.matches(*actual) {
                            return Err(io::Error::other(
                                "candidate promotion identity does not match prepared render domain",
                            ));
                        }
                        Ok(())
                    },
                ));
                let event = match promoted {
                    Ok(Ok(())) => {
                        let (actual, mut next_domain) = candidate_domain
                            .take()
                            .expect("validated candidate remains owned during promotion");
                        let Some(mut old_domain) = domain.take() else {
                            fatal.store(true, Ordering::Release);
                            return;
                        };
                        old_domain.teardown();
                        domain = Some(std::mem::replace(&mut next_domain, old_domain));
                        drop(next_domain);
                        AtomicRenderWorkerEvent::CandidatePromoted { identity: actual }
                    }
                    Ok(Err(_)) | Err(_) => {
                        fatal.store(true, Ordering::Release);
                        AtomicRenderWorkerEvent::Fatal {
                            job_id: Some(identity.job_id),
                            gpu_execution_uncertain: true,
                            reason: "candidate_promotion_failed",
                        }
                    }
                };
                if publish_event(&events, &event_fd, event).is_err() {
                    fatal.store(true, Ordering::Release);
                    return;
                }
            }
            RenderWorkerCommand::DiscardCandidate(identity) => {
                let discarded = candidate_domain.take().ok_or_else(|| {
                    io::Error::other("candidate discard arrived without a prepared render domain")
                });
                let event = match discarded {
                    Ok((actual, mut candidate)) if identity.matches(actual) => {
                        candidate.teardown();
                        AtomicRenderWorkerEvent::CandidateDiscarded { identity: actual }
                    }
                    Ok((actual, candidate)) => {
                        candidate_domain = Some((actual, candidate));
                        fatal.store(true, Ordering::Release);
                        AtomicRenderWorkerEvent::Fatal {
                            job_id: Some(identity.job_id),
                            gpu_execution_uncertain: true,
                            reason: "candidate_discard_identity_mismatch",
                        }
                    }
                    Err(_) => {
                        fatal.store(true, Ordering::Release);
                        AtomicRenderWorkerEvent::Fatal {
                            job_id: Some(identity.job_id),
                            gpu_execution_uncertain: true,
                            reason: "candidate_discard_without_candidate",
                        }
                    }
                };
                if publish_event(&events, &event_fd, event).is_err() {
                    fatal.store(true, Ordering::Release);
                    return;
                }
            }
            RenderWorkerCommand::Capture(mut capture) => {
                let captured = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> io::Result<Vec<u8>> {
                        let domain = domain.as_mut().ok_or_else(|| {
                            io::Error::other("render worker received capture before initialization")
                        })?;
                        validate_render_control_batch(
                            applied_control_epoch,
                            &capture.controls_before,
                            capture.required_control_epoch,
                        )?;
                        for control in capture.controls_before.drain(..) {
                            apply_worker_control(
                                domain,
                                candidate_domain.as_mut().map(|(_, domain)| domain),
                                control.control,
                            )?;
                            applied_control_epoch = control.epoch;
                        }
                        match domain.capture_scene(&mut capture.frozen_frame) {
                            Ok(payload) => Ok(payload),
                            Err(error) => {
                                // Capture may have sampled client buffers before
                                // reporting a readback error. Complete GL work
                                // before the reactor releases its capture owner.
                                domain.finish_render_work();
                                Err(error)
                            }
                        }
                    },
                ));
                let event = match captured {
                    Ok(result) => AtomicRenderWorkerEvent::Captured {
                        job_id: capture.job_id,
                        control_epoch: applied_control_epoch,
                        result,
                    },
                    Err(_) => AtomicRenderWorkerEvent::Fatal {
                        job_id: Some(capture.job_id),
                        gpu_execution_uncertain: true,
                        reason: "panic_during_capture",
                    },
                };
                fatal.store(
                    matches!(&event, AtomicRenderWorkerEvent::Fatal { .. }),
                    Ordering::Release,
                );
                if publish_event(&events, &event_fd, event).is_err() {
                    fatal.store(true, Ordering::Release);
                    return;
                }
            }
            RenderWorkerCommand::ReloadEffects(reload) => {
                let AtomicRenderWorkerEffectReload {
                    job_id,
                    previous_generation,
                    manifest,
                    required_control_epoch,
                    controls_before,
                } = reload;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> io::Result<Result<Option<PreparedWorkerEffectReload>, String>> {
                        let domain = domain.as_mut().ok_or_else(|| {
                            io::Error::other(
                                "render worker received effect reload before initialization",
                            )
                        })?;
                        validate_render_control_batch(
                            applied_control_epoch,
                            &controls_before,
                            required_control_epoch,
                        )?;
                        for control in controls_before {
                            apply_worker_control(
                                domain,
                                candidate_domain.as_mut().map(|(_, domain)| domain),
                                control.control,
                            )?;
                            applied_control_epoch = control.epoch;
                        }
                        let reload_result = (|| {
                            let loaded_manifest = match manifest {
                                Some((path, manifest)) => Some((path, manifest)),
                                None => {
                                    oblivion_one::effects::load_default_trusted_effect_manifest()
                                        .map_err(|error| error.to_string())?
                                        .map(|(path, manifest)| {
                                            (path.display().to_string(), manifest)
                                        })
                                }
                            };
                            let Some((manifest_path, manifest)) = loaded_manifest else {
                                return Ok(None);
                            };
                            let prepared = PreparedEffectRegistryReload::from_previous(
                                &previous_generation,
                                manifest,
                            )
                            .map_err(|error| error.to_string())?;
                            domain
                                .publish_effect_registry_generation(prepared.generation().clone())
                                .map_err(|error| error.to_string())?;
                            Ok(Some(PreparedWorkerEffectReload {
                                manifest_path,
                                prepared,
                            }))
                        })();
                        Ok(reload_result)
                    },
                ));
                let event = match result {
                    Ok(Ok(result)) => AtomicRenderWorkerEvent::EffectsReloaded {
                        job_id,
                        control_epoch: applied_control_epoch,
                        result,
                    },
                    Ok(Err(_)) => AtomicRenderWorkerEvent::Fatal {
                        job_id: Some(job_id),
                        gpu_execution_uncertain: true,
                        reason: "effect_reload_control_application_failed",
                    },
                    Err(_) => AtomicRenderWorkerEvent::Fatal {
                        job_id: Some(job_id),
                        gpu_execution_uncertain: true,
                        reason: "panic_during_effect_reload",
                    },
                };
                fatal.store(
                    matches!(&event, AtomicRenderWorkerEvent::Fatal { .. }),
                    Ordering::Release,
                );
                if publish_event(&events, &event_fd, event).is_err() {
                    fatal.store(true, Ordering::Release);
                    return;
                }
            }
            RenderWorkerCommand::ApplyControls {
                controls,
                required_control_epoch,
                response,
            } => {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                    || -> io::Result<u64> {
                        let domain = domain.as_mut().ok_or_else(|| {
                            io::Error::other(
                                "render worker received recovery controls before initialization",
                            )
                        })?;
                        validate_render_control_batch(
                            applied_control_epoch,
                            &controls,
                            required_control_epoch,
                        )?;
                        for control in controls {
                            apply_worker_control(
                                domain,
                                candidate_domain.as_mut().map(|(_, domain)| domain),
                                control.control,
                            )?;
                            applied_control_epoch = control.epoch;
                        }
                        Ok(applied_control_epoch)
                    },
                ));
                match result {
                    Ok(Ok(applied_epoch)) => {
                        let _ = response.send(Ok(applied_epoch));
                    }
                    Ok(Err(error)) => {
                        fatal.store(true, Ordering::Release);
                        let _ = response.send(Err(io::Error::other(error.to_string())));
                        let _ = publish_event(
                            &events,
                            &event_fd,
                            AtomicRenderWorkerEvent::Fatal {
                                job_id: None,
                                gpu_execution_uncertain: true,
                                reason: "quiesced_control_application_failed",
                            },
                        );
                        return;
                    }
                    Err(_) => {
                        fatal.store(true, Ordering::Release);
                        let _ = response.send(Err(io::Error::other(
                            "render worker panicked while applying quiesced controls",
                        )));
                        let _ = publish_event(
                            &events,
                            &event_fd,
                            AtomicRenderWorkerEvent::Fatal {
                                job_id: None,
                                gpu_execution_uncertain: true,
                                reason: "panic_during_quiesced_control_application",
                            },
                        );
                        return;
                    }
                }
            }
            RenderWorkerCommand::Shutdown => {
                if let Some((_, mut candidate)) = candidate_domain.take() {
                    candidate.teardown();
                }
                if let Some(mut domain) = domain.take() {
                    domain.teardown();
                }
                return;
            }
        }
    }
}

pub(super) fn apply_render_control(
    domain: &mut AtomicRenderDomain,
    control: AtomicRenderControl,
) -> Result<(), io::Error> {
    domain.activate_context()?;
    match control {
        AtomicRenderControl::CommitPresented {
            frame,
            presented_transition_damage,
        } => domain.commit_presented(frame, presented_transition_damage),
        AtomicRenderControl::DiscardRendered { frame } => domain.discard_rendered(frame),
        AtomicRenderControl::InvalidatePresentedDamageHistory => {
            domain.invalidate_presented_damage_history();
        }
        AtomicRenderControl::FrameSwapFailed => domain.frame_swap_failed(),
        AtomicRenderControl::PublishMaterialEffectGeneration(generation) => {
            domain.publish_material_effect_generation(&generation);
        }
        AtomicRenderControl::PublishEffectRegistryGeneration(generation) => domain
            .publish_effect_registry_generation(generation)
            .map_err(|error| io::Error::other(error.to_string()))?,
        AtomicRenderControl::SetCursorImage(image) => domain.set_cursor_image(image),
        AtomicRenderControl::RebindPoolGeneration(generation) => {
            domain.rebind_pool_generation(generation);
        }
        AtomicRenderControl::FinishRenderWork => domain.finish_render_work(),
    }
    Ok(())
}

fn mirrored_candidate_control(control: &AtomicRenderControl) -> Option<AtomicRenderControl> {
    match control {
        AtomicRenderControl::InvalidatePresentedDamageHistory => {
            Some(AtomicRenderControl::InvalidatePresentedDamageHistory)
        }
        AtomicRenderControl::PublishMaterialEffectGeneration(generation) => Some(
            AtomicRenderControl::PublishMaterialEffectGeneration(generation.clone()),
        ),
        AtomicRenderControl::PublishEffectRegistryGeneration(generation) => Some(
            AtomicRenderControl::PublishEffectRegistryGeneration(generation.clone()),
        ),
        AtomicRenderControl::SetCursorImage(image) => {
            Some(AtomicRenderControl::SetCursorImage(Arc::clone(image)))
        }
        AtomicRenderControl::RebindPoolGeneration(generation) => {
            Some(AtomicRenderControl::RebindPoolGeneration(*generation))
        }
        AtomicRenderControl::FinishRenderWork => Some(AtomicRenderControl::FinishRenderWork),
        AtomicRenderControl::CommitPresented { .. }
        | AtomicRenderControl::DiscardRendered { .. }
        | AtomicRenderControl::FrameSwapFailed => None,
    }
}

fn apply_worker_control(
    active: &mut AtomicRenderDomain,
    candidate: Option<&mut AtomicRenderDomain>,
    control: AtomicRenderControl,
) -> io::Result<()> {
    let candidate_control = mirrored_candidate_control(&control);
    apply_render_control(active, control)?;
    if let (Some(candidate), Some(candidate_control)) = (candidate, candidate_control) {
        apply_render_control(candidate, candidate_control)?;
    }
    Ok(())
}

fn create_eventfd() -> io::Result<OwnedFd> {
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: eventfd returned a new owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn clock_ns(clock_id: libc::clockid_t) -> Option<u64> {
    let mut timestamp = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: clock_gettime initializes the provided timespec on success.
    if unsafe { libc::clock_gettime(clock_id, timestamp.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: the successful clock_gettime call initialized the timespec.
    let timestamp = unsafe { timestamp.assume_init() };
    let seconds = u64::try_from(timestamp.tv_sec).ok()?;
    let nanoseconds = u64::try_from(timestamp.tv_nsec).ok()?;
    Some(
        seconds
            .saturating_mul(1_000_000_000)
            .saturating_add(nanoseconds),
    )
}

fn drain_eventfd(fd: &OwnedFd) -> io::Result<()> {
    loop {
        let mut value = 0u64;
        let read = unsafe {
            libc::read(
                fd.as_raw_fd(),
                (&mut value as *mut u64).cast(),
                std::mem::size_of::<u64>(),
            )
        };
        if read == std::mem::size_of::<u64>() as isize {
            continue;
        }
        if read < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == io::ErrorKind::WouldBlock {
                return Ok(());
            }
            return Err(error);
        }
        return Err(io::Error::other("short render eventfd read"));
    }
}

fn signal_eventfd(fd: &OwnedFd) -> io::Result<()> {
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
        if written < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if error.kind() == io::ErrorKind::WouldBlock {
                // The eventfd is already readable, which is the only property
                // required for a bounded fatal notification.
                return Ok(());
            }
            return Err(error);
        }
        return Err(io::Error::other("short render eventfd write"));
    }
}

fn publish_event(
    events: &SyncSender<AtomicRenderWorkerEvent>,
    fd: &OwnedFd,
    event: AtomicRenderWorkerEvent,
) -> io::Result<()> {
    events
        .send(event)
        .map_err(|_| io::Error::other("render completion receiver disconnected"))?;
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
        if written < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return Err(io::Error::other("short render eventfd write"));
    }
}

fn validate_render_control_batch(
    applied_epoch: u64,
    controls: &[SequencedRenderControl],
    required_epoch: u64,
) -> io::Result<()> {
    let mut expected = applied_epoch;
    for control in controls {
        expected = expected
            .checked_add(1)
            .ok_or_else(|| io::Error::other("renderer control epoch exhausted"))?;
        if control.epoch != expected {
            return Err(io::Error::other(
                "renderer control epochs arrived out of order",
            ));
        }
    }
    if expected != required_epoch {
        return Err(io::Error::other(
            "render job control epoch does not match installed renderer state",
        ));
    }
    Ok(())
}

#[allow(dead_code, reason = "Phase 2B.2 transport Send qualification")]
fn assert_atomic_render_worker_transport_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<AtomicRenderWorkerDiscovery>();
    assert_send::<AtomicRenderWorkerJob>();
    assert_send::<RenderWorkerInit>();
    assert_send::<AtomicRenderWorkerInitialFrame>();
    assert_send::<AtomicRenderWorkerInitialFrameResult>();
    assert_send::<AtomicRenderWorkerCapture>();
    assert_send::<AtomicRenderWorkerEffectReload>();
    assert_send::<PreparedWorkerEffectReload>();
    assert_send::<AtomicRenderWorkerCompletion>();
    assert_send::<AtomicRenderWorkerReleaseFence>();
    assert_send::<AtomicRenderWorkerEvent>();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AtomicRenderTransportKind {
    Synchronous,
    Worker,
}

impl AtomicRenderTransportKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Synchronous => "synchronous",
            Self::Worker => "worker",
        }
    }
}

#[cfg(test)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderWorkerAdmission {
    busy: bool,
    render_demand: bool,
    coalesced_demand_count: u64,
}

#[cfg(test)]
impl RenderWorkerAdmission {
    pub(crate) const fn busy(self) -> bool {
        self.busy
    }

    pub(crate) const fn render_demand(self) -> bool {
        self.render_demand
    }

    pub(crate) const fn coalesced_demand_count(self) -> u64 {
        self.coalesced_demand_count
    }

    /// Record visual state changes while a render owns the only output slot.
    /// This is a bit of reactor state, never a queued or prepared render job.
    pub(crate) fn note_visual_demand(&mut self) {
        if self.busy {
            self.render_demand = true;
            self.coalesced_demand_count = self.coalesced_demand_count.saturating_add(1);
        }
    }

    pub(crate) fn begin_job(&mut self) -> bool {
        if self.busy {
            return false;
        }
        self.busy = true;
        self.render_demand = false;
        true
    }

    /// Release the active command and return one latest-state admission hint.
    /// The normal planner still resolves and freezes the current compositor state.
    pub(crate) fn complete_job(&mut self) -> bool {
        self.busy = false;
        std::mem::take(&mut self.render_demand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZeroU64;

    #[test]
    fn worker_policy_defaults_off_and_parses_explicit_modes() {
        assert_eq!(
            AtomicRenderWorkerPolicy::default(),
            AtomicRenderWorkerPolicy::Off
        );
        assert_eq!(
            AtomicRenderWorkerPolicy::parse(None),
            Ok(AtomicRenderWorkerPolicy::Off)
        );
        assert_eq!(
            AtomicRenderWorkerPolicy::parse(Some("off")),
            Ok(AtomicRenderWorkerPolicy::Off)
        );
        assert_eq!(
            AtomicRenderWorkerPolicy::parse(Some("auto")),
            Ok(AtomicRenderWorkerPolicy::Auto)
        );
        assert_eq!(
            AtomicRenderWorkerPolicy::parse(Some("force")),
            Ok(AtomicRenderWorkerPolicy::Force)
        );
        assert!(AtomicRenderWorkerPolicy::parse(Some("yes")).is_err());
    }

    #[test]
    fn busy_worker_coalesces_many_visual_changes_into_one_latest_state_opportunity() {
        let mut admission = RenderWorkerAdmission::default();
        assert!(admission.begin_job());
        for _ in 0..100 {
            admission.note_visual_demand();
        }
        assert!(admission.busy());
        assert!(admission.render_demand());
        assert_eq!(admission.coalesced_demand_count(), 100);
        assert!(admission.complete_job());
        assert!(!admission.busy());
        assert!(!admission.render_demand());
        assert!(admission.begin_job());
        assert!(!admission.begin_job());
    }

    #[test]
    fn busy_admission_does_not_prepare_another_job() {
        let mut admission = RenderWorkerAdmission::default();
        assert!(admission.begin_job());
        admission.note_visual_demand();
        assert!(!admission.begin_job());
        assert!(admission.complete_job());
        assert!(admission.begin_job());
    }

    #[test]
    fn candidate_completion_requires_matching_job_output_and_pool_identity() {
        let expected = AtomicRenderCandidateIdentity::new(
            RenderWorkerJobId::new(7).expect("nonzero job ID"),
            OutputEpochId::new(11).expect("nonzero output epoch"),
            13,
            super::super::OutputSlotId::new(0).expect("slot zero"),
        );

        assert!(expected.matches(expected));
        assert!(!expected.matches(AtomicRenderCandidateIdentity::new(
            RenderWorkerJobId::new(8).expect("nonzero job ID"),
            OutputEpochId::new(11).expect("nonzero output epoch"),
            13,
            super::super::OutputSlotId::new(0).expect("slot zero"),
        )));
        assert!(!expected.matches(AtomicRenderCandidateIdentity::new(
            RenderWorkerJobId::new(7).expect("nonzero job ID"),
            OutputEpochId::new(12).expect("nonzero output epoch"),
            13,
            super::super::OutputSlotId::new(0).expect("slot zero"),
        )));
        assert!(!expected.matches(AtomicRenderCandidateIdentity::new(
            RenderWorkerJobId::new(7).expect("nonzero job ID"),
            OutputEpochId::new(11).expect("nonzero output epoch"),
            14,
            super::super::OutputSlotId::new(0).expect("slot zero"),
        )));
        assert!(!expected.matches(AtomicRenderCandidateIdentity::new(
            RenderWorkerJobId::new(7).expect("nonzero job ID"),
            OutputEpochId::new(11).expect("nonzero output epoch"),
            13,
            super::super::OutputSlotId::new(1).expect("slot one"),
        )));
    }

    #[test]
    fn candidate_worker_payloads_are_naturally_send() {
        fn assert_send<T: Send>() {}
        assert_send::<AtomicRenderCandidateJob>();
        assert_send::<AtomicRenderWorkerEvent>();
    }

    #[test]
    fn worker_discovery_payload_is_naturally_send() {
        fn assert_send<T: Send>() {}
        assert_send::<AtomicRenderWorkerDiscovery>();
    }

    #[test]
    fn control_lane_overflow_is_a_waking_fatal_worker_event() {
        let (commands, _command_rx) = mpsc::sync_channel(1);
        let (_events_tx, events) = mpsc::sync_channel(1);
        let event_fd = Arc::new(create_eventfd().expect("eventfd"));
        let fatal = Arc::new(AtomicBool::new(false));
        let mut worker = AtomicRenderWorkerHandle {
            commands,
            events,
            event_fd,
            join: None,
            busy: true,
            initialized: true,
            fatal: Arc::clone(&fatal),
            control_lane_overflowed: false,
        };

        worker.fail_control_lane();

        assert!(fatal.load(Ordering::Acquire));
        assert!(matches!(
            worker.try_take_event().expect("read fatal eventfd"),
            Some(AtomicRenderWorkerEvent::Fatal {
                job_id: None,
                gpu_execution_uncertain: true,
                reason: "control_lane_overflow",
            })
        ));
    }

    #[test]
    fn effect_reload_admission_is_nonblocking_while_another_command_is_active() {
        let (commands, _command_rx) = mpsc::sync_channel(1);
        let (_events_tx, events) = mpsc::sync_channel(1);
        let event_fd = Arc::new(create_eventfd().expect("eventfd"));
        let fatal = Arc::new(AtomicBool::new(false));
        let mut worker = AtomicRenderWorkerHandle {
            commands,
            events,
            event_fd,
            join: None,
            busy: true,
            initialized: true,
            fatal,
            control_lane_overflowed: false,
        };

        let rejected = worker.try_dispatch_effect_reload(AtomicRenderWorkerEffectReload {
            job_id: RenderWorkerJobId::new(1).expect("nonzero job id"),
            previous_generation: Arc::new(EffectRegistryGeneration::empty()),
            manifest: None,
            controls_before: Vec::new(),
            required_control_epoch: 0,
        });

        assert!(rejected.is_err());
        assert!(worker.busy());
    }

    #[test]
    fn session_control_ack_refuses_to_wait_while_worker_is_busy() {
        let (commands, _command_rx) = mpsc::sync_channel(1);
        let (_events_tx, events) = mpsc::sync_channel(1);
        let event_fd = Arc::new(create_eventfd().expect("eventfd"));
        let fatal = Arc::new(AtomicBool::new(false));
        let mut worker = AtomicRenderWorkerHandle {
            commands,
            events,
            event_fd,
            join: None,
            busy: true,
            initialized: true,
            fatal,
            control_lane_overflowed: false,
        };

        let error = worker
            .apply_controls_while_quiesced(Vec::new(), 0)
            .expect_err("recovery control waits require a quiescent worker");

        assert!(error.to_string().contains("quiesced render worker"));
        assert!(worker.busy());
    }

    #[test]
    fn renderer_control_batch_is_contiguous_and_matches_job_epoch() {
        let controls = [
            SequencedRenderControl {
                epoch: 8,
                control: AtomicRenderControl::InvalidatePresentedDamageHistory,
            },
            SequencedRenderControl {
                epoch: 9,
                control: AtomicRenderControl::FrameSwapFailed,
            },
        ];
        assert!(validate_render_control_batch(7, &controls, 9).is_ok());
        assert!(validate_render_control_batch(7, &controls, 10).is_err());
        let out_of_order = [
            SequencedRenderControl {
                epoch: 9,
                control: AtomicRenderControl::InvalidatePresentedDamageHistory,
            },
            SequencedRenderControl {
                epoch: 8,
                control: AtomicRenderControl::FrameSwapFailed,
            },
        ];
        assert!(validate_render_control_batch(7, &out_of_order, 9).is_err());
    }

    #[test]
    fn controls_created_during_one_render_are_first_visible_to_the_next_job() {
        let j100_controls: Vec<SequencedRenderControl> = Vec::new();
        validate_render_control_batch(0, &j100_controls, 0)
            .expect("J100 retains its dispatch-time control epoch");

        let queued_during_j100 = [SequencedRenderControl {
            epoch: 1,
            control: AtomicRenderControl::InvalidatePresentedDamageHistory,
        }];
        validate_render_control_batch(0, &queued_during_j100, 1)
            .expect("the later command carries the newly queued control");
    }

    #[test]
    fn render_worker_command_lane_is_fixed_capacity_one_and_nonblocking() {
        let (commands, receiver) = mpsc::sync_channel(1);
        commands
            .try_send(100_u64)
            .expect("first command is admitted");
        assert!(matches!(
            commands.try_send(101_u64),
            Err(TrySendError::Full(101))
        ));
        assert_eq!(receiver.try_recv(), Ok(100));
    }

    #[test]
    fn physical_swapchain_bounds_terminal_output_tokens_to_three_slots() {
        assert_eq!(super::super::EXPLICIT_OUTPUT_SLOT_CAPACITY, 3);
    }

    #[test]
    fn render_worker_source_uses_render_role_and_has_no_unsafe_send_override() {
        let source = include_str!("render_worker.rs");
        let production_source = source.split("#[cfg(test)]").next().unwrap_or(source);
        assert!(source.contains("\"typhon-render\""));
        assert!(source.contains("SchedulingThreadRole::Render"));
        assert!(!production_source.contains("unsafe impl Send"));
        assert!(source.contains("sync_channel(1)"));
    }

    #[test]
    fn completion_identity_rejects_stale_job_and_generation_mismatches() {
        let identity = AtomicRenderWorkerIdentity {
            job_id: RenderWorkerJobId::new(10).unwrap(),
            frame_id: 12,
            transaction_id: OutputTransactionId::new(NonZeroU64::new(13).unwrap()),
            output_epoch: OutputEpochId::new(14).unwrap(),
            render_generation: 15,
            control_epoch: 17,
            target: RenderTargetRequest {
                slot: super::super::OutputSlotId::new(1).unwrap(),
                pool_generation: 16,
                buffer_age: crate::egl_renderer::BufferAge::Value(2),
            },
        };

        assert!(completion_matches_job(identity, identity));
        assert!(
            identity.belongs_to_output_epoch(oblivion_one::native::OutputEpochId::new(14).unwrap())
        );
        assert!(
            !identity
                .belongs_to_output_epoch(oblivion_one::native::OutputEpochId::new(15).unwrap())
        );
        assert!(!completion_matches_job(
            identity,
            AtomicRenderWorkerIdentity {
                job_id: RenderWorkerJobId::new(11).unwrap(),
                ..identity
            }
        ));
        assert!(!completion_matches_job(
            identity,
            AtomicRenderWorkerIdentity {
                frame_id: 20,
                ..identity
            }
        ));
        assert!(!completion_matches_job(
            identity,
            AtomicRenderWorkerIdentity {
                transaction_id: OutputTransactionId::new(NonZeroU64::new(21).unwrap()),
                ..identity
            }
        ));
        assert!(!completion_matches_job(
            identity,
            AtomicRenderWorkerIdentity {
                output_epoch: OutputEpochId::new(17).unwrap(),
                ..identity
            }
        ));
        assert!(!completion_matches_job(
            identity,
            AtomicRenderWorkerIdentity {
                render_generation: 18,
                ..identity
            }
        ));
        assert!(!completion_matches_job(
            identity,
            AtomicRenderWorkerIdentity {
                control_epoch: 20,
                ..identity
            }
        ));
        assert!(!completion_matches_job(
            identity,
            AtomicRenderWorkerIdentity {
                target: RenderTargetRequest {
                    slot: super::super::OutputSlotId::new(2).unwrap(),
                    ..identity.target
                },
                ..identity
            }
        ));
        assert!(!completion_matches_job(
            identity,
            AtomicRenderWorkerIdentity {
                target: RenderTargetRequest {
                    pool_generation: 19,
                    ..identity.target
                },
                ..identity
            }
        ));
        assert!(!completion_matches_job(
            identity,
            AtomicRenderWorkerIdentity {
                target: RenderTargetRequest {
                    buffer_age: crate::egl_renderer::BufferAge::Value(3),
                    ..identity.target
                },
                ..identity
            }
        ));
    }
}
