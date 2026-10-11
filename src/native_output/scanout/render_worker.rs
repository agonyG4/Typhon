//! Startup policy and reactor-side admission state for the Atomic render worker.

use super::atomic_render_domain::{
    AtomicRenderDomain, AtomicRenderDomainBootstrap, AtomicRenderDomainInfo,
    AtomicSlotRenderOutcome,
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
    time::Instant,
};

use crate::egl_renderer::native_fence::NativeRenderFence;
use crate::egl_renderer::{EglSceneFrameCommit, OutputDamage};
use crate::native_output::presentation::transaction::OutputTransactionId;
use oblivion_one::cursor_theme::CompositorCursorImage;
use oblivion_one::effects::EffectRegistryGeneration;
use oblivion_one::native::kms::DrmFormatModifierPair;

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
    pub(crate) output_generation: u64,
    pub(crate) render_generation: u64,
    pub(crate) target: RenderTargetRequest,
    pub(crate) frozen_frame: FrozenNativeRenderFrame,
    pub(crate) need_no_visual_release_fence: bool,
    pub(crate) dispatch_at_ns: u64,
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
    pub(crate) output_generation: u64,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AtomicRenderWorkerIdentity {
    pub(crate) job_id: RenderWorkerJobId,
    pub(crate) frame_id: u64,
    pub(crate) transaction_id: OutputTransactionId,
    pub(crate) output_generation: u64,
    pub(crate) render_generation: u64,
    pub(crate) target: RenderTargetRequest,
}

impl AtomicRenderWorkerCompletion {
    pub(crate) const fn identity(&self) -> AtomicRenderWorkerIdentity {
        AtomicRenderWorkerIdentity {
            job_id: self.job_id,
            frame_id: self.frame_id,
            transaction_id: self.transaction_id,
            output_generation: self.output_generation,
            render_generation: self.render_generation,
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
        && expected.output_generation == actual.output_generation
        && expected.render_generation == actual.render_generation
        && expected.target.slot.get() == actual.target.slot.get()
        && expected.target.pool_generation == actual.target.pool_generation
        && expected.target.buffer_age == actual.target.buffer_age
}

pub(crate) enum AtomicRenderWorkerEvent {
    Rendered(AtomicRenderWorkerCompletion),
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
}

enum RenderWorkerCommand {
    Initialize {
        init: RenderWorkerInit,
        response: SyncSender<io::Result<AtomicRenderDomainInfo>>,
    },
    Render(AtomicRenderWorkerJob),
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
}

impl AtomicRenderWorkerHandle {
    pub(crate) fn start(
        render_device_fd: OwnedFd,
    ) -> io::Result<(
        Self,
        Vec<oblivion_one::render_backend::egl_gles::EglGlesDmabufFormat>,
    )> {
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
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render_worker_loop(
                        bootstrap,
                        command_rx,
                        event_tx,
                        worker_event_fd,
                        worker_fatal.clone(),
                    );
                }))
                .is_err()
                {
                    worker_fatal.store(true, Ordering::Release);
                    let _ = publish_event(
                        &panic_events,
                        &panic_fd,
                        AtomicRenderWorkerEvent::Fatal {
                            job_id: None,
                            gpu_execution_uncertain: true,
                            reason: "worker_loop_panic",
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

    pub(crate) fn drain_eventfd(&self) -> io::Result<()> {
        drain_eventfd(&self.event_fd)
    }

    pub(crate) fn try_take_event(&mut self) -> io::Result<Option<AtomicRenderWorkerEvent>> {
        self.drain_eventfd()?;
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
        // Teardown is outside normal operation. If a render is still running,
        // the bounded command slot accepts Shutdown behind it; physical BOs
        // remain owned by the caller until this acknowledgement/join returns.
        let _ = self.commands.send(RenderWorkerCommand::Shutdown);
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
                    let (mut initialized, info) = bootstrap.initialize_renderer(
                        init.width,
                        init.height,
                        init.format_modifier,
                    )?;
                    initialized.install_pool(
                        init.descriptors,
                        init.pool_generation,
                        &init.physical_slot_identities,
                    )?;
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
                    output_generation,
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
                        apply_render_control(domain, control.control)?;
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
                        Some(domain.create_render_fence()?)
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
                            output_generation,
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
            RenderWorkerCommand::Shutdown => {
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
    assert_send::<AtomicRenderWorkerJob>();
    assert_send::<AtomicRenderWorkerCompletion>();
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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RenderWorkerAdmission {
    busy: bool,
    render_demand: bool,
    coalesced_demand_count: u64,
}

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
    fn completion_identity_rejects_stale_job_and_generation_mismatches() {
        let identity = AtomicRenderWorkerIdentity {
            job_id: RenderWorkerJobId::new(10).unwrap(),
            frame_id: 12,
            transaction_id: OutputTransactionId::new(NonZeroU64::new(13).unwrap()),
            output_generation: 14,
            render_generation: 15,
            target: RenderTargetRequest {
                slot: super::super::OutputSlotId::new(1).unwrap(),
                pool_generation: 16,
                buffer_age: crate::egl_renderer::BufferAge::Value(2),
            },
        };

        assert!(completion_matches_job(identity, identity));
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
                output_generation: 17,
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
    }
}
